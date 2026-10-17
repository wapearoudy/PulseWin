//! Pulse v1.9 CodebuffUsageReader (Apache-2.0).
//! `<root>/projects/<project>/chats/<chatId>/chat-messages.json` is a top-level
//! array of messages. Usage is merged across the copies the product writes, and
//! each field comes from the first copy that reported a non-zero value.
use super::measured_readers::{count, document, record, text, timestamp};
use super::*;
use chrono::DateTime;

const INPUT: &[&str] = &["inputTokens", "input_tokens", "promptTokens", "prompt_tokens"];
const OUTPUT: &[&str] = &["outputTokens", "output_tokens", "completionTokens", "completion_tokens"];
const CACHE_READ: &[&str] = &["cacheReadInputTokens", "cache_read_input_tokens", "cachedTokensCreated", "cached_tokens_created"];
const CACHE_WRITE: &[&str] = &["cacheCreationInputTokens", "cache_creation_input_tokens", "cacheCreationTokens", "cache_creation_tokens"];

/// An environment variable the client honours names the base directory; `~/`
/// prefixes expand under the home directory on Windows.
fn env_dir(env: &impl Fn(&str) -> Option<String>, key: &str, home: &Path) -> Option<PathBuf> {
    let raw = env(key)?;
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }
    if let Some(rest) = trimmed.strip_prefix("~/").or_else(|| trimmed.strip_prefix("~\\")) {
        return Some(home.join(rest));
    }
    if trimmed == "~" {
        return Some(home.to_path_buf());
    }
    Some(PathBuf::from(trimmed))
}

pub(super) fn roots(home: &Path, id: &str, env: &impl Fn(&str) -> Option<String>) -> Vec<PathBuf> {
    let _ = id;
    if let Some(base) = env_dir(env, "CODEBUFF_DATA_DIR", home) {
        return vec![base];
    }
    [".config/manicode", ".config/manicode-dev", ".config/manicode-staging"]
        .into_iter()
        .map(|path| home.join(path))
        .collect()
}

pub(super) fn candidate(path: &Path, id: &str) -> bool {
    let _ = id;
    path.file_name().is_some_and(|name| name == "chat-messages.json")
}

/// A store's `0` is "unset", not 1970.
fn event_time(value: &Value, milliseconds: bool) -> Option<DateTime<Utc>> {
    timestamp(value, milliseconds).filter(|at| at.timestamp() > 0)
}

/// A value present as a real count, even zero. Unlike `count`, absence and
/// non-counts are distinguishable.
fn present_count(value: &Value) -> Option<u64> {
    if value.is_null() {
        return None;
    }
    if let Some(raw) = value.as_str() {
        return raw.trim().parse::<u64>().ok().filter(|v| *v <= i64::MAX as u64);
    }
    if let Some(raw) = value.as_u64() {
        return (raw <= i64::MAX as u64).then_some(raw);
    }
    value
        .as_f64()
        .filter(|v| v.is_finite() && *v >= 0. && v.fract() == 0. && *v < 9_223_372_036_854_775_808.)
        .map(|v| v as u64)
}

fn object_ref<'a>(sources: &mut Vec<&'a Value>, value: Option<&'a Value>) {
    if let Some(value) = value {
        if value.is_object() {
            sources.push(value);
        }
    }
}

/// Every usage object the message carries, highest priority first.
fn usage_sources(message: &Value) -> Vec<&Value> {
    let mut sources = Vec::new();
    let metadata = &message["metadata"];
    object_ref(&mut sources, metadata.get("usage"));
    object_ref(&mut sources, metadata.get("codebuff").and_then(|codebuff| codebuff.get("usage")));
    let main = metadata
        .get("runState")
        .and_then(|value| value.get("sessionState"))
        .and_then(|value| value.get("mainAgentState"));
    if let Some(history) = main.and_then(|value| value.get("messageHistory")).and_then(Value::as_array) {
        for row in history.iter().rev() {
            object_ref(&mut sources, row.get("providerOptions").and_then(|value| value.get("usage")));
            object_ref(
                &mut sources,
                row.get("providerOptions")
                    .and_then(|value| value.get("codebuff"))
                    .and_then(|value| value.get("usage")),
            );
        }
    }
    sources
}

