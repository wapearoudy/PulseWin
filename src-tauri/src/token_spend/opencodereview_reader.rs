//! Pulse 1.9 OpenCodeReviewReader: one JSONL per session under
//! `~/.opencodereview/sessions/<encoded-repo>/`. A `session_start` line names
//! the cwd and default model; each `llm_response` carries the response's own
//! usage. The cache relation is **not assumed**: the store persists prompt,
//! completion and cache counts with no record of which provider semantics
//! produced them, so only the store's own total settles the overlap. Lines with
//! a positive cache but no total are skipped (their split is unprovable), and
//! the remaining confirmed subset is reported with one incomplete note.
use super::measured_readers::{count, lines, record, text, timestamp};
use super::*;
use std::io::Read;

pub(super) fn roots(home: &Path, id: &str, env: &impl Fn(&str) -> Option<String>) -> Vec<PathBuf> {
    if id != "opencodereview" { return Vec::new(); }
    let _ = env;
    vec![home.join(".opencodereview/sessions")]
}

pub(super) fn candidate(path: &Path, id: &str) -> bool {
    id == "opencodereview" && path.extension().is_some_and(|e| e.eq_ignore_ascii_case("jsonl"))
}

/// `provider/model` → bare model, as Pulse's price table publishes it.
fn model_id(raw: Option<String>) -> Option<String> {
    let raw = raw?;
    match raw.rfind('/') {
        Some(slash) => {
            let tail = &raw[slash + 1..];
            if tail.is_empty() { Some(raw) } else { Some(tail.to_owned()) }
        }
        None => Some(raw),
    }
}

/// A whole count that was actually written; absent and malformed are not zero.
fn reported(value: &Value) -> Option<u64> {
    if let Some(raw) = value.as_str() { return raw.trim().parse::<u64>().ok().filter(|v| *v <= i64::MAX as u64); }
    if let Some(raw) = value.as_u64() { return (raw <= i64::MAX as u64).then_some(raw); }
    value.as_f64().filter(|v| v.is_finite() && *v >= 0. && v.fract() == 0. && *v < 9_223_372_036_854_775_808.).map(|v| v as u64)
}

/// The first key that carries a whole count, preferring a positive one over a
/// reported zero — a zero in the first spelling is not evidence the second is
/// absent.
fn first_count(usage: &Value, keys: &[&str]) -> Option<u64> {
    let mut zero = None;
    for key in keys {
        let Some(value) = usage.get(*key) else { continue; };
        let Some(amount) = reported(value) else { continue; };
        if amount > 0 { return Some(amount); }
        if zero.is_none() { zero = Some(amount); }
    }
    zero
}

struct Reduction { tally: Tally, unclassified: u64, unprovable: bool }

/// One usage object reduced to disjoint buckets, or flagged unprovable.
fn parts(usage: &Value) -> Reduction {
    let prompt = first_count(usage, &["prompt_tokens", "promptTokens"]);
    let completion = first_count(usage, &["completion_tokens", "completionTokens"]);
    let cache_read = first_count(usage, &["cache_read_tokens", "cacheReadTokens"]);
    let cache_write = first_count(usage, &["cache_write_tokens", "cacheWriteTokens"]);
    let total = first_count(usage, &["total_tokens", "totalTokens", "total"]);

    let names_a_kind = prompt.is_some() || completion.is_some() || cache_read.is_some() || cache_write.is_some();
    if !names_a_kind && total.is_some_and(|total| total > 0) {
        return Reduction { tally: Tally::default(), unclassified: total.unwrap_or(0), unprovable: false };
    }

    let prompt_value = prompt.unwrap_or(0);
    let completion_value = completion.unwrap_or(0);
    let cache_read_value = cache_read.unwrap_or(0);
    let cache_write_value = cache_write.unwrap_or(0);
    let disjoint = prompt_value.saturating_add(completion_value).saturating_add(cache_read_value).saturating_add(cache_write_value);

    // A total of zero is not a usable total; it is treated as absent.
    if let Some(total) = total.filter(|total| *total > 0) {
        if total == disjoint {
            // The total names every kind, so the four are disjoint.
            return Reduction { tally: Tally { input: prompt_value, cache_write: cache_write_value, cache_read: cache_read_value, output: completion_value, ..Default::default() }, unclassified: 0, unprovable: false };
        }
        if total == prompt_value.saturating_add(completion_value) {
            // Only prompt + completion: prompt already contains the cache.
            return Reduction { tally: Tally { input: prompt_value.saturating_sub(cache_read_value.saturating_add(cache_write_value)), cache_write: cache_write_value, cache_read: cache_read_value, output: completion_value, ..Default::default() }, unclassified: 0, unprovable: false };
        }
        // The total fits neither shape: keep it whole rather than price a split
        // that could double count.
        return Reduction { tally: Tally::default(), unclassified: total, unprovable: false };
    }

    // No total to settle the overlap. With no cache nothing can overlap; with a
    // positive cache the split is unproven and no complete-looking number may
    // be produced.
    if cache_read_value != 0 || cache_write_value != 0 { return Reduction { tally: Tally::default(), unclassified: 0, unprovable: true }; }
    Reduction { tally: Tally { input: prompt_value, output: completion_value, ..Default::default() }, unclassified: 0, unprovable: false }
}

