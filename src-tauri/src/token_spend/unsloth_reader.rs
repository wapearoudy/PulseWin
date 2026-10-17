//! Pulse 1.9 UnslothReader (Apache-2.0): `studio.db` keeps real counters in
//! `chat_messages` (assistant metadata) and `api_usage_events`. PulseWin registers
//! this source through StoreHistory, so the reader emits cumulative per-day,
//! per-session, per-model snapshots with a stable id and lets StoreHistory
//! reconcile increments; it never dates a row 1970 and never guesses a split.

use super::measured_readers::{record, text, timestamp};
use super::*;
use rusqlite::{types::ValueRef, Connection, OpenFlags};
use std::time::Instant;

const CHAT: [&str; 5] = ["id", "thread_id", "role", "metadata_json", "created_at"];
const THREADS: [&str; 2] = ["id", "model_id"];
const API: [&str; 7] = ["id", "endpoint", "model", "prompt_tokens", "completion_tokens", "total_tokens", "created_at"];

pub(super) fn roots(home: &Path, id: &str, env: &impl Fn(&str) -> Option<String>) -> Vec<PathBuf> {
    if id != "unsloth" { return Vec::new(); }
    let base = expanded(home, env, "UNSLOTH_STUDIO_HOME").unwrap_or_else(|| home.join(".unsloth").join("studio"));
    vec![base.join("studio.db")]
}

pub(super) fn candidate(path: &Path, id: &str) -> bool {
    id == "unsloth" && path.file_name().is_some_and(|name| name == "studio.db")
}

struct Bucket {
    at: DateTime<Utc>,
    tally: Tally,
    unknown_cache_fields: bool,
}

pub(super) fn read(_path: &Path, id: &str, cancel: &AtomicBool) -> Result<(Vec<SpendRecord>, Vec<String>), String> {
    check(cancel)?;
    let path = _path;
    let db = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX)
        .map_err(|_| format!("{id}：累计数据库无法读取。"))?;
    let _ = db.busy_timeout(Duration::from_millis(150));
    let deadline = Instant::now() + Duration::from_secs(15);
    db.progress_handler(1000, Some(move || Instant::now() > deadline));
    let mut notes = Vec::new();
    let mut buckets: BTreeMap<(String, String, String, String), Bucket> = BTreeMap::new();
    let mut chat_dropped = false;
    let mut api_dropped = false;
    if has_columns(&db, "chat_messages", &CHAT)? && has_columns(&db, "chat_threads", &THREADS)? {
        let sql = format!(
            "SELECT m.id, m.thread_id, CASE WHEN length(CAST(m.metadata_json AS TEXT)) <= 65536 THEN m.metadata_json ELSE NULL END AS metadata_json, m.created_at, t.model_id \
             FROM chat_messages m JOIN chat_threads t ON m.thread_id = t.id WHERE m.role = 'assistant' LIMIT {}",
            MAX_RECORDS + 1,
        );
        for row in query(&db, &sql, &["id", "thread_id", "metadata_json", "created_at", "model_id"], cancel)? {
            check(cancel)?;
            let metadata = row.get("metadata_json").and_then(Value::as_str)
                .and_then(|raw| serde_json::from_str::<Value>(raw).ok())
                .filter(Value::is_object);
            let Some(metadata) = metadata else { continue; };
            let usage = metadata.get("contextUsage").filter(|value| value.is_object());
            let details = metadata.get("responseDetails").filter(|value| value.is_object());
            let prompt = clamped_count(usage.and_then(|usage| usage.get("promptTokens")));
            let completion = clamped_count(usage.and_then(|usage| usage.get("completionTokens")));
            let cached = clamped_count(usage.and_then(|usage| usage.get("cachedTokens")));
            let cache_creation = clamped_count(usage.and_then(|usage| usage.get("cacheWriteTokens")));
            let reported_total = clamped_count(usage.and_then(|usage| usage.get("totalTokens")));
            let total = reported_total.max(prompt.saturating_add(completion));
            // A message with no readable counters adds nothing; it is not an estimate.
            if total == 0 { continue; }
            let Some(message) = row.get("id").and_then(text) else { chat_dropped = true; continue; };
            let Some(at) = row.get("created_at").and_then(event_time) else { chat_dropped = true; continue; };
            let model = details.and_then(|details| details.get("responseModelId").and_then(text))
                .or_else(|| usage.and_then(|usage| usage.get("modelId").and_then(text)))
                .or_else(|| row.get("model_id").and_then(text))
                .unwrap_or_else(|| "unknown".into());
            let cache_read = cached.min(prompt);
            let cache_write = cache_creation.min(prompt.saturating_sub(cache_read));
            let tally = Tally {
                input: total.saturating_sub(completion).saturating_sub(cache_read).saturating_sub(cache_write),
                cache_write,
                cache_read,
                output: completion,
                ..Default::default()
            };
            let session = row.get("thread_id").and_then(text).unwrap_or_else(|| format!("unsloth:chat:{message}"));
            accumulate(&mut buckets, &at, "chat", session, model, tally, false);
        }
    }
    if has_columns(&db, "api_usage_events", &API)? {
        let sql = format!(
            "SELECT id, endpoint, model, prompt_tokens, completion_tokens, total_tokens, created_at FROM api_usage_events LIMIT {}",
            MAX_RECORDS + 1,
        );
        for row in query(&db, &sql, &["id", "endpoint", "model", "prompt_tokens", "completion_tokens", "total_tokens", "created_at"], cancel)? {
            check(cancel)?;
            let prompt = row.get("prompt_tokens").and_then(db_count).unwrap_or(0);
            let completion = row.get("completion_tokens").and_then(db_count).unwrap_or(0);
            let total = row.get("total_tokens").and_then(db_count).unwrap_or(0).max(prompt.saturating_add(completion));
            if total == 0 { continue; }
            let Some(model) = row.get("model").and_then(text) else { api_dropped = true; continue; };
            let Some(at) = row.get("created_at").and_then(event_time) else { api_dropped = true; continue; };
            let tally = Tally {
                input: total.saturating_sub(completion),
                output: completion,
                replies_without_cache_fields: 1,
                ..Default::default()
            };
            accumulate(&mut buckets, &at, "api", "unsloth:api".into(), model, tally, true);
        }
    }
    let mut out: Vec<SpendRecord> = Vec::new();
    for ((day, scope, session, model), bucket) in buckets {
        if out.len() >= MAX_RECORDS { return Err("Unsloth：记录超过读取上限。".into()); }
        let mut tally = bucket.tally;
        tally.replies_without_cache_fields = u64::from(bucket.unknown_cache_fields);
        let mut record = record(
            "unsloth",
            model.clone(),
            bucket.at,
            session.clone(),
            tally,
            0,
            Some(format!("unsloth:{scope}:{day}:{session}:{model}")),
        );
        // Cumulative counters: StoreHistory reconciles them; no increment is computed here.
        record.aggregate_timing = true;
        out.push(record);
    }
    if chat_dropped { notes.push("Unsloth：聊天记录缺少用量元信息或有效时间，计数可能不完整。".into()); }
    if api_dropped { notes.push("Unsloth：接口用量记录缺少模型或有效时间，计数可能不完整。".into()); }
    notes.sort(); notes.dedup();
    check(cancel)?;
    Ok((out, notes))
}

