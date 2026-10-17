//! Pulse 1.9 LMStudioUsageReader (Apache-2.0): `server-logs/**/*.log` holds
//! pretty-printed OpenAI-style records, not JSONL. Each `"usage"` object is capped
//! by the timestamp and model printed before it; `completion` already includes
//! reasoning and is counted once.

use super::measured_readers::record;
use super::*;
use chrono::{Local, NaiveDateTime, TimeZone};

const CONTEXT: usize = 4_096;
const MAX_LOG: u64 = 64 * 1024 * 1024;

pub(super) fn roots(home: &Path, id: &str, env: &impl Fn(&str) -> Option<String>) -> Vec<PathBuf> {
    if id != "lmstudio" { return Vec::new(); }
    let base = expanded(home, env, "LM_STUDIO_HOME").unwrap_or_else(|| home.join(".lmstudio"));
    vec![base.join("server-logs")]
}

pub(super) fn candidate(path: &Path, id: &str) -> bool {
    id == "lmstudio" && path.extension().is_some_and(|extension| extension == "log")
}

pub(super) fn read(path: &Path, _id: &str, cancel: &AtomicBool) -> Result<(Vec<SpendRecord>, Vec<String>), String> {
    check(cancel)?;
    let mut notes = Vec::new();
    let length = fs::metadata(path).map_err(|_| "LM Studio：日志无法读取。".to_string())?.len();
    if length > MAX_LOG {
        notes.push("LM Studio：日志超过 64 MiB，未读取，计数可能不完整。".into());
        return Ok((Vec::new(), notes));
    }
    let bytes = fs::read(path).map_err(|error| format!("LM Studio：日志无法读取（{error}）。"))?;
    let text = String::from_utf8_lossy(&bytes).into_owned();
    let source = path.to_string_lossy().into_owned();
    let session = format!("lmstudio:{source}");
    let mut out: Vec<SpendRecord> = Vec::new();
    let mut cursor = 0usize;
    let mut dropped = false;
    while let Some(relative) = text[cursor..].find("\"usage\"") {
        check(cancel)?;
        let marker = cursor + relative;
        cursor = marker + 7;
        let Some((start, end)) = usage_block(&text, cursor) else { continue; };
        let Ok(usage) = serde_json::from_str::<Value>(&text[start..end]) else { continue; };
        cursor = end;
        if !usage.is_object() { continue; }
        // Counters first: a block without any count is not a lost record.
        let Parts { tally, unclassified } = parts(&usage);
        if tally.total() == 0 && unclassified == 0 { continue; }
        let context = &text[context_start(&text, marker)..marker];
        let Some(at) = last_timestamp(context) else { dropped = true; continue; };
        let Some(model) = last_quoted(context, "model") else { dropped = true; continue; };
        if out.len() >= MAX_RECORDS { return Err("LM Studio：记录超过读取上限。".into()); }
        let identity = last_quoted(context, "id").unwrap_or_else(|| {
            let key = format!("{source}:{marker}:{model}:{}:{}:{}:{}", tally.input, tally.cache_write, tally.cache_read, tally.output);
            format!("{:016x}", hash(key))
        });
        out.push(record("lmstudio", model, at, session.clone(), tally, unclassified, Some(format!("lmstudio:{identity}"))));
    }
    if dropped { notes.push("LM Studio：日志中的用量记录缺少时间或模型，计数可能不完整。".into()); }
    notes.sort(); notes.dedup();
    check(cancel)?;
    Ok((out, notes))
}

struct Parts { tally: Tally, unclassified: u64 }

fn parts(usage: &Value) -> Parts {
    let prompt = first_count(usage, &["prompt_tokens", "promptTokens", "input_tokens", "inputTokens"]);
    let completion = first_count(usage, &["completion_tokens", "completionTokens", "output_tokens", "outputTokens"]);
    let reported_total = first_count(usage, &["total_tokens", "totalTokens"]);
    let prompt_details = first_object(usage, &["prompt_tokens_details", "input_tokens_details", "inputTokensDetails"]);
    let cached = whole_count(prompt_details.and_then(|details| details.get("cached_tokens")))
        .or_else(|| whole_count(usage.get("cached_tokens"))).unwrap_or(0);
    let cache_creation = whole_count(prompt_details.and_then(|details| details.get("cache_creation_input_tokens")))
        .or_else(|| whole_count(usage.get("cache_creation_input_tokens"))).unwrap_or(0);
    if prompt.is_none() && completion.is_none() {
        // A bare total names no kind: preserve the quantity, assign no split.
        return Parts { tally: Tally::default(), unclassified: reported_total.unwrap_or(0) };
    }
    let prompt = prompt.unwrap_or(0);
    let completion = completion.unwrap_or(0);
    let cache_read = cached.min(prompt);
    let cache_write = cache_creation.min(prompt.saturating_sub(cache_read));
    // `output_tokens_details.reasoning_tokens` is already inside completion; it is
    // deliberately not added a second time.
    let total = reported_total.unwrap_or(0).max(prompt.saturating_add(completion));
    let input = total.saturating_sub(completion).saturating_sub(cache_read).saturating_sub(cache_write);
    Parts { tally: Tally { input, cache_write, cache_read, output: completion, ..Default::default() }, unclassified: 0 }
}

