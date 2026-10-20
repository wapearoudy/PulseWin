//! Pulse 1.9 DevinDesktopReader (Apache-2.0): Devin Desktop's ACP event
//! captures, `*.ndjson` under the app's `User/acp-events` directory, with the
//! CLI's session database read only as a **lookup** and, where its usage is
//! counted separately, a source-precedence check.
//!
//! **Two shapes, told apart by evidence, not a version marker.** The canonical
//! ACP `usage_update` carries its figures under `notification._meta`, keys
//! prefixed `cognition.ai/`: `inputTokens` is the complete prompt **including**
//! `cachedReadTokens`, output accumulates per step, and the cached buckets
//! overwrite. One aggregate record is emitted per file — fresh input is
//! `inputTokens − cachedReadTokens`, output is the summed output, cache read and
//! write are the last reported values. A capture with no `_meta` counters — an
//! older or mislabelled event — falls back to a usage object under the metadata
//! locations and emits **one record per metric-bearing event**, keyed by its
//! line index because no other identity exists.
//!
//! **The database supplies identity, not additional tokens on this route.** Its
//! `sessions.title` maps to `{id, model, working_directory}` so a capture whose
//! file name is an unrelated UUID recovers a stable session id, model and
//! workspace. A title two sessions share is ambiguous and resolves to nothing.
//! When the catalogue also counts that database's messages, a capture matched to
//! one of its counted sessions is excluded as a mirror rather than counted
//! twice; only the Devin CLI's own database can suppress a capture, and the test
//! is that reader's actual message parser, not the existence of a metadata row.
//!
//! `reasoning` is never reported here, so it stays zero. A missing timestamp is
//! skipped rather than filled from the file's modification date, and `adaptive`
//! — a routing mode, not a model — is never a model name.
//!
//! **The database is not a root.** The original resolves it from the injected
//! home; here it is found from the capture's own tree — the CLI's home is the
//! directory whose `.local/share/devin/cli/sessions.db` exists above the
//! `acp-events` folder — so a capture carries its own lookup and the module
//! keeps no state between files. A `%APPDATA%` copy is metadata only.

use super::measured_readers::{count, record, text};
use super::*;

pub(super) fn roots(home: &Path, id: &str, env: &impl Fn(&str) -> Option<String>) -> Vec<PathBuf> {
    if id != "devin-desktop" { return Vec::new(); }
    let appdata = env("APPDATA").filter(|value| !value.trim().is_empty()).map(|value| PathBuf::from(value.trim()));
    // The capture trees only, in the original's order. `~/Library/Application
    // Support` is the macOS spelling; on Windows the same directory is
    // `%APPDATA%`, so both are named and existence decides. The borrowed CLI
    // database is deliberately absent: its existence alone is not evidence of
    // Desktop, and a machine with only the CLI must not report a desktop agent
    // with nothing to show.
    let mut roots = Vec::new();
    if let Some(appdata) = &appdata { roots.push(appdata.join("Devin/User/acp-events")); }
    roots.push(home.join("Library/Application Support/Devin/User/acp-events"));
    roots.push(home.join(".config/devin/User/acp-events"));
    roots
}

pub(super) fn candidate(path: &Path, id: &str) -> bool {
    id == "devin-desktop" && path.extension().is_some_and(|extension| extension == "ndjson")
}