/// The first non-zero value for `aliases` across `sources`, in order, so a zero
/// in a higher-priority copy cannot mask the real count below it.
fn merged(sources: &[&Value], aliases: &[&str]) -> u64 {
    for source in sources {
        for alias in aliases {
            let value = count(&(*source)[*alias]);
            if value > 0 {
                return value;
            }
        }
    }
    0
}

/// Cache-read tokens, including the two nested detail spellings.
fn cache_read(sources: &[&Value]) -> u64 {
    let flat = merged(sources, CACHE_READ);
    if flat > 0 {
        return flat;
    }
    for source in sources {
        for key in ["promptTokensDetails", "prompt_tokens_details"] {
            let details = &(*source)[key];
            if !details.is_object() {
                continue;
            }
            for nested in ["cachedTokens", "cached_tokens"] {
                if let Some(value) = present_count(&details[nested]) {
                    return value;
                }
            }
        }
    }
    0
}

/// Whether a row is the assistant's own; `variant` is the newer marker and
/// `role` the older one, either is enough.
fn is_assistant(message: &Value) -> bool {
    [text(&message["variant"]), text(&message["role"])]
        .into_iter()
        .flatten()
        .any(|marker| matches!(marker.to_lowercase().as_str(), "ai" | "agent" | "assistant"))
}

/// `metadata.model` → the last history row's `providerOptions.codebuff.model` →
/// the usage object's own `model`.
fn model(message: &Value, sources: &[&Value]) -> Option<String> {
    let metadata = &message["metadata"];
    if let Some(name) = text(&metadata["model"]) {
        return Some(name);
    }
    let main = metadata
        .get("runState")
        .and_then(|value| value.get("sessionState"))
        .and_then(|value| value.get("mainAgentState"));
    if let Some(history) = main.and_then(|value| value.get("messageHistory")).and_then(Value::as_array) {
        for row in history.iter().rev() {
            if let Some(name) = text(&row["providerOptions"]["codebuff"]["model"]) {
                return Some(name);
            }
        }
    }
    for source in sources {
        if let Some(name) = text(&(*source)["model"]) {
            return Some(name);
        }
    }
    None
}

/// An ISO 8601 chat id whose time separators were written as `-`.
fn iso_from_chat_id(chat_id: &str) -> Option<DateTime<Utc>> {
    let (head, tail) = chat_id.split_once('T')?;
    event_time(&Value::String(format!("{head}T{}", tail.replace('-', ":"))), false)
}

/// The message's own timestamp, else `createdAt`, else the metadata's, else the
/// chat id restored to ISO 8601.
fn message_time(message: &Value, chat_id: &str) -> Option<DateTime<Utc>> {
    event_time(&message["timestamp"], false)
        .or_else(|| event_time(&message["createdAt"], false))
        .or_else(|| event_time(&message["metadata"]["timestamp"], false))
        .or_else(|| iso_from_chat_id(chat_id))
}

/// Channel, project and chat id from `…/projects/<project>/chats/<chatId>/`.
fn location(path: &Path) -> (String, String, String) {
    let parts: Vec<String> = path
        .components()
        .map(|part| part.as_os_str().to_string_lossy().into_owned())
        .collect();
    let chat_id = path
        .parent()
        .and_then(Path::file_name)
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    if let Some(index) = parts.iter().rposition(|part| part == "projects") {
        if index > 0 && index + 1 < parts.len() {
            return (parts[index - 1].clone(), parts[index + 1].clone(), chat_id);
        }
    }
    // A tree that was renamed or moved: the directory above `chats` names the
    // project, the one above that the channel.
    let chats = path.parent().and_then(Path::parent).unwrap_or(Path::new(""));
    let channel = chats
        .parent()
        .and_then(Path::file_name)
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    let project = chats.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_default();
    (channel, project, chat_id)
}

