//! Pulse 1.9 ZCodeReader (Apache-2.0): `~/.zcode/projects/**/*.jsonl` records usage
//! per call, plus `~/.zcode/cli/db/db.sqlite` model_usage rows. A modern total already
//! contains the cache overlap, so it is subtracted; a bare total stays unclassified.
//! Legacy lines with no usage block are estimated-only and contribute nothing.

use super::measured_readers::{lines, record, text, timestamp};
use super::*;
use rusqlite::{types::ValueRef, Connection, OpenFlags};
use std::time::Instant;

const USAGE_KEYS: [&str; 11] = [
    "id",
    "session_id",
    "model_id",
    "started_at",
    "completed_at",
    "input_tokens",
    "output_tokens",
    "reasoning_tokens",
    "cache_read_input_tokens",
    "cache_creation_input_tokens",
    "computed_total_tokens",
];

pub(super) fn roots(home: &Path, id: &str, _env: &impl Fn(&str) -> Option<String>) -> Vec<PathBuf> {
    if id != "zcode" { return Vec::new(); }
    vec![
        home.join(".zcode").join("projects"),
        home.join(".zcode").join("cli").join("db").join("db.sqlite"),
    ]
}

pub(super) fn candidate(path: &Path, id: &str) -> bool {
    id == "zcode"
        && (path.file_name().is_some_and(|name| name == "db.sqlite")
            || path.extension().is_some_and(|extension| extension == "jsonl"))
}

pub(super) fn read(path: &Path, _id: &str, cancel: &AtomicBool) -> Result<(Vec<SpendRecord>, Vec<String>), String> {
    check(cancel)?;
    if path.file_name().is_some_and(|name| name == "db.sqlite") { return database(path, cancel); }
    jsonl(path, cancel)
}

struct Parts { tally: Tally, unclassified: u64 }

fn jsonl(path: &Path, cancel: &AtomicBool) -> Result<(Vec<SpendRecord>, Vec<String>), String> {
    let file = File::open(path).map_err(|_| "ZCode：会话记录无法读取。".to_string())?;
    let mut reader = BufReader::new(file);
    let mut notes = Vec::new();
    let mut out: Vec<SpendRecord> = Vec::new();
    let source = path.to_string_lossy().into_owned();
    let fallback_session = path.file_stem().map(|stem| stem.to_string_lossy().into_owned())
        .unwrap_or_else(|| source.clone());
    let mut dropped = false;
    lines(&mut reader, cancel, &mut notes, |row, index| {
        if out.len() >= MAX_RECORDS { return Err("ZCode：记录超过读取上限。".into()); }
        let mut parts = combine(row.get("usage"));
        if parts.tally.total() == 0 && parts.unclassified == 0 { parts = combine(row.get("token_usage")); }
        // A legacy line carries no usage at all: estimated-only, deliberately not reported.
        if parts.tally.total() == 0 && parts.unclassified == 0 { return Ok(()); }
        let Some(at) = timestamp(row.get("timestamp").unwrap_or(&Value::Null), true) else {
            dropped = true;
            return Ok(());
        };
        let Some(model) = model_id(text(row.get("model").unwrap_or(&Value::Null))) else {
            dropped = true;
            return Ok(());
        };
        let session = text(row.get("sessionId").unwrap_or(&Value::Null)).unwrap_or_else(|| fallback_session.clone());
        let identity = format!("zcode:{source}:{index}:{session}:{}", at.timestamp());
        out.push(record("zcode", model, at, session, parts.tally, parts.unclassified, Some(identity)));
        Ok(())
    })?;
    if dropped { notes.push("ZCode：会话记录有用量但缺少模型或有效时间，计数可能不完整。".into()); }
    notes.sort(); notes.dedup();
    check(cancel)?;
    Ok((out, notes))
}

