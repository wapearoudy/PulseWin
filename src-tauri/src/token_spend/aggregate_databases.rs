//! Pulse 1.9 GooseReader/HermesReader (Apache-2.0), cumulative SQLite counters.
//! Select only declared counters. No conversation columns or invented cache split.
use super::measured_readers::{count, record, text, timestamp};
use super::*;
use rusqlite::{types::ValueRef, Connection, OpenFlags};
use std::time::Instant;
pub(super) fn agent(id: &str) -> bool {
    matches!(id, "goose" | "hermes")
}
pub(super) fn candidate(path: &Path, id: &str) -> bool {
    path.file_name().is_some_and(|n| {
        n == if id == "goose" {
            "sessions.db"
        } else {
            "state.db"
        }
    })
}
pub(super) fn roots(home: &Path, id: &str, env: &impl Fn(&str) -> Option<String>) -> Vec<PathBuf> {
    let directory = |key: &str, default: &str| {
        env(key)
            .filter(|s| !s.trim().is_empty())
            .map(|s| {
                if let Some(tail) = s.strip_prefix("~/").or_else(|| s.strip_prefix("~\\")) {
                    home.join(tail)
                } else {
                    PathBuf::from(s)
                }
            })
            .unwrap_or_else(|| home.join(default))
    };
    if id == "hermes" {
        let base = directory("HERMES_HOME", ".hermes");
        let mut roots = vec![
            base.join("state.db"),
            base.join("profiles"),
            home.join("AppData/Local/hermes/state.db"),
        ];
        if let Some(local) = env("LOCALAPPDATA") {
            roots.push(PathBuf::from(local).join("hermes/state.db"))
        }
        roots
    } else {
        let mut candidates = vec![];
        if let Some(root) = env("GOOSE_PATH_ROOT").filter(|s| !s.trim().is_empty()) {
            candidates.push(PathBuf::from(root).join("data/sessions/sessions.db"))
        }
        if let Some(local) = env("APPDATA") {
            candidates.push(PathBuf::from(local).join("goose/sessions/sessions.db"))
        }
        candidates
            .push(directory("XDG_DATA_HOME", ".local/share").join("goose/sessions/sessions.db"));
        candidates.push(home.join(".local/share/Block/goose/sessions/sessions.db"));
        let first = candidates.iter().find(|p| p.is_file()).cloned();
        first.map(|p| vec![p]).unwrap_or(candidates)
    }
}
fn rows(
    db: &Connection,
    table: &str,
    names: &[&str],
    cancel: &AtomicBool,
) -> Result<Vec<Value>, String> {
    let mut info = db
        .prepare(&format!("PRAGMA table_info({table})"))
        .map_err(|e| e.to_string())?;
    let present = info
        .query_map([], |r| r.get::<_, String>(1))
        .map_err(|e| e.to_string())?
        .collect::<Result<HashSet<_>, _>>()
        .map_err(|e| e.to_string())?;
    if present.is_empty() {
        return Ok(vec![]);
    }
    let columns = names
        .iter()
        .map(|name| {
            if present.contains(*name) {
                format!("CASE WHEN length(CAST({name} AS TEXT)) <= 65536 THEN {name} ELSE NULL END")
            } else {
                "NULL".into()
            }
        })
        .collect::<Vec<_>>()
        .join(",");
    let mut query = db
        .prepare(&format!(
            "SELECT {columns} FROM {table} LIMIT {}",
            MAX_RECORDS + 1
        ))
        .map_err(|e| e.to_string())?;
    let mut cursor = query.query([]).map_err(|e| e.to_string())?;
    let mut result = vec![];
    while let Some(row) = cursor.next().map_err(|e| e.to_string())? {
        check(cancel)?;
        if result.len() >= MAX_RECORDS {
            return Err("累计数据库记录超过读取上限。".into());
        }
        let mut object = serde_json::Map::new();
        for (i, name) in names.iter().enumerate() {
            let value = match row.get_ref(i).map_err(|e| e.to_string())? {
                ValueRef::Integer(n) => Value::from(n),
                ValueRef::Real(n) => Value::from(n),
                ValueRef::Text(s) => Value::from(String::from_utf8_lossy(s).into_owned()),
                _ => Value::Null,
            };
            object.insert((*name).into(), value);
        }
        result.push(Value::Object(object));
    }
    Ok(result)
}
pub(super) fn dated(value: &Value) -> Option<DateTime<Utc>> {
    let numeric = value
        .as_f64()
        .or_else(|| value.as_str()?.parse::<f64>().ok());
    if let Some(n) = numeric {
        return if n.is_finite() && n > 0. {
            timestamp(value, n >= 1e12)
        } else {
            None
        };
    }
    timestamp(value, false).or_else(|| {
        let s = value.as_str()?;
        let utc = chrono::NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M:%S")
            .ok()
            .or_else(|| {
                chrono::NaiveDate::parse_from_str(s, "%Y-%m-%d")
                    .ok()?
                    .and_hms_opt(0, 0, 0)
            })?;
        Some(utc.and_utc())
    })
}
fn goose(data: Vec<Value>) -> (Vec<SpendRecord>, Vec<String>) {
    let mut out = vec![];
    let mut notes = vec![];
    for row in data {
        let input = count(if row["accumulated_input_tokens"].is_null() {
            &row["input_tokens"]
        } else {
            &row["accumulated_input_tokens"]
        });
        let output = count(if row["accumulated_output_tokens"].is_null() {
            &row["output_tokens"]
        } else {
            &row["accumulated_output_tokens"]
        });
        let total = count(if row["accumulated_total_tokens"].is_null() {
            &row["total_tokens"]
        } else {
            &row["accumulated_total_tokens"]
        });
        if input == 0 && output == 0 && total == 0 {
            continue;
        }
        let model = row["model_config_json"]
            .as_str()
            .and_then(|s| serde_json::from_str::<Value>(s).ok())
            .and_then(|v| text(&v["model_name"]));
        if let (Some(session), Some(model), Some(at)) =
            (text(&row["id"]), model, dated(&row["created_at"]))
        {
            let tally = Tally {
                input,
                output,
                replies_without_cache_fields: 1,
                ..Default::default()
            };
            let bare = total.saturating_sub(tally.total());
            let mut r = record(
                "goose",
                model,
                at,
                session.clone(),
                tally,
                bare,
                Some(format!("goose:{session}")),
            );
            r.aggregate_timing = true;
            out.push(r)
        } else {
            notes.push("Goose：累计计数缺少会话、模型或有效时间。".into())
        }
    }
    (out, notes)
}
fn hermes(sessions: Vec<Value>, models: Vec<Value>) -> (Vec<SpendRecord>, Vec<String>) {
    let sessions = sessions
        .into_iter()
        .filter_map(|v| text(&v["id"]).map(|id| (id, v)))
        .collect::<HashMap<_, _>>();
    let mut groups: BTreeMap<(String, String, Option<String>), [u64; 5]> = BTreeMap::new();
    let mut notes = vec![];
    for row in models {
        if let (Some(session), Some(model)) = (text(&row["session_id"]), text(&row["model"])) {
            let key = (
                session,
                model,
                row["billing_provider"].as_str().map(str::to_owned),
            );
            let sums = groups.entry(key).or_default();
            for (i, name) in [
                "input_tokens",
                "output_tokens",
                "cache_read_tokens",
                "cache_write_tokens",
                "reasoning_tokens",
            ]
            .iter()
            .enumerate()
            {
                sums[i] = sums[i].saturating_add(count(&row[*name]));
            }
        } else {
            notes.push("Hermes：模型累计计数缺少会话或模型。".into())
        }
    }
    let mut out = vec![];
    let mut covered = HashSet::new();
    let mut emit = |session: &str, model: String, sums: [u64; 5], identity: String| -> bool {
        let [input, output, cache_read, cache_write, reasoning] = sums;
        if input == 0 && output == 0 {
            return false;
        }
        let Some(at) = sessions.get(session).and_then(|s| dated(&s["started_at"])) else {
            notes.push("Hermes：累计计数缺少有效会话时间。".into());
            return false;
        };
        let cache = cache_read > 0 || cache_write > 0;
        let tally = Tally {
            input: if cache { 0 } else { input },
            output,
            replies_without_cache_fields: u64::from(cache || reasoning > 0),
            ..Default::default()
        };
        let mut r = record(
            "hermes",
            model,
            at,
            session.into(),
            tally,
            if cache { input } else { 0 },
            Some(identity),
        );
        r.aggregate_timing = true;
        out.push(r);
        true
    };
    for ((session, model, provider), sums) in groups {
        let identity = format!(
            "hermes:model:{}",
            serde_json::to_string(&(&session, &model, &provider)).unwrap()
        );
        if emit(&session, model, sums, identity) {
            covered.insert(session);
        }
    }
    for (session, row) in &sessions {
        if !covered.contains(session) {
            if let Some(model) = text(&row["model"]) {
                emit(
                    session,
                    model,
                    [
                        count(&row["input_tokens"]),
                        count(&row["output_tokens"]),
                        count(&row["cache_read_tokens"]),
                        count(&row["cache_write_tokens"]),
                        count(&row["reasoning_tokens"]),
                    ],
                    format!("hermes:session:{session}"),
                );
            }
        }
    }
    if out
        .iter()
        .any(|r| r.unclassified_tokens > 0 || r.tally.replies_without_cache_fields > 0)
    {
        notes.push(
            "Hermes：输入与缓存或输出与推理的包含关系未声明，仅统计可证明的计数；结果可能不完整。"
                .into(),
        )
    }
    (out, notes)
}
pub(super) fn read(
    path: &Path,
    id: &str,
    cancel: &AtomicBool,
) -> Result<(Vec<SpendRecord>, Vec<String>), String> {
    check(cancel)?;
    let db = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .map_err(|_| format!("{id}：累计数据库无法读取。"))?;
    db.busy_timeout(Duration::from_millis(150))
        .map_err(|e| e.to_string())?;
    let began = Instant::now();
    db.progress_handler(
        1000,
        Some(move || began.elapsed() > Duration::from_secs(15)),
    );
    let result = if id == "goose" {
        goose(rows(
            &db,
            "sessions",
            &[
                "id",
                "model_config_json",
                "created_at",
                "input_tokens",
                "output_tokens",
                "total_tokens",
                "accumulated_input_tokens",
                "accumulated_output_tokens",
                "accumulated_total_tokens",
            ],
            cancel,
        )?)
    } else {
        hermes(
            rows(
                &db,
                "sessions",
                &[
                    "id",
                    "model",
                    "started_at",
                    "input_tokens",
                    "output_tokens",
                    "cache_read_tokens",
                    "cache_write_tokens",
                    "reasoning_tokens",
                ],
                cancel,
            )?,
            rows(
                &db,
                "session_model_usage",
                &[
                    "session_id",
                    "model",
                    "billing_provider",
                    "input_tokens",
                    "output_tokens",
                    "cache_read_tokens",
                    "cache_write_tokens",
                    "reasoning_tokens",
                ],
                cancel,
            )?,
        )
    };
    check(cancel)?;
    Ok(result)
}
#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn goose_prefers_cumulative_and_keeps_unknown_remainder() {
        let (r, n) = goose(vec![
            json!({"id":"s","model_config_json":"{\"model_name\":\"m\"}","created_at":"2026-10-11 08:00:00","input_tokens":999,"accumulated_input_tokens":100,"output_tokens":999,"accumulated_output_tokens":20,"accumulated_total_tokens":150}),
        ]);
        assert!(n.is_empty());
        assert_eq!(r[0].tally.total(), 120);
        assert_eq!(r[0].unclassified_tokens, 30);
        assert!(r[0].aggregate_timing);
        assert_eq!(r[0].tally.replies_without_cache_fields, 1);
    }
    #[test]
    fn hermes_groups_models_without_reemitting_session_or_double_counting_cache() {
        let sessions = vec![
            json!({"id":"s","model":"fallback","started_at":1791705600,"input_tokens":999,"output_tokens":999}),
        ];
        let models = vec![
            json!({"session_id":"s","model":"m","billing_provider":null,"input_tokens":100,"output_tokens":20,"cache_read_tokens":50,"reasoning_tokens":10}),
            json!({"session_id":"s","model":"m","billing_provider":null,"input_tokens":30,"output_tokens":5}),
        ];
        let (r, n) = hermes(sessions, models);
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].tally.total(), 25);
        assert_eq!(r[0].unclassified_tokens, 130);
        assert!(!n.is_empty());
    }
    #[test]
    fn missing_dates_are_never_bucketed_at_epoch() {
        let (r, n) = goose(vec![
            json!({"id":"s","model_config_json":"{\"model_name\":\"m\"}","input_tokens":1}),
        ]);
        assert!(r.is_empty());
        assert_eq!(n.len(), 1);
        assert!(dated(&Value::Null).is_none());
        assert!(dated(&Value::from(0)).is_none());
        assert_eq!(
            dated(&Value::from(1791705600000u64)),
            dated(&Value::from(1791705600u64))
        );
    }
}