pub(super) fn read(path: &Path, _id: &str, cancel: &AtomicBool) -> Result<(Vec<SpendRecord>, Vec<String>), String> {
    let root: Value = document(path, cancel)?;
    let Some(messages) = root.as_array() else { return Ok((Vec::new(), Vec::new())); };
    let (channel, project, chat_id) = location(path);
    let session = format!("{channel}/{project}/{chat_id}");
    let mut records = Vec::new();
    let mut notes = Vec::new();
    for (index, message) in messages.iter().enumerate() {
        check(cancel)?;
        if records.len() >= MAX_RECORDS {
            notes.push("Codebuff：记录数超过上限，计数可能不完整".into());
            break;
        }
        if !is_assistant(message) {
            continue;
        }
        let sources = usage_sources(message);
        if sources.is_empty() {
            continue;
        }
        let tally = Tally {
            input: merged(&sources, INPUT),
            cache_write: merged(&sources, CACHE_WRITE),
            cache_read: cache_read(&sources),
            output: merged(&sources, OUTPUT),
            ..Default::default()
        };
        if tally.total() == 0 {
            continue;
        }
        let Some(model) = model(message, &sources) else { continue; };
        let Some(at) = message_time(message, &chat_id) else { continue; };
        let identity = text(&message["id"]).unwrap_or_else(|| {
            format!(
                "{session}:{index}:{}:{model}:{}:{}:{}:{}",
                at.timestamp_millis(),
                tally.input,
                tally.cache_write,
                tally.cache_read,
                tally.output
            )
        });
        // `credits` is a cost, not a token kind; Pulse's price table prices the
        // counts, so no dollar figure is read here.
        records.push(record(
            "codebuff",
            model,
            at,
            session.clone(),
            tally,
            0,
            Some(format!("codebuff:{session}:{identity}")),
        ));
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


        fn scan(&self, roots: Vec<PathBuf>) -> SpendSnapshot {
            scan_roots(
                &self.root.join("cache"),
                &Prices::new(),
                &AtomicBool::new(false),
                |_, _| {},
                |agent| if agent == "codebuff" { roots.clone() } else { Vec::new() },
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

    const CHAT: &str = ".config/manicode/projects/ring/chats/2024-01-02T03-04-05.000Z/chat-messages.json";

    #[test]
    fn roots_defaults_override_and_tilde() {
        let home = Path::new("fixture-home");
        let clean = |_: &str| -> Option<String> { None };
        assert_eq!(
            roots(home, "codebuff", &clean),
            vec![
                home.join(".config/manicode"),
                home.join(".config/manicode-dev"),
                home.join(".config/manicode-staging"),
            ]
        );
        let custom = |key: &str| -> Option<String> { (key == "CODEBUFF_DATA_DIR").then(|| "D:/codebuff".into()) };
        assert_eq!(roots(home, "codebuff", &custom), vec![PathBuf::from("D:/codebuff")]);
        let tilde = |key: &str| -> Option<String> { (key == "CODEBUFF_DATA_DIR").then(|| "~/cb".into()) };
        assert_eq!(roots(home, "codebuff", &tilde), vec![home.join("cb")]);
        let blank = |key: &str| -> Option<String> { (key == "CODEBUFF_DATA_DIR").then(|| "  ".into()) };
        assert_eq!(roots(home, "codebuff", &blank).len(), 3);
        assert!(candidate(Path::new("chat-messages.json"), "codebuff"));
        assert!(!candidate(Path::new("index.json"), "codebuff"));
    }

    #[test]
    fn assistant_usage_is_merged_not_summed_and_hidden_once() {
        let fixture = Fixture::new("codebuff-usage");
        let messages = json!([
            { "role": "user", "content": "TOP-SECRET-CONTENT", "metadata": { "usage": { "input_tokens": 99 } } },
            {
                "variant": "ai",
                "id": "m1",
                "timestamp": "2024-01-02T03:04:06Z",
                "metadata": {
                    "model": "claude",
                    "usage": { "input_tokens": 0, "output_tokens": 20, "cache_read_input_tokens": 30 },
                    "codebuff": { "usage": { "input_tokens": 100 } }
                }
            },
            { "role": "assistant", "id": "m2", "createdAt": "2024-01-02T03:04:05Z", "metadata": { "usage": { "input_tokens": 7, "model": "source-model" } } },
            { "role": "assistant", "id": "m3", "timestamp": 1_700_000_000_000i64, "metadata": { "usage": { "input_tokens": 0 } } }
        ])
        .to_string();
        let file = fixture.put(CHAT, messages);
        let (records, notes) = read(&file, "codebuff", &AtomicBool::new(false)).unwrap();
        assert!(notes.is_empty());
        assert_eq!(records.len(), 2);
        let claude = records.iter().find(|record| record.model == "claude").unwrap();
        // A zero in the higher-priority copy does not mask the real count below.
        assert_eq!(claude.tally.input, 100);
        assert_eq!(claude.tally.output, 20);
        assert_eq!(claude.tally.cache_read, 30);
        assert_eq!(claude.session, "manicode/ring/2024-01-02T03-04-05.000Z");
        assert_eq!(
            claude.deduplication_id.as_deref(),
            Some("codebuff:manicode/ring/2024-01-02T03-04-05.000Z:m1")
        );
        // `eventTime` defaults to seconds; an ISO string is the realistic spelling.
        assert_eq!(claude.source_timestamp, Some(1_704_164_646));
        let fallback = records.iter().find(|record| record.model == "source-model").unwrap();
        assert_eq!(fallback.tally.input, 7);
        assert_eq!(fallback.source_timestamp, Some(1_704_164_645));
        assert!(!serde_json::to_string(&records).unwrap().contains("TOP-SECRET"));

        // The same authoritative row replayed in the array folds globally in a
        // scan; a user row contributes nothing.
        let snapshot = fixture.scan(vec![fixture.home().join(".config/manicode")]);
        assert_eq!(snapshot.records.len(), 2);
    }

    #[test]
    fn run_state_history_supplies_usage_model_and_chat_id_time() {
        let fixture = Fixture::new("codebuff-history");
        let messages = json!([
            {
                "variant": "agent",
                "metadata": {
                    "runState": { "sessionState": { "mainAgentState": { "messageHistory": [
                        { "providerOptions": { "codebuff": { "model": "gpt-x", "usage": { "prompt_tokens": 11, "completion_tokens": 4 } } } }
                    ] } } }
                }
            }
        ])
        .to_string();
        let file = fixture.put(".config/manicode/projects/ring/chats/2024-05-06T07-08-09Z/chat-messages.json", messages);
        let (records, notes) = read(&file, "codebuff", &AtomicBool::new(false)).unwrap();
        assert!(notes.is_empty());
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].model, "gpt-x");
        assert_eq!(records[0].tally.input, 11);
        assert_eq!(records[0].tally.output, 4);
        // The chat id is the ISO start with dashes in the time part.
        assert_eq!(records[0].source_timestamp, Some(1_714_979_289));
    }

    #[test]
    fn replayed_rows_and_chat_timestamps() {
        let fixture = Fixture::new("codebuff-replay");
        let row = json!({
            "variant": "ai",
            "id": "same",
            "timestamp": 1_700_000_000_000i64,
            "metadata": { "model": "claude", "usage": { "input_tokens": 5 } }
        });
        let messages = json!([row.clone(), row]).to_string();
        let file = fixture.put(CHAT, messages);
        let (records, _) = read(&file, "codebuff", &AtomicBool::new(false)).unwrap();
        assert_eq!(records.len(), 2);
        let snapshot = fixture.scan(vec![fixture.home().join(".config/manicode")]);
        assert_eq!(snapshot.records.len(), 1);
        assert_eq!(snapshot.records[0].tally.input, 5);
    }

    #[test]
    fn malformed_and_wrong_shape_files() {
        let fixture = Fixture::new("codebuff-bad");
        let malformed = fixture.put(".config/manicode/projects/a/chats/b/chat-messages.json", "{not json");
        assert!(read(&malformed, "codebuff", &AtomicBool::new(false)).is_err());
        let wrong = fixture.put(".config/manicode/projects/c/chats/d/chat-messages.json", json!({ "messages": [] }).to_string());
        assert!(read(&wrong, "codebuff", &AtomicBool::new(false)).unwrap().0.is_empty());
    }
}