fn first_count(usage: &Value, keys: &[&str]) -> Option<u64> {
    keys.iter().find_map(|key| whole_count(usage.get(*key)))
}

fn first_object<'a>(usage: &'a Value, keys: &[&str]) -> Option<&'a Value> {
    keys.iter().find_map(|key| usage.get(*key).filter(|value| value.is_object()))
}

/// The `{...}` value that follows the `"usage"` key, if the text is shaped like JSON.
fn usage_block(text: &str, from: usize) -> Option<(usize, usize)> {
    let bytes = text.as_bytes();
    let mut index = skip_whitespace(bytes, from);
    if bytes.get(index) != Some(&b':') { return None; }
    index = skip_whitespace(bytes, index + 1);
    if bytes.get(index) != Some(&b'{') { return None; }
    let start = index;
    let mut depth = 0usize;
    let mut string = false;
    let mut escaped = false;
    while index < bytes.len() {
        let byte = bytes[index];
        if string {
            if escaped { escaped = false; }
            else if byte == b'\\' { escaped = true; }
            else if byte == b'"' { string = false; }
        } else if byte == b'"' { string = true; }
        else if byte == b'{' { depth += 1; }
        else if byte == b'}' {
            depth = depth.saturating_sub(1);
            if depth == 0 { return Some((start, index + 1)); }
        }
        index += 1;
    }
    None
}

fn skip_whitespace(bytes: &[u8], mut index: usize) -> usize {
    while bytes.get(index).is_some_and(u8::is_ascii_whitespace) { index += 1; }
    index
}

/// At most 4096 characters of text before the marker, on a character boundary.
fn context_start(text: &str, marker: usize) -> usize {
    text[..marker].char_indices().rev().take(CONTEXT).last().map(|(index, _)| index).unwrap_or(0)
}

/// The last `YYYY-MM-DD HH:MM:SS` (or `T` separated) instant before the marker,
/// printed by the LM Studio server in local time.
fn last_timestamp(context: &str) -> Option<DateTime<Utc>> {
    let bytes = context.as_bytes();
    let mut found = None;
    let mut index = 0usize;
    while index + 19 <= bytes.len() {
        if is_timestamp_at(bytes, index) { found = Some(index); index += 19; } else { index += 1; }
    }
    let start = found?;
    let naive = NaiveDateTime::parse_from_str(&context[start..start + 19].replace('T', " "), "%Y-%m-%d %H:%M:%S").ok()?;
    Local.from_local_datetime(&naive).earliest().map(|at| at.with_timezone(&Utc))
}

fn is_timestamp_at(bytes: &[u8], index: usize) -> bool {
    let digit = |offset: usize| bytes[index + offset].is_ascii_digit();
    digit(0) && digit(1) && digit(2) && digit(3)
        && bytes[index + 4] == b'-' && digit(5) && digit(6)
        && bytes[index + 7] == b'-' && digit(8) && digit(9)
        && (bytes[index + 10] == b' ' || bytes[index + 10] == b'T')
        && digit(11) && digit(12) && bytes[index + 13] == b':' && digit(14) && digit(15)
        && bytes[index + 16] == b':' && digit(17) && digit(18)
}