fn accumulate(
    buckets: &mut BTreeMap<(String, String, String, String), Bucket>,
    at: &DateTime<Utc>,
    scope: &str,
    session: String,
    model: String,
    tally: Tally,
    unknown_cache_fields: bool,
) {
    let day = at.with_timezone(&Local).format("%Y-%m-%d").to_string();
    let bucket = buckets.entry((day, scope.to_owned(), session, model)).or_insert_with(|| Bucket {
        at: *at,
        tally: Tally::default(),
        unknown_cache_fields,
    });
    bucket.at = bucket.at.max(*at);
    bucket.unknown_cache_fields |= unknown_cache_fields;
    bucket.tally.add(&tally);
}

fn query(db: &Connection, sql: &str, labels: &[&str], cancel: &AtomicBool) -> Result<Vec<Value>, String> {
    let mut statement = db.prepare(sql).map_err(|error| format!("Unsloth：数据库读取失败（{error}）。"))?;
    let mut rows = statement.query([]).map_err(|error| format!("Unsloth：数据库读取失败（{error}）。"))?;
    let mut data = Vec::new();
    while let Some(row) = rows.next().map_err(|error| format!("Unsloth：数据库读取失败（{error}）。"))? {
        check(cancel)?;
        if data.len() >= MAX_RECORDS { return Err("Unsloth：累计数据库记录超过读取上限。".into()); }
        let mut object = serde_json::Map::new();
        for (index, label) in labels.iter().enumerate() {
            let value = match row.get_ref(index).map_err(|error| format!("Unsloth：数据库读取失败（{error}）。"))? {
                ValueRef::Null => Value::Null,
                ValueRef::Integer(number) => Value::from(number),
                ValueRef::Real(number) => Value::from(number),
                ValueRef::Text(bytes) | ValueRef::Blob(bytes) => Value::from(String::from_utf8_lossy(bytes).into_owned()),
            };
            object.insert((*label).to_owned(), value);
        }
        data.push(Value::Object(object));
    }
    Ok(data)
}

