//! Pulse 1.9 CherryStudioReader: Cherry Studio's Claude-shaped agent
//! transcripts, under the app's `.claude/projects` in both the V1 layout and the
//! V2 `Data/Agents` one. Cherry appends the same API call several times while a
//! response streams, each copy with a fresh uuid but one `requestId`, so the
//! identity is the call and its counters merge by field-wise maximum — counting
//! lines would multiply every call by four. Nothing but counters is retained.
use super::measured_readers::{count, lines, record, text, timestamp};
use super::*;

fn expand(home: &Path, value: String) -> PathBuf {
    if let Some(rest) = value.strip_prefix("~/").or_else(|| value.strip_prefix("~\\")) { home.join(rest) } else { PathBuf::from(value) }
}

/// The V2 tree is listed before V1 everywhere so a same-named session in both
/// resolves to the current one. macOS `~/Library/Application Support` becomes
/// the Windows roaming app data directory.
pub(super) fn roots(home: &Path, id: &str, env: &impl Fn(&str) -> Option<String>) -> Vec<PathBuf> {
    if id != "cherrystudio" { return Vec::new(); }
    let support = env("APPDATA").filter(|v| !v.trim().is_empty())
        .map(|v| expand(home, v.trim().to_owned()))
        .unwrap_or_else(|| home.join("AppData/Roaming"));
    let base = support.join("CherryStudio");
    vec![
        base.join("Data/Agents/.claude/projects"),
        base.join(".claude/projects"),
        home.join(".config/CherryStudio/Data/Agents/.claude/projects"),
        home.join(".config/CherryStudio/.claude/projects"),
    ]
}

fn components(path: &Path) -> Vec<String> {
    path.components().map(|c| c.as_os_str().to_string_lossy().into_owned()).collect()
}

fn projects_index(parts: &[String]) -> Option<usize> {
    (0..parts.len().saturating_sub(1)).find(|i| parts[*i] == ".claude" && parts[i + 1] == "projects")
}

pub(super) fn candidate(path: &Path, id: &str) -> bool {
    if id != "cherrystudio" { return false; }
    if !path.extension().is_some_and(|e| e.eq_ignore_ascii_case("jsonl")) { return false; }
    let parts = components(path);
    let Some(index) = projects_index(&parts) else { return false; };
    // A V2 session is current; a V1 copy of the same relative path is skipped.
    if index >= 2 && parts[index - 1] == "Agents" && parts[index - 2] == "Data" { return true; }
    let mut mirror: Vec<String> = parts[..index].to_vec();
    mirror.push("Data".into()); mirror.push("Agents".into());
    mirror.extend_from_slice(&parts[index..]);
    !mirror.iter().fold(PathBuf::new(), |path, part| path.join(part)).exists()
}

/// The session path below `.claude/projects`: both trees hold the same relative
/// session path, and it is part of the record identity.
fn relative_path(path: &Path) -> String {
    let parts = components(path);
    match projects_index(&parts) {
        Some(index) if index + 2 < parts.len() => parts[index + 2..].join("/"),
        _ => path.file_name().map(|v| v.to_string_lossy().into_owned()).unwrap_or_default(),
    }
}

#[derive(Default)]
struct Snapshot { input: u64, cache_write: u64, cache_read: u64, output: u64, timestamp: Option<DateTime<Utc>>, model: Option<String> }

