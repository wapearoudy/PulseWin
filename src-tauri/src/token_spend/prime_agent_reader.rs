//! Pulse 1.9 `PrimeAgentSessionReader`: the Pi RLM transcript plus a
//! parent/child accounting problem.
//!
//! A parent assistant row can persist a **cumulative** `aggregateUsage` that
//! already includes the usage of a child invocation, and a
//! `child_usage_attributed` row states which child and how much. The child
//! transcript is also scanned as its own usage under `session-artifacts`, so
//! adding the parent's aggregate unchanged counts the child twice. The fix is not
//! a guess: the parent message whose own tally **equals** the stated aggregate is
//! the one injected, and it is reduced by exactly the stated child usage, clamped
//! at zero.
//!
//! **When the child is not present, the parent keeps its aggregate.** Subtracting
//! a child Pulse cannot find would silently drop tokens nobody else accounts for;
//! keeping them is the conservative, checkable choice, and the note says so.
//!
//! Two facts make this a file **set** rather than a file, and the dispatcher
//! hands a reader one file:
//!
//! - A child transcript carries `rlmDepth > 0` and a `parentSession` naming the
//!   file it was spawned from, so which children exist is only knowable by
//!   reading the whole tree.
//! - Attribution ids are eight hex characters that are unique only **inside one
//!   session**, and a fork copies a session's rows into a new file. Pairing the
//!   id with the resolved fork-lineage root collapses copies of one attribution
//!   while keeping two unrelated sessions' collisions apart. A `parentSession`
//!   cycle resolves every member to the lexicographically smallest path, so the
//!   lineage is deterministic rather than traversal-order dependent.
//!
//! So [`read`] recovers the store from the file's own place in it and reconciles
//! the same way the original does. The cost is one pass per file rather than one
//! per store; the alternative is a cached parent that keeps a reduction after the
//! child it names has changed, which is the wrong number rather than a slow one.

use super::measured_readers::{count, lines, record, text, timestamp};
use super::*;

/// How far above a transcript the store's session root can sit. A child is
/// `<sessions>/…` or `<session-artifacts>/run/sub/child.jsonl`; the bound keeps a
/// fixture path from walking to the filesystem root.
const ANCESTORS: usize = 4;

/// One parsed transcript. Only what this reader decides with is kept: the
/// counters and the bookkeeping fields, never a message body.
struct Transcript {
    path: String,
    id: Option<String>,
    cwd: Option<String>,
    parent: Option<String>,
    depth: u64,
    valid: bool,
    messages: Vec<Message>,
    attributions: Vec<Attribution>,
}

struct Message {
    id: Option<String>,
    response: Option<String>,
    at: Option<DateTime<Utc>>,
    provider: Option<String>,
    model: Option<String>,
    tally: Tally,
    unclassified: u64,
}

struct Attribution {
    id: Option<String>,
    target: Option<String>,
    child: Tally,
    aggregate: Tally,
}

pub(super) fn roots(home: &Path, id: &str, env: &impl Fn(&str) -> Option<String>) -> Vec<PathBuf> {
    if id != "prime-agent" { return Vec::new(); }
    let mut roots = if let Some(dir) = stated(env, "PRIME_AGENT_SESSION_DIR", home)
        .or_else(|| stated(env, "PRIME_AGENT_CODING_AGENT_SESSION_DIR", home)) {
        let artifacts = dir.parent().map(|parent| parent.join("session-artifacts")).unwrap_or_else(|| dir.join("session-artifacts"));
        vec![dir, artifacts]
    } else if let Some(dir) = stated(env, "PRIME_AGENT_CODING_AGENT_DIR", home) {
        vec![dir.join("sessions"), dir.join("session-artifacts")]
    } else {
        let base = home.join(".prime/agent");
        vec![base.join("sessions"), base.join("session-artifacts")]
    };
    // The store's own `settings.json` can redirect the sessions directory, and so
    // can a project's `.prime/agent/settings.json` for every working directory a
    // header here recorded.
    if let Some(settings) = roots.first().and_then(|root| root.parent()).map(|parent| parent.join("settings.json")) {
        if let Some(redirected) = session_dir(&settings, home) { roots.extend(redirected); }
    }
    for cwd in cwd_values(&roots) {
        if let Some(redirected) = session_dir(&PathBuf::from(cwd).join(".prime/agent/settings.json"), home) { roots.extend(redirected); }
    }
    unique(roots)
}

