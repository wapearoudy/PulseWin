//! Pulse v1.9 JunieUsageReader (Apache-2.0).
//! Junie writes `<session>/events.jsonl`; only `LlmResponseMetadataEvent` rows
//! carry usage, keyed by an array of `modelUsage` entries that report latency
//! in milliseconds. A reported reasoning count is never added to output and
//! never classified, so it is surfaced as an incomplete reading instead.
use super::measured_readers::{count, lines, record, text, timestamp};
use super::*;
use chrono::TimeZone;

pub(super) fn roots(home: &Path, id: &str, env: &impl Fn(&str) -> Option<String>) -> Vec<PathBuf> {
    let _ = (id, env);
    vec![home.join(".junie/sessions")]
}

pub(super) fn candidate(path: &Path, id: &str) -> bool {
    let _ = id;
    path.file_name().is_some_and(|name| name == "events.jsonl")
}

/// A store's `0` is "unset", not 1970.
fn event_time(value: &Value) -> Option<DateTime<Utc>> {
    timestamp(value, true).filter(|at| at.timestamp() > 0)
}

/// The session id embeds its start time (`session-YYMMDD-HHMMSS`, local time);
/// it is the fallback when a row carries no timestamp of its own.
fn session_start(session: &str) -> Option<DateTime<Utc>> {
    let index = session.find("session-")?;
    let stamp = session.get(index + "session-".len()..)?.get(..13)?;
    let local = chrono::NaiveDateTime::parse_from_str(stamp, "%y%m%d-%H%M%S").ok()?;
    Local.from_local_datetime(&local).single().map(|at| at.with_timezone(&Utc))
}

/// First non-zero value across the spellings the product has used.
fn merged(entry: &Value, aliases: &[&str]) -> u64 {
    aliases.iter().map(|key| count(&entry[*key])).find(|value| *value > 0).unwrap_or(0)
}