pub(super) fn read(path: &Path, _id: &str, cancel: &AtomicBool) -> Result<(Vec<SpendRecord>, Vec<String>), String> {
    check(cancel)?;
    let lookup = lookup(path, cancel);
    let file = File::open(path).map_err(|error| format!("Devin Desktop：捕获文件无法读取（{error}）。"))?;
    let mut reader = BufReader::with_capacity(64 * 1024, file);
    let mut notes = Vec::new();
    let source = path.to_string_lossy().into_owned();
    let stem = path.file_stem().map(|name| name.to_string_lossy().into_owned()).unwrap_or_default();
    let mut title = None::<String>;
    let mut out: Vec<SpendRecord> = Vec::new();
    // The canonical aggregate across the file.
    let mut latest_input = None::<u64>;
    let mut latest_read = 0u64;
    let mut latest_write = 0u64;
    let mut summed_output = 0u64;
    let mut model = None::<String>;
    let mut timestamp = None::<DateTime<Utc>>;
    measured_readers::lines(&mut reader, cancel, &mut notes, |row, ordinal| {
        let notification = &row["notification"];
        if !notification.is_object() { return Ok(()); }
        if notification["sessionUpdate"].as_str() == Some("session_info_update") {
            if let Some(stated) = text(&notification["title"]) { title = Some(stated); }
        }
        if let Some(canonical) = canonical(notification) {
            // `inputTokens` is the complete prompt, so it overwrites; the cached
            // buckets overwrite too; output is summed per step.
            if let Some(input) = canonical.input { latest_input = Some(input); }
            if let Some(read) = canonical.cache_read { latest_read = read; }
            if let Some(write) = canonical.cache_write { latest_write = write; }
            summed_output = summed_output.saturating_add(canonical.output);
            if let Some(hint) = model_hint(notification, &canonical.meta) { model = Some(hint); }
            if canonical.timestamp.is_some() { timestamp = canonical.timestamp; }
            return Ok(());
        }
        // A non-canonical event: an older capture that kept its usage under the
        // metadata locations. One record per event.
        let Some((usage, content_metadata, metadata)) = legacy(notification) else { return Ok(()); };
        let tally = Tally {
            input: count(&usage["input_tokens"]),
            cache_write: count(&usage["cache_creation_tokens"]),
            cache_read: count(&usage["cache_read_tokens"]),
            output: count(&usage["output_tokens"]),
            ..Default::default()
        };
        if tally.total() == 0 { return Ok(()); }
        let at = content_metadata.and_then(|value| flexible(&value["created_at"]))
            .or_else(|| metadata.and_then(|value| flexible(&value["created_at"])))
            .or_else(|| flexible(&notification["created_at"]))
            .or_else(|| flexible(&notification["timestamp"]));
        // A capture with no stated time cannot be placed; it is skipped, never
        // dated from the file.
        let Some(at) = at else { return Ok(()); };
        let hinted = content_metadata.and_then(|value| text(&value["generation_model"]))
            .or_else(|| metadata.and_then(|value| text(&value["generation_model"])))
            .or_else(|| notification["_meta"].as_object().and_then(|meta| text(&meta["cognition.ai/model"])));
        let resolved = resolve(title.as_deref(), &lookup);
        // A capture the native ledger already counts is a mirror, not work.
        if resolved.is_some_and(|session| session.has_counted_usage) { return Ok(()); }
        if out.len() >= MAX_RECORDS { return Err("Devin Desktop：记录超过读取上限。".into()); }
        let mut spent = record(
            "devin-desktop", model_name(hinted.or_else(|| resolved.and_then(|session| session.model.clone()))), at,
            resolved.map(|session| session.id.clone()).unwrap_or_else(|| stem.clone()), tally, 0,
            Some(format!("devin-desktop:{source}:{}", ordinal - 1)),
        );
        spent.project = resolved.and_then(|session| session.directory.clone());
        out.push(spent);
        Ok(())
    })?;
    if let Some(input) = latest_input {
        let tally = Tally {
            input: input.saturating_sub(latest_read), cache_write: latest_write, cache_read: latest_read,
            output: summed_output, ..Default::default()
        };
        // No stated time and no counters are both reasons not to emit, and
        // neither is a reason to guess one.
        if let Some(at) = timestamp.filter(|_| tally.total() > 0) {
            let resolved = resolve(title.as_deref(), &lookup);
            if !resolved.is_some_and(|session| session.has_counted_usage) {
                if out.len() >= MAX_RECORDS { return Err("Devin Desktop：记录超过读取上限。".into()); }
                let mut spent = record(
                    "devin-desktop", model_name(model.or_else(|| resolved.and_then(|session| session.model.clone()))),
                    at, resolved.map(|session| session.id.clone()).unwrap_or_else(|| stem.clone()), tally, 0,
                    Some(format!("devin-desktop:{source}:usage")),
                );
                spent.project = resolved.and_then(|session| session.directory.clone());
                spent.aggregate_timing = true;
                out.push(spent);
            }
        }
    }
    notes.sort();
    notes.dedup();
    check(cancel)?;
    Ok((out, notes))
}