/// The last `"key": "value"` capture in the context, the way the Swift regex reads it.
fn last_quoted(context: &str, key: &str) -> Option<String> {
    let pattern = format!("\"{key}\"");
    let bytes = context.as_bytes();
    let mut found: Option<String> = None;
    let mut from = 0usize;
    while let Some(relative) = context[from..].find(&pattern) {
        let after = from + relative + pattern.len();
        from = after;
        let mut index = skip_whitespace(bytes, after);
        if bytes.get(index) != Some(&b':') { continue; }
        index = skip_whitespace(bytes, index + 1);
        if bytes.get(index) != Some(&b'"') { continue; }
        let start = index + 1;
        let Some(length) = context[start..].find('"') else { continue; };
        let value = context[start..start + length].trim();
        if !value.is_empty() { found = Some(value.to_owned()); }
    }
    found
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

    fn home() -> PathBuf { temp_dir("lmstudio-home") }

    fn expected(at: (u32, u32, u32, u32, u32, u32)) -> i64 {
        let naive = chrono::NaiveDate::from_ymd_opt(at.0 as i32, at.1, at.2).unwrap().and_hms_opt(at.3, at.4, at.5).unwrap();
        Local.from_local_datetime(&naive).earliest().unwrap().timestamp()
    }

    #[test]
    fn roots_follow_lm_studio_home() {
        let home = home();
        assert_eq!(roots(&home, "lmstudio", &|_: &str| None), vec![home.join(".lmstudio").join("server-logs")]);
        let base = roots(&home, "lmstudio", &|key: &str| (key == "LM_STUDIO_HOME").then(|| "~/lm".to_string()));
        assert_eq!(base, vec![home.join("lm").join("server-logs")]);
        assert!(roots(&home, "reasonix", &|_: &str| None).is_empty());
        let _ = fs::remove_dir_all(&home);
    }

    #[test]
    fn candidate_accepts_logs_only() {
        assert!(candidate(Path::new("C:\\lmstudio\\server-logs\\2026-10-11.log"), "lmstudio"));
        assert!(!candidate(Path::new("C:\\lmstudio\\server-logs\\2026-10-11.jsonl"), "lmstudio"));
        assert!(!candidate(Path::new("C:\\lmstudio\\server-logs\\2026-10-11.log"), "reasonix"));
    }

    #[test]
    fn counts_follow_the_usage_block_and_reasoning_is_kept_once() {
        let home = home();
        let path = home.join("server.log");
        fs::write(&path, [
            "\"usage\": {\"prompt_tokens\": 9, \"completion_tokens\": 1}",
            "2026-10-11 10:00:00 [INFO] messages: [{\"role\": \"user\", \"content\": \"SECRET PROMPT\"}]",
            "2026-10-11 10:00:00 [INFO] \"model\": \"qwen2.5-7b\" \"id\": \"resp-0\"",
            "2026-10-11 10:00:01 [INFO] \"usage\": {\"prompt_tokens\": 100, \"completion_tokens\": 20, \"total_tokens\": 120, \"prompt_tokens_details\": {\"cached_tokens\": 40, \"cache_creation_input_tokens\": 10}, \"output_tokens_details\": {\"reasoning_tokens\": 5}}",
            "2026-10-11 10:00:02 [INFO] \"model\": \"qwen2.5-7b\" \"id\": \"resp-1\"",
            "2026-10-11 10:00:03 [INFO] \"usage\": {\"total_tokens\": 30}",
            "2026-10-11 10:00:04 [INFO] \"model\": \"qwen2.5-7b\" \"id\": \"resp-2\"",
            "2026-10-11 10:00:05 [INFO] \"usage\": {\"prompt_tokens\": 7, \"completion_tokens\": 3}",
        ].join("\n")).unwrap();
        let (records, notes) = read(&path, "lmstudio", &AtomicBool::new(false)).unwrap();
        assert_eq!(records.len(), 3, "{records:?}");
        let first = &records[0];
        assert_eq!(first.model, "qwen2.5-7b");
        assert_eq!(first.tally.input, 50); // 120 - 20 completion - 40 read - 10 write
        assert_eq!(first.tally.output, 20); // reasoning already inside completion
        assert_eq!(first.tally.cache_read, 40);
        assert_eq!(first.tally.cache_write, 10);
        assert_eq!(first.tally.total(), 120);
        assert_eq!(first.unclassified_tokens, 0);
        assert_eq!(first.deduplication_id.as_deref(), Some("lmstudio:resp-0"));
        assert_eq!(first.session, format!("lmstudio:{}", path.display()));
        assert_eq!(first.source_timestamp, Some(expected((2026, 10, 11, 10, 0, 1))));
        assert!(!first.aggregate_timing);
        let bare = &records[1];
        assert_eq!(bare.tally, Tally::default());
        assert_eq!(bare.unclassified_tokens, 30);
        // The first block has no timestamp before it: dropped, and said once.
        assert!(notes.iter().any(|note| note.contains("缺少时间或模型")), "{notes:?}");
        let serialized = serde_json::to_string(&records).unwrap();
        assert!(!serialized.contains("SECRET"));
        let _ = fs::remove_dir_all(&home);
    }

    #[test]
    fn fallback_identity_is_stable_when_the_block_has_no_id() {
        let home = home();
        let path = home.join("server.log");
        let fixture = [
            "2026-10-11 11:00:00 [INFO] \"model\": \"qwen2.5-7b\"",
            "2026-10-11 11:00:01 [INFO] \"usage\": {\"prompt_tokens\": 12, \"completion_tokens\": 4}",
            "2026-10-11 11:00:02 [INFO] \"usage\": {\"input_tokens\": 3, \"output_tokens\": 1}",
        ].join("\n");
        fs::write(&path, fixture).unwrap();
        let (first, notes) = read(&path, "lmstudio", &AtomicBool::new(false)).unwrap();
        let (second, _) = read(&path, "lmstudio", &AtomicBool::new(false)).unwrap();
        assert_eq!(first.len(), 2, "{first:?}");
        assert!(notes.is_empty(), "{notes:?}");
        assert_eq!(first[0].tally.input, 12);
        assert_eq!(first[0].tally.output, 4);
        assert_eq!(first[0].deduplication_id, second[0].deduplication_id);
        assert_ne!(first[0].deduplication_id, first[1].deduplication_id);
        let none = read(&path, "lmstudio", &AtomicBool::new(true)).unwrap_err();
        assert_eq!(none, "cancelled");
        let _ = fs::remove_dir_all(&home);
    }
}