/// EditorLogSupport.combine with `inputMayIncludeCache: true`: a total that equals
/// input+output while the cache buckets are still reported is the modern shape where
/// input already contains the cache overlap; subtract it. Elsewhere the buckets are
/// already disjoint.
fn combine(usage: Option<&Value>) -> Parts {
    let object = usage.filter(|value| value.is_object());
    let get = |keys: &[&str]| object.and_then(|value| first_count(value, keys));
    let input = get(&["input_tokens", "prompt_tokens", "inputTokens"]);
    let output = get(&["output_tokens", "completion_tokens", "outputTokens"]);
    let cache_read = get(&["input_cache_read", "cache_read_tokens", "cacheReadTokens"]);
    let cache_write = get(&["input_cache_creation", "cache_write_tokens", "cacheCreationTokens"]);
    let reasoning = get(&["reasoningTokens", "reasoning_tokens"]);
    let total = get(&["totalTokens", "total_tokens"]);
    if input.is_none() && output.is_none() && cache_read.is_none() && cache_write.is_none() && reasoning.is_none() {
        // A bare total names no kind: preserve the quantity, assign no split.
        return Parts { tally: Tally::default(), unclassified: total.unwrap_or(0) };
    }
    let reported_input = input.unwrap_or(0);
    let raw_output = output.unwrap_or(0);
    let cache_read = cache_read.unwrap_or(0);
    let cache_write = cache_write.unwrap_or(0);
    let reasoning = reasoning.unwrap_or(0);
    let disjoint = reported_input.saturating_add(raw_output).saturating_add(cache_read)
        .saturating_add(cache_write).saturating_add(reasoning);
    let inclusive = total == Some(reported_input.saturating_add(raw_output)) && total != Some(disjoint);
    let input = if inclusive {
        reported_input.saturating_sub(cache_read).saturating_sub(cache_write)
    } else {
        reported_input
    };
    let unclassified = total.map(|total| total.saturating_sub(disjoint)).unwrap_or(0);
    // Reasoning is only added back when the total proves it sits outside the output.
    let output = if total.is_some() && total == Some(disjoint) { raw_output.saturating_add(reasoning) } else { raw_output };
    Parts {
        tally: Tally { input, output, cache_write, cache_read, ..Default::default() },
        unclassified,
    }
}

fn database(path: &Path, cancel: &AtomicBool) -> Result<(Vec<SpendRecord>, Vec<String>), String> {
    let db = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX)
        .map_err(|_| "ZCode：数据库无法读取。".to_string())?;
    let _ = db.busy_timeout(Duration::from_millis(150));
    let deadline = Instant::now() + Duration::from_secs(15);
    db.progress_handler(1000, Some(move || Instant::now() > deadline));
    let mut notes = Vec::new();
    let mut out: Vec<SpendRecord> = Vec::new();
    let columns = table_columns(&db, "model_usage")?;
    if columns.is_empty() { return Ok((out, notes)); }
    let has_session = table_exists(&db, "session")?;
    let prefix = if has_session { "mu." } else { "" };
    let mut labels: Vec<&str> = Vec::new();
    let mut expressions: Vec<String> = Vec::new();
    for label in USAGE_KEYS {
        if columns.contains(label) {
            labels.push(label);
            expressions.push(format!("{prefix}{label}"));
        }
    }
    if has_session {
        labels.push("directory");
        expressions.push("s.directory".into());
        labels.push("path");
        expressions.push("s.path".into());
    }
    let sql = if has_session {
        format!("SELECT {} FROM model_usage mu LEFT JOIN session s ON s.id = mu.session_id", expressions.join(", "))
    } else {
        format!("SELECT {} FROM model_usage", expressions.join(", "))
    };
    let index = |label: &str| labels.iter().position(|candidate| *candidate == label);
    let (id, session_id, model, started, completed, input, output, cache_read, cache_write, computed) = (
        index("id"), index("session_id"), index("model_id"), index("started_at"), index("completed_at"),
        index("input_tokens"), index("output_tokens"), index("cache_read_input_tokens"),
        index("cache_creation_input_tokens"), index("computed_total_tokens"),
    );
    let directory = if has_session { Some(labels.len() - 2) } else { None };
    let ancestor = if has_session { Some(labels.len() - 1) } else { None };
    let source = path.to_string_lossy().into_owned();
    let mut statement = db.prepare(&sql).map_err(|error| format!("ZCode：数据库读取失败（{error}）。"))?;
    let mut rows = statement.query([]).map_err(|error| format!("ZCode：数据库读取失败（{error}）。"))?;
    let mut dropped = false;
    while let Some(row) = rows.next().map_err(|error| format!("ZCode：数据库读取失败（{error}）。"))? {
        check(cancel)?;
        if out.len() >= MAX_RECORDS { return Err("ZCode：记录超过读取上限。".into()); }
        let started = field(row, started).and_then(|raw| timestamp(&Value::String(raw), true));
        let completed = field(row, completed).and_then(|raw| timestamp(&Value::String(raw), true));
        let raw_input = field(row, input).map(db_count).unwrap_or(0);
        let raw_output = field(row, output).map(db_count).unwrap_or(0);
        let cache_read = field(row, cache_read).map(db_count).unwrap_or(0);
        let cache_write = field(row, cache_write).map(db_count).unwrap_or(0);
        let computed = field(row, computed).map(db_count).unwrap_or(0);
        let reported = raw_input.saturating_add(raw_output).saturating_add(cache_read)
            .saturating_add(cache_write).saturating_add(computed);
        if reported == 0 { continue; }
        let Some(at) = started.or(completed) else { dropped = true; continue; };
        // Swift drops a row without a session id; PulseWin falls back to a stable file hash.
        let session = field(row, session_id).filter(|raw| !raw.trim().is_empty())
            .unwrap_or_else(|| format!("{:016x}", hash(&source)));
        let model = model_id(field(row, model)).unwrap_or_else(|| "auto".into());
        let fresh_input = raw_input.saturating_sub(cache_read).saturating_sub(cache_write);
        let unclassified = computed.saturating_sub(raw_input.saturating_add(raw_output));
        let tally = Tally { input: fresh_input, cache_write, cache_read, output: raw_output, ..Default::default() };
        if tally.total() == 0 && unclassified == 0 { continue; }
        let identifier = field(row, id)
            .unwrap_or_else(|| format!("{session}#{}", started.map(|at| at.timestamp()).unwrap_or(0)));
        let project = field(row, directory).or_else(|| field(row, ancestor));
        let mut record = record("zcode", model, at, session, tally, unclassified, Some(format!("zcode:{source}:{identifier}")));
        record.project = project;
        out.push(record);
    }
    if dropped { notes.push("ZCode：数据库中有用量记录缺少有效时间，计数可能不完整。".into()); }
    notes.sort(); notes.dedup();
    check(cancel)?;
    Ok((out, notes))
}

