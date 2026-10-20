//! Pulse 1.9 DevinCLIStore (Apache-2.0): Devin keeps its conversations in
//! SQLite at `~/.local/share/devin/cli/sessions.db`. `message_nodes.chat_message`
//! is the message as JSON, and an assistant's carries its own metrics under
//! `metadata.metrics`; `sessions` supplies the working directory and title.
//!
//! **One message, two nodes.** The CLI keeps a message's node and a sibling
//! under the same parent with the same `message_id` and the same metrics; read
//! node by node, every reply counted twice (5 messages, 10 metered rows, on the
//! Mac this was found on). A message is counted once, by its id, or by its
//! request where it has none.
//!
//! **Input is taken as reported.** Unlike the Codex-shaped stores, Devin's
//! `input_tokens` is not a prompt that already contains `cache_read_tokens`, so
//! the cache read is a separate bucket and is not subtracted from it. A message
//! whose four counters are all zero is not usage and is skipped.
//!
//! **Not the same store as the quota route.** `DevinUsageService` reads the
//! desktop app's saved plan for the ring; this is the CLI's own transcript
//! database, and the two know nothing about each other. `created_at` is an
//! integer of seconds, or milliseconds once it is past `1e10`; a row at or
//! below zero is undated and skipped, never dated at the epoch or from the
//! file's modification time. The database is opened read-only: a running Devin
//! is never written to, and its WAL is read, never checkpointed.
//!
//! The `sessions` table is read here for the directory, and this module also
//! owns the title lookup `devin_desktop_reader` uses, so the two routes cannot
//! disagree about which sessions are counted.

use super::measured_readers::{count, record, text};
use super::*;
use rusqlite::{Connection, OpenFlags};
use std::time::Instant;

pub(super) fn roots(home: &Path, id: &str, _env: &impl Fn(&str) -> Option<String>) -> Vec<PathBuf> {
    if id != "devin-cli" { return Vec::new(); }
    // The Swift catalogue names exactly this one path for Devin's CLI and
    // honours no environment override: the Windows `%APPDATA%` root is a
    // documented but unimplemented extra, and this port does not invent a path
    // nobody wrote. A Windows machine without the Unix layout reports
    // not-detected rather than reading one.
    vec![home.join(".local/share/devin/cli/sessions.db")]
}

pub(super) fn candidate(path: &Path, id: &str) -> bool {
    id == "devin-cli" && path.file_name().is_some_and(|name| name == "sessions.db")
}

pub(super) fn read(path: &Path, _id: &str, cancel: &AtomicBool) -> Result<(Vec<SpendRecord>, Vec<String>), String> {
    check(cancel)?;
    let db = open(path)?;
    let directories = directories(&db, cancel)?;
    let mut notes = Vec::new();
    let mut out: Vec<SpendRecord> = Vec::new();
    let mut skipped = Skipped::default();
    each_usage(&db, cancel, |session, model, at, tally, identity| {
        if out.len() >= MAX_RECORDS { return Err("Devin：数据库记录超过读取上限。".into()); }
        let mut spent = record(
            "devin-cli", model.to_owned(), at, session.to_owned(), tally, 0,
            identity.map(|identity| format!("devin:{identity}")),
        );
        spent.project = directories.get(session).and_then(|(directory, _)| directory.clone());
        out.push(spent);
        Ok(())
    }, &mut skipped)?;
    // The original reports no note for this store; the only note is the port's
    // own bound, where a message too large to parse would otherwise vanish.
    if skipped.oversize { notes.push("Devin：部分消息超出大小上限或无法解码，计数可能不完整。".into()); }
    notes.sort();
    notes.dedup();
    check(cancel)?;
    Ok((out, notes))
}

fn open(path: &Path) -> Result<Connection, String> {
    let db = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX)
        .map_err(|_| format!("Devin：会话数据库无法读取（{}）。", path.display()))?;
    let _ = db.busy_timeout(Duration::from_millis(150));
    let deadline = Instant::now() + Duration::from_secs(15);
    db.progress_handler(1000, Some(move || Instant::now() >= deadline));
    Ok(db)
}