fn has_columns(db: &Connection, table: &str, required: &[&str]) -> Result<bool, String> {
    let mut statement = db.prepare(&format!("PRAGMA table_info({table})"))
        .map_err(|error| format!("Unsloth：数据库表结构无法读取（{error}）。"))?;
    let mut rows = statement.query([]).map_err(|error| format!("Unsloth：数据库表结构无法读取（{error}）。"))?;
    let mut columns = HashSet::new();
    while let Some(row) = rows.next().map_err(|error| format!("Unsloth：数据库表结构无法读取（{error}）。"))? {
        if let Ok(name) = row.get::<_, String>(1) { columns.insert(name); }
    }
    Ok(!columns.is_empty() && required.iter().all(|name| columns.contains(*name)))
}

/// AgentLogIO.timestamp / eventTime: finite, strictly after the epoch (never 1970).
fn event_time(value: &Value) -> Option<DateTime<Utc>> {
    let raw = number(value).filter(|raw| *raw > 0.0)?;
    let at = timestamp(value, raw >= 1e12)?;
    (at.timestamp() > 0).then_some(at)
}

/// AgentLogIO.number: a finite Double from a number or a trimmed numeric string.
fn number(value: &Value) -> Option<f64> {
    if value.is_boolean() { return None; }
    if let Some(number) = value.as_f64() { return number.is_finite().then_some(number); }
    value.as_str().and_then(|raw| raw.trim().parse::<f64>().ok()).filter(|number| number.is_finite())
}

/// DatabaseReaderSupport.clampedCount: only a positive number counts, truncated.
fn clamped_count(value: Option<&Value>) -> u64 {
    value.and_then(number)
        .filter(|number| *number > 0.0 && *number < u64::MAX as f64)
        .map(|number| number.trunc() as u64)
        .unwrap_or(0)
}

/// DatabaseReaderSupport.intColumn: a non-negative integer, truncated like sqlite.
fn db_count(value: &Value) -> Option<u64> {
    if let Some(number) = value.as_i64() { return (number >= 0).then_some(number as u64); }
    if let Some(number) = value.as_f64() {
        if number.is_finite() && number >= 0.0 && number < u64::MAX as f64 { return Some(number.trunc() as u64); }
    }
    value.as_str().and_then(|raw| raw.trim().parse::<i64>().ok()).filter(|number| *number >= 0).map(|number| number as u64)
}

