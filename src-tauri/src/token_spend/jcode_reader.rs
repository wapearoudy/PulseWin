//! Pulse 1.9 JcodeUsageReader: `~/.jcode/sessions/session_*.json` (or
//! `$JCODE_HOME/sessions`) carries a session's messages, and the sibling
//! `session_*.journal.jsonl` carries later appends and the model a journal
//! entry was written under. The format states neither the cache convention nor
//! the reasoning relationship, so neither is guessed: an ambiguous positive
//! cache leaves the input unclassified and the cache uncounted, and a
//! separately reported reasoning count is left out of output with the record
//! incomplete. One record per message that actually reported usage.
use super::measured_readers::{count, document, lines, record, text, timestamp};
use super::*;

pub(super) fn roots(home: &Path, id: &str, env: &impl Fn(&str) -> Option<String>) -> Vec<PathBuf> {
    if id != "jcode" { return Vec::new(); }
    let base = env("JCODE_HOME")
        .map(|raw| raw.trim().to_owned())
        .filter(|raw| !raw.is_empty())
        .map(|raw| {
            if let Some(rest) = raw.strip_prefix("~/").or_else(|| raw.strip_prefix("~\\")) { home.join(rest) } else { PathBuf::from(raw) }
        })
        .unwrap_or_else(|| home.join(".jcode"));
    vec![base.join("sessions")]
}

pub(super) fn candidate(path: &Path, id: &str) -> bool {
    if id != "jcode" { return false; }
    let Some(name) = path.file_name().and_then(|name| name.to_str()) else { return false; };
    name.starts_with("session_") && path.extension().is_some_and(|extension| extension.eq_ignore_ascii_case("json"))
}

/// A reported event time must be strictly after the epoch; a zero is the
/// store's "not set", not 1970-01-01.
fn event_time(value: &Value) -> Option<DateTime<Utc>> {
    let at = timestamp(value, false)?;
    (at.timestamp() > 0).then_some(at)
}

enum CacheShape { AnthropicDisjoint, OpenAiContained, Ambiguous }

/// Jcode never states which provider's cache convention wrote a usage object,
/// and the documentation says not to infer it from magnitude: only an
/// Anthropic-shaped key set (creation accompanies read, disjoint from input) or
/// an OpenAI-shaped details object names the convention.
fn cache_shape(usage: &Value) -> CacheShape {
    if usage.get("cache_creation_input_tokens").is_some() { return CacheShape::AnthropicDisjoint; }
    for key in ["prompt_tokens_details", "promptTokensDetails", "input_tokens_details", "inputTokensDetails"] {
        if usage.get(key).is_some() { return CacheShape::OpenAiContained; }
    }
    CacheShape::Ambiguous
}

