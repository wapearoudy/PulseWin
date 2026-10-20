//! Pulse 1.9 `PiFamilySessionReader`'s `senpi` configuration ("OmO Native").
//!
//! Senpi writes the Pi transcript unchanged — a `session` header carrying the
//! session id and working directory, then `message` rows whose `message.usage`
//! holds the four independent buckets — so the row parsing is
//! [`super::session_readers`]' and this file owns only what makes Senpi a source
//! of its own: its roots, and the OmO child trees those roots imply.
//!
//! **`Configuration(dedup: .crossSession, discoversProjectChildren: true,
//! sessionInfoNameIsTitle: true, providerFallback: nil)`.** Cross-session dedup
//! is the family default: a fork copies prior assistant rows into a new file
//! verbatim, so the identity must not carry a session id or the copy would count
//! twice. A nil provider fallback is also the default. The title flag has
//! nowhere to land — a record has no title field, and prompt text is never
//! retained — so it changes nothing here.
//!
//! **The OmO child tree is why Senpi cannot reuse Pi's roots.** An OmO task runs
//! under the project's own working directory, at
//! `<cwd>/.omo/senpi-task/children`, which is outside `~/.senpi/agent/sessions`.
//! Those working directories exist only inside the session headers, so discovery
//! has to read the headers: without it an OmO child's usage is simply never
//! counted, and the session that spawned it stays invisible.

use super::measured_readers::text;
use super::*;

/// A `session` header is one of the first rows; a file that has not produced one
/// after this many rows is not a transcript. Bounded so a pathological file
/// cannot make root discovery read a whole conversation for every scan.
const HEADER_ROWS: usize = 64;

pub(super) fn roots(home: &Path, id: &str, env: &impl Fn(&str) -> Option<String>) -> Vec<PathBuf> {
    if id != "senpi" { return Vec::new(); }
    let base = stated(env, "SENPI_CODING_AGENT_DIR", home).unwrap_or_else(|| home.join(".senpi/agent"));
    let mut roots = vec![base.join("sessions")];
    if let Some(state) = stated(env, "SENPI_CODING_AGENT_SESSION_DIR", home) { roots.push(state); }
    // Discovery reads the headers under the roots just named; a child tree that
    // is itself a root contributes no further working directories to look at.
    let children: Vec<PathBuf> = cwd_values(&roots)
        .into_iter()
        .map(|cwd| PathBuf::from(cwd).join(".omo/senpi-task/children"))
        .collect();
    roots.extend(children);
    unique(roots)
}

pub(super) fn candidate(path: &Path, id: &str) -> bool {
    id == "senpi" && path.extension().is_some_and(|extension| extension == "jsonl")
}

