//! Pulse 1.9 CommandCodeReader: one JSONL per session under
//! `~/.commandcode/projects/<slug>/`. The modern format is a tree — every entry
//! names a `parentId` and `/rewind` leaves the abandoned branch's usage on disk,
//! which still counts. Message ids fold replays; ancestry only resolves the model
//! a reply used, so a model change on a sibling branch cannot reprice it. The
//! legacy flat format without a `usage` block contributes nothing: token counts
//! estimated from string lengths are not reported tokens.
use super::measured_readers::{count, document, lines, record, text, timestamp};
use super::*;

pub(super) fn roots(home: &Path, id: &str, env: &impl Fn(&str) -> Option<String>) -> Vec<PathBuf> {
    if id != "commandcode" { return Vec::new(); }
    let _ = env;
    vec![
        home.join(".commandcode/projects"),
        // The legacy model fallback lives here, so it is an input too.
        home.join(".commandcode/config.json"),
    ]
}

pub(super) fn candidate(path: &Path, id: &str) -> bool {
    if id != "commandcode" { return false; }
    let name = path.file_name().unwrap_or_default().to_string_lossy();
    path.extension().is_some_and(|e| e.eq_ignore_ascii_case("jsonl")) && !name.ends_with(".checkpoints.jsonl")
}

/// `provider/model` → bare model: Pulse's price table is keyed by the id
/// models.dev publishes, not by the gateway prefix.
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

struct Entry {
    index: usize,
    id: Option<String>,
    parent: Option<String>,
    kind: Option<String>,
    model: Option<String>,
    timestamp: Option<DateTime<Utc>>,
    session: Option<String>,
    tally: Option<Tally>,
}

/// The nearest stated model on this reply's own ancestry, never the last model
/// change in file order. Missing parents and cycles stop without borrowing a
/// sibling's model.
fn ancestor_model(parent: Option<String>, by_id: &HashMap<String, usize>, entries: &[Entry], cache: &mut HashMap<String, String>) -> Option<String> {
    let mut visited: Vec<String> = Vec::new();
    let mut cursor = parent;
    let mut model = None;
    while let Some(id) = cursor {
        if visited.iter().any(|seen| seen == &id) { break; }
        visited.push(id.clone());
        if let Some(cached) = cache.get(&id) { model = Some(cached.clone()); break; }
        let Some(ancestor) = by_id.get(&id).and_then(|index| entries.get(*index)) else { break; };
        if let Some(stated) = &ancestor.model { model = Some(stated.clone()); break; }
        cursor = ancestor.parent.clone();
    }
    if let Some(model) = &model { for id in visited { cache.insert(id, model.clone()); } }
    model
}

/// The legacy fallback model in the sibling `config.json`, read without an
/// ambient root list: the session path itself names the store directory.
fn config_model(path: &Path, cancel: &AtomicBool) -> Option<String> {
    let parts: Vec<String> = path.components().map(|c| c.as_os_str().to_string_lossy().into_owned()).collect();
    let index = parts.iter().position(|part| part == ".commandcode")?;
    let config = parts[..=index].iter().fold(PathBuf::new(), |path, part| path.join(part)).join("config.json");
    if !config.is_file() { return None; }
    let value: Value = document(&config, cancel).ok()?;
    model_id(text(&value["model"]))
}