/// The column names of a table. Empty when the table does not exist, which is
/// how a reader built against one schema stays silent against another.
fn columns(db: &Connection, table: &str) -> HashSet<String> {
    let mut names = HashSet::new();
    let Ok(mut query) = db.prepare(&format!("PRAGMA table_info({table})")) else { return names };
    let Ok(rows) = query.query_map([], |row| row.get::<_, String>(1)) else { return names };
    for name in rows.flatten() { names.insert(name); }
    names
}

/// `sessions.id` → its working directory and title. A schema without those
/// columns is metadata this reader does not have, not an error: the query names
/// `NULL` for them, exactly as the original's optional columns read.
fn directories(db: &Connection, cancel: &AtomicBool) -> Result<HashMap<String, (Option<String>, Option<String>)>, String> {
    let mut table = HashMap::new();
    let names = columns(db, "sessions");
    if !names.contains("id") { return Ok(table); }
    let directory = if names.contains("working_directory") { "working_directory" } else { "NULL" };
    let title = if names.contains("title") { "title" } else { "NULL" };
    let mut query = db.prepare(&format!("SELECT id, {directory}, {title} FROM sessions"))
        .map_err(|_| "Devin：sessions 表无法读取。".to_string())?;
    let mut rows = query.query([]).map_err(|_| "Devin：sessions 表无法读取。".to_string())?;
    while let Some(row) = rows.next().map_err(|_| "Devin：sessions 表读取中断。".to_string())? {
        check(cancel)?;
        let Some(id) = row.get::<_, Option<String>>(0).ok().flatten() else { continue; };
        let value = |index: usize| row.get::<_, Option<String>>(index).ok().flatten();
        table.insert(id, (value(1), value(2)));
    }
    Ok(table)
}

#[derive(Default)]
struct Skipped {
    /// A message the port's size bound or a decode failure kept out.
    oversize: bool,
}

/// The one parser both Devin routes share: a message is counted once, by its
/// id, with the four counters as reported, and only when the row states a
/// usable time. `counted_session_ids` calls this same parser so a capture can
/// never be suppressed by a session this reader does not actually count.
fn each_usage(
    db: &Connection,
    cancel: &AtomicBool,
    mut consume: impl FnMut(&str, &str, DateTime<Utc>, Tally, Option<&str>) -> Result<(), String>,
    skipped: &mut Skipped,
) -> Result<(), String> {
    let mut seen: HashSet<String> = HashSet::new();
    let mut query = db.prepare(
        "SELECT session_id, CASE WHEN length(chat_message) <= ?1 THEN chat_message ELSE NULL END, created_at FROM message_nodes",
    ).map_err(|_| "Devin：不支持的 message_nodes 表结构，未编造用量。".to_string())?;
    let mut rows = query.query([MAX_LINE as i64]).map_err(|_| "Devin：消息表无法读取。".to_string())?;
    while let Some(row) = rows.next().map_err(|_| "Devin：数据库读取中断，计数可能不完整。".to_string())? {
        check(cancel)?;
        let session = row.get::<_, Option<String>>(0).ok().flatten();
        let message = row.get::<_, Option<String>>(1).ok().flatten();
        let (Some(session), Some(message)) = (session, message) else { skipped.oversize = true; continue; };
        let root: Value = match serde_json::from_str(&message) {
            Ok(value) => value,
            Err(_) => { skipped.oversize = true; continue; }
        };
        let metadata = &root["metadata"];
        if !metadata.is_object() { continue; }
        let metrics = &metadata["metrics"];
        if !metrics.is_object() { continue; }
        let tally = Tally {
            input: count(&metrics["input_tokens"]),
            cache_write: count(&metrics["cache_creation_tokens"]),
            cache_read: count(&metrics["cache_read_tokens"]),
            output: count(&metrics["output_tokens"]),
            ..Default::default()
        };
        if tally.total() == 0 { continue; }
        // A message is one message however many nodes carry it. Claimed before
        // the time check, so a duplicate cannot resurface as a dated copy.
        let identity = text(&root["message_id"]).or_else(|| text(&metadata["request_id"]));
        if let Some(identity) = &identity {
            if !seen.insert(identity.clone()) { continue; }
        }
        let Some(at) = epoch(seconds(&row)) else { continue; };
        let model = text(&metadata["generation_model"]).unwrap_or_else(|| "devin".into());
        consume(&session, &model, at, tally, identity.as_deref())?;
    }
    Ok(())
}

