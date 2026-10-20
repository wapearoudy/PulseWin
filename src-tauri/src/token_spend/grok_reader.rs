//! Pulse 1.9 GrokStore (Apache-2.0): Grok Build's transcripts under
//! `~/.grok/sessions/<percent-encoded cwd>/<run>/updates.jsonl`, one JSON object
//! per event. The event that counts is the end of a turn: `params.update` with
//! `sessionUpdate == "turn_completed"` and a `usage` object.
//!
//! **`modelUsage` is what names the model**, and a turn can touch more than one,
//! so its per-model counts are read and the flat totals beside it are used only
//! when it is absent. `inputTokens` already holds the cached read and
//! `outputTokens` the reasoning: `totalTokens` is their sum alone, so neither is
//! added again — adding them doubled the totals and held the hit rate under 50%
//! (#91). The run directory's parent is the working directory **percent-encoded**
//! (`%2FUsers%2Fme%2FCode`), the one agent here that gives its project away in
//! the path without losing anything.
//!
//! Nothing is estimated: a turn with usage but no stated `timestamp` is skipped
//! rather than dated from the file's modification time, and prompt text is never
//! retained — the opening message is not read at all, because a record has no
//! field for it.

use super::measured_readers::{lines, record};
use super::*;

pub(super) fn roots(home: &Path, id: &str, _env: &impl Fn(&str) -> Option<String>) -> Vec<PathBuf> {
    if id != "grok" { return Vec::new(); }
    // The Swift reader names this one root and honours no environment override;
    // `GROK_HOME` is a documented but unimplemented extra.
    vec![home.join(".grok/sessions")]
}

pub(super) fn candidate(path: &Path, id: &str) -> bool {
    id == "grok" && path.file_name().is_some_and(|name| name == "updates.jsonl")
}

pub(super) fn read(path: &Path, _id: &str, cancel: &AtomicBool) -> Result<(Vec<SpendRecord>, Vec<String>), String> {
    check(cancel)?;
    let file = File::open(path).map_err(|error| format!("Grok：会话记录无法读取（{error}）。"))?;
    let mut reader = BufReader::with_capacity(64 * 1024, file);
    let mut notes = Vec::new();
    let source = path.to_string_lossy().into_owned();
    // `sessions/<encoded cwd>/<run>/updates.jsonl`: the run directory is the
    // session, and the folder above it is the working directory as one
    // percent-encoded path component.
    let run = path.ancestors().nth(1).and_then(Path::file_name).map(|name| name.to_string_lossy().into_owned()).unwrap_or_default();
    let project = path.ancestors().nth(2).and_then(Path::file_name)
        .map(|name| name.to_string_lossy().into_owned())
        .and_then(|name| percent_decoded(&name));
    let mut out: Vec<SpendRecord> = Vec::new();
    let mut undated = false;
    lines(&mut reader, cancel, &mut notes, |row, index| {
        let update = &row["params"]["update"];
        if update["sessionUpdate"].as_str() != Some("turn_completed") { return Ok(()); }
        let usage = &update["usage"];
        if !usage.is_object() { return Ok(()); }
        let Some(at) = epoch(&row["timestamp"]) else { undated = true; return Ok(()); };
        // A `modelUsage` present but not a map of maps is absent, exactly as the
        // original's dictionary cast reads it. An empty one is a real answer.
        let per_model = usage["modelUsage"].as_object().filter(|models| models.values().all(Value::is_object));
        let models: Vec<(String, &Value)> = match per_model {
            Some(models) => models.iter().map(|(model, counts)| (model.clone(), counts)).collect(),
            None => vec![("grok".to_owned(), usage)],
        };
        for (model, counts) in models {
            // `inputTokens` already holds the cached read: subtract it once, and
            // never add the reasoning to output.
            let cache_read = whole(&counts["cachedReadTokens"]).unwrap_or(0);
            let tally = Tally {
                input: whole(&counts["inputTokens"]).unwrap_or(0).saturating_sub(cache_read),
                cache_write: whole(&counts["cacheCreationTokens"]).unwrap_or(0),
                cache_read,
                output: whole(&counts["outputTokens"]).unwrap_or(0),
                ..Default::default()
            };
            if tally.total() == 0 { continue; }
            if out.len() >= MAX_RECORDS { return Err("Grok：记录数超过读取上限。".into()); }
            let mut spent = record(
                "grok", model.clone(), at, run.clone(), tally, 0,
                Some(format!("grok:{source}:{index}:{model}")),
            );
            spent.project = project.clone();
            out.push(spent);
        }
        Ok(())
    })?;
    if undated { notes.push("Grok：部分回合缺少有效时间戳，计数可能不完整。".into()); }
    notes.sort();
    notes.dedup();
    check(cancel)?;
    Ok((out, notes))
}