pub(super) fn read(path: &Path, id: &str, cancel: &AtomicBool) -> Result<(Vec<SpendRecord>, Vec<String>), String> {
    if id != "jcode" { return Ok((Vec::new(), Vec::new())); }
    check(cancel)?;
    let stem = path.file_stem().unwrap_or_default().to_string_lossy().into_owned();
    let Ok(session) = document::<Value>(path, cancel) else {
        return Ok((Vec::new(), vec!["JCode：无法解析会话文件，计数可能不完整".into()]));
    };
    let session_id = text(&session["id"]).unwrap_or_else(|| stem.clone());
    let workspace = text(&session["working_dir"]);
    let session_model = text(&session["model"]);

    let mut records: Vec<SpendRecord> = Vec::new();
    let mut notes: Vec<String> = Vec::new();
    let mut ambiguous_cache = false;
    let mut omitted_reasoning = false;
    let mut emit = |message: &Value, tagged: Option<&String>| -> Result<(), String> {
        check(cancel)?;
        let Some(usage) = message.get("token_usage").filter(|usage| usage.is_object()) else { return Ok(()); };
        let Some(at) = event_time(&message["timestamp"]) else { return Ok(()); };
        let reported_input = count(&usage["input_tokens"]);
        let cache_read = count(&usage["cache_read_input_tokens"]);
        let cache_write = count(&usage["cache_creation_input_tokens"]);
        let reported_output = count(&usage["output_tokens"]);
        // A separately reported reasoning count stays out of output: the format
        // does not say whether output already contains it, and adding it would
        // be a guess that double counts when it does.
        let reasoning = count(&usage["reasoning_output_tokens"]);
        if reasoning > 0 { omitted_reasoning = true; }
        let (tally, unclassified) = match cache_shape(usage) {
            CacheShape::AnthropicDisjoint => (Tally { input: reported_input, cache_write, cache_read, output: reported_output, ..Default::default() }, 0),
            CacheShape::OpenAiContained => (Tally { input: reported_input.saturating_sub(cache_read.min(reported_input)), cache_write, cache_read, output: reported_output, ..Default::default() }, 0),
            CacheShape::Ambiguous => {
                if cache_read > 0 {
                    // Positive cache under no stated convention: the input may
                    // already contain it, so it cannot be classified and the
                    // cache cannot be counted.
                    ambiguous_cache = true;
                    (Tally { output: reported_output, ..Default::default() }, reported_input)
                } else {
                    (Tally { input: reported_input, output: reported_output, ..Default::default() }, 0)
                }
            }
        };
        // The session header names the model; a journal `meta.model` names a
        // later switch for the appends after it. A per-message model is not
        // consulted, matching the store's own tagging.
        let Some(model) = tagged.cloned() else { return Ok(()); };
        if tally.total().saturating_add(unclassified) == 0 { return Ok(()); }
        let name = text(&message["id"]).unwrap_or_else(|| format!("{session_id}:{}:{model}:{reported_input}:{cache_write}:{cache_read}:{reported_output}", at.timestamp()));
        if records.len() >= MAX_RECORDS { return Err("JCode：记录数超过读取上限。".into()); }
        let mut spent = record("jcode", model, at, session_id.clone(), tally, unclassified, Some(format!("jcode:{session_id}:{name}")));
        spent.project = workspace.clone();
        records.push(spent);
        Ok(())
    };

    if let Some(messages) = session["messages"].as_array() {
        for message in messages {
            if message.is_object() { emit(message, session_model.as_ref())?; }
        }
    }

    // The journal beside the session file replays later appends. Each append is
    // tagged with the model in force when it was written, so an entry replayed
    // after a model switch is not repriced.
    let journal = path.with_file_name(format!("{stem}.journal.jsonl"));
    if let Ok(file) = File::open(&journal) {
        let mut reader = BufReader::with_capacity(64 * 1024, file);
        let mut journal_model = session_model.clone();
        lines(&mut reader, cancel, &mut notes, |line, _| {
            check(cancel)?;
            if !line.is_object() { return Ok(()); }
            if let Some(meta) = line.get("meta").filter(|meta| meta.is_object()) {
                if let Some(model) = text(&meta["model"]) { journal_model = Some(model); }
            }
            if let Some(appends) = line["append_messages"].as_array() {
                for message in appends {
                    if message.is_object() { emit(message, journal_model.as_ref())?; }
                }
            }
            Ok(())
        })?;
    }

    // One incompleteness note per ambiguity kind; notes disable this file's
    // cache, so they are stated once rather than per record.
    if ambiguous_cache { notes.push("JCode：缓存关系无法确定，输入未分类且缓存未计入".into()); }
    if omitted_reasoning { notes.push("JCode：推理计数未计入输出，记录可能不完整".into()); }
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
            let mut base = std::env::temp_dir().join("pulsewin-jcode-tests");
            if fs::create_dir_all(&base).is_err() {
                base = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target/test-scratch/jcode");
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
    fn roots_respect_jcode_home_and_home() {
        let home = Path::new("C:/Users/tester");
        assert_eq!(roots(home, "jcode", &environment(&[])), vec![home.join(".jcode/sessions")]);
        assert_eq!(roots(home, "jcode", &environment(&[("JCODE_HOME", "D:/tools/jcode")])), vec![PathBuf::from("D:/tools/jcode/sessions")]);
        assert_eq!(roots(home, "jcode", &environment(&[("JCODE_HOME", "~/jcode-home")])), vec![home.join("jcode-home/sessions")]);
        assert!(roots(home, "cherrystudio", &environment(&[])).is_empty());
    }

    #[test]
    fn candidate_accepts_only_session_json() {
        let accept = [ "C:/u/.jcode/sessions/session_abc.json", "C:/u/.jcode/sessions/sub/session_1.json" ];
        for path in accept { assert!(candidate(Path::new(path), "jcode")); }
        for path in ["C:/u/.jcode/sessions/session_abc.jsonl", "C:/u/.jcode/sessions/other.json", "C:/u/.jcode/sessions/session_abc.txt"] {
            assert!(!candidate(Path::new(path), "jcode"), "{path}");
        }
        assert!(!candidate(Path::new("C:/u/.jcode/sessions/session_abc.json"), "commandcode"));
    }

    const SESSION: &str = r#"{
        "id": "sess-1",
        "model": "session-model",
        "working_dir": "E:/work/proj",
        "provider_key": "local",
        "messages": [
            {"id": "m1", "model": "anthropic-x", "timestamp": 1717200000, "content": "SECRET-PROMPT", "token_usage": {"input_tokens": 100, "output_tokens": 10, "reasoning_output_tokens": 4}},
            {"id": "m2", "timestamp": 1717200001, "token_usage": {"input_tokens": 40, "cache_read_input_tokens": 10, "cache_creation_input_tokens": 5, "output_tokens": 3}},
            {"id": "m3", "timestamp": 1717200002, "token_usage": {"input_tokens": 20, "cache_read_input_tokens": 12, "output_tokens": 2, "prompt_tokens_details": {"cached_tokens": 12}}},
            {"id": "m4", "timestamp": 1717200003, "token_usage": {"input_tokens": 30, "cache_read_input_tokens": 7, "output_tokens": 1}},
            {"id": "stale", "timestamp": 0, "token_usage": {"input_tokens": 999, "output_tokens": 9}},
            {"id": "empty", "timestamp": 1717200004, "token_usage": {}}
        ]
    }"#;

    #[test]
    fn maps_cache_conventions_and_never_guesses() {
        let fixture = Fixture::new();
        let path = fixture.file("sessions/session_1.json", SESSION);
        fixture.file("sessions/session_1.journal.jsonl", "{\"meta\":{\"model\":\"journal-model\"}}\n\
            {\"append_messages\":[{\"id\":\"j1\",\"timestamp\":1717200005,\"token_usage\":{\"input_tokens\":5,\"output_tokens\":2}}]}\n\
            not-json\n");
        let (records, notes) = read(&path, "jcode", &cancel()).unwrap();
        assert_eq!(records.len(), 5, "stale, empty and malformed-session rows contribute nothing");
        assert_eq!(records[0].model, "session-model", "the session header names the model even when a row states one");
        assert_eq!(records[0].tally.input, 100);
        assert_eq!(records[0].tally.output, 10, "unknown reasoning relationship keeps reported output");
        assert_eq!(records[0].unclassified_tokens, 0);
        assert_eq!((records[1].tally.input, records[1].tally.cache_read, records[1].tally.cache_write, records[1].tally.output), (40, 10, 5, 3));
        assert_eq!((records[2].tally.input, records[2].tally.cache_read, records[2].tally.output), (8, 12, 2), "contained cache removed from input");
        assert_eq!((records[3].tally.total(), records[3].unclassified_tokens), (1, 30), "ambiguous cache leaves input unclassified");
        assert_eq!(records[4].model, "journal-model");
        assert_eq!(records[4].session, "sess-1");
        assert_eq!(records[0].project.as_deref(), Some("E:/work/proj"));
        assert_eq!(records[0].deduplication_id.as_deref(), Some("jcode:sess-1:m1"));
        for note in &notes { assert!(!note.trim().is_empty()); }
        assert!(notes.iter().any(|note| note.contains("缓存关系无法确定")));
        assert!(notes.iter().any(|note| note.contains("推理计数未计入输出")));
        assert!(notes.iter().any(|note| note.contains("无法解码")), "malformed journal line is reported");
        let encoded = serde_json::to_string(&records).unwrap();
        assert!(!encoded.contains("SECRET-PROMPT"));
    }

    #[test]
    fn replayed_journal_appends_fold_by_message_id() {
        let fixture = Fixture::new();
        let path = fixture.file("sessions/session_2.json", r#"{"id":"sess-2","model":"session-model","messages":[{"id":"keep","timestamp":1717200010,"token_usage":{"input_tokens":3,"output_tokens":1}}]}"#);
        fixture.file("sessions/session_2.journal.jsonl",
            "{\"append_messages\":[{\"id\":\"keep\",\"timestamp\":1717200010,\"token_usage\":{\"input_tokens\":3,\"output_tokens\":1}}]}\n\
             {\"append_messages\":[{\"id\":\"keep\",\"timestamp\":1717200010,\"token_usage\":{\"input_tokens\":3,\"output_tokens\":1}}]}\n");
        let (records, notes) = read(&path, "jcode", &cancel()).unwrap();
        assert!(notes.is_empty());
        assert_eq!(records.len(), 3, "the reader keeps each observation; identity is what folds replays");
        assert!(records.windows(2).all(|pair| pair[0].deduplication_id == pair[1].deduplication_id));
    }

    #[test]
    fn malformed_session_is_reported_not_fatal() {
        let fixture = Fixture::new();
        let path = fixture.file("sessions/session_3.json", "{ not json");
        let (records, notes) = read(&path, "jcode", &cancel()).unwrap();
        assert!(records.is_empty());
        assert_eq!(notes.len(), 1);
        assert!(notes[0].contains("无法解析会话文件"));
    }
}