fn table_columns(db: &Connection, table: &str) -> Result<HashSet<String>, String> {
    let mut statement = db.prepare(&format!("PRAGMA table_info({table})"))
        .map_err(|error| format!("ZCode：数据库表结构无法读取（{error}）。"))?;
    let mut rows = statement.query([]).map_err(|error| format!("ZCode：数据库表结构无法读取（{error}）。"))?;
    let mut columns = HashSet::new();
    while let Some(row) = rows.next().map_err(|error| format!("ZCode：数据库表结构无法读取（{error}）。"))? {
        if let Ok(name) = row.get::<_, String>(1) { columns.insert(name); }
    }
    Ok(columns)
}

fn table_exists(db: &Connection, table: &str) -> Result<bool, String> {
    let mut statement = db.prepare(&format!("PRAGMA table_info({table})"))
        .map_err(|error| format!("ZCode：数据库表结构无法读取（{error}）。"))?;
    let mut rows = statement.query([]).map_err(|error| format!("ZCode：数据库表结构无法读取（{error}）。"))?;
    Ok(rows.next().map_err(|error| format!("ZCode：数据库表结构无法读取（{error}）。"))?.is_some())
}

/// AgentSQLite.text converts any column type to its SQLite text form (NULL stays nil).
fn field(row: &rusqlite::Row, index: Option<usize>) -> Option<String> {
    let index = index?;
    match row.get_ref(index).ok()? {
        ValueRef::Null => None,
        ValueRef::Integer(number) => Some(number.to_string()),
        ValueRef::Real(number) => Some(format!("{number}")),
        ValueRef::Text(bytes) | ValueRef::Blob(bytes) => Some(String::from_utf8_lossy(bytes).into_owned()),
    }
}

/// AgentLogIO.count on a text column: only a whole, non-negative integer reads.
fn db_count(raw: String) -> u64 {
    raw.trim().parse::<i64>().ok().filter(|number| *number >= 0).map(|number| number as u64).unwrap_or(0)
}

/// EditorLog.firstCount: the first positive count, otherwise a present zero.
fn first_count(object: &Value, keys: &[&str]) -> Option<u64> {
    let mut zero = None;
    for key in keys {
        if let Some(number) = whole_count(object.get(*key)) {
            if number > 0 { return Some(number); }
            zero = Some(0);
        }
    }
    zero
}