fn expanded(home: &Path, env: &impl Fn(&str) -> Option<String>, key: &str) -> Option<PathBuf> {
    let raw = env(key)?;
    let raw = raw.trim();
    if raw.is_empty() { return None; }
    if raw == "~" { return Some(home.to_path_buf()); }
    if let Some(rest) = raw.strip_prefix("~/").or_else(|| raw.strip_prefix("~\\")) { return Some(home.join(rest)); }
    Some(PathBuf::from(raw))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicUsize;

    static NEXT: AtomicUsize = AtomicUsize::new(0);

    fn temp_dir(label: &str) -> PathBuf {
        let suffix = format!("{label}-{}-{}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed));
        let candidate = std::env::temp_dir().join(format!("pulsewin-{suffix}"));
        if fs::create_dir_all(&candidate).is_ok() { return candidate; }
        // Sandboxed test binaries can be denied writes outside the workspace.
        let fallback = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target").join("test-scratch").join(format!("pulsewin-{suffix}"));
        fs::create_dir_all(&fallback).unwrap_or_else(|error| panic!("create_dir_all {}: {error:?}", fallback.display()));
        fallback
    }

    fn studio(path: &Path) -> Connection {
        let db = Connection::open(path).unwrap();
        db.execute_batch(
            "CREATE TABLE chat_threads (id TEXT, model_id TEXT); \
             CREATE TABLE chat_messages (id TEXT, thread_id TEXT, role TEXT, metadata_json TEXT, created_at INTEGER); \
             CREATE TABLE api_usage_events (id TEXT, endpoint TEXT, model TEXT, prompt_tokens INTEGER, completion_tokens INTEGER, total_tokens INTEGER, created_at INTEGER);",
        ).unwrap();
        db
    }

    #[test]
    fn roots_follow_unsloth_studio_home() {
        let home = temp_dir("unsloth-home");
        assert_eq!(
            roots(&home, "unsloth", &|_: &str| None),
            vec![home.join(".unsloth").join("studio").join("studio.db")]
        );
        let custom = roots(&home, "unsloth", &|key: &str| (key == "UNSLOTH_STUDIO_HOME").then(|| "~/custom".to_string()));
        assert_eq!(custom, vec![home.join("custom").join("studio.db")]);
        assert!(roots(&home, "lmstudio", &|_: &str| None).is_empty());
        let _ = fs::remove_dir_all(&home);
    }

    #[test]
    fn candidate_accepts_studio_database_only() {
        assert!(candidate(Path::new("C:\\unsloth\\studio\\studio.db"), "unsloth"));
        assert!(!candidate(Path::new("C:\\unsloth\\studio\\other.db"), "unsloth"));
        assert!(!candidate(Path::new("C:\\unsloth\\studio\\studio.db"), "zcode"));
    }

    #[test]
    fn cumulative_chat_and_api_counters_aggregate_without_deltas() {
        let home = temp_dir("unsloth-db");
        let path = home.join("studio.db");
        let db = studio(&path);
        db.execute("INSERT INTO chat_threads (id, model_id) VALUES ('t1', 'qwen3-8b')", []).unwrap();
        db.execute(
            "INSERT INTO chat_messages VALUES ('m1', 't1', 'assistant', '{\"contextUsage\":{\"promptTokens\":100,\"completionTokens\":20,\"cachedTokens\":10,\"totalTokens\":120},\"responseDetails\":{\"responseModelId\":\"qwen3-8b\"}}', 1700000000)",
            [],
        ).unwrap();
        db.execute(
            "INSERT INTO chat_messages VALUES ('m2', 't1', 'assistant', '{\"contextUsage\":{\"promptTokens\":50,\"completionTokens\":10,\"totalTokens\":60}}', 1700000005)",
            [],
        ).unwrap();
        db.execute("INSERT INTO chat_messages VALUES ('m3', 't1', 'user', '{\"contextUsage\":{\"promptTokens\":999}}', 1700000010)", []).unwrap();
        db.execute("INSERT INTO chat_messages VALUES ('m4', 't1', 'assistant', NULL, 1700000015)", []).unwrap();
        db.execute("INSERT INTO chat_messages VALUES ('m5', 't1', 'assistant', '{\"contextUsage\":{\"promptTokens\":7,\"completionTokens\":3,\"totalTokens\":10}}', NULL)", []).unwrap();
        db.execute("INSERT INTO api_usage_events VALUES ('a1', '/v1/chat', 'qwen2', 30, 10, 40, 1700000200)", []).unwrap();
        db.execute("INSERT INTO api_usage_events VALUES ('a2', '/v1/chat', 'qwen2', 5, 5, 10, 1700000205)", []).unwrap();
        // A row without an id still carries real counters; cumulative identity ignores row ids.
        db.execute("INSERT INTO api_usage_events VALUES (NULL, '/v1/chat', 'qwen2', 1, 1, 2, 1700000210)", []).unwrap();
        db.execute("INSERT INTO api_usage_events VALUES ('a3', '/v1/chat', NULL, 7, 7, 14, 1700000215)", []).unwrap();
        drop(db);
        let (records, notes) = read(&path, "unsloth", &AtomicBool::new(false)).unwrap();
        assert_eq!(records.len(), 2, "{records:?}");
        assert!(records.iter().all(|record| record.aggregate_timing));
        assert!(records.iter().all(|record| record.unclassified_tokens == 0));
        let chat = records.iter().find(|record| record.session == "t1").unwrap();
        assert_eq!(chat.model, "qwen3-8b"); // from responseDetails, else thread
        assert_eq!(chat.tally.input, 140); // (120-20-10) + (60-10)
        assert_eq!(chat.tally.output, 30);
        assert_eq!(chat.tally.cache_read, 10);
        assert_eq!(chat.tally.total(), 180);
        assert_eq!(chat.deduplication_id.as_deref(), Some(format!("unsloth:chat:{}:t1:qwen3-8b", chat.day).as_str()));
        let api = records.iter().find(|record| record.session == "unsloth:api").unwrap();
        assert_eq!(api.model, "qwen2");
        assert_eq!(api.tally.input, 36); // (40-10) + (10-5) + (2-1)
        assert_eq!(api.tally.output, 16);
        assert_eq!(api.tally.replies_without_cache_fields, 1);
        assert!(notes.iter().any(|note| note.contains("聊天记录缺少用量元信息")), "{notes:?}");
        assert!(notes.iter().any(|note| note.contains("接口用量记录缺少模型或有效时间")), "{notes:?}");
        let serialized = serde_json::to_string(&records).unwrap();
        assert!(!serialized.contains("SECRET"));
        let _ = fs::remove_dir_all(&home);
    }

    #[test]
    fn missing_schema_is_silent_and_cancellation_aborts() {
        let home = temp_dir("unsloth-schema");
        let path = home.join("studio.db");
        let db = studio(&path);
        db.execute("DROP TABLE api_usage_events", []).unwrap();
        db.execute("DROP TABLE chat_messages", []).unwrap();
        drop(db);
        let (records, notes) = read(&path, "unsloth", &AtomicBool::new(false)).unwrap();
        assert!(records.is_empty());
        assert!(notes.is_empty(), "{notes:?}");
        let cancelled = read(&path, "unsloth", &AtomicBool::new(true));
        assert_eq!(cancelled.unwrap_err(), "cancelled");
        let _ = fs::remove_dir_all(&home);
    }
}
