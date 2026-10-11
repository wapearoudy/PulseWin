//! Pulse 1.9 ZedReader (Apache-2.0): native zed.dev thread counters only.
use super::measured_readers::{count, record, text};
use super::*;
use rusqlite::{types::ValueRef, Connection, OpenFlags};
use std::io::Read;
use std::time::Instant;
const LIMIT: usize = 32 * 1024 * 1024;
pub(super) fn candidate(path: &Path) -> bool {
    path.file_name().is_some_and(|n| n == "threads.db")
}
pub(super) fn roots(home: &Path, env: &impl Fn(&str) -> Option<String>) -> Vec<PathBuf> {
    let base = env("XDG_DATA_HOME")
        .filter(|s| !s.trim().is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| home.join(".local/share"));
    let mut roots = vec![base.join("zed/threads/threads.db")];
    if let Some(local) = env("LOCALAPPDATA") {
        roots.push(PathBuf::from(local).join("Zed/threads/threads.db"))
    }
    roots
}
fn entry(value: &Value) -> Tally {
    Tally {
        input: count(&value["input_tokens"]),
        output: count(&value["output_tokens"]),
        cache_write: count(&value["cache_creation_input_tokens"]),
        cache_read: count(&value["cache_read_input_tokens"]),
        ..Default::default()
    }
}
fn payload(kind: &str, raw: &[u8], cancel: &AtomicBool) -> Result<Option<Value>, String> {
    check(cancel)?;
    if raw.len() > LIMIT {
        return Err("Zed：线程数据超过 32 MiB。".into());
    }
    let compressed = kind == "zstd" || raw.starts_with(&[0x28, 0xb5, 0x2f, 0xfd]);
    let bytes = if compressed {
        let mut decoder =
            zstd::stream::read::Decoder::new(raw).map_err(|_| "Zed：压缩线程无法读取。")?;
        decoder
            .window_log_max(25)
            .map_err(|_| "Zed：压缩窗口超出上限。")?;
        let mut out = vec![];
        let mut buffer = [0u8; 65536];
        loop {
            check(cancel)?;
            let n = decoder
                .read(&mut buffer)
                .map_err(|_| "Zed：压缩线程损坏。")?;
            if n == 0 {
                break;
            }
            if out.len() + n > LIMIT {
                return Err("Zed：解压后线程超过 32 MiB。".into());
            }
            out.extend_from_slice(&buffer[..n]);
        }
        out
    } else if kind == "json" {
        raw.to_vec()
    } else {
        return Ok(None);
    };
    serde_json::from_slice(&bytes)
        .map(Some)
        .map_err(|_| "Zed：线程 JSON 无法读取。".into())
}
fn thread(row: &Value, thread: &Value) -> Result<Option<SpendRecord>, String> {
    if thread["imported"] == true || thread["model"]["provider"] != "zed.dev" {
        return Ok(None);
    }
    let Some(model) = text(&thread["model"]["model"]) else {
        return Err("Zed：线程缺少模型。".into());
    };
    let mut tally = Tally::default();
    let mut add = |u: &Value| {
        let t = entry(u);
        tally.input = tally.input.saturating_add(t.input);
        tally.output = tally.output.saturating_add(t.output);
        tally.cache_read = tally.cache_read.saturating_add(t.cache_read);
        tally.cache_write = tally.cache_write.saturating_add(t.cache_write);
    };
    match &thread["request_token_usage"] {
        Value::Array(a) => {
            for u in a {
                add(u)
            }
        }
        Value::Object(o) => {
            for u in o.values() {
                add(u)
            }
        }
        _ => {}
    }
    if tally.total() == 0 {
        tally = entry(&thread["cumulative_token_usage"])
    }
    if tally.total() == 0 {
        return Ok(None);
    }
    let Some(at) = super::aggregate_databases::dated(&row["created_at"])
        .or_else(|| super::aggregate_databases::dated(&row["updated_at"]))
        .or_else(|| super::aggregate_databases::dated(&thread["updated_at"]))
    else {
        return Err("Zed：线程缺少有效时间。".into());
    };
    let Some(id) = text(&row["id"]) else {
        return Err("Zed：线程缺少标识。".into());
    };
    let mut r = record(
        "zed",
        model,
        at,
        id.clone(),
        tally,
        0,
        Some(format!("zed:{id}")),
    );
    r.aggregate_timing = true;
    if let Some(paths) = row["folder_paths"].as_str() {
        let paths = paths
            .lines()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .collect::<Vec<_>>();
        let index = row["folder_paths_order"]
            .as_str()
            .and_then(|s| s.split(',').next()?.trim().parse::<usize>().ok())
            .unwrap_or(0);
        r.project = paths
            .get(index)
            .or_else(|| paths.first())
            .map(|s| (*s).into())
    }
    Ok(Some(r))
}
pub(super) fn read(
    path: &Path,
    cancel: &AtomicBool,
) -> Result<(Vec<SpendRecord>, Vec<String>), String> {
    check(cancel)?;
    let db = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .map_err(|_| "Zed：线程数据库无法读取。")?;
    db.busy_timeout(Duration::from_millis(150))
        .map_err(|e| e.to_string())?;
    let began = Instant::now();
    db.progress_handler(
        1000,
        Some(move || began.elapsed() > Duration::from_secs(15)),
    );
    let mut info = db
        .prepare("PRAGMA table_info(threads)")
        .map_err(|e| e.to_string())?;
    let columns = info
        .query_map([], |r| r.get::<_, String>(1))
        .map_err(|e| e.to_string())?
        .collect::<Result<HashSet<_>, _>>()
        .map_err(|e| e.to_string())?;
    if columns.is_empty() {
        return Ok((vec![], vec![]));
    }
    let names = [
        "id",
        "created_at",
        "updated_at",
        "folder_paths",
        "folder_paths_order",
        "data_type",
        "data",
    ];
    let fields = names
        .iter()
        .map(|n| {
            if columns.contains(*n) {
                format!("CASE WHEN length({n}) <= {LIMIT} THEN {n} ELSE NULL END")
            } else {
                "NULL".into()
            }
        })
        .collect::<Vec<_>>()
        .join(",");
    let mut statement = db
        .prepare(&format!(
            "SELECT {fields} FROM threads LIMIT {}",
            MAX_RECORDS + 1
        ))
        .map_err(|e| e.to_string())?;
    let mut rows = statement.query([]).map_err(|e| e.to_string())?;
    let mut out = vec![];
    let mut notes = vec![];
    let mut visited = 0;
    while let Some(row) = rows.next().map_err(|e| e.to_string())? {
        check(cancel)?;
        visited += 1;
        if visited > MAX_RECORDS {
            notes.push("Zed：线程数量超出上限。".into());
            break;
        }
        let mut metadata = serde_json::Map::new();
        for (i, n) in names[..6].iter().enumerate() {
            let v = match row.get_ref(i).map_err(|e| e.to_string())? {
                ValueRef::Text(s) => Value::from(String::from_utf8_lossy(s).into_owned()),
                ValueRef::Integer(n) => Value::from(n),
                ValueRef::Real(n) => Value::from(n),
                _ => Value::Null,
            };
            metadata.insert((*n).into(), v);
        }
        let meta = Value::Object(metadata);
        let raw = row.get_ref(6).map_err(|e| e.to_string())?;
        let data = match raw {
            ValueRef::Text(s) | ValueRef::Blob(s) => s,
            _ => {
                notes.push("Zed：线程数据缺失或超出读取上限。".into());
                continue;
            }
        };
        match payload(meta["data_type"].as_str().unwrap_or(""), data, cancel).and_then(
            |v| match v {
                Some(v) => thread(&meta, &v),
                None => Ok(None),
            },
        ) {
            Ok(Some(r)) => out.push(r),
            Ok(None) => {}
            Err(e) => {
                check(cancel)?;
                notes.push(e)
            }
        }
    }
    notes.sort();
    notes.dedup();
    Ok((out, notes))
}
#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn requests_win_over_cumulative_and_acp_imports_are_skipped() {
        let meta = json!({"id":"s","created_at":"2026-10-11","folder_paths":"a\nb","folder_paths_order":"1,0"});
        let mut t = json!({"model":{"provider":"zed.dev","model":"m"},"request_token_usage":{"r":{"input_tokens":"100","output_tokens":20,"cache_read_input_tokens":30,"cache_creation_input_tokens":5}},"cumulative_token_usage":{"input_tokens":999}});
        let r = thread(&meta, &t).unwrap().unwrap();
        assert_eq!(r.tally.total(), 155);
        assert_eq!(r.project.as_deref(), Some("b"));
        assert!(r.aggregate_timing);
        t["model"]["provider"] = json!("acp");
        assert!(thread(&meta, &t).unwrap().is_none());
        t["model"]["provider"] = json!("zed.dev");
        t["imported"] = json!(true);
        assert!(thread(&meta, &t).unwrap().is_none());
    }
    #[test]
    fn plain_mislabelled_compressed_and_corrupt_threads() {
        let cancel = AtomicBool::new(false);
        let json = b"{\"model\":{}}";
        assert!(payload("json", json, &cancel).unwrap().is_some());
        let zipped = zstd::stream::encode_all(&json[..], 1).unwrap();
        assert_eq!(
            payload("json", &zipped, &cancel).unwrap(),
            payload("json", json, &cancel).unwrap()
        );
        assert!(payload("zstd", b"corrupt", &cancel).is_err());
        assert!(payload("json", json, &AtomicBool::new(true)).is_err());
    }
}
