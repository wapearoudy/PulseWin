//! Pulse 1.9 ReasonixUsageReader (Apache-2.0): `stats/YYYY-MM-DD.jsonl` holds one
//! aggregate per request group, never the transcript. `completion` already contains
//! reasoning and is counted once; a bare total stays unclassified rather than guessed.

use super::measured_readers::{record, text, timestamp};
use super::*;

pub(super) fn roots(home: &Path, id: &str, env: &impl Fn(&str) -> Option<String>) -> Vec<PathBuf> {
    if id != "reasonix" { return Vec::new(); }
    let directory = expanded(home, env, "REASONIX_STATE_HOME")
        .or_else(|| expanded(home, env, "REASONIX_HOME").map(|base| base.join("stats")))
        .unwrap_or_else(|| home.join(".reasonix").join("stats"));
    vec![directory]
}

pub(super) fn candidate(path: &Path, id: &str) -> bool {
    id == "reasonix" && path.extension().is_some_and(|extension| extension == "jsonl")
}

pub(super) fn read(path: &Path, _id: &str, cancel: &AtomicBool) -> Result<(Vec<SpendRecord>, Vec<String>), String> {
    check(cancel)?;
    let file = File::open(path).map_err(|error| format!("Reasonix：统计文件无法读取（{error}）。"))?;
    let mut reader = BufReader::new(file);
    let mut notes = Vec::new();
    let source = path.to_string_lossy().into_owned();
    let session = format!("reasonix-stats:{source}");
    let mut out: Vec<SpendRecord> = Vec::new();
    let mut dropped = false;
    measured_readers::lines(&mut reader, cancel, &mut notes, |row, index| {
        if out.len() >= MAX_RECORDS { return Err("Reasonix：记录超过读取上限。".into()); }
        // `turn` marks where a request group ends; it is not a request.
        if row.get("turn").and_then(Value::as_bool) == Some(true) { return Ok(()); }
        let total = whole_count(row.get("total"));
        let requests = whole_count(row.get("requests"));
        let Some(model) = row.get("model").and_then(text) else {
            if total.unwrap_or(0) > 0 || requests.unwrap_or(0) > 0 { dropped = true; }
            return Ok(());
        };
        if total.unwrap_or(0) == 0 && requests.unwrap_or(0) == 0 { return Ok(()); }
        let Some(at) = timestamp(row.get("ts").unwrap_or(&Value::Null), false).filter(|at| at.timestamp() > 0) else {
            dropped = true;
            return Ok(());
        };
        let prompt = whole_count(row.get("prompt"));
        let completion = whole_count(row.get("completion"));
        let cache_read = whole_count(row.get("cache_hit")).unwrap_or(0);
        let (tally, unclassified) = if prompt.is_none() && completion.is_none() {
            (Tally::default(), total.unwrap_or(0))
        } else {
            // `cache_miss` is the reported fresh input; otherwise the prompt minus
            // the cache hit. `completion` already includes reasoning, kept once.
            let miss = whole_count(row.get("cache_miss")).unwrap_or(0);
            let input = if miss > 0 { miss } else { prompt.unwrap_or(0).saturating_sub(cache_read) };
            (Tally { input, output: completion.unwrap_or(0), cache_read, ..Default::default() }, 0)
        };
        if tally.total() == 0 && unclassified == 0 { return Ok(()); }
        out.push(record(
            "reasonix",
            model,
            at,
            session.clone(),
            tally,
            unclassified,
            Some(format!("reasonix:{source}:{index}:{}:{}", requests.unwrap_or(0), total.unwrap_or(0))),
        ));
        Ok(())
    })?;
    if dropped { notes.push("Reasonix：统计记录缺少模型或有效时间，计数可能不完整。".into()); }
    notes.sort(); notes.dedup();
    check(cancel)?;
    Ok((out, notes))
}