/// The title lookup this capture can reach: the CLI database in the home its own
/// tree sits under. Walking up is what lets a capture be read on its own, with
/// no state carried from `roots`.
fn lookup(path: &Path, cancel: &AtomicBool) -> HashMap<String, Vec<super::devin_reader::Lookup>> {
    let mut table: HashMap<String, Vec<super::devin_reader::Lookup>> = HashMap::new();
    for ancestor in path.ancestors().skip(1).take(8) {
        // The CLI's own database is authoritative and can exclude a mirror.
        let authoritative = ancestor.join(".local/share/devin/cli/sessions.db");
        if authoritative.is_file() {
            table = super::devin_reader::lookup(&authoritative, cancel);
            break;
        }
        // A `%APPDATA%` copy is metadata only: it names a session but cannot
        // suppress a record.
        let roaming = ancestor.join("devin/cli/sessions.db");
        if ancestor.join("Devin/User/acp-events").is_dir() && roaming.is_file() {
            for (title, mut sessions) in super::devin_reader::lookup(&roaming, cancel) {
                for session in &mut sessions { session.has_counted_usage = false; }
                table.entry(title).or_insert_with(Vec::new).extend(sessions);
            }
        }
    }
    table
}

struct Canonical {
    input: Option<u64>,
    cache_read: Option<u64>,
    cache_write: Option<u64>,
    output: u64,
    timestamp: Option<DateTime<Utc>>,
    meta: Value,
}

/// The canonical ACP usage under `notification._meta`, or nil when the event
/// carries none of its counters.
fn canonical(notification: &Value) -> Option<Canonical> {
    if notification["sessionUpdate"].as_str() != Some("usage_update") { return None; }
    let meta = notification["_meta"].as_object()?;
    let meta = Value::Object(meta.clone());
    let field = |name: &str| counted(&meta[&format!("cognition.ai/{name}")]);
    let (input, cache_read, cache_write, output) =
        (field("inputTokens"), field("cachedReadTokens"), field("cachedWriteTokens"), field("outputTokens"));
    if input.is_none() && cache_read.is_none() && cache_write.is_none() && output.is_none() { return None; }
    Some(Canonical {
        input, cache_read, cache_write, output: output.unwrap_or(0),
        timestamp: flexible(&notification["created_at"]), meta,
    })
}

/// The legacy shape: the first candidate usage object carrying any of the four
/// underscore-named counters, plus the metadata objects the model and time are
/// read from.
fn legacy(notification: &Value) -> Option<(&Value, Option<&Value>, Option<&Value>)> {
    let content = &notification["content"];
    let content_metadata = content["metadata"].as_object().map(|_| &content["metadata"]);
    let metadata = notification["metadata"].as_object().map(|_| &notification["metadata"]);
    let candidates = [
        content_metadata.and_then(|value| value["metrics"].as_object().map(|_| &value["metrics"])),
        metadata.and_then(|value| value["metrics"].as_object().map(|_| &value["metrics"])),
        notification["metrics"].as_object().map(|_| &notification["metrics"]),
        content_metadata,
        metadata,
    ];
    for usage in candidates.into_iter().flatten() {
        let has_tokens = ["input_tokens", "output_tokens", "cache_read_tokens", "cache_creation_tokens"]
            .iter().any(|key| counted(&usage[*key]).is_some());
        if has_tokens { return Some((usage, content_metadata, metadata)); }
    }
    None
}

