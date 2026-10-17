//! Pulse v1.9 GJCUsageReader (Apache-2.0).
//! Gajae Code's JSONL transcripts: a `{"type":"session","id","cwd"}` header and
//! then `{"type":"message","id","message":{role,model,timestamp,usage}}` rows.
//! Only assistant rows with a model and a real timestamp emit; `cost.total` is
//! the product's own dollars and reasoning is not exposed, so output is kept as
//! reported.
use super::measured_readers::{count, lines, record, text, timestamp};
use super::*;
use chrono::DateTime;

/// An environment variable the client honours names a base directory; `~/`
/// prefixes expand under the home directory on Windows.
fn env_dir(env: &impl Fn(&str) -> Option<String>, key: &str, home: &Path) -> Option<PathBuf> {
    let raw = env(key)?;
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }
    if let Some(rest) = trimmed.strip_prefix("~/").or_else(|| trimmed.strip_prefix("~\\")) {
        return Some(home.join(rest));
    }
    if trimmed == "~" {
        return Some(home.to_path_buf());
    }
    Some(PathBuf::from(trimmed))
}

pub(super) fn roots(home: &Path, id: &str, env: &impl Fn(&str) -> Option<String>) -> Vec<PathBuf> {
    let _ = id;
    let mut roots = Vec::new();
    let agent = env_dir(env, "GJC_CODING_AGENT_DIR", home).unwrap_or_else(|| home.join(".gjc/agent"));
    roots.push(agent.join("sessions"));
    for key in ["GJC_CONFIG_DIR", "PI_CONFIG_DIR"] {
        if let Some(base) = env_dir(env, key, home) {
            roots.push(base.join("agent/sessions"));
        }
    }
    // `$XDG_DATA_HOME/gjc/sessions`, and its default `~/.local/share` spelling
    // when the variable is unset. The same tree, adapted for Windows.
    let data_home = env_dir(env, "XDG_DATA_HOME", home).unwrap_or_else(|| home.join(".local/share"));
    roots.push(data_home.join("gjc/sessions"));
    let mut seen = HashSet::new();
    roots.retain(|path| seen.insert(path.clone()));
    roots
}

pub(super) fn candidate(path: &Path, id: &str) -> bool {
    let _ = id;
    path.extension().is_some_and(|extension| extension.eq_ignore_ascii_case("jsonl"))
}

/// A store's `0` is "unset", not 1970.
fn event_time(value: &Value, milliseconds: bool) -> Option<DateTime<Utc>> {
    timestamp(value, milliseconds).filter(|at| at.timestamp() > 0)
}

/// The message's own unix-millisecond time, else the envelope's RFC 3339 time.
fn row_time(message: &Value, event: &Value) -> Option<DateTime<Utc>> {
    event_time(&message["timestamp"], true).or_else(|| event_time(&event["timestamp"], false))
}

/// One JSONL line's header pass: first session id and workspace.
fn header(path: &Path, cancel: &AtomicBool) -> Result<(Option<String>, Option<String>), String> {
    let file = File::open(path).map_err(|error| format!("无法读取 {}：{error}", path.display()))?;
    let mut reader = BufReader::with_capacity(64 * 1024, file);
    let mut session = None;
    let mut workspace = None;
    let mut ignored = Vec::new();
    lines(&mut reader, cancel, &mut ignored, |event, _| {
        if text(&event["type"]).as_deref() != Some("session") {
            return Ok(());
        }
        if session.is_none() {
            session = text(&event["id"]);
        }
        if workspace.is_none() {
            workspace = text(&event["cwd"]);
        }
        Ok(())
    })?;
    Ok((session, workspace))
}