pub(super) fn candidate(path: &Path, id: &str) -> bool {
    id == "prime-agent" && path.extension().is_some_and(|extension| extension == "jsonl")
}

pub(super) fn read(path: &Path, id: &str, cancel: &AtomicBool) -> Result<(Vec<SpendRecord>, Vec<String>), String> {
    check(cancel)?;
    let mut notes = Vec::new();
    let wanted = path.to_string_lossy().into_owned();
    let mut transcripts = Vec::new();
    for file in store_files(path) {
        check(cancel)?;
        // A sibling that cannot be decoded is not this file's failure to report;
        // the store is still reconciled from what is readable, and the shortfall
        // is said out loud below rather than becoming a zero.
        match parse(&file, cancel) {
            Ok(transcript) => transcripts.push(transcript),
            Err(error) if check(cancel).is_err() => return Err(error),
            Err(_) => notes.push(format!("Prime Agent：无法读取同一会话库中的 {}，计数可能不完整。", file.display())),
        }
    }
    let Some(mine) = transcripts.iter().position(|transcript| transcript.path == wanted) else {
        notes.sort();
        notes.dedup();
        return Ok((Vec::new(), notes));
    };
    let reconciled = reconcile(&transcripts, &mut notes);
    let transcript = &transcripts[mine];
    let session = transcript.id.clone();
    let project = transcript.cwd.clone();
    let mut out: Vec<SpendRecord> = Vec::new();
    let mut incomplete = false;
    for message in &transcript.messages {
        let identity = identity(message);
        let tally = match reconciled.get(&identity) { Some(subtract) => reduce(&message.tally, subtract), None => message.tally.clone() };
        if tally.total() == 0 && message.unclassified == 0 { continue; }
        // Real usage in a file with no usable header, time or model: a readable
        // subset, not the whole story.
        let (Some(session), Some(at), Some(model)) = (transcript.valid.then(|| session.clone()).flatten(), message.at, message.model.clone()) else {
            incomplete = true;
            continue;
        };
        if out.len() >= MAX_RECORDS { return Err("Prime Agent：记录数超过读取上限。".into()); }
        let mut spent = record(id, model, at, session, tally, message.unclassified, Some(identity));
        spent.project = project.clone();
        out.push(spent);
    }
    if incomplete { notes.push("Prime Agent：部分用量缺少会话头、时间或模型，未归属；计数可能不完整。".into()); }
    notes.sort();
    notes.dedup();
    check(cancel)?;
    Ok((out, notes))
}

fn parse(path: &Path, cancel: &AtomicBool) -> Result<Transcript, String> {
    check(cancel)?;
    let file = File::open(path).map_err(|error| format!("Prime Agent：会话记录无法读取（{error}）。"))?;
    let mut reader = BufReader::with_capacity(64 * 1024, file);
    let mut transcript = Transcript {
        path: path.to_string_lossy().into_owned(), id: None, cwd: None, parent: None, depth: 0,
        valid: false, messages: Vec::new(), attributions: Vec::new(),
    };
    let mut notes = Vec::new();
    let mut header = false;
    let mut malformed = false;
    lines(&mut reader, cancel, &mut notes, |row, _| {
        if malformed { return Ok(()); }
        let kind = row["type"].as_str();
        if !header {
            // A descendant may open with title metadata; anything else is the
            // session header, or the file is malformed and names nobody.
            if kind == Some("title") { return Ok(()); }
            if kind != Some("session") || text(&row["id"]).is_none() {
                malformed = true;
                return Ok(());
            }
            header = true;
            transcript.id = text(&row["id"]);
            transcript.cwd = text(&row["cwd"]);
            transcript.parent = text(&row["parentSession"]);
            transcript.depth = count(&row["rlmDepth"]);
            return Ok(());
        }
        match kind {
            Some("message") => { if let Some(message) = message(&row) { transcript.messages.push(message); } }
            Some("child_usage_attributed") => { if let Some(attribution) = attribution(&row) { transcript.attributions.push(attribution); } }
            _ => {}
        }
        Ok(())
    })?;
    // Without a header there is no session id and no working directory, so a
    // record built from the remainder would be attributed to nobody.
    transcript.valid = header && !malformed;
    if !transcript.valid { transcript.messages.clear(); transcript.attributions.clear(); }
    check(cancel)?;
    Ok(transcript)
}