/// AgentLogIO.count: a whole, non-negative count, or nil. nil is not zero.
fn whole_count(value: Option<&Value>) -> Option<u64> {
    let value = value?;
    if value.is_boolean() { return None; }
    if let Some(number) = value.as_u64() { return (number <= i64::MAX as u64).then_some(number); }
    if let Some(number) = value.as_f64() {
        if number.is_finite() && number >= 0.0 && number.fract() == 0.0 && number < 9_223_372_036_854_775_808.0 {
            return Some(number as u64);
        }
    }
    if let Some(raw) = value.as_str() {
        if let Ok(number) = raw.trim().parse::<u64>() { return (number <= i64::MAX as u64).then_some(number); }
    }
    None
}

/// EditorLog.modelID: strip the provider prefix through the last slash.
fn model_id(raw: Option<String>) -> Option<String> {
    let raw = raw?;
    let raw = raw.trim();
    if raw.is_empty() { return None; }
    let tail = raw.rsplit('/').next().unwrap_or(raw);
    Some(if tail.is_empty() { raw.to_owned() } else { tail.to_owned() })
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

    #[test]
    fn roots_keep_projects_and_database() {
        let home = temp_dir("zcode-home");
        assert_eq!(
            roots(&home, "zcode", &|_: &str| None),
            vec![home.join(".zcode").join("projects"), home.join(".zcode").join("cli").join("db").join("db.sqlite")]
        );
        assert!(roots(&home, "reasonix", &|_: &str| None).is_empty());
        let _ = fs::remove_dir_all(&home);
    }

    #[test]
    fn candidate_accepts_jsonl_and_database() {
        assert!(candidate(Path::new("C:\\zcode\\projects\\a\\b.jsonl"), "zcode"));
        assert!(candidate(Path::new("C:\\zcode\\cli\\db\\db.sqlite"), "zcode"));
        assert!(!candidate(Path::new("C:\\zcode\\cli\\db\\other.sqlite"), "zcode"));
        assert!(!candidate(Path::new("C:\\zcode\\projects\\a\\b.txt"), "zcode"));
        assert!(!candidate(Path::new("C:\\zcode\\projects\\a\\b.jsonl"), "lmstudio"));
    }

    #[test]
    fn jsonl_subtracts_cache_overlap_and_leaves_bare_totals_unclassified() {
        let home = temp_dir("zcode-jsonl");
        let path = home.join("session.jsonl");
        fs::write(&path, [
            r#"{"timestamp":1700000000000,"model":"anthropic/claude-x","sessionId":"s1","usage":{"input_tokens":100,"output_tokens":20,"input_cache_read":40,"input_cache_creation":10,"totalTokens":120}}"#,
            r#"{"timestamp":1700000100000,"model":"gpt-5","usage":{"totalTokens":30}}"#,
            r#"{"timestamp":1700000200000,"model":"gpt-5"}"#,
            r#"{"timestamp":1700000300000,"model":"gpt-5","usage":{},"token_usage":{"prompt_tokens":7,"completion_tokens":3}}"#,
            r#"{"model":"gpt-5","usage":{"input_tokens":5,"output_tokens":1}}"#,
            "{not json",
        ].join("\n")).unwrap();
        let (records, notes) = read(&path, "zcode", &AtomicBool::new(false)).unwrap();
        assert_eq!(records.len(), 3, "{records:?}");
        let first = &records[0];
        assert_eq!(first.model, "claude-x"); // provider prefix stripped
        assert_eq!(first.session, "s1");
        assert_eq!(first.tally.input, 50); // 100 input - 40 read - 10 write
        assert_eq!(first.tally.output, 20);
        assert_eq!(first.tally.cache_read, 40);
        assert_eq!(first.tally.cache_write, 10);
        assert_eq!(first.tally.total(), 120);
        assert_eq!(first.unclassified_tokens, 0);
        assert_eq!(first.source_timestamp, Some(1_700_000_000));
        assert!(!first.aggregate_timing);
        let bare = &records[1];
        assert_eq!(bare.tally, Tally::default());
        assert_eq!(bare.unclassified_tokens, 30);
        let fallback = &records[2];
        assert_eq!(fallback.tally.input, 7); // token_usage retry
        assert_eq!(fallback.tally.output, 3);
        assert_eq!(fallback.session, "session"); // file stem
        assert!(records.iter().all(|record| record.deduplication_id.as_deref().is_some_and(|id| id.starts_with("zcode:"))));
        assert!(notes.iter().any(|note| note.contains("缺少模型或有效时间")), "{notes:?}");
        assert!(notes.iter().any(|note| note.contains("无法解码")), "{notes:?}");
        let serialized = serde_json::to_string(&records).unwrap();
        assert!(!serialized.contains("SECRET"));
        let cancelled = read(&path, "zcode", &AtomicBool::new(true)).unwrap_err();
        assert_eq!(cancelled, "cancelled");
        let _ = fs::remove_dir_all(&home);
    }

    fn create_database(path: &Path, session_table: bool) {
        let db = Connection::open(path).unwrap();
        db.execute_batch(
            "CREATE TABLE model_usage (id TEXT, session_id TEXT, model_id TEXT, started_at INTEGER, completed_at INTEGER, \
             input_tokens INTEGER, output_tokens INTEGER, reasoning_tokens INTEGER, cache_read_input_tokens INTEGER, \
             cache_creation_input_tokens INTEGER, computed_total_tokens INTEGER);",
        ).unwrap();
        if session_table {
            db.execute_batch("CREATE TABLE session (id TEXT, directory TEXT, path TEXT);").unwrap();
            db.execute("INSERT INTO session (id, directory, path) VALUES ('s1', 'C:\\project', 'C:\\project\\main.rs')", []).unwrap();
        }
    }

    #[test]
    fn database_maps_counters_and_keeps_project() {
        let home = temp_dir("zcode-db");
        let path = home.join("db.sqlite");
        create_database(&path, true);
        let db = Connection::open(&path).unwrap();
        db.execute(
            "INSERT INTO model_usage VALUES ('r1', 's1', 'anthropic/claude-x', 1700000000000, NULL, 100, 20, 5, 40, 10, 200)",
            [],
        ).unwrap();
        db.execute("INSERT INTO model_usage VALUES (NULL, 's1', NULL, NULL, 1700001000000, 10, 5, 0, 0, 0, 15)", []).unwrap();
        db.execute("INSERT INTO model_usage VALUES ('r3', 's1', 'gpt-5', NULL, NULL, 5, 1, 0, 0, 0, NULL)", []).unwrap();
        drop(db);
        let (records, notes) = read(&path, "zcode", &AtomicBool::new(false)).unwrap();
        assert_eq!(records.len(), 2, "{records:?}");
        let first = records.iter().find(|record| record.deduplication_id.as_deref().is_some_and(|id| id.ends_with(":r1"))).unwrap();
        assert_eq!(first.model, "claude-x");
        assert_eq!(first.tally.input, 50); // 100 - 40 - 10
        assert_eq!(first.tally.output, 20);
        assert_eq!(first.tally.cache_read, 40);
        assert_eq!(first.tally.cache_write, 10);
        assert_eq!(first.unclassified_tokens, 80); // computed 200 > 120
        assert_eq!(first.project.as_deref(), Some("C:\\project"));
        assert_eq!(first.source_timestamp, Some(1_700_000_000));
        assert_eq!(first.deduplication_id.as_deref(), Some(format!("zcode:{}:r1", path.display()).as_str()));
        let second = records.iter().find(|record| record.model == "auto").unwrap();
        assert_eq!(second.tally.input, 10);
        assert_eq!(second.tally.output, 5);
        assert_eq!(second.unclassified_tokens, 0);
        assert_eq!(second.source_timestamp, Some(1_700_001_000));
        assert!(notes.iter().any(|note| note.contains("缺少有效时间")), "{notes:?}");
        let _ = fs::remove_dir_all(&home);
    }

    #[test]
    fn database_without_session_table_uses_a_stable_session_hash() {
        let home = temp_dir("zcode-db-plain");
        let path = home.join("db.sqlite");
        create_database(&path, false);
        let db = Connection::open(&path).unwrap();
        db.execute("INSERT INTO model_usage VALUES ('r1', NULL, 'gpt-5', 1700000000000, NULL, 10, 2, 0, 0, 0, 12)", []).unwrap();
        drop(db);
        let (records, _) = read(&path, "zcode", &AtomicBool::new(false)).unwrap();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].session, format!("{:016x}", hash(path.to_string_lossy().as_ref())));
        assert_eq!(records[0].tally.total(), 12);
        let _ = fs::remove_dir_all(&home);
    }
}