/// The original reads a `Double` or an `Int` and nothing else, so a numeric
/// string is not a time here. A stated value is used as written, including a
/// zero: the field is the product's own, not a guess.
fn epoch(value: &Value) -> Option<DateTime<Utc>> {
    let seconds = value.as_f64().filter(|seconds| seconds.is_finite())?;
    let whole = seconds.trunc();
    if whole >= i64::MAX as f64 || whole <= i64::MIN as f64 { return None; }
    let fraction = ((seconds - whole).abs() * 1e9).round().min(999_999_999.) as u32;
    DateTime::from_timestamp(whole as i64, fraction)
}

/// `String.removingPercentEncoding`: `%XX` bytes decoded back to UTF-8. An
/// invalid escape or a byte sequence that is not UTF-8 is no project at all,
/// never a half-decoded name.
fn percent_decoded(name: &str) -> Option<String> {
    let bytes = name.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' {
            let hex = name.get(index + 1..index + 3)?;
            out.push(u8::from_str_radix(hex, 16).ok()?);
            index += 3;
        } else {
            out.push(bytes[index]);
            index += 1;
        }
    }
    String::from_utf8(out).ok()
}

/// AgentLogIO.count: a whole, non-negative count, or nil. nil is not zero, so a
/// missing field stays missing rather than being counted as a reported zero.
fn whole(value: &Value) -> Option<u64> {
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

    fn home() -> PathBuf { temp_dir("grok-home") }

    /// The run directory as Grok writes it: the working directory encoded as one
    /// path component, then the run's own id.
    fn run(root: &Path) -> PathBuf {
        let run = root.join("%2FUsers%2Fme%2FCode%2FPulse").join("01a01492");
        fs::create_dir_all(&run).unwrap();
        run
    }

    #[test]
    fn roots_and_candidate_follow_the_store_layout() {
        let home = home();
        assert_eq!(roots(&home, "grok", &|_: &str| None), vec![home.join(".grok/sessions")]);
        assert!(roots(&home, "devin-cli", &|_: &str| None).is_empty());
        assert!(candidate(Path::new("C:/Users/me/.grok/sessions/p/run/updates.jsonl"), "grok"));
        assert!(!candidate(Path::new("C:/Users/me/.grok/sessions/p/run/signals.json"), "grok"));
        assert!(!candidate(Path::new("a/updates.jsonl"), "devin-cli"));
        let _ = fs::remove_dir_all(&home);
    }

    #[test]
    fn a_turn_is_counted_per_model_and_the_cache_is_not_added_twice() {
        let home = home();
        let run = run(&home);
        let path = run.join("updates.jsonl");
        fs::write(&path, [
            r#"{"timestamp":1789372800,"params":{"update":{"sessionUpdate":"user_message_chunk","content":"Fix the ring"}}}"#,
            r#"{"timestamp":1789372800,"params":{"update":{"sessionUpdate":"turn_completed","usage":{"inputTokens":999,"outputTokens":999,"modelUsage":{"priced":{"inputTokens":33379,"outputTokens":91,"reasoningTokens":42,"cachedReadTokens":22272,"cacheCreationTokens":0,"totalTokens":33470}}}}}}"#,
        ].join("\n")).unwrap();
        let (records, notes) = read(&path, "grok", &AtomicBool::new(false)).unwrap();
        assert!(notes.is_empty(), "{notes:?}");
        assert_eq!(records.len(), 1);
        let spent = &records[0];
        assert_eq!(spent.model, "priced"); // modelUsage wins over the flat totals
        assert_eq!(spent.tally.input, 11107); // inputTokens already holds the cached read
        assert_eq!(spent.tally.cache_read, 22272);
        assert_eq!(spent.tally.cache_write, 0);
        assert_eq!(spent.tally.output, 91); // outputTokens already holds the reasoning
        assert_eq!(spent.tally.total(), 33470); // Grok Build's own totalTokens
        assert_eq!(spent.unclassified_tokens, 0);
        assert!(!spent.aggregate_timing);
        assert_eq!(spent.source_timestamp, Some(1_789_372_800));
        assert_eq!(spent.project.as_deref(), Some("/Users/me/Code/Pulse"));
        assert_eq!(spent.session, "01a01492");
        assert_eq!(spent.deduplication_id, Some(format!("grok:{}:2:priced", path.display())));
        let _ = fs::remove_dir_all(&home);
    }

    #[test]
    fn several_models_in_one_turn_are_separate_records_and_flat_totals_are_the_fallback() {
        let home = home();
        let run = run(&home);
        let path = run.join("updates.jsonl");
        fs::write(&path, [
            r#"{"timestamp":1789372800,"params":{"update":{"sessionUpdate":"turn_completed","usage":{"modelUsage":{"a":{"inputTokens":100,"outputTokens":10},"b":{"inputTokens":50,"cachedReadTokens":20,"outputTokens":5}}}}}}"#,
            r#"{"timestamp":1789372900,"params":{"update":{"sessionUpdate":"turn_completed","usage":{"inputTokens":30,"outputTokens":3}}}}"#,
            // Not a completed turn, and a completed turn with no usage.
            r#"{"timestamp":1789373000,"params":{"update":{"sessionUpdate":"agent_message_chunk"}}}"#,
            r#"{"timestamp":1789373100,"params":{"update":{"sessionUpdate":"turn_completed"}}}"#,
            // An empty modelUsage is a real answer, not a missing one.
            r#"{"timestamp":1789373200,"params":{"update":{"sessionUpdate":"turn_completed","usage":{"inputTokens":70,"outputTokens":7,"modelUsage":{}}}}}"#,
        ].join("\n")).unwrap();
        let (records, notes) = read(&path, "grok", &AtomicBool::new(false)).unwrap();
        assert_eq!(records.len(), 3, "{records:?}");
        assert_eq!(records[0].model, "a");
        assert_eq!(records[0].tally.total(), 110);
        assert_eq!(records[1].model, "b");
        assert_eq!(records[1].tally.input, 30); // 50 − 20 cached
        assert_eq!(records[1].tally.cache_read, 20);
        assert_eq!(records[2].model, "grok"); // the original's fallback key
        assert_eq!(records[2].tally.total(), 33);
        assert!(notes.is_empty(), "{notes:?}");
        let _ = fs::remove_dir_all(&home);
    }

    #[test]
    fn a_turn_without_a_stated_time_is_skipped_not_dated_from_the_file() {
        let home = home();
        let run = run(&home);
        let path = run.join("updates.jsonl");
        fs::write(&path, [
            r#"{"params":{"update":{"sessionUpdate":"turn_completed","usage":{"inputTokens":10,"outputTokens":1}}}}"#,
            r#"{"timestamp":"1789372800","params":{"update":{"sessionUpdate":"turn_completed","usage":{"inputTokens":20,"outputTokens":2}}}}"#,
            r#"{"timestamp":1789372800,"params":{"update":{"sessionUpdate":"turn_completed","usage":{"inputTokens":0,"outputTokens":0}}}}"#,
            "{not json",
        ].join("\n")).unwrap();
        let (records, notes) = read(&path, "grok", &AtomicBool::new(false)).unwrap();
        assert!(records.is_empty(), "{records:?}");
        assert!(notes.iter().any(|note| note.contains("缺少有效时间戳")), "{notes:?}");
        assert!(notes.iter().any(|note| note.contains("无法解码")), "{notes:?}");
        let _ = fs::remove_dir_all(&home);
    }

    #[test]
    fn an_invalid_percent_escape_names_no_project_and_prompt_text_is_never_retained() {
        let home = home();
        let run = home.join("%2FUsers%2Fme%2Fbad%ZZ").join("run-1");
        fs::create_dir_all(&run).unwrap();
        let path = run.join("updates.jsonl");
        fs::write(&path, [
            r#"{"timestamp":1789372800,"params":{"update":{"sessionUpdate":"user_message_chunk","content":"TOP-SECRET-GROK-PROMPT"}}}"#,
            r#"{"timestamp":1789372800,"params":{"update":{"sessionUpdate":"turn_completed","usage":{"modelUsage":{"priced":{"inputTokens":5,"outputTokens":1}}}}}}"#,
        ].join("\n")).unwrap();
        let (records, notes) = read(&path, "grok", &AtomicBool::new(false)).unwrap();
        assert!(notes.is_empty(), "{notes:?}");
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].project, None); // invalid escape, not a half-decoded path
        let serialized = serde_json::to_string(&records).unwrap();
        assert!(!serialized.contains("TOP-SECRET"), "{serialized}");
        let _ = fs::remove_dir_all(&home);
    }

    #[test]
    fn cancellation_stops_before_any_record_is_built() {
        let home = home();
        let run = run(&home);
        let path = run.join("updates.jsonl");
        fs::write(&path, r#"{"timestamp":1789372800,"params":{"update":{"sessionUpdate":"turn_completed","usage":{"inputTokens":5}}}}"#).unwrap();
        assert_eq!(read(&path, "grok", &AtomicBool::new(true)).unwrap_err(), "cancelled");
        let _ = fs::remove_dir_all(&home);
    }
}