pub(super) fn read(path: &Path, id: &str, cancel: &AtomicBool) -> Result<(Vec<SpendRecord>, Vec<String>), String> {
    if id != "commandcode" { return Ok((Vec::new(), Vec::new())); }
    check(cancel)?;
    if path.file_name().is_some_and(|name| name == "config.json") { return Ok((Vec::new(), Vec::new())); }
    let fallback = config_model(path, cancel);
    let file = File::open(path).map_err(|e| format!("无法读取 {}：{e}", path.display()))?;
    let mut reader = BufReader::with_capacity(64 * 1024, file);
    let stem = path.file_stem().unwrap_or_default().to_string_lossy().into_owned();
    let mut header_session: Option<String> = None;
    let mut entries: Vec<Entry> = Vec::new();
    let mut notes = Vec::new();
    let mut decoded_index = 0usize;
    lines(&mut reader, cancel, &mut notes, |row, _| {
        check(cancel)?;
        if !row.is_object() { return Ok(()); }
        let row_index = decoded_index;
        decoded_index += 1;
        let kind = text(&row["type"]);
        if kind.as_deref() == Some("session") {
            if let Some(id) = text(&row["id"]) { header_session = Some(id); }
            return Ok(());
        }
        let message = row.get("message").filter(|value| value.is_object());
        let role = message.and_then(|message| text(&message["role"])).or_else(|| text(&row["role"]));
        let tally = row.get("usage").filter(|value| value.is_object()).map(|usage| Tally {
            input: count(&usage["inputTokens"]),
            cache_write: count(&usage["cacheWriteTokens"]),
            cache_read: count(&usage["cacheReadTokens"]),
            output: count(&usage["outputTokens"]),
            ..Default::default()
        });
        if entries.len() >= MAX_RECORDS { return Err("Command Code：记录数超过读取上限。".into()); }
        let at = if role.as_deref() == Some("assistant") { timestamp(&row["timestamp"], false) } else { None };
        // Every object row is an entry, including non-assistant ones: they are
        // the nodes ancestry walks through.
        entries.push(Entry {
            index: row_index, id: text(&row["id"]), parent: text(&row["parentId"]), kind,
            model: model_id(text(&row["model"])), timestamp: at, session: text(&row["sessionId"]), tally,
        });
        Ok(())
    })?;
    let has_tree = entries.iter().any(|entry| entry.parent.is_some());
    let mut by_id: HashMap<String, usize> = HashMap::new();
    for (position, entry) in entries.iter().enumerate() {
        if let Some(id) = &entry.id { by_id.insert(id.clone(), position); }
    }
    let mut inherited: HashMap<String, String> = HashMap::new();
    let mut current_model: Option<String> = None;
    let mut records = Vec::new();
    for entry in &entries {
        check(cancel)?;
        if entry.kind.as_deref() == Some("model_change") {
            if let Some(model) = &entry.model { current_model = Some(model.clone()); }
            continue;
        }
        let (Some(timestamp), Some(tally)) = (entry.timestamp, &entry.tally) else { continue; };
        if tally.total() == 0 { continue; }
        let model = entry.model.clone()
            .or_else(|| if has_tree { ancestor_model(entry.parent.clone(), &by_id, &entries, &mut inherited) } else { current_model.clone() })
            .or_else(|| fallback.clone());
        let Some(model) = model else { continue; };
        let session = header_session.clone().or_else(|| entry.session.clone()).unwrap_or_else(|| stem.clone());
        let identity = match &entry.id {
            Some(id) => format!("commandcode:{session}:{id}"),
            None => format!("commandcode:{session}:line{}:{}", entry.index, timestamp.timestamp()),
        };
        records.push(record("commandcode", model, timestamp, session, tally.clone(), 0, Some(identity)));
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
            let mut base = std::env::temp_dir().join("pulsewin-commandcode-tests");
            if fs::create_dir_all(&base).is_err() {
                base = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target/test-scratch/commandcode");
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
    fn roots_and_candidate() {
        let home = Path::new("C:/Users/tester");
        let discovered = roots(home, "commandcode", &environment(&[]));
        assert_eq!(discovered, vec![home.join(".commandcode/projects"), home.join(".commandcode/config.json")]);
        assert!(roots(home, "jcode", &environment(&[])).is_empty());
        assert!(candidate(Path::new("C:/u/.commandcode/projects/p/s.jsonl"), "commandcode"));
        assert!(!candidate(Path::new("C:/u/.commandcode/projects/p/s.checkpoints.jsonl"), "commandcode"));
        assert!(!candidate(Path::new("C:/u/.commandcode/config.json"), "commandcode"));
        assert!(!candidate(Path::new("C:/u/.commandcode/projects/p/s.jsonl"), "cherrystudio"));
    }

    #[test]
    fn tree_replies_count_ancestry_resolves_model_and_replays_fold() {
        let fixture = Fixture::new();
        fixture.file(".commandcode/config.json", "{\"model\":\"gateway/config-model\"}");
        let path = fixture.file(".commandcode/projects/demo/session-1.jsonl",
            "{\"type\":\"session\",\"id\":\"sess-1\"}\n\
             {\"type\":\"message\",\"role\":\"user\",\"id\":\"u1\",\"model\":\"gateway/model-a\",\"content\":\"MAGIC-PROMPT\"}\n\
             {\"type\":\"model_change\",\"model\":\"gateway/model-b\"}\n\
             {\"type\":\"message\",\"role\":\"assistant\",\"id\":\"a1\",\"parentId\":\"u1\",\"timestamp\":\"2024-06-01T01:00:00Z\",\"usage\":{\"inputTokens\":10,\"cacheWriteTokens\":2,\"cacheReadTokens\":3,\"outputTokens\":4}}\n\
             {\"type\":\"message\",\"role\":\"assistant\",\"id\":\"a2\",\"parentId\":\"u1\",\"model\":\"gateway/model-c\",\"timestamp\":\"2024-06-01T01:00:05Z\",\"usage\":{\"inputTokens\":1,\"outputTokens\":1}}\n\
             {\"type\":\"message\",\"role\":\"assistant\",\"id\":\"a3\",\"parentId\":\"gone\",\"timestamp\":\"2024-06-01T01:00:06Z\",\"usage\":{\"inputTokens\":2}}\n\
             {\"type\":\"message\",\"role\":\"assistant\",\"id\":\"legacy\",\"timestamp\":\"2024-06-01T01:00:07Z\"}\n\
             {\"type\":\"message\",\"role\":\"assistant\",\"id\":\"a1\",\"parentId\":\"u1\",\"timestamp\":\"2024-06-01T01:00:00Z\",\"usage\":{\"inputTokens\":10,\"cacheWriteTokens\":2,\"cacheReadTokens\":3,\"outputTokens\":4}}\n\
             not json\n");
        let (records, notes) = read(&path, "commandcode", &cancel()).unwrap();
        assert!(!notes.is_empty());
        assert_eq!(records.len(), 4, "abandoned branches count, no-usage rows do not, replay stays a replay");
        assert_eq!(records[0].model, "model-a");
        assert_eq!((records[0].tally.input, records[0].tally.cache_write, records[0].tally.cache_read, records[0].tally.output), (10, 2, 3, 4));
        assert_eq!(records[0].session, "sess-1");
        assert_eq!(records[0].deduplication_id.as_deref(), Some("commandcode:sess-1:a1"));
        assert_eq!(records[1].model, "model-c");
        assert_eq!(records[2].model, "config-model", "no ancestry falls back to the legacy config model");
        assert_eq!(records[2].deduplication_id.as_deref(), Some("commandcode:sess-1:a3"));
        assert_eq!(records[3].deduplication_id, records[0].deduplication_id);
        assert_eq!(records[0].project, None);
        let encoded = serde_json::to_string(&records).unwrap();
        assert!(!encoded.contains("MAGIC-PROMPT"));
    }

    #[test]
    fn no_id_fallback_identity_is_stable_and_flat_format_works() {
        let fixture = Fixture::new();
        let path = fixture.file(".commandcode/projects/p/flat.jsonl",
            "{\"role\":\"assistant\",\"model\":\"m/n\",\"timestamp\":\"2024-06-02T00:00:00Z\",\"usage\":{\"inputTokens\":3}}\n");
        let (records, notes) = read(&path, "commandcode", &cancel()).unwrap();
        assert!(notes.is_empty());
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].model, "n");
        assert_eq!(records[0].session, "flat");
        assert_eq!(records[0].deduplication_id.as_deref(), Some("commandcode:flat:line0:1717286400"));
        assert_eq!(read(&fixture.dir.join("missing.jsonl"), "commandcode", &cancel()).unwrap_err().contains("无法读取"), true);
    }
}