fn expanded(home: &Path, env: &impl Fn(&str) -> Option<String>, key: &str) -> Option<PathBuf> {
    let raw = env(key)?;
    let raw = raw.trim();
    if raw.is_empty() { return None; }
    if raw == "~" { return Some(home.to_path_buf()); }
    if let Some(rest) = raw.strip_prefix("~/").or_else(|| raw.strip_prefix("~\\")) { return Some(home.join(rest)); }
    Some(PathBuf::from(raw))
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

    fn home() -> PathBuf { temp_dir("reasonix-home") }

    #[test]
    fn roots_follow_reasonix_overrides() {
        let home = home();
        assert_eq!(roots(&home, "reasonix", &|_: &str| None), vec![home.join(".reasonix").join("stats")]);
        let state = roots(&home, "reasonix", &|key: &str| (key == "REASONIX_STATE_HOME").then(|| "~/state".to_string()));
        assert_eq!(state, vec![home.join("state")]);
        let base = roots(&home, "reasonix", &|key: &str| (key == "REASONIX_HOME").then(|| "~/reasonix".to_string()));
        assert_eq!(base, vec![home.join("reasonix").join("stats")]);
        let absolute = roots(&home, "reasonix", &|key: &str| (key == "REASONIX_STATE_HOME").then(|| "D:\\stats".to_string()));
        assert_eq!(absolute, vec![PathBuf::from("D:\\stats")]);
        assert!(roots(&home, "lmstudio", &|_: &str| None).is_empty());
        let _ = fs::remove_dir_all(&home);
    }

    #[test]
    fn candidate_accepts_stats_jsonl_only() {
        assert!(candidate(Path::new("C:\\reasonix\\stats\\2026-10-11.jsonl"), "reasonix"));
        assert!(!candidate(Path::new("C:\\reasonix\\stats\\readme.md"), "reasonix"));
        assert!(!candidate(Path::new("C:\\reasonix\\stats\\2026-10-11.jsonl"), "lmstudio"));
    }

    #[test]
    fn counts_are_mapped_once_and_bare_totals_unclassified() {
        let home = home();
        let path = home.join("2026-10-11.jsonl");
        fs::write(&path, [
            r#"{"ts":1700000000,"model":"openai/gpt-5","prompt":100,"completion":20,"reasoning":5,"cache_hit":40,"cache_miss":60,"total":120,"requests":2}"#,
            r#"{"ts":1700000100,"model":"gpt-5","prompt":50,"completion":10,"total":60}"#,
            r#"{"turn":true,"ts":1700000200,"model":"gpt-5","total":9999}"#,
            r#"{"ts":1700000300,"model":"gpt-5","total":30}"#,
            r#"{"ts":1700000400,"model":"gpt-5","requests":3}"#,
            r#"{"ts":1700000500,"total":10}"#,
            "{not json",
        ].join("\n")).unwrap();
        let (records, notes) = read(&path, "reasonix", &AtomicBool::new(false)).unwrap();
        assert_eq!(records.len(), 3, "{records:?}");
        let first = &records[0];
        assert_eq!(first.model, "openai/gpt-5"); // kept exactly as reported
        assert_eq!(first.session, format!("reasonix-stats:{}", path.display()));
        assert_eq!(first.tally.input, 60); // cache_miss is the reported fresh input
        assert_eq!(first.tally.output, 20); // reasoning is inside completion
        assert_eq!(first.tally.cache_read, 40);
        assert_eq!(first.tally.total(), 120);
        assert_eq!(first.unclassified_tokens, 0);
        assert!(!first.aggregate_timing);
        assert_eq!(first.source_timestamp, Some(1_700_000_000));
        let second = &records[1];
        assert_eq!(second.tally.input, 50); // no cache_miss: prompt minus cache hit
        assert_eq!(second.tally.output, 10);
        assert_eq!(second.tally.total(), 60);
        let bare = &records[2];
        assert_eq!(bare.tally, Tally::default());
        assert_eq!(bare.unclassified_tokens, 30);
        assert!(records.iter().all(|record| record.deduplication_id.as_deref().is_some_and(|id| id.starts_with("reasonix:"))));
        assert!(notes.iter().any(|note| note.contains("缺少模型或有效时间")), "{notes:?}");
        assert!(notes.iter().any(|note| note.contains("无法解码")), "{notes:?}");
        let serialized = serde_json::to_string(&records).unwrap();
        assert!(!serialized.contains("SECRET"));
        let _ = fs::remove_dir_all(&home);
    }

    #[test]
    fn prompt_only_and_cancellation_are_safe() {
        let home = home();
        let path = home.join("2026-10-12.jsonl");
        fs::write(&path, [
            r#"{"ts":1700000000,"model":"gpt-5","prompt":80,"cache_hit":20,"total":80}"#,
            r#"{"ts":1700000100,"model":"gpt-5","completion":15,"total":15}"#,
        ].join("\n")).unwrap();
        let (records, _) = read(&path, "reasonix", &AtomicBool::new(false)).unwrap();
        assert_eq!(records.len(), 2);
        assert_eq!(records[0].tally.input, 60);
        assert_eq!(records[0].tally.total(), 80);
        assert_eq!(records[1].tally.output, 15);
        let cancelled = read(&path, "reasonix", &AtomicBool::new(true));
        assert_eq!(cancelled.unwrap_err(), "cancelled");
        let _ = fs::remove_dir_all(&home);
    }
}
