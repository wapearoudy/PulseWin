//! Pulse v1.9 FxUsageReader (Apache-2.0).
//! `usage-v2.json` is a per-session snapshot with per-model totals; timing comes
//! from the sibling `session.json` sidecar. A reported reasoning count is never
//! added to output and never classified, so it is surfaced as partial data.
use super::measured_readers::{count, document, record, text, timestamp};
use super::*;

pub(super) fn roots(home: &Path, id: &str, env: &impl Fn(&str) -> Option<String>) -> Vec<PathBuf> {
    let _ = (id, env);
    vec![home.join(".fx/sessions")]
}

pub(super) fn candidate(path: &Path, id: &str) -> bool {
    let _ = id;
    path.file_name().is_some_and(|name| name == "usage-v2.json")
}

/// A store's `0` is "unset", not 1970.
fn sidecar(path: &Path, cancel: &AtomicBool) -> Option<Value> {
    check(cancel).ok()?;
    let bytes = fs::read(path).ok()?;
    let bytes = bytes.strip_prefix(&[0xEF, 0xBB, 0xBF]).unwrap_or(&bytes);
    serde_json::from_slice(bytes).ok()
}

pub(super) fn read(path: &Path, _id: &str, cancel: &AtomicBool) -> Result<(Vec<SpendRecord>, Vec<String>), String> {
    let total = document::<Value>(path, cancel)?;
    if !total.is_object() || !total["snapshot"].is_object() {
        return Ok((Vec::new(), Vec::new()));
    }
    let snapshot = &total["snapshot"];
    let session = text(&total["session_id"]).unwrap_or_else(|| {
        path.parent()
            .and_then(Path::file_name)
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned()
    });
    // The sibling sidecar dates the snapshot and names the workspace. Without a
    // timestamp the file is not read; the product itself has no fallback.
    let meta = path.parent().map(|parent| sidecar(&parent.join("session.json"), cancel)).unwrap_or(None);
    let at = meta
        .as_ref()
        .and_then(|meta| timestamp(&meta["updated_at_ms"], true))
        .filter(|at| at.timestamp() > 0)
        .or_else(|| {
            meta.as_ref()
                .and_then(|meta| timestamp(&meta["created_at_ms"], true))
                .filter(|at| at.timestamp() > 0)
        });
    let Some(at) = at else { return Ok((Vec::new(), Vec::new())); };
    let workspace = meta.as_ref().and_then(|meta| text(&meta["workspace_root"]));
    let mut records = Vec::new();
    let mut notes = Vec::new();
    let mut partial = false;
    let mut emit = |model: String, entry: &Value, reasoning: u64| {
        let tally = Tally {
            input: count(&entry["input_tokens"]),
            output: count(&entry["output_tokens"]),
            cache_write: count(&entry["cache_write_tokens"]),
            cache_read: count(&entry["cache_read_tokens"]),
            ..Default::default()
        };
        if tally.total() == 0 {
            return;
        }
        partial |= reasoning > 0;
        if records.len() >= MAX_RECORDS {
            return;
        }
        let identity = format!("fx:{session}:{model}");
        let mut spend = record("fx", model, at, session.clone(), tally, 0, Some(identity));
        spend.aggregate_timing = true;
        spend.project = workspace.clone();
        records.push(spend);
    };
    if let Some(models) = snapshot["models"].as_array().filter(|models| !models.is_empty()) {
        for entry in models {
            check(cancel)?;
            let model = text(&entry["model"]).unwrap_or_else(|| "fx-unknown".into());
            emit(model, entry, count(&entry["reasoning_tokens"]));
        }
    } else {
        emit("fx-unknown".into(), snapshot, count(&snapshot["reasoning_tokens"]));
    }
    if records.len() >= MAX_RECORDS {
        notes.push("Fx：记录数超过上限，计数可能不完整".into());
    }
    if partial {
        notes.push("Fx：存在无法归类的 reasoning token，计数可能不完整".into());
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
            let sessions = self.root.join("home/.fx/sessions");
            scan_roots(
                &self.root.join("cache"),
                &Prices::new(),
                &AtomicBool::new(false),
                |_, _| {},
                |agent| if agent == "fx" { vec![sessions.clone()] } else { Vec::new() },
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

    fn usage(session: &str, snapshot: Value) -> String {
        json!({ "session_id": session, "snapshot": snapshot }).to_string()
    }

    fn sidecar(updated: Option<i64>, created: Option<i64>, workspace: &str) -> String {
        let mut meta = json!({ "workspace_root": workspace });
        if let Some(ms) = updated {
            meta["updated_at_ms"] = json!(ms);
        }
        if let Some(ms) = created {
            meta["created_at_ms"] = json!(ms);
        }
        meta.to_string()
    }

    #[test]
    fn roots_and_candidate() {
        let home = Path::new("fixture-home");
        let clean = |_: &str| -> Option<String> { None };
        assert_eq!(roots(home, "fx", &clean), vec![home.join(".fx/sessions")]);
        assert!(candidate(Path::new("a/s1/usage-v2.json"), "fx"));
        assert!(!candidate(Path::new("a/s1/usage.json"), "fx"));
        assert!(!candidate(Path::new("a/s1/usage-v2.json.bak"), "fx"));
    }

    #[test]
    fn per_model_totals_are_aggregate_records() {
        let fixture = Fixture::new("fx-models");
        let snapshot = json!({
            "models": [
                {
                    "model": "claude-sonnet-4",
                    "input_tokens": 30,
                    "output_tokens": 9,
                    "cache_write_tokens": 4,
                    "cache_read_tokens": 5,
                    "reasoning_tokens": 0
                },
                { "model": "gpt-5", "input_tokens": 2, "output_tokens": 1 }
            ]
        });
        let file = fixture.put(".fx/sessions/sid/usage-v2.json", usage("sid", snapshot));
        fixture.put(".fx/sessions/sid/session.json", sidecar(Some(1_700_000_000_000), None, "C:/work/fx"));
        let (records, notes) = read(&file, "fx", &AtomicBool::new(false)).unwrap();
        assert!(notes.is_empty());
        assert_eq!(records.len(), 2);
        assert_eq!(records[0].session, "sid");
        assert_eq!(records[0].project.as_deref(), Some("C:/work/fx"));
        assert_eq!(records[0].tally.input, 30);
        assert_eq!(records[0].tally.output, 9);
        assert_eq!(records[0].tally.cache_write, 4);
        assert_eq!(records[0].tally.cache_read, 5);
        assert_eq!(records[0].source_timestamp, Some(1_700_000_000));
        assert_eq!(records[0].aggregate_timing, true);
        assert_eq!(records[0].deduplication_id.as_deref(), Some("fx:sid:claude-sonnet-4"));
        assert_eq!(records[1].deduplication_id.as_deref(), Some("fx:sid:gpt-5"));
    }

    #[test]
    fn sidecar_created_time_is_the_fallback_and_no_time_skips_the_file() {
        let fixture = Fixture::new("fx-time");
        let snapshot = json!({ "models": [{ "model": "m", "input_tokens": 1 }] });
        let file = fixture.put(".fx/sessions/s1/usage-v2.json", usage("s1", snapshot.clone()));
        fixture.put(".fx/sessions/s1/session.json", sidecar(None, Some(1_600_000_000_000), ""));
        let (records, _) = read(&file, "fx", &AtomicBool::new(false)).unwrap();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].source_timestamp, Some(1_600_000_000));

        let undated = fixture.put(".fx/sessions/s2/usage-v2.json", usage("s2", snapshot));
        let (records, notes) = read(&undated, "fx", &AtomicBool::new(false)).unwrap();
        assert!(records.is_empty());
        assert!(notes.is_empty());
    }

    #[test]
    fn snapshot_totals_are_used_when_no_models_are_listed() {
        let fixture = Fixture::new("fx-total");
        let snapshot = json!({
            "input_tokens": 11,
            "output_tokens": 3,
            "cache_write_tokens": 2,
            "cache_read_tokens": 1
        });
        let file = fixture.put(".fx/sessions/s3/usage-v2.json", usage("s3", snapshot));
        fixture.put(".fx/sessions/s3/session.json", sidecar(Some(1_700_000_000_000), None, ""));
        let (records, _) = read(&file, "fx", &AtomicBool::new(false)).unwrap();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].model, "fx-unknown");
        assert_eq!(records[0].tally.total(), 17);
        assert_eq!(records[0].aggregate_timing, true);
        assert_eq!(records[0].deduplication_id.as_deref(), Some("fx:s3:fx-unknown"));
    }

    #[test]
    fn reasoning_is_never_counted_and_reports_partial_once() {
        let fixture = Fixture::new("fx-partial");
        let snapshot = json!({
            "models": [
                { "model": "a", "input_tokens": 5, "output_tokens": 2, "reasoning_tokens": 7 },
                { "model": "b", "input_tokens": 1, "output_tokens": 1, "reasoning_tokens": 2 }
            ]
        });
        let file = fixture.put(".fx/sessions/s4/usage-v2.json", usage("s4", snapshot));
        fixture.put(".fx/sessions/s4/session.json", sidecar(Some(1_700_000_000_000), None, ""));
        let (records, notes) = read(&file, "fx", &AtomicBool::new(false)).unwrap();
        assert_eq!(records.len(), 2);
        for record in &records {
            assert_eq!(record.unclassified_tokens, 0);
            assert_eq!(record.aggregate_timing, true);
        }
        assert_eq!(notes.len(), 1);
        assert!(notes[0].contains("reasoning") && notes[0].contains("计数可能不完整"));
    }

    #[test]
    fn replayed_snapshots_fold_in_a_scan() {
        let fixture = Fixture::new("fx-replay");
        let snapshot = json!({ "models": [{ "model": "m", "input_tokens": 6 }] });
        for relative in [".fx/sessions/same/usage-v2.json", ".fx/sessions/other/usage-v2.json"] {
            let file = fixture.put(relative, usage("same", snapshot.clone()));
            let parent = file.parent().unwrap();
            fs::write(parent.join("session.json"), sidecar(Some(1_700_000_000_000), None, "")).unwrap();
        }
        let snapshot = fixture.scan();
        // The two files describe the same session: same deduplication id, one record.
        assert_eq!(snapshot.records.len(), 1);
        assert_eq!(snapshot.records[0].tally.input, 6);
    }

    #[test]
    fn malformed_json_and_missing_snapshot_are_handled() {
        let fixture = Fixture::new("fx-bad");
        let broken = fixture.put(".fx/sessions/s5/usage-v2.json", "{not json");
        fixture.put(".fx/sessions/s5/session.json", sidecar(Some(1_700_000_000_000), None, ""));
        let error = read(&broken, "fx", &AtomicBool::new(false)).unwrap_err();
        assert!(error.contains("无法解码"));

        let no_snapshot = fixture.put(".fx/sessions/s6/usage-v2.json", json!({ "session_id": "s6" }).to_string());
        fixture.put(".fx/sessions/s6/session.json", sidecar(Some(1_700_000_000_000), None, ""));
        let (records, notes) = read(&no_snapshot, "fx", &AtomicBool::new(false)).unwrap();
        assert!(records.is_empty());
        assert!(notes.is_empty());
    }

    #[test]
    fn sensitive_text_is_never_retained() {
        let fixture = Fixture::new("fx-secret");
        let snapshot = json!({
            "models": [{ "model": "m", "input_tokens": 1 }],
            "prompt": "TOP-SECRET-FX"
        });
        let file = fixture.put(".fx/sessions/s7/usage-v2.json", usage("s7", snapshot));
        fixture.put(".fx/sessions/s7/session.json", sidecar(Some(1_700_000_000_000), None, "TOP-SECRET-WORKSPACE"));
        let (records, _) = read(&file, "fx", &AtomicBool::new(false)).unwrap();
        assert_eq!(records.len(), 1);
        // Workspace is a path, not conversation text; prompt content is dropped.
        assert!(!serde_json::to_string(&records).unwrap().contains("TOP-SECRET-FX"));
    }
}
