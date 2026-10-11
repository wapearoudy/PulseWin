//! Pulse 1.9 OpenClawSessionReader (Apache-2.0): current SQLite and legacy JSONL.
//! All sources use the same event identity, including fork/import copies.
use super::measured_readers::{count, record, text, timestamp};
use super::*;
use rusqlite::{Connection, OpenFlags};
use std::time::Instant;
pub(super) fn database(path: &Path) -> bool {
    path.file_name()
        .is_some_and(|s| s == "openclaw-agent.sqlite")
}
pub(super) fn candidate(path: &Path) -> bool {
    if database(path) {
        return true;
    }
    let name = path.file_name().unwrap_or_default().to_string_lossy();
    let parts = path
        .components()
        .map(|p| p.as_os_str().to_string_lossy())
        .collect::<Vec<_>>();
    name.contains(".jsonl")
        && !name.ends_with(".zst")
        && !name.ends_with(".zstd")
        && parts
            .iter()
            .any(|p| p == "sessions" || p == "session-sqlite-import-archive")
        && !parts.iter().any(|p| p == "codex-home" || p == "cli-auth")
}
fn model(row: &Value) -> Option<String> {
    match row["type"].as_str() {
        Some("model_change") => text(&row["modelId"]),
        Some("custom") if row["customType"] == "model-snapshot" => text(&row["data"]["modelId"]),
        _ => None,
    }
}
fn event(
    row: &Value,
    session: &str,
    fallback: &str,
    carried: Option<&str>,
) -> Result<Option<SpendRecord>, String> {
    let m = &row["message"];
    if row["type"] != "message" || m["role"] != "assistant" || !m["usage"].is_object() {
        return Ok(None);
    }
    let api = text(&m["api"]).or_else(|| text(&row["api"]));
    let provider = text(&m["provider"]).or_else(|| text(&row["provider"]));
    let named = text(&m["model"]).or_else(|| text(&row["model"]));
    if api.as_deref() == Some("openclaw-transcript")
        || (provider.as_deref() == Some("openclaw")
            && named
                .as_deref()
                .is_some_and(|m| m == "delivery-mirror" || m == "gateway-injected"))
    {
        return Ok(None);
    }
    let u = &m["usage"];
    let mut tally = Tally {
        input: count(&u["input"]),
        output: if u.get("output").is_some() {
            count(&u["output"])
        } else {
            count(&u["reasoningTokens"])
        },
        cache_read: count(&u["cacheRead"]),
        cache_write: count(&u["cacheWrite"]),
        replies_without_cache_fields: u64::from(
            u.get("cacheRead").is_none() && u.get("cacheWrite").is_none(),
        ),
        ..Default::default()
    };
    let known = ["input", "output", "cacheRead", "cacheWrite"]
        .iter()
        .any(|k| u.get(*k).is_some());
    let bare = if known {
        count(&u["totalTokens"]).saturating_sub(tally.total())
    } else {
        tally = Tally::default();
        count(&u["totalTokens"])
    };
    if tally.total() == 0 && bare == 0 {
        return Ok(None);
    }
    let Some(at) = timestamp(&m["timestamp"], true).or_else(|| timestamp(&row["timestamp"], true))
    else {
        return Err("OpenClaw：计数缺少有效时间。".into());
    };
    let Some(model) = text(&m["model"]).or_else(|| carried.map(str::to_owned)) else {
        return Err("OpenClaw：计数缺少可定位模型。".into());
    };
    let id = text(&row["id"]).unwrap_or_else(|| fallback.into());
    let identity = format!(
        "openclaw:{id}:{}:{}:{}",
        at.timestamp_millis(),
        tally.input,
        tally.output
    );
    Ok(Some(record(
        "openclaw",
        model,
        at,
        session.into(),
        tally,
        bare,
        Some(identity),
    )))
}
fn registry(path: &Path, cancel: &AtomicBool) -> Result<Option<String>, String> {
    let file = path.with_file_name("sessions.json");
    if !file.is_file() {
        return Ok(None);
    }
    let root: Value = measured_readers::document(&file, cancel)?;
    let name = path.file_name().unwrap_or_default().to_string_lossy();
    Ok(root.as_object().and_then(|r| {
        r.values().find_map(|v| {
            let f = v["sessionFile"].as_str()?.rsplit(['/', '\\']).next()?;
            if f == name {
                text(&v["sessionId"])
            } else {
                None
            }
        })
    }))
}
fn jsonl(path: &Path, cancel: &AtomicBool) -> Result<(Vec<SpendRecord>, Vec<String>), String> {
    let name = path.file_name().unwrap_or_default().to_string_lossy();
    let session = registry(path, cancel)?
        .unwrap_or_else(|| name.split(".jsonl").next().unwrap_or(&name).into());
    let mut carried = None;
    let mut records = vec![];
    let mut notes = vec![];
    let mut issues = vec![];
    let file = fs::File::open(path).map_err(|_| "OpenClaw：会话无法读取。")?;
    measured_readers::lines(&mut BufReader::new(file), cancel, &mut notes, |row, i| {
        if let Some(m) = model(&row) {
            carried = Some(m)
        }
        match event(
            &row,
            &session,
            &format!("line-{}", i - 1),
            carried.as_deref(),
        ) {
            Ok(Some(r)) => records.push(r),
            Ok(None) => {}
            Err(e) => issues.push(e),
        }
        if records.len() > MAX_RECORDS {
            return Err("OpenClaw：超过记录上限。".into());
        }
        Ok(())
    })?;
    notes.extend(issues);
    Ok((records, notes))
}
fn metadata(conn: &Connection, cancel: &AtomicBool) -> Result<HashMap<String, String>, String> {
    let mut out = HashMap::new();
    for table in ["session_windows", "sessions"] {
        let Ok(mut statement)=conn.prepare(&format!("SELECT session_id, model FROM {table} WHERE length(session_id)<=512 AND length(model)<=512 LIMIT 100001"))else{continue};
        let mut rows = statement
            .query([])
            .map_err(|_| "OpenClaw：无法读取会话信息。")?;
        while let Some(row) = rows.next().map_err(|_| "OpenClaw：会话信息读取中断。")? {
            check(cancel)?;
            if out.len() >= 100000 {
                return Err("OpenClaw：会话数量超过上限。".into());
            }
            if let Ok((id, m)) = row
                .get::<_, String>(0)
                .and_then(|id| row.get::<_, String>(1).map(|m| (id, m)))
            {
                out.insert(id, m);
            }
        }
        if !out.is_empty() {
            break;
        }
    }
    Ok(out)
}
fn sqlite(path: &Path, cancel: &AtomicBool) -> Result<(Vec<SpendRecord>, Vec<String>), String> {
    check(cancel)?;
    let conn = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .map_err(|_| "OpenClaw：无法只读打开数据库。")?;
    conn.busy_timeout(Duration::from_millis(150))
        .map_err(|_| "OpenClaw：无法设置读取等待时间。")?;
    let deadline = Instant::now() + Duration::from_secs(15);
    conn.progress_handler(10000, Some(move || Instant::now() > deadline));
    let meta = metadata(&conn, cancel)?;
    let mut carried: HashMap<String, String> = HashMap::new();
    let mut records = vec![];
    let mut notes = vec![];
    let mut statement=conn.prepare("SELECT session_id, seq, CASE WHEN length(event_json)<=8388608 THEN event_json ELSE NULL END FROM transcript_events WHERE event_json LIKE '%\"usage\"%' OR event_json LIKE '%\"model_change\"%' OR event_json LIKE '%model-snapshot%' ORDER BY session_id,seq LIMIT 4000001").map_err(|_|"OpenClaw：不支持的 transcript_events 表结构。")?;
    let mut rows = statement
        .query([])
        .map_err(|_| "OpenClaw：无法查询计数。")?;
    let mut visited = 0;
    loop {
        check(cancel)?;
        let row = match rows.next() {
            Ok(Some(r)) => r,
            Ok(None) => break,
            Err(_) => {
                notes.push("OpenClaw：数据库读取中断，结果可能不完整。".into());
                break;
            }
        };
        visited += 1;
        if visited > MAX_RECORDS * 4 || records.len() >= MAX_RECORDS {
            notes.push("OpenClaw：达到读取上限。".into());
            break;
        }
        let (Ok(session), Ok(seq), Ok(raw)) = (
            row.get::<_, String>(0),
            row.get::<_, i64>(1),
            row.get::<_, String>(2),
        ) else {
            notes.push("OpenClaw：事件过大或字段缺失。".into());
            continue;
        };
        let v: Value = match serde_json::from_str(&raw) {
            Ok(v) => v,
            Err(_) => {
                notes.push("OpenClaw：事件 JSON 无效。".into());
                continue;
            }
        };
        if let Some(m) = model(&v) {
            carried.insert(session.clone(), m);
        }
        match event(
            &v,
            &session,
            &format!("seq-{seq}"),
            carried
                .get(&session)
                .or_else(|| meta.get(&session))
                .map(String::as_str),
        ) {
            Ok(Some(r)) => records.push(r),
            Ok(None) => {}
            Err(e) => notes.push(e),
        }
    }
    check(cancel)?;
    Ok((records, notes))
}
pub(super) fn parse(
    path: &Path,
    cancel: &AtomicBool,
) -> Result<(Vec<SpendRecord>, Vec<String>), String> {
    if database(path) {
        sqlite(path, cancel)
    } else {
        jsonl(path, cancel)
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    fn message() -> Value {
        json!({"type":"message","id":"m","timestamp":"2026-10-11T00:00:00Z","message":{"role":"assistant","model":"gpt-5.6","provider":"openai","usage":{"input":100,"output":20,"reasoningTokens":10,"cacheRead":30,"cacheWrite":5,"totalTokens":155}}})
    }
    #[test]
    fn reasoning_is_output_subset_and_identity_is_cross_store() {
        let r = event(&message(), "s", "line-1", None).unwrap().unwrap();
        let copy = event(&message(), "fork", "seq-100", None).unwrap().unwrap();
        assert_eq!(r.tally.total(), 155);
        assert_eq!(r.deduplication_id, copy.deduplication_id);
        assert_eq!(r.tally.output, 20)
    }
    #[test]
    fn plumbing_and_unclassifiable_total_do_not_create_fake_input() {
        let mut m = message();
        m["message"]["api"] = json!("openclaw-transcript");
        assert!(event(&m, "s", "1", None).unwrap().is_none());
        m["message"]["api"] = Value::Null;
        m["message"]["usage"] = json!({"totalTokens":200});
        let r = event(&m, "s", "1", None).unwrap().unwrap();
        assert_eq!((r.tally.total(), r.unclassified_tokens), (0, 200));
    }
    #[test]
    fn archive_candidates_exclude_codex_and_compressed_unreadable_formats() {
        assert!(candidate(Path::new(
            "agents/a/sessions/s.jsonl.deleted.2026"
        )));
        assert!(candidate(Path::new("agents/a/agent/openclaw-agent.sqlite")));
        assert!(!candidate(Path::new(
            "agents/a/codex-home/sessions/x.jsonl"
        )));
        assert!(!candidate(Path::new("agents/a/sessions/x.jsonl.zst")));
    }
}