/// The model hint priority for the canonical shape.
fn model_hint(notification: &Value, meta: &Value) -> Option<String> {
    text(&notification["notification_model"]).or_else(|| text(&meta["cognition.ai/model"]))
}

/// `adaptive` is a routing mode, not a model, and an unresolved file gets
/// `unknown` — an unpriced name — rather than a concrete model invented for the
/// product, which would be priced at the wrong rate.
fn model_name(value: Option<String>) -> String {
    match value {
        Some(value) if value != "adaptive" => value,
        _ => "unknown".into(),
    }
}

/// The CLI session whose title is this file's, when exactly one matches. A title
/// held by more than one session is ambiguous and is not an identity.
fn resolve<'a>(title: Option<&str>, lookup: &'a HashMap<String, Vec<super::devin_reader::Lookup>>) -> Option<&'a super::devin_reader::Lookup> {
    let matches = lookup.get(title?)?;
    (matches.len() == 1).then(|| &matches[0])
}

/// AgentLogIO.count: a whole, non-negative count, or nil. nil is not zero, so a
/// missing field stays missing rather than being counted as a reported zero.
fn counted(value: &Value) -> Option<u64> {
    if value.is_boolean() { return None; }
    if let Some(number) = value.as_u64() { return (number <= i64::MAX as u64).then_some(number); }
    if let Some(number) = value.as_f64() {
        if number.is_finite() && number >= 0. && number.fract() == 0. && number < 9_223_372_036_854_775_808. {
            return Some(number as u64);
        }
    }
    if let Some(raw) = value.as_str() { return raw.trim().parse::<u64>().ok().filter(|number| *number <= i64::MAX as u64); }
    None
}

/// DatabaseReaderSupport.flexible: an ISO 8601 string, or a numeric epoch in
/// seconds or milliseconds. A numeric string is read as an epoch **before** the
/// ISO parser sees it, so `"1789372800000"` lands in the millisecond it is
/// rather than the year 58,000.
fn flexible(value: &Value) -> Option<DateTime<Utc>> {
    if let Some(raw) = value.as_str() {
        let trimmed = raw.trim();
        if trimmed.is_empty() { return None; }
        if let Ok(number) = trimmed.parse::<f64>() { return epoch(number); }
        return DateTime::parse_from_rfc3339(trimmed).ok().map(|at| at.with_timezone(&Utc));
    }
    if value.is_boolean() { return None; }
    value.as_f64().and_then(epoch)
}