pub(super) fn read(path: &Path, _id: &str, cancel: &AtomicBool) -> Result<(Vec<SpendRecord>, Vec<String>), String> {
    let (header_session, workspace) = header(path, cancel)?;
    let stem = path.file_stem().unwrap_or_default().to_string_lossy().into_owned();
    let session = header_session.unwrap_or(stem);
    let file_name = path.file_name().unwrap_or_default().to_string_lossy().into_owned();
    let file = File::open(path).map_err(|error| format!("无法读取 {}：{error}", path.display()))?;
    let mut reader = BufReader::with_capacity(64 * 1024, file);
    let mut records = Vec::new();
    let mut notes = Vec::new();
    let mut capped = false;
    lines(&mut reader, cancel, &mut notes, |event, _| {
        if text(&event["type"]).as_deref() != Some("message") {
            return Ok(());
        }
        let message = &event["message"];
        if !message.is_object() {
            return Ok(());
        }
        if text(&message["role"]).map(|role| role.to_lowercase()).as_deref() != Some("assistant") {
            return Ok(());
        }
        let usage = &message["usage"];
        if !usage.is_object() {
            return Ok(());
        }
        let Some(model) = text(&message["model"]) else { return Ok(()); };
        let Some(at) = row_time(message, &event) else { return Ok(()); };
        let tally = Tally {
            input: count(&usage["input"]),
            output: count(&usage["output"]),
            cache_write: count(&usage["cacheWrite"]),
            cache_read: count(&usage["cacheRead"]),
            ..Default::default()
        };
        if tally.total() == 0 {
            return Ok(());
        }
        if records.len() >= MAX_RECORDS {
            capped = true;
            return Ok(());
        }
        // Only a real entry id is the product's identity. A row without one is
        // named by its own content and file so a byte-identical mirror at two
        // depths, or a replay of the same row, folds once instead of counting
        // twice; genuinely separate requests differ in time or counts.
        let identity = text(&event["id"])
            .map(|id| format!("gjc:{session}:{id}"))
            .unwrap_or_else(|| {
                format!(
                    "gjc:message:{session}:{file_name}:{}:{model}:{}:{}:{}:{}",
                    at.timestamp_millis(),
                    tally.input,
                    tally.output,
                    tally.cache_read,
                    tally.cache_write
                )
            });
        let mut spend = record("gjc", model, at, session.clone(), tally, 0, Some(identity));
        spend.project = workspace.clone();
        records.push(spend);
        Ok(())
    })?;
    if capped {
        notes.push("GJC：记录数超过上限，计数可能不完整".into());
    }
    Ok((records, notes))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::sync::atomic::AtomicUsize;

    static NEXT: AtomicUsize = AtomicUsize::new(0);

    struct Fixture {
        root: PathBuf,
    }

    impl Fixture {
        fn new(tag: &str) -> Self {
            let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../test-results").join(format!(
                "pulsewin-{tag}-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir_all(root.join("cache")).unwrap();
            Self { root }
        }

        fn home(&self) -> PathBuf {
            self.root.join("home")
        }

        fn put_root(&self, relative: &str, content: impl AsRef<[u8]>) -> PathBuf {
            let path = self.root.join(relative);
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent).unwrap();
            }
            fs::write(&path, content).unwrap();
            path
        }

        fn put(&self, relative: &str, content: impl AsRef<[u8]>) -> PathBuf {
            let path = self.home().join(relative);
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent).unwrap();
            }
            fs::write(&path, content).unwrap();
            path
        }

        fn scan(&self, roots: Vec<PathBuf>) -> SpendSnapshot {
            scan_roots(
                &self.root.join("cache"),
                &Prices::new(),
                &AtomicBool::new(false),
                |_, _| {},
                |agent| if agent == "gjc" { roots.clone() } else { Vec::new() },
            )
            .unwrap()
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let base = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../test-results").canonicalize();
            if let (Ok(base), Ok(target)) = (base, self.root.canonicalize()) {
                if target.starts_with(base) {
                    let _ = fs::remove_dir_all(target);
                }
            }
        }
    }

    /// Transcripts are JSONL: one object per line, never one JSON array.
    fn jsonl(rows: Value) -> String {
        rows.as_array()
            .map(|rows| rows.iter().map(Value::to_string).collect::<Vec<_>>().join("\n") + "\n")
            .unwrap_or_default()
    }

    fn transcript(session: &str) -> String {
        jsonl(json!([
            { "type": "session", "id": session, "timestamp": "2024-01-01T00:00:00Z", "cwd": "C:/work/gjc" },
            { "type": "service_tier_change", "tier": "pro" },
            {
                "type": "message",
                "id": "e1",
                "message": {
                    "role": "assistant",
                    "model": "gpt-5",
                    "provider": "openai",
                    "timestamp": 1_700_000_000_000i64,
                    "usage": { "input": 10, "output": 5, "cacheRead": 2, "cacheWrite": 1, "totalTokens": 18, "cost": { "total": 0.5 } },
                    "content": "TOP-SECRET-GJC"
                }
            },
            { "type": "message", "message": { "role": "user", "content": "hi" } }
        ]))
    }

    #[test]
    fn roots_defaults_and_environment_spellings() {
        let home = Path::new("fixture-home");
        let clean = |_: &str| -> Option<String> { None };
        assert_eq!(
            roots(home, "gjc", &clean),
            vec![home.join(".gjc/agent/sessions"), home.join(".local/share/gjc/sessions")]
        );
        let env = |key: &str| -> Option<String> {
            match key {
                "GJC_CODING_AGENT_DIR" => Some("~/gjc-agent".into()),
                "GJC_CONFIG_DIR" => Some("D:/gjc-config".into()),
                "PI_CONFIG_DIR" => Some("D:/pi-config".into()),
                "XDG_DATA_HOME" => Some("~/xdg".into()),
                _ => None,
            }
        };
        assert_eq!(
            roots(home, "gjc", &env),
            vec![
                home.join("gjc-agent/sessions"),
                PathBuf::from("D:/gjc-config/agent/sessions"),
                PathBuf::from("D:/pi-config/agent/sessions"),
                home.join("xdg/gjc/sessions"),
            ]
        );
        assert!(candidate(Path::new("a/session.jsonl"), "gjc"));
        assert!(candidate(Path::new("a/session.JSONL"), "gjc"));
        assert!(!candidate(Path::new("a/session.json"), "gjc"));
    }

    #[test]
    fn assistant_rows_map_counters_and_identity() {
        let fixture = Fixture::new("gjc-usage");
        let file = fixture.put(".gjc/agent/sessions/slug/session.jsonl", transcript("g1"));
        let (records, notes) = read(&file, "gjc", &AtomicBool::new(false)).unwrap();
        assert!(notes.is_empty());
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].model, "gpt-5");
        assert_eq!(records[0].session, "g1");
        assert_eq!(records[0].tally.input, 10);
        assert_eq!(records[0].tally.output, 5);
        assert_eq!(records[0].tally.cache_read, 2);
        assert_eq!(records[0].tally.cache_write, 1);
        assert_eq!(records[0].tally.total(), 18);
        assert_eq!(records[0].unclassified_tokens, 0);
        assert_eq!(records[0].source_timestamp, Some(1_700_000_000));
        assert_eq!(records[0].project.as_deref(), Some("C:/work/gjc"));
        assert_eq!(records[0].deduplication_id.as_deref(), Some("gjc:g1:e1"));
        assert_eq!(records[0].aggregate_timing, false);
        assert!(!serde_json::to_string(&records).unwrap().contains("TOP-SECRET"));
    }

    #[test]
    fn envelope_time_is_the_fallback_and_missing_pieces_skip() {
        let fixture = Fixture::new("gjc-time");
        let envelope = jsonl(json!([
            { "type": "session", "id": "g2" },
            {
                "type": "message",
                "message": {
                    "role": "assistant", "model": "m", "timestamp": 0,
                    "usage": { "input": 3 }
                },
                "timestamp": "2024-01-02T03:04:05Z"
            },
            {
                "type": "message",
                "message": { "role": "assistant", "model": "m", "timestamp": 0, "usage": { "input": 9 } }
            },
            {
                "type": "message",
                "message": { "role": "assistant", "timestamp": 1_700_000_000_000i64, "usage": { "input": 9 } }
            },
            {
                "type": "message",
                "message": { "role": "assistant", "model": "m", "timestamp": 1_700_000_000_000i64, "usage": { "input": 0 } }
            }
        ]));
        let file = fixture.put(".gjc/agent/sessions/slug2/session.jsonl", envelope);
        let (records, _) = read(&file, "gjc", &AtomicBool::new(false)).unwrap();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].tally.input, 3);
        assert_eq!(records[0].source_timestamp, Some(1_704_164_645));
        // A transcript with no session header still uses its own file name.
        let bare = fixture.put(".gjc/agent/sessions/slug3/part.jsonl", jsonl(json!([
            { "type": "message", "id": "x", "message": { "role": "assistant", "model": "m", "timestamp": 1_700_000_000_000i64, "usage": { "input": 4 } } }
        ])));
        let (records, _) = read(&bare, "gjc", &AtomicBool::new(false)).unwrap();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].session, "part");
        assert_eq!(records[0].deduplication_id.as_deref(), Some("gjc:part:x"));
    }

    #[test]
    fn id_less_rows_are_kept_and_content_identity_folds_replays() {
        let fixture = Fixture::new("gjc-identity");
        let anonymous = jsonl(json!([
            { "type": "session", "id": "same" },
            { "type": "message", "message": { "role": "assistant", "model": "m", "timestamp": 1_700_000_000_000i64, "usage": { "input": 10 } } },
            { "type": "message", "message": { "role": "assistant", "model": "m", "timestamp": 1_700_000_000_000i64, "usage": { "input": 10 } } }
        ]));
        let file = fixture.put(".gjc/agent/sessions/same/session.jsonl", anonymous);
        let (records, _) = read(&file, "gjc", &AtomicBool::new(false)).unwrap();
        assert_eq!(records.len(), 2);
        assert_eq!(records[0].deduplication_id, records[1].deduplication_id);
        let first = records[0].deduplication_id.clone();
        assert!(first.as_deref().unwrap().starts_with("gjc:message:same:"));

        // A replay of the same row with an explicit id folds globally in a scan.
        let replay = jsonl(json!([
            { "type": "session", "id": "r1" },
            { "type": "message", "id": "dup", "message": { "role": "assistant", "model": "m", "timestamp": 1_700_000_000_000i64, "usage": { "input": 10 } } },
            { "type": "message", "id": "dup", "message": { "role": "assistant", "model": "m", "timestamp": 1_700_000_000_000i64, "usage": { "input": 10 } } }
        ]));
        let file = fixture.put(".gjc/agent/sessions/r1/history.jsonl", replay);
        let (records, _) = read(&file, "gjc", &AtomicBool::new(false)).unwrap();
        assert_eq!(records.len(), 2);
        let snapshot = fixture.scan(vec![fixture.home().join(".gjc/agent/sessions")]);
        // two id-less rows fold once; two replays of id `dup` fold once: 2 total.
        assert_eq!(snapshot.records.len(), 2);
    }

    #[test]
    fn byte_identical_mirrors_fold_but_extra_rows_count() {
        let fixture = Fixture::new("gjc-mirror");
        let body = jsonl(json!([
            { "type": "session", "id": "m1" },
            { "type": "message", "message": { "role": "assistant", "model": "m", "timestamp": 1_700_000_000_000i64, "usage": { "input": 7 } } }
        ]));
        fixture.put_root("a/sessions/m1/session.jsonl", &body);
        fixture.put_root("b/sessions/deeper/m1/session.jsonl", &body);
        let snapshot = fixture.scan(vec![fixture.root.join("a/sessions"), fixture.root.join("b/sessions")]);
        assert_eq!(snapshot.records.len(), 1);
        assert_eq!(snapshot.records[0].tally.input, 7);
    }

    #[test]
    fn malformed_lines_do_not_hide_later_usage() {
        let fixture = Fixture::new("gjc-bad");
        let file = fixture.put(
            ".gjc/agent/sessions/bad/session.jsonl",
            "not json\n{\"type\":\"message\",\"id\":\"ok\",\"message\":{\"role\":\"assistant\",\"model\":\"m\",\"timestamp\":1700000000000,\"usage\":{\"input\":4}}}\n",
        );
        let (records, notes) = read(&file, "gjc", &AtomicBool::new(false)).unwrap();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].deduplication_id.as_deref(), Some("gjc:session:ok"));
        assert_eq!(notes.len(), 1);
        assert!(notes[0].contains("无法解码"));
    }
}