pub(super) fn read(path: &Path, id: &str, cancel: &AtomicBool) -> Result<(Vec<SpendRecord>, Vec<String>), String> {
    if id != "cherrystudio" { return Ok((Vec::new(), Vec::new())); }
    check(cancel)?;
    let file = File::open(path).map_err(|e| format!("无法读取 {}：{e}", path.display()))?;
    let mut reader = BufReader::with_capacity(64 * 1024, file);
    let session = path.file_stem().unwrap_or_default().to_string_lossy().into_owned();
    let project = path.parent().and_then(|v| v.file_name()).map(|v| v.to_string_lossy().into_owned()).filter(|v| !v.trim().is_empty());
    let relative = relative_path(path);

    let mut identified: HashMap<String, Snapshot> = HashMap::new();
    let mut order: Vec<String> = Vec::new();
    let mut unidentified: Vec<SpendRecord> = Vec::new();
    let mut notes = Vec::new();
    lines(&mut reader, cancel, &mut notes, |row, _| {
        check(cancel)?;
        if !row.is_object() || text(&row["type"]).as_deref() != Some("assistant") { return Ok(()); }
        let Some(message) = row.get("message").filter(|v| v.is_object()) else { return Ok(()); };
        let Some(usage) = message.get("usage").filter(|v| v.is_object()) else { return Ok(()); };
        let input = count(&usage["input_tokens"]);
        let cache_read = count(&usage["cache_read_input_tokens"]);
        let cache_write = count(&usage["cache_creation_input_tokens"]);
        let output = count(&usage["output_tokens"]);
        if input.saturating_add(cache_read).saturating_add(cache_write).saturating_add(output) == 0 { return Ok(()); }
        // The timestamp may sit on the row, on its message, or be absent; absent
        // is not newer evidence.
        let at = timestamp(&row["timestamp"], false).or_else(|| timestamp(&message["timestamp"], false));
        let identity = text(&row["requestId"]).or_else(|| text(&message["requestId"]))
            .or_else(|| text(&message["id"])).or_else(|| text(&row["uuid"]));
        let Some(model) = text(&message["model"]) else { return Ok(()); };
        let tally = Tally { input, cache_write, cache_read, output, ..Default::default() };
        let Some(identity) = identity else {
            let Some(at) = at else { return Ok(()); };
            if unidentified.len() >= MAX_RECORDS { return Err("Cherry Studio：记录数超过读取上限。".into()); }
            let mut spent = record("cherrystudio", model, at, session.clone(), tally, 0, None);
            spent.project = project.clone();
            unidentified.push(spent);
            return Ok(());
        };
        let snapshot = identified.entry(identity.clone()).or_insert_with(|| { order.push(identity.clone()); Snapshot::default() });
        if order.len() > MAX_RECORDS { return Err("Cherry Studio：记录数超过读取上限。".into()); }
        // Streaming snapshots are cumulative: max, never a sum.
        snapshot.input = snapshot.input.max(input);
        snapshot.cache_write = snapshot.cache_write.max(cache_write);
        snapshot.cache_read = snapshot.cache_read.max(cache_read);
        snapshot.output = snapshot.output.max(output);
        snapshot.model = snapshot.model.take().or(Some(model));
        if snapshot.timestamp.map_or(true, |seen| at.is_some_and(|at| at > seen)) { snapshot.timestamp = at; }
        Ok(())
    })?;
    let mut records = unidentified;
    for identity in order {
        let Some(snapshot) = identified.remove(&identity) else { continue; };
        let (Some(at), Some(model)) = (snapshot.timestamp, snapshot.model) else { continue; };
        let tally = Tally { input: snapshot.input, cache_write: snapshot.cache_write, cache_read: snapshot.cache_read, output: snapshot.output, ..Default::default() };
        let mut spent = record("cherrystudio", model, at, session.clone(), tally, 0, Some(format!("cherrystudio:{relative}:{identity}")));
        spent.project = project.clone();
        records.push(spent);
    }
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
            // The harness sandbox can deny writes to %TEMP% for test binaries;
            // fall back to a workspace-local scratch tree so fixtures stay
            // self-contained without depending on that access.
            let mut base = std::env::temp_dir().join("pulsewin-cherrystudio-tests");
            if fs::create_dir_all(&base).is_err() {
                base = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target/test-scratch/cherrystudio");
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

    #[test]
    fn roots_follow_windows_app_data_and_v2_first() {
        let home = Path::new("C:/Users/tester");
        let discovered = roots(home, "cherrystudio", &environment(&[("APPDATA", "C:/Users/tester/AppData/Roaming")]));
        assert_eq!(discovered.len(), 4);
        assert!(discovered[0].ends_with("CherryStudio/Data/Agents/.claude/projects"));
        assert!(discovered[1].ends_with("CherryStudio/.claude/projects"));
        assert_eq!(discovered[2], home.join(".config/CherryStudio/Data/Agents/.claude/projects"));
        assert_eq!(discovered[3], home.join(".config/CherryStudio/.claude/projects"));
        assert_eq!(roots(home, "commandcode", &environment(&[("APPDATA", "C:/app")])).len(), 0);
        let expanded = roots(home, "cherrystudio", &environment(&[("APPDATA", "~/roaming")]));
        assert_eq!(expanded[0], home.join("roaming/CherryStudio/Data/Agents/.claude/projects"));
        let fallback = roots(home, "cherrystudio", &environment(&[]));
        assert_eq!(fallback[0], home.join("AppData/Roaming/CherryStudio/Data/Agents/.claude/projects"));
    }

    #[test]
    fn candidate_takes_jsonl_and_skips_v1_mirrors() {
        let fixture = Fixture::new();
        let v2 = fixture.file("AppData/Roaming/CherryStudio/Data/Agents/.claude/projects/proj/s1.jsonl", "");
        let v1 = fixture.file("AppData/Roaming/CherryStudio/.claude/projects/proj/s1.jsonl", "");
        let v1_only = fixture.file("AppData/Roaming/CherryStudio/.claude/projects/proj/s2.jsonl", "");
        assert!(candidate(&v2, "cherrystudio"));
        assert!(!candidate(&v1, "cherrystudio"), "V2 copy is current");
        assert!(candidate(&v1_only, "cherrystudio"));
        assert!(!candidate(&fixture.file("AppData/Roaming/CherryStudio/notes.txt", ""), "cherrystudio"));
        assert!(!candidate(&v2, "commandcode"));
    }

    #[test]
    fn streaming_snapshots_merge_by_max_and_count_once() {
        let fixture = Fixture::new();
        let path = fixture.file("AppData/Roaming/CherryStudio/Data/Agents/.claude/projects/demo/session-a.jsonl",
            "{\"type\":\"assistant\",\"requestId\":\"req-1\",\"message\":{\"model\":\"claude-3-5-sonnet\",\"usage\":{\"input_tokens\":10,\"output_tokens\":2,\"cache_read_input_tokens\":5,\"cache_creation_input_tokens\":1}},\"timestamp\":\"2024-05-01T10:00:00Z\"}\n\
             {\"type\":\"assistant\",\"requestId\":\"req-1\",\"message\":{\"model\":\"claude-3-5-sonnet\",\"usage\":{\"input_tokens\":10,\"output_tokens\":40,\"cache_read_input_tokens\":5,\"cache_creation_input_tokens\":1}},\"timestamp\":\"2024-05-01T10:00:02Z\"}\n\
             {\"type\":\"assistant\",\"requestId\":\"req-1\",\"message\":{\"model\":\"claude-3-5-sonnet\",\"usage\":{\"input_tokens\":10,\"output_tokens\":41,\"cache_read_input_tokens\":25,\"cache_creation_input_tokens\":1}},\"timestamp\":\"2024-05-01T10:00:03Z\"}\n\
             {\"type\":\"assistant\",\"requestId\":\"req-2\",\"message\":{\"model\":\"claude-3-haiku\",\"usage\":{\"input_tokens\":7,\"output_tokens\":3}},\"timestamp\":\"2024-05-01T10:01:00Z\"}\n\
             {\"type\":\"user\",\"message\":{\"content\":\"MAGIC-PROMPT-TEXT\"}}\n\
             {\"type\":\"assistant\",\"message\":{\"model\":\"claude-3-5-sonnet\",\"usage\":{\"input_tokens\":11}}}\n\
             not json at all\n");
        let (records, notes) = read(&path, "cherrystudio", &cancel()).unwrap();
        assert_eq!(records.len(), 2, "three snapshots of one call collapse; no-attribution rows skipped");
        let first = &records[0];
        assert_eq!(first.agent, "cherrystudio");
        assert_eq!(first.model, "claude-3-5-sonnet");
        assert_eq!((first.tally.input, first.tally.output, first.tally.cache_read, first.tally.cache_write), (10, 41, 25, 1));
        assert_eq!(first.session, "session-a");
        assert_eq!(first.project.as_deref(), Some("demo"));
        assert_eq!(first.deduplication_id.as_deref(), Some("cherrystudio:demo/session-a.jsonl:req-1"));
        assert_eq!(first.day, "2024-05-01");
        assert_eq!(first.source_timestamp, Some(1_714_557_603));
        assert_eq!(records[1].tally.total() + records[1].unclassified_tokens, 10);
        assert!(!notes.is_empty(), "a broken line is reported");
        let encoded = serde_json::to_string(&records).unwrap();
        assert!(!encoded.contains("MAGIC-PROMPT-TEXT"));
    }

    #[test]
    fn unidentified_usage_is_kept_and_untimestamped_rows_skipped() {
        let fixture = Fixture::new();
        let path = fixture.file("AppData/Roaming/CherryStudio/.claude/projects/p/s.jsonl",
            "{\"type\":\"assistant\",\"message\":{\"model\":\"m\",\"usage\":{\"input_tokens\":4},\"timestamp\":\"2024-05-02T00:00:00Z\"}}\n\
             {\"type\":\"assistant\",\"message\":{\"model\":\"m\",\"usage\":{\"input_tokens\":9}}}\n\
             {\"type\":\"assistant\",\"message\":{\"model\":\"\",\"usage\":{\"input_tokens\":9}},\"timestamp\":\"2024-05-02T00:00:00Z\"}\n");
        let (records, notes) = read(&path, "cherrystudio", &cancel()).unwrap();
        assert!(notes.is_empty());
        assert_eq!(records.len(), 1, "no identity + no model + no timestamp are not records");
        assert_eq!(records[0].deduplication_id, None);
        assert_eq!(records[0].tally.total(), 4);
        let missing = read(&fixture.dir.join("nothing.jsonl"), "cherrystudio", &cancel()).unwrap_err();
        assert!(missing.contains("无法读取"));
    }
}