/// A numeric epoch in seconds or milliseconds; zero and negatives are nil — an
/// unset field is not 1970, and a record must not be bucketed on a date nobody
/// wrote.
fn epoch(raw: f64) -> Option<DateTime<Utc>> {
    if !raw.is_finite() || raw <= 0. { return None; }
    let seconds = if raw < 1e12 { raw } else { raw / 1000. };
    if !seconds.is_finite() || seconds <= 0. { return None; }
    let whole = seconds.trunc();
    if whole >= i64::MAX as f64 { return None; }
    DateTime::from_timestamp(whole as i64, ((seconds - whole) * 1e9).round().clamp(0., 999_999_999.) as u32)
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::Connection;
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

    fn home() -> PathBuf { temp_dir("devin-desktop-home") }

    /// A capture file under the macOS application-support tree, plus the CLI
    /// database the reader borrows for identity.
    fn capture(home: &Path, name: &str, lines: &[&str]) -> PathBuf {
        let directory = home.join("Library/Application Support/Devin/User/acp-events");
        fs::create_dir_all(&directory).unwrap();
        let path = directory.join(format!("{name}.ndjson"));
        fs::write(&path, lines.join("\n")).unwrap();
        path
    }

    fn database(home: &Path, statements: &[&str]) {
        let path = home.join(".local/share/devin/cli/sessions.db");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        let db = Connection::open(&path).unwrap();
        for statement in statements { db.execute_batch(statement).unwrap(); }
        drop(db);
    }

    const SCHEMA: &str = "CREATE TABLE sessions (id TEXT, title TEXT, model TEXT, working_directory TEXT);";

    fn canonical_event(at: &str, input: u64, read: u64, write: u64, output: u64) -> String {
        format!(r#"{{"notification":{{"sessionUpdate":"usage_update","created_at":"{at}","notification_model":"priced","_meta":{{"cognition.ai/inputTokens":{input},"cognition.ai/cachedReadTokens":{read},"cognition.ai/cachedWriteTokens":{write},"cognition.ai/outputTokens":{output}}}}}}}"#)
    }

    #[test]
    fn roots_name_the_capture_trees_and_candidate_takes_ndjson_only() {
        let home = home();
        let env = |_: &str| None;
        assert_eq!(roots(&home, "devin-desktop", &env), vec![
            home.join("Library/Application Support/Devin/User/acp-events"),
            home.join(".config/devin/User/acp-events"),
        ]);
        // On Windows the same application-support tree is `%APPDATA%`.
        assert_eq!(roots(&home, "devin-desktop", &|key: &str| (key == "APPDATA").then(|| "C:/appdata".into())), vec![
            PathBuf::from("C:/appdata/Devin/User/acp-events"),
            home.join("Library/Application Support/Devin/User/acp-events"),
            home.join(".config/devin/User/acp-events"),
        ]);
        assert!(roots(&home, "devin-cli", &env).is_empty());
        assert!(candidate(Path::new("C:\\Users\\me\\AppData\\Roaming\\Devin\\User\\acp-events\\e.ndjson"), "devin-desktop"));
        assert!(!candidate(Path::new("a/e.jsonl"), "devin-desktop"));
        assert!(!candidate(Path::new("a/sessions.db"), "devin-desktop"));
        let _ = fs::remove_dir_all(&home);
    }

    #[test]
    fn the_cli_database_is_a_lookup_and_not_evidence_of_desktop() {
        let home = home();
        database(&home, &[SCHEMA, "INSERT INTO sessions VALUES ('d1','Fix the ring','priced','/Users/me/Code/Pulse')"]);
        // The database alone makes no capture tree, so Desktop declares no root
        // and cannot be reported as present.
        assert!(roots(&home, "devin-desktop", &|_: &str| None).iter().all(|root| !root.exists()));
        let _ = fs::remove_dir_all(&home);
    }

    #[test]
    fn canonical_usage_aggregates_once_per_file() {
        let home = home();
        database(&home, &[SCHEMA, "INSERT INTO sessions VALUES ('d1','Fix the ring','priced','/Users/me/Code/Pulse')"]);
        let path = capture(&home, "event", &[
            r#"{"notification":{"sessionUpdate":"session_info_update","title":"Fix the ring"}}"#,
            &canonical_event("2026-09-15T12:00:00Z", 100, 30, 20, 10),
            &canonical_event("2026-09-15T12:01:00Z", 150, 40, 25, 5),
        ]);
        let (records, notes) = read(&path, "devin-desktop", &AtomicBool::new(false)).unwrap();
        assert!(notes.is_empty(), "{notes:?}");
        assert_eq!(records.len(), 1, "{records:?}");
        let spent = &records[0];
        // latest input 150 − read 40 = 110 fresh; summed output 10 + 5 = 15.
        assert_eq!(spent.tally, Tally { input: 110, cache_write: 25, cache_read: 40, output: 15, ..Default::default() });
        assert_eq!(spent.session, "d1"); // resolved by the unambiguous title
        assert_eq!(spent.project.as_deref(), Some("/Users/me/Code/Pulse"));
        assert!(spent.aggregate_timing);
        assert_eq!(spent.model, "priced");
        assert_eq!(spent.source_timestamp, Some(1_789_473_660)); // 12:01Z
        assert_eq!(spent.deduplication_id.as_deref(), Some(format!("devin-desktop:{}:usage", path.display()).as_str()));
        let _ = fs::remove_dir_all(&home);
    }

    #[test]
    fn legacy_events_are_one_record_each_keyed_by_line_index() {
        let home = home();
        let path = capture(&home, "legacy", &[
            r#"{"notification":{"content":{"metadata":{"metrics":{"input_tokens":100,"output_tokens":10,"cache_read_tokens":5,"cache_creation_tokens":2},"generation_model":"priced","created_at":"2026-09-15T12:00:00Z"}}}}"#,
            r#"{"notification":{"sessionUpdate":"usage_update","created_at":"2026-09-15T12:01:00Z","content":{"metadata":{"metrics":{"input_tokens":7,"output_tokens":1}}}}}"#,
        ]);
        let (records, notes) = read(&path, "devin-desktop", &AtomicBool::new(false)).unwrap();
        assert!(notes.is_empty(), "{notes:?}");
        assert_eq!(records.len(), 2, "{records:?}");
        // The legacy shape keeps the four counters as reported: input is not
        // reduced by the cache read here.
        assert_eq!(records[0].tally, Tally { input: 100, cache_write: 2, cache_read: 5, output: 10, ..Default::default() });
        assert_eq!(records[0].model, "priced");
        assert!(!records[0].aggregate_timing);
        assert_eq!(records[0].session, "legacy"); // no lookup, so the file stem
        assert_eq!(records[0].deduplication_id.as_deref(), Some(format!("devin-desktop:{}:0", path.display()).as_str()));
        assert_eq!(records[1].deduplication_id.as_deref(), Some(format!("devin-desktop:{}:1", path.display()).as_str()));
        let _ = fs::remove_dir_all(&home);
    }

    #[test]
    fn a_capture_with_no_usage_contributes_nothing_and_adaptive_is_not_a_model() {
        let home = home();
        database(&home, &[SCHEMA,
            "INSERT INTO sessions VALUES ('d1','Fix the ring','priced','/a')",
            "INSERT INTO sessions VALUES ('d2','Fix the ring','priced','/b')",
        ]);
        let path = capture(&home, "cloudy", &[
            r#"{"notification":{"sessionUpdate":"session_info_update","title":"Fix the ring"}}"#,
            // A cloud event with no usage at all.
            r#"{"notification":{"sessionUpdate":"usage_update","created_at":"2026-09-15T12:00:00Z"}}"#,
            r#"{"notification":{"sessionUpdate":"usage_update","created_at":"2026-09-15T12:00:00Z","notification_model":"adaptive","_meta":{"cognition.ai/inputTokens":50,"cognition.ai/outputTokens":3}}}"#,
        ]);
        let (records, _) = read(&path, "devin-desktop", &AtomicBool::new(false)).unwrap();
        assert_eq!(records.len(), 1, "{records:?}");
        // No recoverable model, and `adaptive` is a routing mode: the record gets
        // an unpriced `unknown` rather than an invented product model.
        assert_eq!(records[0].model, "unknown");
        // The ambiguous title is not used as a session id; the file stem is.
        assert_eq!(records[0].session, "cloudy");
        assert_eq!(records[0].project, None);
        assert_eq!(records[0].tally, Tally { input: 50, output: 3, ..Default::default() });
        let _ = fs::remove_dir_all(&home);
    }

    #[test]
    fn a_counted_native_session_suppresses_both_capture_shapes() {
        let home = home();
        let message = r#"{"message_id":"m1","metadata":{"generation_model":"priced","metrics":{"input_tokens":100,"output_tokens":0}}}"#;
        database(&home, &[SCHEMA,
            "CREATE TABLE message_nodes (session_id TEXT, chat_message TEXT, created_at INTEGER);",
            "INSERT INTO sessions VALUES ('shared','Shared task','priced','/work/review')",
            &format!("INSERT INTO message_nodes VALUES ('shared','{message}',1789560000)"),
        ]);
        let canonical = capture(&home, "canonical", &[
            r#"{"notification":{"sessionUpdate":"session_info_update","title":"Shared task"}}"#,
            &canonical_event("2026-09-15T12:00:00Z", 100, 0, 0, 0),
        ]);
        let legacy = capture(&home, "legacy", &[
            r#"{"notification":{"sessionUpdate":"session_info_update","title":"Shared task"}}"#,
            r#"{"notification":{"content":{"metadata":{"metrics":{"input_tokens":100},"generation_model":"priced","created_at":"2026-09-15T12:00:00Z"}}}}"#,
        ]);
        assert!(read(&canonical, "devin-desktop", &AtomicBool::new(false)).unwrap().0.is_empty());
        assert!(read(&legacy, "devin-desktop", &AtomicBool::new(false)).unwrap().0.is_empty());
        // Another session's capture is separate work and survives.
        let other = capture(&home, "other", &[
            r#"{"notification":{"sessionUpdate":"session_info_update","title":"Another task"}}"#,
            &canonical_event("2026-09-15T12:00:00Z", 100, 0, 0, 0),
        ]);
        assert_eq!(read(&other, "devin-desktop", &AtomicBool::new(false)).unwrap().0.len(), 1);
        let _ = fs::remove_dir_all(&home);
    }

    #[test]
    fn a_capture_without_a_stated_time_is_skipped_not_dated_from_the_file() {
        let home = home();
        let path = capture(&home, "undated", &[
            r#"{"notification":{"sessionUpdate":"usage_update","_meta":{"cognition.ai/inputTokens":100,"cognition.ai/outputTokens":10}}}"#,
            r#"{"notification":{"content":{"metadata":{"metrics":{"input_tokens":100},"generation_model":"priced"}}}}"#,
        ]);
        let (records, notes) = read(&path, "devin-desktop", &AtomicBool::new(false)).unwrap();
        assert!(records.is_empty(), "{records:?}");
        assert!(notes.is_empty(), "{notes:?}");
        let _ = fs::remove_dir_all(&home);
    }

    #[test]
    fn a_numeric_string_timestamp_is_read_as_an_epoch_in_milliseconds() {
        assert_eq!(flexible(&Value::String("1789372800000".into())).unwrap().timestamp(), 1_789_372_800);
        assert_eq!(flexible(&Value::String("1789372800".into())).unwrap().timestamp(), 1_789_372_800);
        assert_eq!(flexible(&Value::from(1_789_372_800u64)).unwrap().timestamp(), 1_789_372_800);
        assert_eq!(flexible(&Value::String("2026-09-15T12:00:00Z".into())).unwrap().timestamp(), 1_789_473_600);
        assert!(flexible(&Value::from(0)).is_none()); // an unset field is not 1970
        assert!(flexible(&Value::from(-5)).is_none());
        assert!(flexible(&Value::Bool(true)).is_none());
        assert!(flexible(&Value::String("  ".into())).is_none());
    }

    #[test]
    fn prompt_text_is_never_retained_and_cancellation_stops_the_read() {
        let home = home();
        let path = capture(&home, "secret", &[
            r#"{"notification":{"sessionUpdate":"session_info_update","title":"TOP-SECRET-TITLE"}}"#,
            r#"{"notification":{"sessionUpdate":"usage_update","created_at":"2026-09-15T12:00:00Z","_meta":{"cognition.ai/inputTokens":5,"cognition.ai/outputTokens":1}},"content":"TOP-SECRET-BODY"}"#,
        ]);
        let (records, _) = read(&path, "devin-desktop", &AtomicBool::new(false)).unwrap();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].session, "secret"); // the title is not an identity here
        let serialized = serde_json::to_string(&records).unwrap();
        assert!(!serialized.contains("TOP-SECRET"), "{serialized}");
        assert_eq!(read(&path, "devin-desktop", &AtomicBool::new(true)).unwrap_err(), "cancelled");
        let _ = fs::remove_dir_all(&home);
    }
}
