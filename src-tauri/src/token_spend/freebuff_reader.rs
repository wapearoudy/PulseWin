//! Pulse 1.9 `FreebuffUsageReader`: the same `manicode*` trees as Codebuff, and
//! no counters at all.
//!
//! A Freebuff chat is one whose
//! `metadata.runState.sessionState.mainAgentState.agentType` starts with
//! `base2-free`; the `base2*` agent types are Codebuff's. A chat that carries
//! authoritative usage is Codebuff's even when its agent type says otherwise, so
//! [`super::codebuff_reader`] claims it and this reader never does — the two
//! readers enumerate one tree and only one of them may produce a record.
//!
//! **Nothing is emitted, and that is the whole reader.** The only figures
//! Freebuff leaves are estimates from message character counts, roughly four
//! characters to a token. Pulse does not show an inferred count as if it were
//! reported, and a "0 tokens" from a store that persisted none is a fabricated
//! zero — so the answer is no records plus the reason, and the pane reports no
//! Freebuff usage rather than an estimate dressed as a reading.

use super::measured_readers::document;
use super::*;

/// The line shown in place of a figure. Freebuff is the one source whose note
/// does not depend on the store being present: there is never a counter to read,
/// so "not detected" would blame a path for a fact about the format.
pub(super) const LIMITATION: &str =
    "Freebuff 不保存 token 计数器，只有按字符数得到的估算值，Pulse 不会把估算值当作用量上报。";

pub(super) fn roots(home: &Path, id: &str, env: &impl Fn(&str) -> Option<String>) -> Vec<PathBuf> {
    if id != "freebuff" { return Vec::new(); }
    // The client's own variable replaces the base outright rather than adding to
    // it: a relocated store must not also read the default trees, or one chat
    // found in both would be enumerated twice.
    if let Some(base) = stated(env, "FREEBUFF_DATA_DIR", home) { return vec![base]; }
    [".config/manicode", ".config/manicode-dev", ".config/manicode-staging"]
        .into_iter()
        .map(|path| home.join(path))
        .collect()
}

pub(super) fn candidate(path: &Path, id: &str) -> bool {
    id == "freebuff" && path.file_name().is_some_and(|name| name == "chat-messages.json")
}

/// Always empty: the store has no reported counters to hand over. The note is
/// added only for a chat whose agent type is actually Freebuff's, so a shared
/// Codebuff chat is left entirely to the reader that owns it.
pub(super) fn read(path: &Path, _id: &str, cancel: &AtomicBool) -> Result<(Vec<SpendRecord>, Vec<String>), String> {
    check(cancel)?;
    let mut notes = Vec::new();
    let messages = document::<Value>(path, cancel)?;
    let chats = messages.as_array().map(Vec::as_slice).unwrap_or(std::slice::from_ref(&messages));
    if chats.iter().any(is_freebuff) { notes.push(LIMITATION.to_owned()); }
    check(cancel)?;
    Ok((Vec::new(), notes))
}

/// `metadata.runState.sessionState.mainAgentState.agentType`, lowercased with a
/// `base2-free` prefix. Read strictly by that path: a loose search for the string
/// anywhere in the metadata would claim a Codebuff chat that merely mentions it.
fn is_freebuff(message: &Value) -> bool {
    message["metadata"]["runState"]["sessionState"]["mainAgentState"]["agentType"]
        .as_str()
        .is_some_and(|kind| kind.trim().to_lowercase().starts_with("base2-free"))
}

fn stated(env: &impl Fn(&str) -> Option<String>, key: &str, home: &Path) -> Option<PathBuf> {
    env(key).and_then(|raw| path_value(&raw, home))
}