fn message(row: &Value) -> Option<Message> {
    let body = &row["message"];
    if body["role"].as_str() != Some("assistant") || !body["usage"].is_object() { return None; }
    let (tally, unclassified) = usage(&body["usage"]);
    if tally.total() == 0 && unclassified == 0 { return None; }
    Some(Message {
        id: text(&row["id"]),
        response: text(&body["responseId"]),
        // The row's timestamp wins; the message's own is the fallback the
        // original reads second.
        at: timestamp(&row["timestamp"], false).or_else(|| timestamp(&body["timestamp"], false)),
        provider: text(&body["provider"]),
        model: text(&body["model"]),
        tally,
        unclassified,
    })
}

fn attribution(row: &Value) -> Option<Attribution> {
    if !row["childUsage"].is_object() || !row["aggregateUsage"].is_object() { return None; }
    let child = usage(&row["childUsage"]).0;
    let aggregate = usage(&row["aggregateUsage"]).0;
    if child.total() == 0 && aggregate.total() == 0 { return None; }
    Some(Attribution { id: text(&row["id"]), target: text(&row["targetId"]), child, aggregate })
}

/// The four buckets, plus any reported total the buckets do not explain. A total
/// standing in for a kind nobody named is carried as unclassified rather than
/// poured into input, because a fabricated kind is a number nobody can check.
fn usage(object: &Value) -> (Tally, u64) {
    let input = whole(&object["input"]);
    let output = whole(&object["output"]);
    let cache_read = whole(&object["cacheRead"]);
    let cache_write = whole(&object["cacheWrite"]);
    let tally = Tally {
        input: input.unwrap_or(0),
        output: output.unwrap_or(0),
        cache_read: cache_read.unwrap_or(0),
        cache_write: cache_write.unwrap_or(0),
        replies_without_cache_fields: u64::from(cache_read.is_none() && cache_write.is_none()),
        ..Default::default()
    };
    // nil is not zero: a store that states no total has no remainder to place.
    let Some(total) = whole(&object["totalTokens"]) else { return (tally, 0); };
    if input.is_none() && output.is_none() && cache_read.is_none() && cache_write.is_none() {
        return (Tally { replies_without_cache_fields: tally.replies_without_cache_fields, ..Default::default() }, total);
    }
    let remainder = total.saturating_sub(tally.total());
    (tally, remainder)
}

fn reduce(tally: &Tally, subtract: &Tally) -> Tally {
    Tally {
        input: tally.input.saturating_sub(subtract.input),
        output: tally.output.saturating_sub(subtract.output),
        cache_write: tally.cache_write.saturating_sub(subtract.cache_write),
        cache_read: tally.cache_read.saturating_sub(subtract.cache_read),
        replies_without_cache_fields: tally.replies_without_cache_fields,
        ..Default::default()
    }
}