/// The session ids this reader actually counts, for `devin_desktop_reader` to
/// exclude a capture that mirrors one. A metadata-only row, an empty metrics
/// object or an undated message is not usage.
pub(super) fn counted_session_ids(db: &Connection, cancel: &AtomicBool) -> HashSet<String> {
    let mut ids = HashSet::new();
    let _ = each_usage(db, cancel, |session, _, _, _, _| { ids.insert(session.to_owned()); Ok(()) }, &mut Skipped::default());
    ids
}

/// One `sessions` row as Devin Desktop's lookup needs it.
pub(super) struct Lookup {
    pub id: String,
    pub model: Option<String>,
    pub directory: Option<String>,
    pub has_counted_usage: bool,
}

/// `sessions.title` → the sessions that carry it, from the one database named.
/// Only a database the CLI route also reads can suppress a capture, so every
/// session here has its counted flag computed by the shared parser. A missing
/// database is no lookup, never an error.
pub(super) fn lookup(path: &Path, cancel: &AtomicBool) -> HashMap<String, Vec<Lookup>> {
    let mut table: HashMap<String, Vec<Lookup>> = HashMap::new();
    if !path.is_file() { return table; }
    let Ok(db) = open(path) else { return table; };
    let names = columns(&db, "sessions");
    if !names.contains("id") || !names.contains("title") { return table; }
    let model = if names.contains("model") { "model" } else { "NULL" };
    let directory = if names.contains("working_directory") { "working_directory" } else { "NULL" };
    let counted = counted_session_ids(&db, cancel);
    let Ok(mut query) = db.prepare(&format!("SELECT id, title, {model}, {directory} FROM sessions")) else { return table; };
    let Ok(mut rows) = query.query([]) else { return table; };
    while let Ok(Some(row)) = rows.next() {
        if check(cancel).is_err() { return table; }
        let Some(id) = row.get::<_, Option<String>>(0).ok().flatten() else { continue; };
        let Some(title) = row.get::<_, Option<String>>(1).ok().flatten().and_then(|title| text(&Value::String(title))) else { continue; };
        let value = |index: usize| row.get::<_, Option<String>>(index).ok().flatten().and_then(|value| text(&Value::String(value)));
        table.entry(title).or_default().push(Lookup {
            has_counted_usage: counted.contains(&id), id, model: value(2), directory: value(3),
        });
    }
    table
}

/// The row's `created_at` as SQLite stored it. The original reads the column
/// through `sqlite3_column_int64`, which applies SQLite's own numeric coercion,
/// so a real or numeric-text column is read the same way rather than dropped.
fn seconds(row: &rusqlite::Row<'_>) -> f64 {
    if let Ok(Some(value)) = row.get::<_, Option<i64>>(2) { return value as f64; }
    if let Ok(Some(value)) = row.get::<_, Option<f64>>(2) { return value; }
    row.get::<_, Option<String>>(2).ok().flatten().and_then(|value| value.trim().parse::<f64>().ok()).unwrap_or(0.)
}