/// The Pi reader, unchanged: same rows, same buckets, same cross-session
/// identity. Only the roots and the family's `senpi` run differ.
pub(super) fn read(path: &Path, id: &str, cancel: &AtomicBool) -> Result<(Vec<SpendRecord>, Vec<String>), String> {
    session_readers::parse(path, id, cancel)
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

fn unique(paths: Vec<PathBuf>) -> Vec<PathBuf> {
    let mut seen = HashSet::new();
    paths.into_iter().filter(|path| seen.insert(path.to_string_lossy().into_owned())).collect()
}

/// `PiTranscript.cwdValues`: the working directory each session header records.
/// Only the header is read — this runs once per scan per root, not once per
/// conversation — and a file that never states one contributes nothing rather
/// than being retried later.
fn cwd_values(roots: &[PathBuf]) -> Vec<String> {
    let mut found = Vec::new();
    for path in jsonl_files(roots) {
        if let Some(cwd) = header_cwd(&path) { found.push(cwd); }
    }
    found.sort();
    found.dedup();
    found
}

fn header_cwd(path: &Path) -> Option<String> {
    let file = File::open(path).ok()?;
    let mut reader = BufReader::with_capacity(16 * 1024, file);
    let mut line = String::new();
    for _ in 0..HEADER_ROWS {
        line.clear();
        if reader.read_line(&mut line).ok()? == 0 { return None; }
        if line.len() > MAX_LINE { return None; }
        let trimmed = line.trim_start_matches(['\u{feff}', ' ', '\t', '\r', '\n']);
        // A row that does not decode is skipped, exactly as the original's line
        // reader skips it, so one damaged line cannot hide the header below it.
        let Ok(row) = serde_json::from_str::<Value>(trimmed) else { continue };
        match row["type"].as_str() {
            Some("title") => continue,
            Some("session") if text(&row["id"]).is_some() => return text(&row["cwd"]),
            _ => return None,
        }
    }
    None
}

fn jsonl_files(roots: &[PathBuf]) -> Vec<PathBuf> {
    let mut files = Vec::new();
    for root in roots {
        let mut pending = vec![(root.clone(), 0usize)];
        while let Some((directory, depth)) = pending.pop() {
            let Ok(entries) = fs::read_dir(&directory) else { continue };
            for entry in entries.flatten() {
                if files.len() >= MAX_FILES { break; }
                let Ok(kind) = entry.file_type() else { continue };
                // A symlink is not followed: a loop would make one root read the
                // same tree forever.
                if kind.is_symlink() { continue; }
                let path = entry.path();
                if kind.is_dir() { if depth < 32 { pending.push((path, depth + 1)); } }
                else if path.extension().is_some_and(|extension| extension == "jsonl") { files.push(path); }
            }
        }
    }
    files.sort();
    files.dedup();
    files
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
        let fallback = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target").join("test-scratch").join(format!("pulsewin-{suffix}"));
        fs::create_dir_all(&fallback).unwrap_or_else(|error| panic!("create_dir_all {}: {error:?}", fallback.display()));
        fallback
    }

    /// One Pi transcript: a header, then rows as Senpi writes them.
    fn transcript(id: &str, cwd: &str, rows: &[&str]) -> String {
        let mut text = format!(r#"{{"type":"session","id":"{id}","cwd":"{cwd}"}}"#);
        for row in rows {
            text.push('\n');
            text.push_str(row);
        }
        text
    }

    #[test]
    fn roots_follow_the_base_state_and_project_children_layout() {
        let home = temp_dir("senpi-roots");
        assert_eq!(roots(&home, "senpi", &|_: &str| None), vec![home.join(".senpi/agent/sessions")]);
        assert!(roots(&home, "pi", &|_: &str| None).is_empty());

        let env = |key: &str| match key {
            "SENPI_CODING_AGENT_DIR" => Some("~/senpi-base".into()),
            "SENPI_CODING_AGENT_SESSION_DIR" => Some("/state/senpi".into()),
            _ => None,
        };
        let stated = roots(&home, "senpi", &env);
        assert_eq!(stated, vec![home.join("senpi-base/sessions"), PathBuf::from("/state/senpi")]);

        // A header's working directory adds its OmO child tree to the same scan.
        let sessions = home.join(".senpi/agent/sessions");
        fs::create_dir_all(&sessions).unwrap();
        fs::write(sessions.join("one.jsonl"), transcript("s1", "/Users/me/Code/Pulse", &[])).unwrap();
        fs::write(sessions.join("notes.txt"), "not a transcript").unwrap();
        assert_eq!(roots(&home, "senpi", &|_: &str| None), vec![
            sessions.clone(),
            Path::new("/Users/me/Code/Pulse/.omo/senpi-task/children").to_path_buf(),
        ]);
        // A title row ahead of the header is metadata, not the header.
        fs::write(sessions.join("two.jsonl"), format!("{}\n{}", r#"{"type":"title","title":"t"}"#,
            transcript("s2", "/work/second", &[]))).unwrap();
        assert_eq!(roots(&home, "senpi", &|_: &str| None).len(), 3);
        let _ = fs::remove_dir_all(&home);
    }

    #[test]
    fn candidate_accepts_only_jsonl_transcripts() {
        assert!(candidate(Path::new("C:/Users/me/.senpi/agent/sessions/s1.jsonl"), "senpi"));
        assert!(!candidate(Path::new("C:/Users/me/.senpi/agent/sessions/s1.json"), "senpi"));
        assert!(!candidate(Path::new("C:/Users/me/.senpi/agent/sessions/s1.jsonl"), "prime-agent"));
    }

    #[test]
    fn one_reply_counts_its_four_buckets_and_the_identity_is_session_free() {
        let home = temp_dir("senpi-count");
        let root = home.join(".senpi/agent/sessions");
        fs::create_dir_all(&root).unwrap();
        let row = r#"{"type":"message","id":"m1","timestamp":"2026-10-02T12:00:00Z","message":{"role":"assistant","model":"claude-sonnet-4","responseId":"reply-1","usage":{"input":100,"output":20,"cacheRead":30,"cacheWrite":5,"totalTokens":155}}}"#;
        let first = root.join("one.jsonl");
        let second = root.join("two.jsonl");
        fs::write(&first, transcript("s1", "/work/pulse", &[row])).unwrap();
        fs::write(&second, transcript("s2", "/work/pulse", &[row])).unwrap();

        let (records, notes) = read(&first, "senpi", &AtomicBool::new(false)).unwrap();
        assert!(notes.is_empty(), "{notes:?}");
        assert_eq!(records.len(), 1);
        let spent = &records[0];
        assert_eq!(spent.agent, "senpi");
        assert_eq!(spent.model, "claude-sonnet-4");
        assert_eq!(spent.tally.input, 100);
        assert_eq!(spent.tally.output, 20);
        assert_eq!(spent.tally.cache_read, 30);
        assert_eq!(spent.tally.cache_write, 5);
        assert_eq!(spent.tally.total(), 155);
        assert_eq!(spent.unclassified_tokens, 0);
        assert_eq!(spent.project.as_deref(), Some("/work/pulse"));
        assert_eq!(spent.session, "s1");
        // The row states UTC; `source_timestamp` is the same instant in epoch seconds,
        // not a local-clock restatement of it.
        assert_eq!(spent.source_timestamp, Some(1_790_942_400));

        // A fork copies the reply into another session's file: the identity has to
        // be equal, or the scan counts the copy as a second reply.
        let (forked, _) = read(&second, "senpi", &AtomicBool::new(false)).unwrap();
        assert_eq!(forked[0].deduplication_id, spent.deduplication_id);
        assert_eq!(spent.deduplication_id.as_deref(), Some("senpi:response:reply-1"));
        let _ = fs::remove_dir_all(&home);
    }

    #[test]
    fn an_undated_or_unmodelled_reply_is_skipped_and_prompt_text_is_never_retained() {
        let home = temp_dir("senpi-no-estimate");
        let root = home.join(".senpi/agent/sessions");
        fs::create_dir_all(&root).unwrap();
        let path = root.join("s1.jsonl");
        let rows = [
            // Prompt text is written by the product and must not survive a read.
            r#"{"type":"message","id":"u1","message":{"role":"user","content":"SENPI_PRIVATE_PROMPT"}}"#,
            // Usage with no stated time cannot be dated from the file.
            r#"{"type":"message","id":"m1","message":{"role":"assistant","model":"m","usage":{"input":10,"output":1}}}"#,
            // Usage with no model cannot be priced, and is not guessed.
            r#"{"type":"message","id":"m2","timestamp":"2026-10-02T12:00:00Z","message":{"role":"assistant","usage":{"input":10,"output":1}}}"#,
            // A reply with no buckets at all is not a zero record.
            r#"{"type":"message","id":"m3","timestamp":"2026-10-02T12:00:00Z","message":{"role":"assistant","model":"m","usage":{}}}"#,
        ];
        fs::write(&path, transcript("s1", "/work/pulse", &rows)).unwrap();
        let (records, notes) = read(&path, "senpi", &AtomicBool::new(false)).unwrap();
        assert!(records.is_empty(), "{records:?}");
        assert!(notes.iter().any(|note| note.contains("缺少有效时间")), "{notes:?}");
        assert!(notes.iter().any(|note| note.contains("缺少模型")), "{notes:?}");
        assert!(!serde_json::to_string(&records).unwrap().contains("SENPI_PRIVATE_PROMPT"));

        // A file whose header is unusable attributes nothing to anybody.
        fs::write(&path, format!("{}\n{}", r#"{"type":"message","id":"m1"}"#, r#"{"type":"session","id":"s1"}"#)).unwrap();
        let (broken, notes) = read(&path, "senpi", &AtomicBool::new(false)).unwrap();
        assert!(broken.is_empty());
        assert!(notes.iter().any(|note| note.contains("session 头")), "{notes:?}");
        let _ = fs::remove_dir_all(&home);
    }

    #[test]
    fn cancellation_stops_before_any_record_is_built() {
        let home = temp_dir("senpi-cancel");
        let root = home.join(".senpi/agent/sessions");
        fs::create_dir_all(&root).unwrap();
        let path = root.join("s1.jsonl");
        fs::write(&path, transcript("s1", "/work/pulse", &[r#"{"type":"message","id":"m1","timestamp":"2026-10-02T12:00:00Z","message":{"role":"assistant","model":"m","usage":{"input":5,"output":1}}}"#])).unwrap();
        assert_eq!(read(&path, "senpi", &AtomicBool::new(true)).unwrap_err(), "cancelled");
        let _ = fs::remove_dir_all(&home);
    }
}