/// A deterministic whole-file digest used only as a fragment identity: a
/// byte-identical mirror folds, two different files do not. Pulse used SHA-256;
/// PulseWin has no hash dependency, so the same purpose is served by FNV-1a 64.
fn fragment(path: &Path, cancel: &AtomicBool) -> Result<String, String> {
    let mut file = File::open(path).map_err(|e| format!("无法读取 {}：{e}", path.display()))?;
    let mut buffer = [0u8; 64 * 1024];
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    loop {
        check(cancel)?;
        let read = file.read(&mut buffer).map_err(|e| format!("无法读取 {}：{e}", path.display()))?;
        if read == 0 { break; }
        for byte in &buffer[..read] { hash = (hash ^ *byte as u64).wrapping_mul(0x0000_0100_0000_01b3); }
    }
    Ok(format!("{hash:016x}"))
}

pub(super) fn read(path: &Path, id: &str, cancel: &AtomicBool) -> Result<(Vec<SpendRecord>, Vec<String>), String> {
    if id != "opencodereview" { return Ok((Vec::new(), Vec::new())); }
    check(cancel)?;
    let stem = path.file_stem().unwrap_or_default().to_string_lossy().into_owned();
    let digest = fragment(path, cancel)?;

    // First pass: the session header, which the response rows may omit.
    let mut session_from_start: Option<String> = None;
    let mut cwd: Option<String> = None;
    let mut start_model: Option<String> = None;
    {
        let file = File::open(path).map_err(|e| format!("无法读取 {}：{e}", path.display()))?;
        let mut reader = BufReader::with_capacity(64 * 1024, file);
        let mut scratch = Vec::new();
        lines(&mut reader, cancel, &mut scratch, |row, _| {
            check(cancel)?;
            if !row.is_object() || text(&row["type"]).as_deref() != Some("session_start") { return Ok(()); }
            if let Some(session) = text(&row["sessionId"]) { session_from_start = Some(session); }
            if let Some(directory) = text(&row["cwd"]) { cwd = Some(directory); }
            if let Some(model) = model_id(text(&row["model"])) { start_model = Some(model); }
            Ok(())
        })?;
    }

    // Second pass: the responses. The line index is the object's position, not
    // the physical line, matching the source order the Swift reader recorded.
    let mut records: Vec<SpendRecord> = Vec::new();
    let mut notes = Vec::new();
    let mut skipped_unprovable = false;
    let mut object_index = 0usize;
    {
        let file = File::open(path).map_err(|e| format!("无法读取 {}：{e}", path.display()))?;
        let mut reader = BufReader::with_capacity(64 * 1024, file);
        lines(&mut reader, cancel, &mut notes, |row, _| {
            check(cancel)?;
            if !row.is_object() { return Ok(()); }
            let index = object_index;
            object_index += 1;
            if text(&row["type"]).as_deref() != Some("llm_response") { return Ok(()); }
            let (Some(end), Some(usage)) = (timestamp(&row["timestamp"], true), row.get("usage").filter(|value| value.is_object())) else { return Ok(()); };
            let reduction = parts(usage);
            if reduction.unprovable { skipped_unprovable = true; return Ok(()); }
            if reduction.tally.total().saturating_add(reduction.unclassified) == 0 { return Ok(()); }
            // A real end timestamp plus a positive duration puts the request's
            // start at the end minus the duration.
            let duration = count(&row["duration_ms"]);
            let start = if duration > 0 && duration <= i64::MAX as u64 {
                end.checked_sub_signed(chrono::Duration::milliseconds(duration as i64)).unwrap_or(end)
            } else { end };
            let session = text(&row["sessionId"]).or_else(|| session_from_start.clone()).unwrap_or_else(|| stem.clone());
            let model = model_id(text(&row["model"])).or_else(|| start_model.clone());
            let Some(model) = model else { return Ok(()); };
            // A real per-request uuid folds a replay; without one the file's own
            // digest keeps two same-session fragments apart while folding a
            // byte-identical mirror.
            let identity = match text(&row["uuid"]) {
                Some(uuid) => format!("opencodereview:{session}:uuid:{uuid}"),
                None => format!("opencodereview:{session}:fragment:{digest}:line:{index}"),
            };
            if records.len() >= MAX_RECORDS { return Err("OpenCodeReview：记录数超过读取上限。".into()); }
            let mut spent = record("opencodereview", model, start, session, reduction.tally, reduction.unclassified, Some(identity));
            spent.project = cwd.clone();
            records.push(spent);
            Ok(())
        })?;
    }
    // A recognized usage was dropped because its cache relation could not be
    // proven; what remains is a confirmed subset, not the whole. PulseWin has no
    // partial flag, so the incompleteness is stated once in the notes.
    if skipped_unprovable { notes.push("OpenCodeReview：缓存关系无法确定，部分响应未计入，计数可能不完整".into()); }
    check(cancel)?;
    Ok((records, notes))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    static NEXT: AtomicUsize = AtomicUsize::new(0);

    struct Fixture { dir: PathBuf }
    impl Fixture {
        fn new() -> Self {
            let mut base = std::env::temp_dir().join("pulsewin-opencodereview-tests");
            if fs::create_dir_all(&base).is_err() {
                base = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target/test-scratch/opencodereview");
            }
            let dir = base.join(format!("{}-{}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
            let _ = fs::remove_dir_all(&dir); fs::create_dir_all(&dir).unwrap();
            Self { dir }
        }
        fn file(&self, relative: &str, content: &str) -> PathBuf {
            let path = self.dir.join(relative);
            fs::create_dir_all(path.parent().unwrap()).unwrap(); fs::write(&path, content).unwrap(); path
        }
    }
    impl Drop for Fixture { fn drop(&mut self) { let _ = fs::remove_dir_all(&self.dir); } }

    fn environment(pairs: &'static [(&'static str, &'static str)]) -> impl Fn(&str) -> Option<String> {
        move |key| pairs.iter().find(|(name, _)| *name == key).map(|(_, value)| (*value).to_owned())
    }
    fn cancel() -> AtomicBool { AtomicBool::new(false) }

    const HEADER: &str = "{\"type\":\"session_start\",\"sessionId\":\"repo-sess\",\"cwd\":\"E:/work/repo\",\"model\":\"gateway/start-model\"}\n";

    #[test]
    fn roots_and_candidate() {
        let home = Path::new("C:/Users/tester");
        assert_eq!(roots(home, "opencodereview", &environment(&[])), vec![home.join(".opencodereview/sessions")]);
        assert!(roots(home, "jcode", &environment(&[])).is_empty());
        assert!(candidate(Path::new("C:/u/.opencodereview/sessions/repo/s.jsonl"), "opencodereview"));
        assert!(!candidate(Path::new("C:/u/.opencodereview/sessions/repo/s.json"), "opencodereview"));
        assert!(!candidate(Path::new("C:/u/.opencodereview/sessions/repo/s.jsonl"), "commandcode"));
    }

    #[test]
    fn total_settles_cache_relation_and_unprovable_lines_are_skipped() {
        let fixture = Fixture::new();
        let path = fixture.file(".opencodereview/sessions/repo/session-1.jsonl", &format!("{HEADER}\
            {{\"type\":\"llm_response\",\"uuid\":\"u1\",\"timestamp\":1717200000000,\"duration_ms\":2000,\"usage\":{{\"prompt_tokens\":100,\"completion_tokens\":10,\"cache_read_tokens\":40,\"cache_write_tokens\":5,\"total_tokens\":110}}}}\n\
            {{\"type\":\"llm_response\",\"timestamp\":1717200001000,\"usage\":{{\"prompt_tokens\":10,\"completion_tokens\":5,\"cache_read_tokens\":2,\"cache_write_tokens\":1,\"total_tokens\":18}}}}\n\
            {{\"type\":\"llm_response\",\"uuid\":\"u-cache\",\"timestamp\":1717200002000,\"usage\":{{\"prompt_tokens\":50,\"completion_tokens\":5,\"cache_read_tokens\":9}}}}\n\
            {{\"type\":\"llm_response\",\"uuid\":\"no-cache\",\"timestamp\":1717200003000,\"usage\":{{\"prompt_tokens\":7,\"completion_tokens\":3}}}}\n\
            {{\"type\":\"llm_response\",\"uuid\":\"bare-total\",\"timestamp\":1717200004000,\"usage\":{{\"total_tokens\":99}}}}\n\
            {{\"type\":\"llm_response\",\"uuid\":\"odd-total\",\"timestamp\":1717200005000,\"usage\":{{\"prompt_tokens\":10,\"completion_tokens\":5,\"total_tokens\":12}}}}\n\
            {{\"type\":\"llm_response\",\"uuid\":\"u2\",\"sessionId\":\"repo-sess-2\",\"model\":\"gateway/model-x\",\"timestamp\":1717200006000,\"usage\":{{\"prompt_tokens\":4,\"completion_tokens\":2}}}}\n"));
        let (records, notes) = read(&path, "opencodereview", &cancel()).unwrap();
        assert_eq!(records.len(), 6, "the positive-cache-without-total line is unprovable and skipped");
        assert_eq!(notes.len(), 1);
        assert!(notes[0].contains("缓存关系无法确定"));
        let included = &records[0];
        assert_eq!(included.tally.input, 55, "cache overlap removed from prompt");
        assert_eq!((included.tally.output, included.tally.cache_read, included.tally.cache_write), (10, 40, 5));
        assert_eq!(included.model, "start-model");
        assert_eq!(included.session, "repo-sess");
        assert_eq!(included.project.as_deref(), Some("E:/work/repo"));
        assert_eq!(included.deduplication_id.as_deref(), Some("opencodereview:repo-sess:uuid:u1"));
        let disjoint = &records[1];
        assert_eq!((disjoint.tally.input, disjoint.tally.output, disjoint.tally.cache_read, disjoint.tally.cache_write), (10, 5, 2, 1));
        assert!(disjoint.deduplication_id.as_deref().unwrap().contains(":fragment:"));
        assert_eq!((records[2].tally.input, records[2].tally.output), (7, 3));
        assert_eq!(records[3].tally.total(), 0);
        assert_eq!(records[3].unclassified_tokens, 99);
        assert_eq!(records[4].unclassified_tokens, 12);
        assert_eq!(records[5].session, "repo-sess-2");
        assert_eq!(records[5].model, "model-x");
        // The start is the end minus the stated duration.
        assert_eq!(records[0].source_timestamp, Some(1_717_199_998));
        let encoded = serde_json::to_string(&records).unwrap();
        assert!(!encoded.contains("MAGIC"));
    }

    #[test]
    fn identical_mirror_folds_and_other_file_does_not() {
        let fixture = Fixture::new();
        let body = format!("{HEADER}{{\"type\":\"llm_response\",\"timestamp\":1717200000000,\"usage\":{{\"prompt_tokens\":1,\"completion_tokens\":1}}}}\n");
        let first = fixture.file(".opencodereview/sessions/a/session.jsonl", &body);
        let mirror = fixture.file(".opencodereview/sessions/mirror/session.jsonl", &body);
        let other = fixture.file(".opencodereview/sessions/other/session.jsonl",
            &format!("{HEADER}{{\"type\":\"llm_response\",\"timestamp\":1717200000000,\"usage\":{{\"prompt_tokens\":2,\"completion_tokens\":2}}}}\n"));
        let (a, _) = read(&first, "opencodereview", &cancel()).unwrap();
        let (b, _) = read(&mirror, "opencodereview", &cancel()).unwrap();
        let (c, _) = read(&other, "opencodereview", &cancel()).unwrap();
        assert_eq!(a[0].deduplication_id, b[0].deduplication_id, "byte-identical mirror folds");
        assert_ne!(a[0].deduplication_id, c[0].deduplication_id, "a different file stays distinct");
        assert!(!a.is_empty() && !b.is_empty() && !c.is_empty());
    }
}
