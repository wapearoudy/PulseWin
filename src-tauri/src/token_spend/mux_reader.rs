//! Pulse v1.9 MuxUsageReader (Apache-2.0).
//! `~/.mux/sessions/<workspaceId>/session-usage.json` holds one cumulative
//! session total per model, and every model shares `lastRequest.timestamp`.
//! Reasoning is left out: the file states no total, so whether it is inside
//! output is unknown and the reported output is kept as is.
use super::measured_readers::{count, document, record, text, timestamp};
use super::*;

pub(super) fn roots(home: &Path, id: &str, env: &impl Fn(&str) -> Option<String>) -> Vec<PathBuf> {
    let _ = (id, env);
    vec![home.join(".mux").join("sessions")]
}

pub(super) fn candidate(path: &Path, id: &str) -> bool {
    let _ = id;
    path.file_name().is_some_and(|name| name == "session-usage.json")
}

/// A store's `0` is "unset", not 1970: such a file has no date to report.
fn event_time(value: &Value, milliseconds: bool) -> Option<DateTime<Utc>> {
    timestamp(value, milliseconds).filter(|at| at.timestamp() > 0)
}

fn non_blank(value: &str) -> Option<String> {
    let trimmed = value.trim();
    (!trimmed.is_empty()).then(|| trimmed.to_owned())
}

/// `"<provider>:<model>"` → the model after the first colon; the provider is
/// routing, not part of the model name.
fn key_model(key: &str) -> Option<String> {
    match key.split_once(':') {
        Some((_, model)) => non_blank(model),
        None => non_blank(key),
    }
}

/// A named bucket's `tokens`, where the bucket is `{ tokens, cost_usd }`.
fn bucket(entry: &Value, key: &str) -> u64 {
    count(&entry[key]["tokens"])
}