/// Which parent identity each stated child usage reduces, once.
fn reconcile(transcripts: &[Transcript], notes: &mut Vec<String>) -> HashMap<String, Tally> {
    let mut parent_of: HashMap<String, String> = HashMap::new();
    for transcript in transcripts {
        if let Some(parent) = transcript.parent.as_deref() {
            // `parentSession` names a file path; a relative one is resolved as
            // written, which is what the original's URL standardisation does.
            parent_of.insert(transcript.path.clone(), parent.to_owned());
        }
    }
    /// The top of a fork lineage, collapsing a cycle to its smallest path.
    fn lineage_root(parent_of: &HashMap<String, String>, start: &str) -> String {
        let mut chain: Vec<&str> = Vec::new();
        let mut current = start;
        loop {
            if let Some(index) = chain.iter().position(|path| *path == current) {
                return chain[index..].iter().copied().min().unwrap_or(current).to_owned();
            }
            chain.push(current);
            match parent_of.get(current) { Some(parent) => current = parent.as_str(), None => return current.to_owned() }
        }
    }
    fn descendant_of(parent_of: &HashMap<String, String>, child: &str, ancestor: &str) -> bool {
        let mut current = Some(child);
        let mut visited: HashSet<&str> = HashSet::new();
        while let Some(path) = current {
            if path == ancestor { return true; }
            if !visited.insert(path) { return false; }
            current = parent_of.get(path).map(String::as_str);
        }
        false
    }
    // Every child transcript's total, for matching a stated child usage. Only a
    // positive `rlmDepth` is a child: a fork's `parentSession` names the file it
    // copied, which is not one.
    let totals: Vec<(String, Tally)> = transcripts.iter()
        .filter(|transcript| transcript.depth > 0)
        .map(|transcript| {
            let mut total = Tally::default();
            for message in &transcript.messages { total.add(&message.tally); }
            (transcript.path.clone(), total)
        })
        .collect();
    let mut consumed: HashSet<String> = HashSet::new();
    let mut seen: HashSet<String> = HashSet::new();
    let mut reductions: HashMap<String, Tally> = HashMap::new();
    let mut unmatched = false;
    for transcript in transcripts {
        for stated in &transcript.attributions {
            let (Some(id), Some(target)) = (stated.id.as_deref(), stated.target.as_deref()) else { continue };
            let root = lineage_root(&parent_of, &transcript.path);
            // A fork copies an attribution verbatim; the id alone is only unique
            // inside one session, so the lineage root is what makes it stable.
            if !seen.insert(format!("{root}#{id}")) { continue; }
            if stated.aggregate.total() == 0 || stated.child.total() == 0 { continue; }
            let Some(parent) = transcript.messages.iter().find(|message| message.id.as_deref() == Some(target) && message.tally == stated.aggregate) else { continue };
            let Some((child, _)) = totals.iter().find(|(path, total)| !consumed.contains(path) && *total == stated.child && descendant_of(&parent_of, path, &root)) else {
                unmatched = true;
                continue;
            };
            consumed.insert(child.clone());
            reductions.entry(identity(parent)).or_default().add(&stated.child);
        }
    }
    if !reductions.is_empty() { notes.push("Prime Agent：父会话的聚合用量已扣除其归属的子会话用量。".into()); }
    // The parent keeps the aggregate it reported, so the shortfall is stated
    // rather than silently assumed away — and because the answer depends on a
    // child that may arrive later, this read must not be cached as final.
    if unmatched { notes.push("Prime Agent：一处子会话用量没有找到对应转录本，父会话保留其聚合用量。".into()); }
    reductions
}

/// A fork copy must fold onto its original, so the key is session-independent.
fn identity(message: &Message) -> String {
    if let Some(response) = message.response.as_deref() { return format!("prime-agent:response:{response}"); }
    let milliseconds = message.at.map(|at| at.timestamp_millis()).unwrap_or(0);
    let tally = &message.tally;
    format!("prime-agent:message:{}:{milliseconds}:{}:{}:{}:{}:{}:{}",
        message.id.as_deref().unwrap_or_default(),
        message.provider.as_deref().unwrap_or_default(),
        message.model.as_deref().unwrap_or_default(),
        tally.input, tally.output, tally.cache_read, tally.cache_write)
}

/// The transcripts a file's answer is computed from: the store's sessions tree
/// and its artifacts tree, found from the file's own place in the store.
fn store_files(path: &Path) -> Vec<PathBuf> {
    let mut directories: Vec<PathBuf> = Vec::new();
    let mut ancestor = path.parent();
    for _ in 0..ANCESTORS {
        let Some(directory) = ancestor else { break };
        match directory.file_name().and_then(|name| name.to_str()) {
            Some("sessions") | Some("session-artifacts") => {
                if !directories.contains(&directory.to_path_buf()) { directories.push(directory.to_path_buf()); }
                // The two trees are siblings of one base: a child of this session
                // is written to the artifacts tree, not beside the parent.
                if let Some(base) = directory.parent() {
                    for name in ["sessions", "session-artifacts"] {
                        let sibling = base.join(name);
                        if sibling.is_dir() && !directories.contains(&sibling) { directories.push(sibling); }
                    }
                }
            }
            _ => {}
        }
        ancestor = directory.parent();
    }
    // A redirect names its own sessions directory, so nothing above it is
    // recognisable: read beside the file, which reconciles a fixture too.
    if directories.is_empty() {
        if let Some(parent) = path.parent() { directories.push(parent.to_path_buf()); }
    }
    let mut files = Vec::new();
    for directory in directories { collect(&directory, 0, &mut files); }
    files.sort();
    files.dedup();
    files
}