pub(super) fn read(path: &Path, _id: &str, cancel: &AtomicBool) -> Result<(Vec<SpendRecord>, Vec<String>), String> {
    let session = path
        .parent()
        .and_then(Path::file_name)
        .unwrap_or_default()
        .to_string_lossy()
        .into_owned();
    let start = session_start(&session);
    let file = File::open(path).map_err(|error| format!("无法读取 {}：{error}", path.display()))?;
    let mut reader = BufReader::with_capacity(64 * 1024, file);
    let mut records = Vec::new();
    let mut notes = Vec::new();
    let mut capped = false;
    let mut partial = false;
    lines(&mut reader, cancel, &mut notes, |row, _| {
        let event = &row["event"];
        let agent_event = &event["agentEvent"];
        if text(&agent_event["kind"]).as_deref() != Some("LlmResponseMetadataEvent") {
            return Ok(());
        }
        let usage = &agent_event["modelUsage"];
        let Some(entries) = usage.as_array() else { return Ok(()); };
        let end = event_time(&row["timestampMs"]);
        for (index, entry) in entries.iter().enumerate() {
            check(cancel)?;
            let Some(model) = text(&entry["model"]) else { continue; };
            let latency = count(&entry["time"]);
            let at = match (end, start) {
                (Some(end), _) if latency == 0 => Some(end),
                (Some(end), _) => {
                    let begun = end - chrono::Duration::milliseconds(latency as i64);
                    Some(if begun.timestamp() > 0 { begun } else { end })
                }
                (None, start) => start,
            };
            let Some(at) = at else { continue; };
            let tally = Tally {
                input: merged(entry, &["inputTokens", "input"]),
                output: merged(entry, &["outputTokens", "output"]),
                cache_read: merged(entry, &["cacheInputTokens", "cacheReadInputTokens", "cacheRead"]),
                cache_write: merged(
                    entry,
                    &["cacheCreateTokens", "cacheCreationInputTokens", "cacheWrite"],
                ),
                ..Default::default()
            };
            // The product's reasoning relationship is not documented, so it is
            // neither added to output nor guessed into a bucket.
            let reasoning = merged(entry, &["reasoningTokens", "reasoningOutputTokens", "thinkingTokens"]);
            if reasoning > 0 {
                partial = true;
            }
            if tally.total() == 0 {
                continue;
            }
            if records.len() >= MAX_RECORDS {
                capped = true;
                continue;
            }
            let identity = format!(
                "{session}:{}:{model}:{}:{}:{}:{}:{}:{index}",
                at.timestamp_millis(),
                tally.input,
                tally.cache_write,
                tally.cache_read,
                tally.output,
                count(&entry["cost"]),
            );
            let spend = record("junie", model, at, session.clone(), tally, 0, Some(format!("junie:{identity}")));
            records.push(spend);
        }
        Ok(())
    })?;
    if capped {
        notes.push("Junie：记录数超过上限，计数可能不完整".into());
    }
    if partial {
        notes.push("Junie：存在无法归类的 reasoning token，计数可能不完整".into());
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

        fn put(&self, relative: &str, content: impl AsRef<[u8]>) -> PathBuf {
            let path = self.root.join("home").join(relative);
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent).unwrap();
            }
            fs::write(&path, content).unwrap();
            path
        }

        fn scan(&self) -> SpendSnapshot {
            let sessions = self.root.join("home/.junie/sessions");
            scan_roots(
                &self.root.join("cache"),
                &Prices::new(),
                &AtomicBool::new(false),
                |_, _| {},
                |agent| if agent == "junie" { vec![sessions.clone()] } else { Vec::new() },
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

    fn event(timestamp_ms: Option<i64>, usage: Value) -> String {
        let mut row = json!({ "event": { "agentEvent": { "kind": "LlmResponseMetadataEvent", "modelUsage": usage } } });
        if let Some(ms) = timestamp_ms {
            row["timestampMs"] = json!(ms);
        }
        row.to_string()
    }

    /// One `modelUsage` entry: counters live on the entry itself, not nested.
    fn metadata(model: &str, counters: Value) -> Value {
        let mut entry = counters;
        entry["model"] = json!(model);
        entry["time"] = json!(1500);
        json!([entry])
    }

    #[test]
    fn roots_and_candidate() {
        let home = Path::new("fixture-home");
        let clean = |_: &str| -> Option<String> { None };
        assert_eq!(roots(home, "junie", &clean), vec![home.join(".junie/sessions")]);
        assert!(candidate(Path::new("a/session-1/events.jsonl"), "junie"));
        assert!(!candidate(Path::new("a/session-1/events.json"), "junie"));
        assert!(candidate(Path::new("a/events.jsonl"), "junie"));
    }

    #[test]
    fn model_usage_maps_counters_and_latency() {
        let fixture = Fixture::new("junie-usage");
        let usage = json!([{
            "model": "claude-sonnet-4",
            "inputTokens": 20,
            "outputTokens": 7,
            "cacheInputTokens": 3,
            "cacheCreateTokens": 2,
            "reasoningTokens": 0,
            "time": 1500
        }]);
        let file = fixture.put(
            ".junie/sessions/session-240102-030405/events.jsonl",
            format!("{}\n", event(Some(1_700_000_000_000), usage)),
        );
        let (records, notes) = read(&file, "junie", &AtomicBool::new(false)).unwrap();
        assert!(notes.is_empty());
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].model, "claude-sonnet-4");
        assert_eq!(records[0].session, "session-240102-030405");
        assert_eq!(records[0].tally.input, 20);
        assert_eq!(records[0].tally.output, 7);
        assert_eq!(records[0].tally.cache_read, 3);
        assert_eq!(records[0].tally.cache_write, 2);
        // The row's time is the response end; latency moves it back 1.5 s.
        assert_eq!(records[0].source_timestamp, Some(1_699_999_998));
        assert_eq!(records[0].aggregate_timing, false);
        assert!(records[0]
            .deduplication_id
            .as_deref()
            .is_some_and(|id| id.starts_with("junie:session-240102-030405:1699999998500:claude-sonnet-4:20:2:3:7:")));
    }

    #[test]
    fn missing_row_time_falls_back_to_the_session_start() {
        let fixture = Fixture::new("junie-start");
        let file = fixture.put(
            ".junie/sessions/session-240102-030405/events.jsonl",
            format!("{}\n", event(None, metadata("m", json!({ "inputTokens": 4 })))),
        );
        let (records, notes) = read(&file, "junie", &AtomicBool::new(false)).unwrap();
        assert_eq!(records.len(), 1);
        let local = chrono::NaiveDateTime::parse_from_str("240102-030405", "%y%m%d-%H%M%S").unwrap();
        let expected = Local.from_local_datetime(&local).single().unwrap().with_timezone(&Utc);
        assert_eq!(records[0].source_timestamp, Some(expected.timestamp()));
        assert!(notes.is_empty());
    }

    #[test]
    fn reasoning_is_never_counted_and_reports_partial_once() {
        let fixture = Fixture::new("junie-partial");
        let usage = json!([
            {
                "model": "m1",
                "inputTokens": 10,
                "outputTokens": 4,
                "reasoningTokens": 6,
                "time": 0
            },
            {
                "model": "m2",
                "inputTokens": 2,
                "outputTokens": 1,
                "thinkingTokens": 3,
                "time": 0
            }
        ]);
        let file = fixture.put(
            ".junie/sessions/session-250101-000000/events.jsonl",
            format!("{}\n", event(Some(1_700_000_000_000), usage)),
        );
        let (records, notes) = read(&file, "junie", &AtomicBool::new(false)).unwrap();
        assert_eq!(records.len(), 2);
        for record in &records {
            assert_eq!(record.unclassified_tokens, 0);
            assert_eq!(record.aggregate_timing, false);
        }
        assert_eq!(records[0].tally.output, 4);
        assert_eq!(records[1].tally.output, 1);
        assert_eq!(notes.len(), 1);
        assert!(notes[0].contains("reasoning") && notes[0].contains("计数可能不完整"));
    }

    #[test]
    fn malformed_lines_are_noted_and_replays_fold() {
        let fixture = Fixture::new("junie-bad");
        let row = event(Some(1_700_000_000_000), metadata("m", json!({ "inputTokens": 5 })));
        let file = fixture.put(
            ".junie/sessions/session-1/events.jsonl",
            format!("not json\n{row}\n{row}\n"),
        );
        let (records, notes) = read(&file, "junie", &AtomicBool::new(false)).unwrap();
        assert_eq!(records.len(), 2);
        assert_eq!(records[0].deduplication_id, records[1].deduplication_id);
        assert_eq!(notes.len(), 1);
        assert!(notes[0].contains("无法解码"));
        let snapshot = fixture.scan();
        assert_eq!(snapshot.records.len(), 1);
    }

    #[test]
    fn rows_without_attribution_are_skipped_without_notes() {
        let fixture = Fixture::new("junie-skip");
        let rows = [
            json!({ "event": { "agentEvent": { "kind": "OtherEvent", "modelUsage": [] } } }).to_string(),
            event(Some(1_700_000_000_000), json!([{ "inputTokens": 9, "time": 0 }])),
            event(Some(1_700_000_000_000), json!([])),
        ]
        .join("\n");
        let file = fixture.put(".junie/sessions/session-2/events.jsonl", rows);
        let (records, notes) = read(&file, "junie", &AtomicBool::new(false)).unwrap();
        assert!(records.is_empty());
        assert!(notes.is_empty());
    }

    #[test]
    fn prompt_text_is_never_retained() {
        let fixture = Fixture::new("junie-secret");
        let mut envelope = serde_json::from_str::<Value>(&event(
            Some(1_700_000_000_000),
            metadata("m", json!({ "inputTokens": 1 })),
        ))
        .unwrap();
        envelope["event"]["prompt"] = json!("TOP-SECRET-JUNIE");
        let file = fixture.put(".junie/sessions/session-1/events.jsonl", envelope.to_string());
        let (records, _) = read(&file, "junie", &AtomicBool::new(false)).unwrap();
        assert_eq!(records.len(), 1);
        assert!(!serde_json::to_string(&records).unwrap().contains("TOP-SECRET"));
    }
}