pub(super) fn read(path: &Path, _id: &str, cancel: &AtomicBool) -> Result<(Vec<SpendRecord>, Vec<String>), String> {
    let root: Value = document(path, cancel)?;
    let Some(by_model) = root["byModel"].as_object() else { return Ok((Vec::new(), Vec::new())); };
    // One timestamp per session. A file without a real one is skipped, never
    // dated from its own modification time.
    let Some(at) = event_time(&root["lastRequest"]["timestamp"], true) else { return Ok((Vec::new(), Vec::new())); };
    let session = path
        .parent()
        .and_then(Path::file_name)
        .map(|name| name.to_string_lossy().into_owned())
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| format!("{:016x}", hash(path.to_string_lossy().to_string())));
    let fallback = text(&root["lastRequest"]["model"]);
    let mut keys: Vec<&String> = by_model.keys().collect();
    keys.sort();
    let mut records = Vec::new();
    let mut notes = Vec::new();
    let mut partial = false;
    let mut capped = false;
    for key in keys {
        check(cancel)?;
        if records.len() >= MAX_RECORDS {
            if !capped {
                notes.push("Mux：记录数超过上限，计数可能不完整".into());
                capped = true;
            }
            continue;
        }
        let Some(entry) = by_model.get(key) else { continue; };
        if !entry.is_object() {
            continue;
        }
        let reasoning = bucket(entry, "reasoning");
        let tally = Tally {
            input: bucket(entry, "input"),
            output: bucket(entry, "output"),
            cache_write: bucket(entry, "cacheCreate"),
            cache_read: bucket(entry, "cached"),
            ..Default::default()
        };
        let Some(model) = key_model(key).or_else(|| fallback.clone()) else { continue; };
        if tally.total() == 0 {
            continue;
        }
        // A reported reasoning count that cannot be placed leaves the record
        // short of the session's real work; say so once.
        if reasoning > 0 {
            partial = true;
        }
        let mut spend = record("mux", model, at, session.clone(), tally, 0, Some(format!("mux:{session}:{key}")));
        // The timestamp is the session's last run, shared by every model: the
        // ledger must not draw an hourly profile from it.
        spend.aggregate_timing = true;
        records.push(spend);
    }
    if partial {
        notes.push("Mux：reasoning 与 output 的包含关系不明；未重复计入，计数可能偏少。".into());
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

        fn put(&self, relative: &str, content: impl AsRef<[u8]>) -> PathBuf {
            let path = self.home().join(relative);
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent).unwrap();
            }
            fs::write(&path, content).unwrap();
            path
        }

        fn put_root(&self, relative: &str, content: impl AsRef<[u8]>) -> PathBuf {
            let path = self.root.join(relative);
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
                |agent| if agent == "mux" { roots.clone() } else { Vec::new() },
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

    fn usage(reasoning: u64) -> String {
        json!({
            "version": 1,
            "byModel": {
                "anthropic:claude": {
                    "input": { "tokens": 100, "cost_usd": 0.01 },
                    "cached": { "tokens": 40, "cost_usd": 0.0 },
                    "cacheCreate": { "tokens": 10, "cost_usd": 0.01 },
                    "output": { "tokens": 50, "cost_usd": 0.1 },
                    "reasoning": { "tokens": reasoning, "cost_usd": 0.01 }
                },
                "broken:": { "input": { "tokens": 3 } },
                "openai:gpt": { "input": { "tokens": 5 }, "output": { "tokens": 6 } }
            },
            "lastRequest": { "model": "fallback-model", "timestamp": 1_700_000_000_000i64 },
            "prompt": "TOP-SECRET-PROMPT"
        })
        .to_string()
    }

    #[test]
    fn roots_and_candidate() {
        let home = Path::new("fixture-home");
        let env = |_: &str| -> Option<String> { None };
        assert_eq!(roots(home, "mux", &env), vec![home.join(".mux").join("sessions")]);
        assert!(candidate(Path::new("ws/session-usage.json"), "mux"));
        assert!(!candidate(Path::new("ws/session.json"), "mux"));
    }

    #[test]
    fn counters_shared_timestamp_and_partial() {
        let fixture = Fixture::new("mux-counters");
        let file = fixture.put(".mux/sessions/workspace-1/session-usage.json", usage(7));
        let (records, notes) = read(&file, "mux", &AtomicBool::new(false)).unwrap();
        assert_eq!(records.len(), 3);
        let claude = records.iter().find(|record| record.model == "claude").unwrap();
        assert_eq!(claude.session, "workspace-1");
        assert_eq!(claude.tally.input, 100);
        assert_eq!(claude.tally.cache_read, 40);
        assert_eq!(claude.tally.cache_write, 10);
        assert_eq!(claude.tally.output, 50);
        assert_eq!(claude.tally.total(), 200);
        // The ambiguous reasoning figure is not counted anywhere.
        assert_eq!(claude.unclassified_tokens, 0);
        assert_eq!(claude.aggregate_timing, true);
        assert_eq!(claude.source_timestamp, Some(1_700_000_000));
        assert_eq!(claude.deduplication_id.as_deref(), Some("mux:workspace-1:anthropic:claude"));
        assert!(claude.day.contains('-'));
        assert!(claude.hour < 24);
        // A blank name after the colon falls back to `lastRequest.model`.
        assert!(records.iter().any(|record| record.model == "fallback-model"));
        assert_eq!(notes.len(), 1);
        assert!(notes[0].contains("reasoning"));
        // Nothing prompt-shaped is kept.
        assert!(!serde_json::to_string(&records).unwrap().contains("TOP-SECRET"));

        let snapshot = fixture.scan(vec![fixture.home().join(".mux/sessions")]);
        assert_eq!(snapshot.records.len(), 3);
        assert_eq!(snapshot.records.iter().map(|record| record.tally.total()).sum::<u64>(), 214);
        // A reader note disables file caching, so no cache file retains text.
        assert_eq!(fs::read_dir(fixture.root.join("cache")).unwrap().count(), 0);
    }

    #[test]
    fn missing_timestamp_skips_the_file() {
        let fixture = Fixture::new("mux-no-time");
        let no_last = fixture.put(".mux/sessions/one/session-usage.json", json!({ "byModel": { "a:b": { "input": { "tokens": 5 } } } }).to_string());
        let zero = fixture.put(".mux/sessions/two/session-usage.json", json!({ "byModel": { "a:b": { "input": { "tokens": 5 } } }, "lastRequest": { "timestamp": 0 } }).to_string());
        assert!(read(&no_last, "mux", &AtomicBool::new(false)).unwrap().0.is_empty());
        assert!(read(&zero, "mux", &AtomicBool::new(false)).unwrap().0.is_empty());
        let snapshot = fixture.scan(vec![fixture.home().join(".mux/sessions")]);
        assert_eq!(snapshot.records.len(), 0);
    }

    #[test]
    fn replayed_snapshot_folds_across_roots() {
        let fixture = Fixture::new("mux-replay");
        let body = json!({
            "byModel": { "anthropic:claude": { "input": { "tokens": 10 }, "output": { "tokens": 2 } } },
            "lastRequest": { "timestamp": 1_700_000_000_000i64 }
        })
        .to_string();
        fixture.put_root("a/.mux/sessions/shared/session-usage.json", &body);
        fixture.put_root("b/.mux/sessions/shared/session-usage.json", &body);
        let snapshot = fixture.scan(vec![
            fixture.root.join("a/.mux/sessions"),
            fixture.root.join("b/.mux/sessions"),
        ]);
        assert_eq!(snapshot.records.len(), 1);
        assert_eq!(snapshot.records[0].tally.total(), 12);
    }

    #[test]
    fn malformed_json_is_reported_and_non_object_is_skipped() {
        let fixture = Fixture::new("mux-bad");
        let malformed = fixture.put(".mux/sessions/bad/session-usage.json", "not json at all");
        assert!(read(&malformed, "mux", &AtomicBool::new(false)).is_err());
        let list = fixture.put(".mux/sessions/list/session-usage.json", "[]");
        assert!(read(&list, "mux", &AtomicBool::new(false)).unwrap().0.is_empty());
    }
}