fn collect(root: &Path, depth: usize, files: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(root) else { return };
    for entry in entries.flatten() {
        if files.len() >= MAX_FILES { return; }
        let Ok(kind) = entry.file_type() else { continue };
        // A symlink is not followed: a loop would make one store read forever.
        if kind.is_symlink() { continue; }
        let path = entry.path();
        if kind.is_dir() {
            if depth < 32 { collect(&path, depth + 1, files); }
        } else if path.extension().is_some_and(|extension| extension == "jsonl") {
            files.push(path);
        }
    }
}

/// A `settings.json`'s `sessionDir`, resolved to a sessions directory and its
/// sibling artifacts directory.
///
/// `null` means the default, and an empty string the settings file's own
/// directory; a path is tilde-expanded. A missing or unreadable file is the
/// default too — nothing is redirected on a guess.
fn session_dir(settings: &Path, home: &Path) -> Option<Vec<PathBuf>> {
    let raw = fs::read(settings).ok()?;
    let data = raw.strip_prefix(&[0xef, 0xbb, 0xbf]).unwrap_or(&raw);
    let object: Value = serde_json::from_slice(data).ok()?;
    let stated = object.get("sessionDir")?;
    if stated.is_null() { return None; }
    let text = stated.as_str()?;
    let sessions = if text.trim().is_empty() { settings.parent()?.to_path_buf() } else { path_value(text, home)? };
    let artifacts = sessions.parent().map(|parent| parent.join("session-artifacts")).unwrap_or_else(|| sessions.join("session-artifacts"));
    Some(vec![sessions, artifacts])
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

/// `PiTranscript.cwdValues`: the working directories the session headers under
/// `roots` recorded. Discovery needs the header alone, not every conversation.
fn cwd_values(roots: &[PathBuf]) -> Vec<String> {
    let mut found = Vec::new();
    for root in roots {
        let mut files = Vec::new();
        collect(root, 0, &mut files);
        for path in files {
            if let Some(cwd) = header_cwd(&path) { found.push(cwd); }
        }
    }
    found.sort();
    found.dedup();
    found
}

fn header_cwd(path: &Path) -> Option<String> {
    let file = File::open(path).ok()?;
    let mut reader = BufReader::with_capacity(16 * 1024, file);
    let mut line = String::new();
    // Bounded: a file that has not stated its header in this many rows is not a
    // transcript, and root discovery must not read whole conversations.
    for _ in 0..64 {
        line.clear();
        if reader.read_line(&mut line).unwrap_or(0) == 0 { return None; }
        if line.len() > MAX_LINE { return None; }
        let trimmed = line.trim_start_matches(['\u{feff}', ' ', '\t', '\r', '\n']);
        // An undecodable row is skipped, exactly as the line reader skips it, so
        // one damaged row cannot hide the header below it.
        let Ok(row) = serde_json::from_str::<Value>(trimmed) else { continue };
        return match row["type"].as_str() {
            Some("title") => continue,
            Some("session") if text(&row["id"]).is_some() => text(&row["cwd"]),
            _ => None,
        };
    }
    None
}

/// AgentLogIO.count: a whole, non-negative count, or nil. nil is not zero, so a
/// missing field stays missing rather than being counted as a reported zero.
fn whole(value: &Value) -> Option<u64> {
    if value.is_boolean() { return None; }
    if let Some(number) = value.as_u64() { return (number <= i64::MAX as u64).then_some(number); }
    if let Some(number) = value.as_f64() {
        if number.is_finite() && number >= 0. && number.fract() == 0. && number < 9_223_372_036_854_775_808. { return Some(number as u64); }
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
        let fallback = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target").join("test-scratch").join(format!("pulsewin-{suffix}"));
        fs::create_dir_all(&fallback).unwrap_or_else(|error| panic!("create_dir_all {}: {error:?}", fallback.display()));
        fallback
    }

    /// A parent that states the child usage its aggregate already contains.
    const PARENT: &str = r#"{"type":"session","id":"parent","cwd":"/work/pulse"}
{"type":"message","id":"pm1","timestamp":"2026-01-02T03:05:00Z","message":{"role":"assistant","model":"gpt-5","responseId":"parent-reply","usage":{"input":200,"output":100,"cacheRead":0,"cacheWrite":0,"totalTokens":300}}}
{"type":"child_usage_attributed","id":"ab12cd34","targetId":"pm1","childUsage":{"input":30,"output":10,"cacheRead":0,"cacheWrite":0},"aggregateUsage":{"input":200,"output":100,"cacheRead":0,"cacheWrite":0}}"#;

    fn child(parent: &str) -> String {
        format!("{}\n{}",
            format_args!(r#"{{"type":"session","id":"child","cwd":"/work/pulse","parentSession":"{parent}","rlmDepth":1}}"#),
            r#"{"type":"message","id":"cm1","timestamp":"2026-01-02T03:04:30Z","message":{"role":"assistant","model":"helper","responseId":"child-reply","usage":{"input":30,"output":10,"cacheRead":0,"cacheWrite":0,"totalTokens":40}}}"#)
    }

    #[test]
    fn roots_cover_the_store_the_environment_and_a_settings_redirect() {
        let home = temp_dir("prime-roots");
        assert_eq!(roots(&home, "prime-agent", &|_: &str| None), vec![home.join(".prime/agent/sessions"), home.join(".prime/agent/session-artifacts")]);
        assert!(roots(&home, "senpi", &|_: &str| None).is_empty());

        let env = |key: &str| match key {
            "PRIME_AGENT_SESSION_DIR" => Some("~/redirect/sessions".into()),
            _ => None,
        };
        assert_eq!(roots(&home, "prime-agent", &env), vec![home.join("redirect/sessions"), home.join("redirect/session-artifacts")]);

        let base = home.join(".prime/agent");
        fs::create_dir_all(&base).unwrap();
        fs::write(base.join("settings.json"), r#"{"sessionDir":"~/otherwise"}"#).unwrap();
        assert!(roots(&home, "prime-agent", &|_: &str| None).contains(&home.join("otherwise")));
        // `null` and a non-string are the default, never a redirect to nowhere.
        fs::write(base.join("settings.json"), r#"{"sessionDir":null}"#).unwrap();
        assert_eq!(roots(&home, "prime-agent", &|_: &str| None), vec![base.join("sessions"), base.join("session-artifacts")]);
        let _ = fs::remove_dir_all(&home);
    }

    #[test]
    fn candidate_accepts_only_jsonl_transcripts() {
        assert!(candidate(Path::new("C:/Users/me/.prime/agent/sessions/a.jsonl"), "prime-agent"));
        assert!(!candidate(Path::new("C:/Users/me/.prime/agent/settings.json"), "prime-agent"));
        assert!(!candidate(Path::new("C:/Users/me/.prime/agent/sessions/a.jsonl"), "senpi"));
    }

    #[test]
    fn a_parent_aggregate_is_reduced_by_the_child_it_states_once() {
        let home = temp_dir("prime-reconcile");
        let sessions = home.join(".prime/agent/sessions");
        let artifacts = home.join(".prime/agent/session-artifacts");
        fs::create_dir_all(&sessions).unwrap();
        fs::create_dir_all(artifacts.join("run/sub-1")).unwrap();
        let parent = sessions.join("parent.jsonl");
        fs::write(&parent, PARENT).unwrap();
        fs::write(artifacts.join("run/sub-1/child.jsonl"), child(&parent.to_string_lossy())).unwrap();

        let (records, notes) = read(&parent, "prime-agent", &AtomicBool::new(false)).unwrap();
        assert_eq!(records.len(), 1, "{records:?}");
        // 300 stated, of which 40 is the child standing beside it as its own record.
        assert_eq!(records[0].tally.input, 170);
        assert_eq!(records[0].tally.output, 90);
        assert_eq!(records[0].tally.total(), 260);
        assert_eq!(records[0].session, "parent");
        assert_eq!(records[0].project.as_deref(), Some("/work/pulse"));
        assert_eq!(records[0].deduplication_id.as_deref(), Some("prime-agent:response:parent-reply"));
        assert!(notes.iter().any(|note| note.contains("已扣除")), "{notes:?}");

        let child_file = artifacts.join("run/sub-1/child.jsonl");
        let (child_records, _) = read(&child_file, "prime-agent", &AtomicBool::new(false)).unwrap();
        assert_eq!(child_records.len(), 1);
        assert_eq!(child_records[0].session, "child");
        assert_eq!(child_records[0].tally.total(), 40);
        let _ = fs::remove_dir_all(&home);
    }

    #[test]
    fn an_absent_child_leaves_the_aggregate_untouched() {
        let home = temp_dir("prime-missing-child");
        let sessions = home.join(".prime/agent/sessions");
        fs::create_dir_all(&sessions).unwrap();
        let parent = sessions.join("parent.jsonl");
        // The stated child usage matches no transcript in this store: subtracting
        // it would delete tokens no file accounts for.
        fs::write(&parent, PARENT).unwrap();
        let (records, notes) = read(&parent, "prime-agent", &AtomicBool::new(false)).unwrap();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].tally.input, 200);
        assert_eq!(records[0].tally.output, 100);
        assert_eq!(records[0].tally.total(), 300);
        assert!(notes.iter().any(|note| note.contains("保留其聚合用量")), "{notes:?}");
        let _ = fs::remove_dir_all(&home);
    }

    #[test]
    fn a_fork_copy_applies_the_attribution_once() {
        let home = temp_dir("prime-fork");
        let sessions = home.join(".prime/agent/sessions");
        let artifacts = home.join(".prime/agent/session-artifacts");
        fs::create_dir_all(&sessions).unwrap();
        fs::create_dir_all(&artifacts).unwrap();
        let first = sessions.join("parent.jsonl");
        fs::write(&first, PARENT).unwrap();
        let body = PARENT.split_once('\n').map(|(_, rest)| rest).unwrap_or_default();
        let fork = sessions.join("fork.jsonl");
        fs::write(&fork, format!("{}\n{body}", format_args!(r#"{{"type":"session","id":"fork","cwd":"/work/pulse","parentSession":"{}"}}"#, first.to_string_lossy()))).unwrap();
        fs::write(artifacts.join("child.jsonl"), child(&first.to_string_lossy())).unwrap();

        // Both copies fold by response id, so the reduction is not applied twice.
        for file in [&first, &fork] {
            let records = read(file, "prime-agent", &AtomicBool::new(false)).unwrap().0;
            assert_eq!(records.len(), 1);
            assert_eq!(records[0].tally.total(), 260);
        }
        let _ = fs::remove_dir_all(&home);
    }

    #[test]
    fn undated_or_unmodelled_usage_is_skipped_and_prompt_text_is_never_retained() {
        let home = temp_dir("prime-no-estimate");
        let sessions = home.join(".prime/agent/sessions");
        fs::create_dir_all(&sessions).unwrap();
        let path = sessions.join("parent.jsonl");
        fs::write(&path, [
            r#"{"type":"session","id":"s1","cwd":"/work/pulse"}"#,
            r#"{"type":"message","id":"u1","message":{"role":"user","content":"PRIME_PRIVATE_PROMPT"}}"#,
            // No stated time and no model: a readable subset, never an estimate.
            r#"{"type":"message","id":"m1","message":{"role":"assistant","model":"m","usage":{"input":10,"output":1}}}"#,
            r#"{"type":"message","id":"m2","timestamp":"2026-01-02T03:05:00Z","message":{"role":"assistant","usage":{"input":10,"output":1}}}"#,
            // A reported total with no named bucket is carried, not poured into input.
            r#"{"type":"message","id":"m3","timestamp":"2026-01-02T03:05:00Z","message":{"role":"assistant","model":"m","usage":{"totalTokens":7}}}"#,
        ].join("\n")).unwrap();
        let (records, notes) = read(&path, "prime-agent", &AtomicBool::new(false)).unwrap();
        assert_eq!(records.len(), 1, "{records:?}");
        assert_eq!(records[0].tally.total(), 0);
        assert_eq!(records[0].unclassified_tokens, 7);
        assert!(notes.iter().any(|note| note.contains("计数可能不完整")), "{notes:?}");
        assert!(!serde_json::to_string(&records).unwrap().contains("PRIME_PRIVATE_PROMPT"));

        // A file with no usable header attributes nothing to anybody.
        fs::write(&path, r#"{"type":"message","id":"m1"}"#).unwrap();
        assert!(read(&path, "prime-agent", &AtomicBool::new(false)).unwrap().0.is_empty());
        let _ = fs::remove_dir_all(&home);
    }

    #[test]
    fn cancellation_stops_before_any_record_is_built() {
        let home = temp_dir("prime-cancel");
        let sessions = home.join(".prime/agent/sessions");
        fs::create_dir_all(&sessions).unwrap();
        let path = sessions.join("parent.jsonl");
        fs::write(&path, PARENT).unwrap();
        assert_eq!(read(&path, "prime-agent", &AtomicBool::new(true)).unwrap_err(), "cancelled");
        let _ = fs::remove_dir_all(&home);
    }
}