/// A stated path, with `~` and `~/…` resolved against home. An empty string is
/// absent rather than the current directory.
fn path_value(raw: &str, home: &Path) -> Option<PathBuf> {
    let trimmed = raw.trim();
    if trimmed.is_empty() { return None; }
    if trimmed == "~" { return Some(home.to_path_buf()); }
    if let Some(rest) = trimmed.strip_prefix("~/").or_else(|| trimmed.strip_prefix("~\\")) { return Some(home.join(rest)); }
    Some(PathBuf::from(trimmed))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::sync::atomic::AtomicUsize;

    static NEXT: AtomicUsize = AtomicUsize::new(0);

    fn temp_dir(label: &str) -> PathBuf {
        let suffix = format!("{label}-{}-{}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed));
        let candidate = std::env::temp_dir().join(format!("pulsewin-{suffix}"));
        if fs::create_dir_all(&candidate).is_ok() { return candidate; }
        let fallback = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target").join("test-scratch").join(format!("pulsewin-{suffix}"));
        fs::create_dir_all(&fallback).unwrap_or_else(|error| panic!("create_dir_all {}: {error:?}", fallback.display()));
        fallback
    }

    fn chat(agent: &str, content: &str) -> String {
        json!([{
            "id": "m1",
            "content": content,
            "metadata": {"runState": {"sessionState": {"mainAgentState": {"agentType": agent}}}},
        }]).to_string()
    }

    #[test]
    fn roots_are_the_three_codebuff_trees_or_one_override() {
        let home = Path::new("C:/Users/me");
        assert_eq!(roots(home, "freebuff", &|_: &str| None), vec![
            home.join(".config/manicode"),
            home.join(".config/manicode-dev"),
            home.join(".config/manicode-staging"),
        ]);
        // The override replaces the base: watching both would enumerate one chat twice.
        let env = |key: &str| (key == "FREEBUFF_DATA_DIR").then(|| "~/freebuff".to_string());
        assert_eq!(roots(home, "freebuff", &env), vec![home.join("freebuff")]);
        assert!(roots(home, "codebuff", &|_: &str| None).is_empty());
    }

    #[test]
    fn candidate_accepts_a_chat_document_and_rejects_anything_else() {
        assert!(candidate(Path::new("C:/Users/me/.config/manicode/projects/p/chats/c/chat-messages.json"), "freebuff"));
        assert!(!candidate(Path::new("C:/Users/me/.config/manicode/settings.json"), "freebuff"));
        assert!(!candidate(Path::new("C:/Users/me/.config/manicode/projects/p/chats/c/chat-messages.json"), "codebuff"));
    }

    #[test]
    fn a_freebuff_chat_reports_no_count_and_says_why() {
        let home = temp_dir("freebuff-no-count");
        let root = home.join(".config/manicode/projects/p/chats/c");
        fs::create_dir_all(&root).unwrap();
        let path = root.join("chat-messages.json");
        // Long text is the only thing this store has: four characters to a token
        // would turn it into an estimate that outranks no figure at all.
        fs::write(&path, chat("base2-free", &"a".repeat(4000))).unwrap();
        let (records, notes) = read(&path, "freebuff", &AtomicBool::new(false)).unwrap();
        assert!(records.is_empty(), "{records:?}");
        assert_eq!(notes, vec![LIMITATION.to_owned()]);
        assert!(!LIMITATION.is_empty());

        // An agent type only counts by prefix, case-insensitively, and never by a
        // mention somewhere else in the metadata.
        assert!(is_freebuff(&json!({"metadata":{"runState":{"sessionState":{"mainAgentState":{"agentType":"BASE2-FREE"}}}}})));
        assert!(!is_freebuff(&json!({"metadata":{"runState":{"sessionState":{"mainAgentState":{"agentType":"base2"}}}}})));
        assert!(!is_freebuff(&json!({"metadata":{"note":"base2-free"}})));
        assert!(!is_freebuff(&json!({"id":"m1"})));
        let _ = fs::remove_dir_all(&home);
    }

    #[test]
    fn a_codebuff_chat_is_left_to_codebuff_without_a_note() {
        let home = temp_dir("freebuff-foreign-chat");
        let root = home.join(".config/manicode/projects/p/chats/c");
        fs::create_dir_all(&root).unwrap();
        let path = root.join("chat-messages.json");
        fs::write(&path, chat("base2", "hello")).unwrap();
        let (records, notes) = read(&path, "freebuff", &AtomicBool::new(false)).unwrap();
        assert!(records.is_empty());
        // Codebuff claims this chat; a Freebuff note here would be a second claim
        // on a store that is not Freebuff's.
        assert!(notes.is_empty(), "{notes:?}");
        let _ = fs::remove_dir_all(&home);
    }

    #[test]
    fn prompt_text_is_never_retained_and_cancellation_stops_before_a_record() {
        let home = temp_dir("freebuff-no-text");
        let root = home.join(".config/manicode/projects/p/chats/c");
        fs::create_dir_all(&root).unwrap();
        let path = root.join("chat-messages.json");
        fs::write(&path, chat("base2-free", "FREEBUFF_PRIVATE_PROMPT")).unwrap();
        let (records, notes) = read(&path, "freebuff", &AtomicBool::new(false)).unwrap();
        assert!(!serde_json::to_string(&records).unwrap().contains("FREEBUFF_PRIVATE_PROMPT"));
        assert!(!notes.join(" ").contains("FREEBUFF_PRIVATE_PROMPT"));
        assert_eq!(read(&path, "freebuff", &AtomicBool::new(true)).unwrap_err(), "cancelled");
        let _ = fs::remove_dir_all(&home);
    }
}