/// Devin's own unit rule: seconds, or milliseconds once past `1e10`; zero and
/// negatives are undated rather than 1970.
fn epoch(raw: f64) -> Option<DateTime<Utc>> {
    if !raw.is_finite() || raw <= 0. { return None; }
    let seconds = if raw > 10_000_000_000. { raw / 1000. } else { raw };
    if !seconds.is_finite() || seconds <= 0. { return None; }
    let whole = seconds.trunc();
    if whole >= i64::MAX as f64 { return None; }
    DateTime::from_timestamp(whole as i64, ((seconds - whole) * 1e9).round().clamp(0., 999_999_999.) as u32)
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

    fn home() -> PathBuf { temp_dir("devin-home") }

    const SCHEMA: &str = "CREATE TABLE sessions (id TEXT, working_directory TEXT, title TEXT); \
        CREATE TABLE message_nodes (session_id TEXT, chat_message TEXT, created_at INTEGER);";

    /// A database in the shape the format states, closed before any reader
    /// opens it.
    fn database(home: &Path, statements: &[&str]) -> PathBuf {
        let path = home.join(".local/share/devin/cli/sessions.db");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        let db = Connection::open(&path).unwrap();
        for statement in statements { db.execute_batch(statement).unwrap(); }
        drop(db);
        path
    }

    #[test]
    fn roots_and_candidate_follow_the_cli_layout() {
        let home = home();
        assert_eq!(roots(&home, "devin-cli", &|_: &str| None), vec![home.join(".local/share/devin/cli/sessions.db")]);
        // The Windows %APPDATA% root is a documented extra the original does not
        // implement; this reader does not claim it either.
        assert_eq!(roots(&home, "devin-cli", &|key: &str| (key == "APPDATA").then(|| "C:/appdata".into())),
            vec![home.join(".local/share/devin/cli/sessions.db")]);
        assert!(roots(&home, "devin-desktop", &|_: &str| None).is_empty());
        assert!(candidate(Path::new("C:/Users/me/.local/share/devin/cli/sessions.db"), "devin-cli"));
        assert!(!candidate(Path::new("a/sessions.db"), "devin-desktop"));
        assert!(!candidate(Path::new("a/message_nodes.db"), "devin-cli"));
        let _ = fs::remove_dir_all(&home);
    }

    #[test]
    fn a_message_two_nodes_share_is_counted_once() {
        let home = home();
        let reply = r#"{"role":"assistant","message_id":"m1","metadata":{"generation_model":"priced","metrics":{"input_tokens":100,"output_tokens":10}}}"#;
        let other = r#"{"role":"assistant","message_id":"m2","metadata":{"generation_model":"priced","metrics":{"input_tokens":50,"output_tokens":5}}}"#;
        let path = database(&home, &[SCHEMA,
            "INSERT INTO sessions VALUES ('s1','/Users/me/Code/Pulse','Fix the ring')",
            &format!("INSERT INTO message_nodes VALUES ('s1','{reply}',1789372800)"),
            &format!("INSERT INTO message_nodes VALUES ('s1','{reply}',1789372800)"),
            &format!("INSERT INTO message_nodes VALUES ('s1','{other}',1789372800)"),
        ]);
        let (records, notes) = read(&path, "devin-cli", &AtomicBool::new(false)).unwrap();
        assert!(notes.is_empty(), "{notes:?}");
        assert_eq!(records.len(), 2, "{records:?}");
        assert_eq!(records[0].tally.total() + records[1].tally.total(), 165);
        assert_eq!(records[0].model, "priced");
        assert_eq!(records[0].project.as_deref(), Some("/Users/me/Code/Pulse"));
        assert_eq!(records[0].session, "s1");
        // The message id is the product's own and is global, so it is not
        // namespaced by the database file: a copy elsewhere still collapses.
        assert_eq!(records[0].deduplication_id.as_deref(), Some("devin:m1"));
        assert_eq!(records[0].source_timestamp, Some(1_789_372_800));
        assert_eq!(records[0].tally.input, 100); // input is taken as reported
        assert!(!records[0].aggregate_timing);
        let _ = fs::remove_dir_all(&home);
    }

    #[test]
    fn request_id_stands_in_for_a_missing_message_id() {
        let home = home();
        let reply = r#"{"role":"assistant","metadata":{"request_id":"r1","metrics":{"input_tokens":7,"output_tokens":1}}}"#;
        let path = database(&home, &[SCHEMA,
            &format!("INSERT INTO message_nodes VALUES ('s1','{reply}',1789372800)"),
            &format!("INSERT INTO message_nodes VALUES ('s1','{reply}',1789372800)"),
        ]);
        let (records, _) = read(&path, "devin-cli", &AtomicBool::new(false)).unwrap();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].model, "devin"); // the original's fallback model
        assert_eq!(records[0].deduplication_id.as_deref(), Some("devin:r1"));
        let _ = fs::remove_dir_all(&home);
    }

    #[test]
    fn millisecond_columns_are_seconds_and_an_undated_row_is_skipped() {
        let home = home();
        let reply = r#"{"message_id":"m1","metadata":{"generation_model":"priced","metrics":{"input_tokens":4,"output_tokens":1}}}"#;
        let empty = r#"{"message_id":"m2","metadata":{"generation_model":"priced","metrics":{"input_tokens":0,"output_tokens":0}}}"#;
        let path = database(&home, &[SCHEMA,
            &format!("INSERT INTO message_nodes VALUES ('s1','{reply}',1789372800000)"),
            &format!("INSERT INTO message_nodes VALUES ('s1','{reply}',0)"),
            &format!("INSERT INTO message_nodes VALUES ('s1','{empty}',1789372800)"),
        ]);
        let (records, notes) = read(&path, "devin-cli", &AtomicBool::new(false)).unwrap();
        // The original is silent about an undated row: it is skipped, not noted.
        assert!(notes.is_empty(), "{notes:?}");
        assert_eq!(records.len(), 1, "{records:?}");
        assert_eq!(records[0].source_timestamp, Some(1_789_372_800));
        let _ = fs::remove_dir_all(&home);
    }

    #[test]
    fn a_schema_without_the_metadata_columns_is_silent_not_broken() {
        let home = home();
        let reply = r#"{"message_id":"m1","metadata":{"generation_model":"priced","metrics":{"input_tokens":4,"output_tokens":1}}}"#;
        let path = database(&home, &[
            "CREATE TABLE sessions (id TEXT)",
            "CREATE TABLE message_nodes (session_id TEXT, chat_message TEXT, created_at INTEGER)",
            &format!("INSERT INTO message_nodes VALUES ('s1','{reply}',1789372800)"),
        ]);
        let (records, notes) = read(&path, "devin-cli", &AtomicBool::new(false)).unwrap();
        assert!(notes.is_empty(), "{notes:?}");
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].project, None); // no directory column, so no project
        let _ = fs::remove_dir_all(&home);
    }

    #[test]
    fn message_text_is_never_retained_and_reading_does_not_change_the_database() {
        let home = home();
        let reply = r#"{"message_id":"m1","content":"TOP-SECRET-DEVIN-BODY","metadata":{"generation_model":"priced","metrics":{"input_tokens":4,"output_tokens":1}}}"#;
        let path = database(&home, &[SCHEMA,
            &format!("INSERT INTO message_nodes VALUES ('s1','{reply}',1789372800)"),
        ]);
        let before = fs::read(&path).unwrap();
        let (records, _) = read(&path, "devin-cli", &AtomicBool::new(false)).unwrap();
        assert_eq!(records.len(), 1);
        assert!(!serde_json::to_string(&records).unwrap().contains("TOP-SECRET"));
        assert_eq!(fs::read(&path).unwrap(), before);
        let _ = fs::remove_dir_all(&home);
    }

    #[test]
    fn a_missing_database_is_an_error_not_an_empty_account() {
        let home = home();
        let path = home.join("absent/sessions.db");
        assert!(read(&path, "devin-cli", &AtomicBool::new(false)).is_err());
        assert_eq!(read(&path, "devin-cli", &AtomicBool::new(true)).unwrap_err(), "cancelled");
        assert!(lookup(&path, &AtomicBool::new(false)).is_empty());
        let _ = fs::remove_dir_all(&home);
    }

    #[test]
    fn the_lookup_maps_titles_and_marks_the_sessions_this_reader_counts() {
        let home = home();
        let reply = r#"{"message_id":"m1","metadata":{"metrics":{"input_tokens":4,"output_tokens":1}}}"#;
        let path = database(&home, &[SCHEMA,
            "INSERT INTO sessions VALUES ('d1','/Users/me/Code/Pulse','Fix the ring')",
            "INSERT INTO sessions VALUES ('d2','/Users/me/Code/Other','Fix the ring')",
            "INSERT INTO sessions VALUES ('d3','/Users/me/Code/Third','Uncounted')",
            &format!("INSERT INTO message_nodes VALUES ('d1','{reply}',1789372800)"),
        ]);
        let table = lookup(&path, &AtomicBool::new(false));
        let ambiguous = &table["Fix the ring"];
        // A shared title stays two rows; the caller resolves it to nothing.
        assert_eq!(ambiguous.len(), 2);
        assert!(ambiguous.iter().all(|session| session.has_counted_usage == (session.id == "d1")));
        assert!(!table["Uncounted"][0].has_counted_usage);
        assert_eq!(table["Uncounted"][0].directory.as_deref(), Some("/Users/me/Code/Third"));
        let _ = fs::remove_dir_all(&home);
    }
}
