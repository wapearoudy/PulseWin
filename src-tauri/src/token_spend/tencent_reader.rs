//! Pulse 1.9 TencentBuddyReader (Apache-2.0): CodeBuddy and WorkBuddy.
//!
//! Three shapes meet here, each dispatched by the file it is found in: a JSONL
//! transcript under `projects/`, an extension log with an `[AgentReporter]`
//! usage line, and WorkBuddy's aggregate `workbuddy.db`. Only the transcript is
//! reconcilable — its `messageId`, `traceId` or line id folds a replayed
//! message — so while one exists the other two channels are not counted at all:
//! they share no identity with its message ids, and appending them would double
//! count whatever both saw. When that excluded fallback did hold records the
//! note says the counted set is a confirmed subset, not the whole usage.
//!
//! **Nothing here estimates.** A row without a usage object is not a record, a
//! row the product itself calls incomplete is skipped, and a database row is one
//! session-level total placed on its own timestamp and reported as
//! `unclassifiedTokens` rather than passed off as fresh input.

use super::measured_readers::{record, text, timestamp};
use super::*;
use chrono::TimeZone;
use rusqlite::{types::ValueRef, Connection, OpenFlags, Row};
use std::io::Read;
use std::time::Instant;

/// The Swift reader names no environment override: both trees live directly
/// under the user's home on every platform, so nothing is invented here.
pub(super) fn roots(home: &Path, id: &str, _env: &impl Fn(&str) -> Option<String>) -> Vec<PathBuf> {
    match id {
        // The product tree holds `projects/` transcripts and any extension logs.
        "codebuddy" => vec![home.join(".codebuddy")],
        // `workbuddy` is the 5.0 tree and `workbuddy-ai` the 5.5 one; both can
        // exist on one machine.
        "workbuddy" => vec![home.join(".workbuddy"), home.join(".workbuddy-ai")],
        _ => Vec::new(),
    }
}

pub(super) fn candidate(path: &Path, id: &str) -> bool {
    if !matches!(id, "codebuddy" | "workbuddy") {
        return false;
    }
    if path.file_name().and_then(|name| name.to_str()).is_some_and(|name| name.ends_with("-shm")) {
        return false;
    }
    // AgentLogIO excludes a root-level `binaries/` subtree from reading and from
    // fingerprinting; the dispatcher walks without that filter, so a file below
    // `<client tree>/binaries/` is refused here instead. Only that depth is
    // excluded, exactly as the Swift walker's `skipDescendants` does.
    if under_binaries(path) {
        return false;
    }
    let extension = path.extension().and_then(|extension| extension.to_str());
    matches!(extension, Some("jsonl") | Some("log"))
        || path.file_name().is_some_and(|name| name == "workbuddy.db")
}

pub(super) fn read(
    path: &Path,
    id: &str,
    cancel: &AtomicBool,
) -> Result<(Vec<SpendRecord>, Vec<String>), String> {
    check(cancel)?;
    if !matches!(id, "codebuddy" | "workbuddy") {
        return Ok((Vec::new(), Vec::new()));
    }
    let label = label(id);
    let mut notes = Vec::new();
    let extension = path.extension().and_then(|extension| extension.to_str());
    let records = if extension == Some("jsonl") {
        // The transcript is the reconcilable channel, but a fallback holding
        // records means only a subset of the product's work could be confirmed.
        let probe = probe_roots(path, id);
        let (records, said) = jsonl(id, path, cancel, false)?;
        notes.extend(said);
        if !records.is_empty() && fallback_present(&probe, id, cancel)? {
            notes.push(format!("{label}：扩展日志或汇总数据库另有记录，未计入；已计数的只是可核对的子集，可能不完整。"));
        }
        records
    } else if extension == Some("log") {
        // The transcript wins and the fallback is not appended. Only the
        // existence of a fallback *record* matters, so the scan stops at the
        // first one; none of its counters are kept.
        let probe = probe_roots(path, id);
        let superseded = transcript_present(&probe, id, cancel)?;
        let (excluded, said) = extension_log(id, path, cancel, superseded)?;
        notes.extend(said);
        if superseded {
            if !excluded.is_empty() {
                notes.push(format!("{label}：转录本已存在，扩展日志与汇总数据库未计入；已计数的只是可核对的子集，可能不完整。"));
            }
            Vec::new()
        } else {
            excluded
        }
    } else if path.file_name().is_some_and(|name| name == "workbuddy.db") {
        let probe = probe_roots(path, id);
        let superseded = transcript_present(&probe, id, cancel)?;
        let (excluded, said) = sqlite(id, path, cancel)?;
        notes.extend(said);
        if superseded {
            if !excluded.is_empty() {
                notes.push(format!("{label}：转录本已存在，扩展日志与汇总数据库未计入；已计数的只是可核对的子集，可能不完整。"));
            }
            Vec::new()
        } else {
            excluded
        }
    } else {
        Vec::new()
    };
    let mut records = records;
    records.sort_by_key(|record| record.source_timestamp.unwrap_or(0));
    notes.sort();
    notes.dedup();
    check(cancel)?;
    Ok((records, notes))
}

fn label(id: &str) -> &'static str {
    if id == "workbuddy" { "WorkBuddy" } else { "CodeBuddy" }
}

/// Whether `path` sits in a `<client tree>/binaries/` subtree — the one place
/// AgentLogIO skips descendants of a root.
fn under_binaries(path: &Path) -> bool {
    const TREES: [&str; 3] = [".codebuddy", ".workbuddy", ".workbuddy-ai"];
    let parts: Vec<String> = path
        .components()
        .map(|part| part.as_os_str().to_string_lossy().into_owned())
        .collect();
    parts
        .iter()
        .enumerate()
        .any(|(index, part)| part == "binaries" && index > 0 && TREES.contains(&parts[index - 1].as_str()))
}

// MARK: - Root probing

/// The client trees a file belongs to. The dispatcher hands one file at a time,
/// but the transcript-supersedes-fallback rule is a client-wide decision, so the
/// file's own tree (and, for WorkBuddy, its sibling 5.5 tree) is probed.
fn probe_roots(path: &Path, id: &str) -> Vec<PathBuf> {
    let names: &[&str] = if id == "workbuddy" { &[".workbuddy", ".workbuddy-ai"] } else { &[".codebuddy"] };
    let mut ancestor = path.parent();
    while let Some(directory) = ancestor {
        if directory.file_name().and_then(|name| name.to_str()).is_some_and(|name| names.contains(&name)) {
            let mut roots = Vec::new();
            if let Some(parent) = directory.parent() {
                for name in names {
                    let sibling = parent.join(name);
                    if sibling.is_dir() && !roots.contains(&sibling) { roots.push(sibling); }
                }
            }
            if roots.is_empty() { roots.push(directory.to_path_buf()); }
            return roots;
        }
        ancestor = directory.parent();
    }
    // A path outside the known trees (a fixture): probe beside it.
    path.parent().map(Path::to_path_buf).into_iter().collect()
}

/// Whether any transcript in these trees produced a record. Stops at the first
/// one: only existence matters for the supersede decision.
fn transcript_present(roots: &[PathBuf], id: &str, cancel: &AtomicBool) -> Result<bool, String> {
    for path in probe_files(roots, cancel)? {
        if path.extension().and_then(|extension| extension.to_str()) != Some("jsonl") {
            continue;
        }
        let (records, _) = jsonl(id, &path, cancel, true)?;
        if !records.is_empty() {
            return Ok(true);
        }
    }
    Ok(false)
}

/// Whether the excluded fallback channels hold any record. Stops at the first
/// one; none of its counters are decoded into a count.
fn fallback_present(roots: &[PathBuf], id: &str, cancel: &AtomicBool) -> Result<bool, String> {
    for path in probe_files(roots, cancel)? {
        let extension = path.extension().and_then(|extension| extension.to_str());
        let (records, _) = if extension == Some("log") {
            extension_log(id, &path, cancel, true)?
        } else if path.file_name().is_some_and(|name| name == "workbuddy.db") {
            sqlite(id, &path, cancel)?
        } else {
            continue;
        };
        if !records.is_empty() {
            return Ok(true);
        }
    }
    Ok(false)
}

/// The candidate files under these roots, skipping the root-level `binaries/`
/// subtree exactly as AgentLogIO's walker does.
fn probe_files(roots: &[PathBuf], cancel: &AtomicBool) -> Result<Vec<PathBuf>, String> {
    let mut files = Vec::new();
    for root in roots {
        let mut pending = vec![(root.clone(), 0usize)];
        let mut visited = 0usize;
        while let Some((directory, depth)) = pending.pop() {
            check(cancel)?;
            let Ok(entries) = fs::read_dir(&directory) else { continue };
            for entry in entries.flatten() {
                if visited >= MAX_FILES {
                    return Ok(files);
                }
                visited += 1;
                let Ok(kind) = entry.file_type() else { continue };
                if kind.is_symlink() {
                    continue;
                }
                let path = entry.path();
                if kind.is_dir() {
                    if depth < 32 && entry.file_name() != "binaries" {
                        pending.push((path, depth + 1));
                    }
                    continue;
                }
                if !under_binaries(&path) {
                    files.push(path);
                }
            }
        }
    }
    files.sort();
    files.dedup();
    Ok(files)
}

// MARK: - JSONL transcript

fn jsonl(
    client: &str,
    path: &Path,
    cancel: &AtomicBool,
    first_only: bool,
) -> Result<(Vec<SpendRecord>, Vec<String>), String> {
    let label = label(client);
    let stem = path.file_stem().map(|stem| stem.to_string_lossy().into_owned()).unwrap_or_default();
    let mut notes = Vec::new();
    let mut identified: HashMap<String, SpendRecord> = HashMap::new();
    let mut unidentified: Vec<SpendRecord> = Vec::new();
    each_line(path, cancel, &mut notes, label, |line| {
        let Ok(row) = serde_json::from_slice::<Value>(line) else { return Ok(false) };
        if !row.is_object() {
            return Ok(false);
        }
        let Some(at) = row.get("timestamp").and_then(|value| timestamp(value, true)) else { return Ok(false) };
        let kind = row.get("type").and_then(text);
        let role = row.get("role").and_then(text);
        let assistant = kind.as_deref() == Some("message") && role.as_deref() == Some("assistant");
        if !assistant && kind.as_deref() != Some("function_call") {
            return Ok(false);
        }
        // A record the product itself calls incomplete is not counted.
        if row.get("status").and_then(text).is_some_and(|status| status != "completed") {
            return Ok(false);
        }
        let message = row.get("message").filter(|value| value.is_object());
        let provider = row.get("providerData").filter(|value| value.is_object());
        let usage = [message.and_then(|value| value.get("usage")),
                     provider.and_then(|value| value.get("usage")),
                     provider.and_then(|value| value.get("rawUsage"))]
            .into_iter()
            .flatten()
            .find(|value| value.is_object());
        let Some(usage) = usage else { return Ok(false) };
        let parts = transcript_usage(usage);
        if parts.tally.total().saturating_add(parts.unclassified) == 0 {
            return Ok(false);
        }
        let session = row.get("sessionId").and_then(text).unwrap_or_else(|| stem.clone());
        let model = provider.and_then(|value| value.get("model")).and_then(text)
            .or_else(|| provider.and_then(|value| value.get("requestModelId")).and_then(text))
            .or_else(|| message.and_then(|value| value.get("model")).and_then(text));
        let model = model_id(model.as_deref()).unwrap_or_else(|| client.to_owned());
        let identity = provider.and_then(|value| value.get("messageId")).and_then(text)
            .or_else(|| provider.and_then(|value| value.get("traceId")).and_then(text))
            .or_else(|| row.get("id").and_then(text));
        let mut spend = record(
            client,
            model,
            at,
            session.clone(),
            parts.tally,
            parts.unclassified,
            identity.clone().map(|identity| format!("{client}:{session}:{identity}")),
        );
        spend.project = row.get("cwd").and_then(text);
        // Mirrored writes of one message land on one identity; the more complete
        // snapshot wins rather than both being added.
        match identity {
            Some(identity) => {
                if identified.get(&identity).is_some_and(|existing| total(existing) >= total(&spend)) {
                    return Ok(false);
                }
                identified.insert(identity, spend);
            }
            None => unidentified.push(spend),
        }
        Ok(first_only)
    })?;
    let mut records: Vec<SpendRecord> = identified.into_values().chain(unidentified).collect();
    records.sort_by_key(|record| record.source_timestamp.unwrap_or(0));
    Ok((records, notes))
}

fn total(record: &SpendRecord) -> u64 {
    record.tally.total().saturating_add(record.unclassified_tokens)
}

fn transcript_usage(usage: &Value) -> Parts {
    combine(
        first_count(usage, &["input_tokens", "inputTokens", "prompt_tokens"]),
        first_count(usage, &["output_tokens", "outputTokens", "completion_tokens"]),
        first_count(usage, &[
            "cache_read_input_tokens", "cacheReadInputTokens", "cacheTokens",
            "prompt_cache_hit_tokens", "cached_tokens",
        ]),
        first_count(usage, &[
            "cache_creation_input_tokens", "cacheCreationInputTokens",
            "cachedWriteTokens", "prompt_cache_write_tokens",
        ]),
        first_count(usage, &["completion_thinking_tokens", "completionThinkingTokens", "reasoningTokens"]),
        first_count(usage, &["total_tokens", "totalTokens"]),
        first_count(usage, &["cachedMissTokens", "cacheMissTokens"]),
    )
}

// MARK: - Extension log

fn extension_log(
    client: &str,
    path: &Path,
    cancel: &AtomicBool,
    first_only: bool,
) -> Result<(Vec<SpendRecord>, Vec<String>), String> {
    let label = label(client);
    let workspace = path
        .file_name()
        .and_then(|name| name.to_str())
        .and_then(|name| name.split("__").next())
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .map(str::to_owned);
    let mut models: HashMap<String, String> = HashMap::new();
    let mut records: Vec<SpendRecord> = Vec::new();
    let mut notes = Vec::new();
    each_line(path, cancel, &mut notes, label, |line| {
        let Ok(raw) = std::str::from_utf8(line) else { return Ok(false) };
        if !raw.contains("[CraftInvokableAgent]") && !raw.contains("[AgentReporter]") {
            return Ok(false);
        }
        let Some(at) = naive_timestamp(raw) else { return Ok(false) };
        if let Some((agent, model)) = prepared(raw) {
            models.insert(agent, model);
            return Ok(false);
        }
        let Some((agent, usage)) = reported(raw) else { return Ok(false) };
        let parts = extension_usage(&usage);
        if parts.tally.total().saturating_add(parts.unclassified) == 0 {
            return Ok(false);
        }
        let model = model_id(models.get(&agent).map(String::as_str)).unwrap_or_else(|| client.to_owned());
        // The line carries no message, request or trace id, so it has no
        // business identity: two real requests in the same second with the same
        // counts stay two records. Folding them by the timestamp would silently
        // drop one; a mirrored duplicate is the cheaper error.
        let mut spend = record(client, model, at, agent, parts.tally, parts.unclassified, None);
        spend.project = workspace.clone();
        records.push(spend);
        Ok(first_only)
    })?;
    Ok((records, notes))
}

fn extension_usage(usage: &Value) -> Parts {
    combine(
        first_count(usage, &["inputTokens", "prompt_tokens"]),
        first_count(usage, &["outputTokens", "output_tokens"]),
        first_count(usage, &["cacheTokens", "cachedReadTokens", "cache_read_input_tokens"]),
        first_count(usage, &["cachedWriteTokens", "cacheCreationTokens", "cache_creation_input_tokens"]),
        first_count(usage, &["reasoningTokens", "completionThinkingTokens"]),
        first_count(usage, &["totalTokens", "total_tokens"]),
        first_count(usage, &["cachedMissTokens", "cacheMissTokens"]),
    )
}

/// `[CraftInvokableAgent] [agent] Model prepared: GPT Five (gpt-5)`
fn prepared(line: &str) -> Option<(String, String)> {
    const MARKER: &str = "[CraftInvokableAgent]";
    let after = line.find(MARKER)? + MARKER.len();
    let agent = bracketed(line, after)?;
    const LABEL: &str = "Model prepared:";
    let label = line.get(after..)?.find(LABEL)? + after + LABEL.len();
    let rest = line.get(label..)?;
    let open = rest.rfind('(')?;
    let close = rest.rfind(')')?;
    if open >= close {
        return None;
    }
    let model = rest.get(open + 1..close)?.trim();
    if model.is_empty() {
        return None;
    }
    Some((agent, model.to_owned()))
}

/// `[AgentReporter] [agent] Agent execution successful with usage: {...}`
fn reported(line: &str) -> Option<(String, Value)> {
    const MARKER: &str = "[AgentReporter]";
    let after = line.find(MARKER)? + MARKER.len();
    let agent = bracketed(line, after)?;
    const LABEL: &str = "usage:";
    let usage = line.get(after..)?.find(LABEL)? + after + LABEL.len();
    let object = brace_object(line.get(usage..)?)?;
    Some((agent, object))
}

/// The first `[value]` after `from`, which is where the agent id sits.
fn bracketed(line: &str, from: usize) -> Option<String> {
    let open = line.get(from..)?.find('[')? + from;
    let close = line.get(open + 1..)?.find(']')? + open + 1;
    let value = line.get(open + 1..close)?.trim();
    (!value.is_empty()).then(|| value.to_owned())
}

/// The first balanced, unescaped `{…}` parsed as a JSON object.
fn brace_object(text: &str) -> Option<Value> {
    let bytes = text.as_bytes();
    let start = bytes.iter().position(|byte| *byte == b'{')?;
    let mut depth = 0i64;
    let mut in_string = false;
    let mut escaped = false;
    for index in start..bytes.len() {
        let byte = bytes[index];
        if in_string {
            if escaped {
                escaped = false;
            } else if byte == b'\\' {
                escaped = true;
            } else if byte == b'"' {
                in_string = false;
            }
            continue;
        }
        match byte {
            b'"' => in_string = true,
            b'{' => depth += 1,
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    return serde_json::from_slice::<Value>(&bytes[start..=index])
                        .ok()
                        .filter(Value::is_object);
                }
            }
            _ => {}
        }
    }
    None
}

// MARK: - WorkBuddy SQLite fallback

fn sqlite(
    client: &str,
    path: &Path,
    cancel: &AtomicBool,
) -> Result<(Vec<SpendRecord>, Vec<String>), String> {
    let label = label(client);
    // Only WorkBuddy keeps this aggregate store; CodeBuddy has none.
    if client != "workbuddy" {
        return Ok((Vec::new(), Vec::new()));
    }
    let db = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX)
        .map_err(|_| format!("{label}：汇总数据库无法读取。"))?;
    let _ = db.busy_timeout(Duration::from_millis(150));
    let deadline = Instant::now() + Duration::from_secs(15);
    db.progress_handler(1000, Some(move || Instant::now() > deadline));
    let mut sessions: HashMap<String, (Option<String>, Option<String>)> = HashMap::new();
    each_row(&db, "SELECT id, cwd, model FROM sessions", cancel, |row| {
        let Some(id) = column_text(row, 0) else { return Ok(()) };
        sessions.insert(id, (column_text(row, 1), column_text(row, 2)));
        Ok(())
    })?;
    let mut found: Vec<SpendRecord> = Vec::new();
    each_row(&db, "SELECT session_id, used, updated_at FROM session_usage", cancel, |row| {
        let Some(session) = column_text(row, 0) else { return Ok(()) };
        let Some(used) = column_count(row, 1).filter(|used| *used > 0) else { return Ok(()) };
        let Some(updated) = column_count(row, 2).filter(|updated| *updated > 0) else { return Ok(()) };
        let Some(at) = auto_epoch(updated) else { return Ok(()) };
        let (cwd, model) = sessions.get(&session).cloned().unwrap_or((None, None));
        let model = model_id(model.as_deref()).unwrap_or_else(|| "auto".into());
        // One row is the session's whole aggregate, so the identity includes the
        // write that produced it.
        let mut spend = record(
            "workbuddy",
            model,
            at,
            session.clone(),
            Tally::default(),
            used,
            Some(format!("workbuddy:{session}:{updated}")),
        );
        spend.project = cwd.map(|cwd| cwd.trim().to_owned()).filter(|cwd| !cwd.is_empty());
        spend.aggregate_timing = true;
        found.push(spend);
        if found.len() >= MAX_RECORDS {
            return Err(format!("{label}：汇总数据库记录超过读取上限。"));
        }
        Ok(())
    })?;
    Ok((found, Vec::new()))
}

// MARK: - SQLite / line IO

/// AgentSQLite.each: a statement that will not prepare yields no rows and no
/// error, so a schema mismatch reads as "no rows" rather than a failure.
fn each_row(
    db: &Connection,
    sql: &str,
    cancel: &AtomicBool,
    mut visit: impl FnMut(&Row) -> Result<(), String>,
) -> Result<(), String> {
    let Ok(mut statement) = db.prepare(sql) else { return Ok(()) };
    let Ok(mut rows) = statement.query([]) else { return Ok(()) };
    while let Some(row) = rows.next().map_err(|error| error.to_string())? {
        check(cancel)?;
        visit(row)?;
    }
    Ok(())
}

fn column_text(row: &Row, index: usize) -> Option<String> {
    match row.get_ref(index).ok()? {
        ValueRef::Null => None,
        ValueRef::Integer(number) => Some(number.to_string()),
        ValueRef::Real(number) => Some(number.to_string()),
        ValueRef::Text(bytes) | ValueRef::Blob(bytes) => Some(String::from_utf8_lossy(bytes).into_owned()),
    }
}

/// AgentLogIO.count over a column read as text: a whole, non-negative count.
fn column_count(row: &Row, index: usize) -> Option<u64> {
    match row.get_ref(index).ok()? {
        ValueRef::Integer(number) => u64::try_from(number).ok(),
        ValueRef::Real(number) => whole_number(number),
        ValueRef::Text(bytes) => std::str::from_utf8(bytes).ok().and_then(|raw| raw.trim().parse::<u64>().ok()),
        _ => None,
    }
}

/// LogLines: one line at a time, bounded, with a cancellation checkpoint. A
/// physical line longer than the cap is skipped whole rather than counted as a
/// truncated row.
fn each_line(
    path: &Path,
    cancel: &AtomicBool,
    notes: &mut Vec<String>,
    label: &str,
    mut visit: impl FnMut(&[u8]) -> Result<bool, String>,
) -> Result<(), String> {
    let file = File::open(path).map_err(|error| format!("{label}：文件无法读取（{error}）。"))?;
    let mut reader = BufReader::with_capacity(64 * 1024, file);
    let mut line: Vec<u8> = Vec::new();
    loop {
        check(cancel)?;
        line.clear();
        let read = (&mut reader)
            .take(MAX_LINE as u64)
            .read_until(b'\n', &mut line)
            .map_err(|error| error.to_string())?;
        if read == 0 {
            return Ok(());
        }
        if line.len() == MAX_LINE && line.last() != Some(&b'\n') {
            loop {
                let mut rest = Vec::new();
                let read = (&mut reader)
                    .take(MAX_LINE as u64)
                    .read_until(b'\n', &mut rest)
                    .map_err(|error| error.to_string())?;
                if read == 0 || rest.last() == Some(&b'\n') {
                    break;
                }
            }
            notes.push(format!("{label}：一条记录超过 8 MiB，计数可能不完整。"));
            continue;
        }
        if line.last() == Some(&b'\n') {
            line.pop();
        }
        if line.last() == Some(&b'\r') {
            line.pop();
        }
        if visit(&line)? {
            return Ok(());
        }
    }
}

// MARK: - Counters

#[derive(Default)]
struct Parts {
    tally: Tally,
    unclassified: u64,
}

/// EditorLog.combine. Only the store's own total can prove that a reported input
/// still contains the cache counts, or that reasoning is separate from output.
fn combine(
    input: Option<u64>,
    output: Option<u64>,
    cache_read: Option<u64>,
    cache_write: Option<u64>,
    reasoning: Option<u64>,
    total: Option<u64>,
    exclusive_input: Option<u64>,
) -> Parts {
    let cache_read_value = cache_read.unwrap_or(0);
    let cache_write_value = cache_write.unwrap_or(0);
    let reasoning_value = reasoning.unwrap_or(0);
    let names_a_kind = input.is_some()
        || output.is_some()
        || cache_read.is_some()
        || cache_write.is_some()
        || reasoning.is_some()
        || exclusive_input.is_some();
    // A bare total names no kind: it is a session total, never fresh input.
    if !names_a_kind && total.is_some_and(|total| total > 0) {
        return Parts { tally: Tally::default(), unclassified: total.unwrap_or(0) };
    }
    let reported_input = input.unwrap_or(0);
    let raw_output = output.unwrap_or(0);
    let base_input = exclusive_input.unwrap_or(reported_input);
    let disjoint = base_input
        .saturating_add(raw_output)
        .saturating_add(cache_read_value)
        .saturating_add(cache_write_value)
        .saturating_add(reasoning_value);
    let inclusive = exclusive_input.is_none()
        && total.is_some_and(|total| total == reported_input.saturating_add(raw_output) && total != disjoint);
    let fresh_input = if inclusive {
        reported_input.saturating_sub(cache_read_value).saturating_sub(cache_write_value)
    } else {
        base_input
    };
    let unclassified = total.filter(|total| *total > disjoint).map(|total| total - disjoint).unwrap_or(0);
    let reasoning_is_separate = total.is_some_and(|total| total == disjoint);
    let folded_output = if reasoning_is_separate { raw_output.saturating_add(reasoning_value) } else { raw_output };
    Parts {
        tally: Tally {
            input: fresh_input,
            output: folded_output,
            cache_write: cache_write_value,
            cache_read: cache_read_value,
            ..Default::default()
        },
        unclassified,
    }
}

/// EditorLog.firstCount: the first positive count, else the first present zero.
fn first_count(object: &Value, keys: &[&str]) -> Option<u64> {
    let mut zero = None;
    for key in keys {
        if let Some(count) = object.get(*key).and_then(whole_count) {
            if count > 0 {
                return Some(count);
            }
            zero.get_or_insert(count);
        }
    }
    zero
}

/// AgentLogIO.count: a whole, non-negative count, or nil. nil is not zero.
fn whole_count(value: &Value) -> Option<u64> {
    // A boolean is not a number, however serde represents it.
    if value.is_boolean() {
        return None;
    }
    if let Some(number) = value.as_f64() {
        return whole_number(number);
    }
    let raw = value.as_str()?.trim();
    raw.parse::<u64>().ok().filter(|number| *number <= i64::MAX as u64)
}

fn whole_number(number: f64) -> Option<u64> {
    (number.is_finite() && number >= 0.0 && number.fract() == 0.0 && number < 9_223_372_036_854_775_808.0)
        .then_some(number as u64)
}

/// EditorLog.modelID: a gateway `provider/model` prefix is dropped.
fn model_id(raw: Option<&str>) -> Option<String> {
    let raw = raw.map(str::trim).filter(|raw| !raw.is_empty())?;
    match raw.rsplit_once('/') {
        Some((_, tail)) if !tail.is_empty() => Some(tail.to_owned()),
        _ => Some(raw.to_owned()),
    }
}

/// EditorLog.autoEpoch: a positive epoch, milliseconds above 1e10.
fn auto_epoch(value: u64) -> Option<DateTime<Utc>> {
    if value == 0 {
        return None;
    }
    if value > 10_000_000_000 {
        DateTime::from_timestamp_millis(i64::try_from(value).ok()?)
    } else {
        DateTime::from_timestamp(i64::try_from(value).ok()?, 0)
    }
}

/// EditorLog.naiveTimestamp: the log writes a naive local wall-clock string, and
/// that string is the timestamp. Nothing is filled from mtime or the clock.
fn naive_timestamp(line: &str) -> Option<DateTime<Utc>> {
    let line = line.trim_start_matches([' ', '\t']);
    const FORMATS: [(&str, usize); 4] = [
        ("%Y/%m/%d %H:%M:%S%.3f", 23),
        ("%Y/%m/%d %H:%M:%S", 19),
        ("%Y-%m-%d %H:%M:%S%.3f", 23),
        ("%Y-%m-%d %H:%M:%S", 19),
    ];
    for (format, length) in FORMATS {
        let Some(candidate) = line.get(..length) else { continue };
        let Ok(naive) = chrono::NaiveDateTime::parse_from_str(candidate, format) else { continue };
        if let Some(at) = Local.from_local_datetime(&naive).earliest() {
            return Some(at.with_timezone(&Utc));
        }
    }
    None
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
        if fs::create_dir_all(&candidate).is_ok() {
            return candidate;
        }
        // Sandboxed test binaries can be denied writes outside the workspace.
        let fallback = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target").join("test-scratch").join(format!("pulsewin-{suffix}"));
        fs::create_dir_all(&fallback).unwrap_or_else(|error| panic!("create_dir_all {}: {error:?}", fallback.display()));
        fallback
    }

    fn home() -> PathBuf {
        temp_dir("tencent-home")
    }

    fn write(path: &Path, text: &str) {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(path, text).unwrap();
    }

    fn transcript_line(id: &str, message_id: Option<&str>, usage: Value, at: i64, status: Option<&str>, kind: &str) -> String {
        let mut line = json!({
            "id": id,
            "timestamp": at * 1000,
            "type": kind,
            "sessionId": "sess-a",
            "cwd": "/work/pulse",
        });
        if let Some(status) = status {
            line["status"] = Value::from(status);
        }
        let mut provider = json!({ "model": "gpt-5" });
        if let Some(message_id) = message_id {
            provider["messageId"] = Value::from(message_id);
        }
        if kind == "message" {
            line["role"] = Value::from("assistant");
            line["message"] = json!({ "model": "gpt-5", "usage": usage });
        } else {
            provider["usage"] = usage;
        }
        line["providerData"] = provider;
        line.to_string()
    }

    /// The dispatcher's own walk: read each candidate file, concatenate.
    fn read_tree(roots: &[PathBuf], id: &str) -> (Vec<SpendRecord>, Vec<String>) {
        let cancel = AtomicBool::new(false);
        let mut files = Vec::new();
        for root in roots {
            walk(root, &mut files);
        }
        files.sort();
        let mut records = Vec::new();
        let mut notes = Vec::new();
        for file in files {
            if !candidate(&file, id) {
                continue;
            }
            let (mut found, mut said) = read(&file, id, &cancel).unwrap();
            records.append(&mut found);
            notes.append(&mut said);
        }
        records.sort_by_key(|record| record.source_timestamp.unwrap_or(0));
        notes.sort();
        notes.dedup();
        (records, notes)
    }

    fn walk(directory: &Path, files: &mut Vec<PathBuf>) {
        let Ok(entries) = fs::read_dir(directory) else { return };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                walk(&path, files);
            } else {
                files.push(path);
            }
        }
    }

    #[test]
    fn roots_are_the_two_tencent_trees() {
        let home = home();
        assert_eq!(roots(&home, "codebuddy", &|_: &str| None), vec![home.join(".codebuddy")]);
        assert_eq!(
            roots(&home, "workbuddy", &|_: &str| None),
            vec![home.join(".workbuddy"), home.join(".workbuddy-ai")]
        );
        assert!(roots(&home, "antigravity-cli", &|_: &str| None).is_empty());
        // The trees live under home on every platform; no environment is invented.
        let overridden = roots(&home, "codebuddy", &|key: &str| (key == "APPDATA").then(|| "D:\\roaming".to_string()));
        assert_eq!(overridden, vec![home.join(".codebuddy")]);
        let _ = fs::remove_dir_all(&home);
    }

    #[test]
    fn candidate_takes_transcripts_logs_and_the_workbuddy_database() {
        let home = home();
        let tree = home.join(".codebuddy");
        assert!(candidate(&tree.join("projects/slug/s.jsonl"), "codebuddy"));
        assert!(candidate(&tree.join("logs/Pulse__2026.log"), "codebuddy"));
        assert!(candidate(&home.join(".workbuddy/workbuddy.db"), "workbuddy"));
        // AgentLogIO skips the root-level `binaries/` subtree entirely.
        assert!(!candidate(&tree.join("binaries/tool.jsonl"), "codebuddy"));
        assert!(!candidate(&tree.join("binaries/nested/tool.log"), "codebuddy"));
        // SQLite's shared-memory sidecar is never a store.
        assert!(!candidate(&home.join(".workbuddy/workbuddy.db-shm"), "workbuddy"));
        assert!(!candidate(&tree.join("projects/slug/readme.md"), "codebuddy"));
        assert!(!candidate(&tree.join("projects/slug/s.jsonl"), "antigravity-cli"));
        let _ = fs::remove_dir_all(&home);
    }

    #[test]
    fn transcript_keeps_the_larger_snapshot_and_skips_incomplete_rows() {
        let home = home();
        let file = home.join(".codebuddy/projects/slug/s.jsonl");
        let lines = [
            transcript_line("line-1", Some("msg-1"), json!({
                "input_tokens": 100, "output_tokens": 40,
                "cache_read_input_tokens": 30, "cache_creation_input_tokens": 20,
            }), 1_767_343_200, Some("completed"), "message"),
            // The same message, a more complete snapshot.
            transcript_line("line-2", Some("msg-1"), json!({
                "input_tokens": 150, "output_tokens": 50,
                "cache_read_input_tokens": 40, "cache_creation_input_tokens": 25,
            }), 1_767_343_200, Some("completed"), "message"),
            // A row the product itself calls incomplete.
            transcript_line("line-3", Some("msg-3"), json!({
                "input_tokens": 999, "output_tokens": 999,
            }), 1_767_343_200, Some("failed"), "message"),
            // A function call with its own identity.
            transcript_line("line-4", Some("fn-1"), json!({
                "input_tokens": 1, "output_tokens": 2,
                "cache_read_input_tokens": 3, "cache_creation_input_tokens": 4,
            }), 1_767_346_800, Some("completed"), "function_call"),
            // A bare total with no split.
            transcript_line("line-5", Some("msg-5"), json!({ "total_tokens": 500 }), 1_767_350_400, Some("completed"), "message"),
            // A total equal to input + output proves input still contains the cache counts.
            transcript_line("line-6", Some("msg-6"), json!({
                "input_tokens": 100, "output_tokens": 30,
                "cache_read_input_tokens": 20, "cache_creation_input_tokens": 5,
                "total_tokens": 130,
            }), 1_767_354_000, Some("completed"), "message"),
            // No timestamp: skipped rather than dated from the file.
            json!({
                "id": "line-7", "type": "message", "role": "assistant", "sessionId": "sess-a",
                "providerData": { "model": "gpt-5", "messageId": "msg-7" },
                "message": { "usage": { "input_tokens": 77, "output_tokens": 7 } },
            }).to_string(),
        ];
        write(&file, &lines.join("\n"));
        let (records, notes) = read_tree(&roots(&home, "codebuddy", &|_: &str| None), "codebuddy");
        assert_eq!(records.len(), 4, "{records:?}");
        assert_eq!(records.iter().filter(|record| record.deduplication_id.as_deref() == Some("codebuddy:sess-a:msg-1")).count(), 1);
        let merged = records.iter().find(|record| record.deduplication_id.as_deref() == Some("codebuddy:sess-a:msg-1")).unwrap();
        assert_eq!(merged.tally, Tally { input: 150, cache_write: 25, cache_read: 40, output: 50, ..Default::default() });
        assert_eq!(merged.project.as_deref(), Some("/work/pulse"));
        let bare = records.iter().find(|record| record.deduplication_id.as_deref() == Some("codebuddy:sess-a:msg-5")).unwrap();
        assert_eq!(bare.tally, Tally::default());
        assert_eq!(bare.unclassified_tokens, 500);
        assert!(!bare.aggregate_timing);
        let exclusive = records.iter().find(|record| record.deduplication_id.as_deref() == Some("codebuddy:sess-a:msg-6")).unwrap();
        assert_eq!(exclusive.tally, Tally { input: 75, cache_write: 5, cache_read: 20, output: 30, ..Default::default() });
        // No fallback was present, so nothing was excluded.
        assert!(notes.is_empty(), "{notes:?}");
        let _ = fs::remove_dir_all(&home);
    }

    #[test]
    fn transcript_supersedes_the_extension_log_and_reports_the_subset() {
        let home = home();
        write(
            &home.join(".codebuddy/projects/slug/s.jsonl"),
            &transcript_line("line-1", Some("m1"), json!({
                "input_tokens": 100, "output_tokens": 40,
                "cache_read_input_tokens": 30, "cache_creation_input_tokens": 20,
            }), 1_767_343_200, Some("completed"), "message"),
        );
        let usage = json!({
            "inputTokens": 100, "outputTokens": 40, "cacheTokens": 30, "cachedWriteTokens": 20,
        }).to_string();
        write(
            &home.join(".codebuddy/logs/Pulse__2026.log"),
            &format!(
                "2026/01/02 03:05:00.123 [CraftInvokableAgent] [agent-7] Model prepared: GPT Five (gpt-5)\n\
                 2026/01/02 03:05:04.000 [AgentReporter] [agent-7] Agent execution successful with usage: {usage}\n"
            ),
        );
        let (records, notes) = read_tree(&roots(&home, "codebuddy", &|_: &str| None), "codebuddy");
        // Only the transcript is counted; appending the log would double count.
        assert_eq!(records.len(), 1, "{records:?}");
        assert_eq!(records[0].deduplication_id.as_deref(), Some("codebuddy:sess-a:m1"));
        assert_eq!(records[0].tally, Tally { input: 100, cache_write: 20, cache_read: 30, output: 40, ..Default::default() });
        // The excluded fallback's scope is unknown, so the note says the read is a subset.
        assert!(notes.iter().any(|note| note.contains("可核对的子集")), "{notes:?}");
        let _ = fs::remove_dir_all(&home);
    }

    #[test]
    fn reasoning_is_added_to_output_only_when_the_total_proves_it_separate() {
        let home = home();
        let file = home.join(".codebuddy/projects/slug/reasoning.jsonl");
        let lines = [
            transcript_line("r1", Some("r1"), json!({
                "input_tokens": 100, "output_tokens": 50, "cache_read_input_tokens": 40,
                "cache_creation_input_tokens": 5, "reasoningTokens": 7, "total_tokens": 202,
            }), 1_767_343_200, Some("completed"), "message"),
            transcript_line("r2", Some("r2"), json!({
                "input_tokens": 100, "output_tokens": 50, "cache_read_input_tokens": 40,
                "reasoningTokens": 7, "total_tokens": 150,
            }), 1_767_346_800, Some("completed"), "message"),
            transcript_line("r3", Some("r3"), json!({
                "input_tokens": 100, "output_tokens": 50, "reasoningTokens": 7,
            }), 1_767_350_400, Some("completed"), "message"),
        ];
        write(&file, &lines.join("\n"));
        let (records, _) = read_tree(&roots(&home, "codebuddy", &|_: &str| None), "codebuddy");
        assert_eq!(records.len(), 3, "{records:?}");
        let separate = records.iter().find(|record| record.deduplication_id.as_deref() == Some("codebuddy:sess-a:r1")).unwrap();
        assert_eq!(separate.tally, Tally { input: 100, cache_write: 5, cache_read: 40, output: 57, ..Default::default() });
        let inclusive = records.iter().find(|record| record.deduplication_id.as_deref() == Some("codebuddy:sess-a:r2")).unwrap();
        assert_eq!(inclusive.tally, Tally { input: 60, cache_read: 40, output: 50, ..Default::default() });
        assert_ne!(inclusive.tally.output, 57);
        let no_total = records.iter().find(|record| record.deduplication_id.as_deref() == Some("codebuddy:sess-a:r3")).unwrap();
        assert_eq!(no_total.tally, Tally { input: 100, output: 50, ..Default::default() });
        let _ = fs::remove_dir_all(&home);
    }

    #[test]
    fn extension_log_counts_same_second_lines_separately() {
        let home = home();
        let file = home.join(".workbuddy/logs/Pulse__2026.log");
        let usage = json!({
            "inputTokens": 100, "outputTokens": 40,
            "cacheTokens": 30, "cachedWriteTokens": 20, "totalTokens": 190,
        }).to_string();
        write(&file, &format!(
            "2026/01/02 03:05:00.123 [CraftInvokableAgent] [agent-7] Model prepared: GPT Five (gpt-5)\n\
             2026/01/02 03:05:04.000 [AgentReporter] [agent-7] Agent execution successful with usage: {usage}\n\
             2026/01/02 03:05:04.400 [AgentReporter] [agent-7] Agent execution successful with usage: {usage}\n\
             2026/01/02 03:06:00.000 [AgentReporter] [agent-7] Agent execution successful with usage: {{\"totalTokens\":500}}\n"
        ));
        let (records, notes) = read_tree(&roots(&home, "workbuddy", &|_: &str| None), "workbuddy");
        // The two same-second, same-value lines are two real requests, so both
        // survive; the bare total is a third.
        assert_eq!(records.len(), 3, "{records:?}");
        let counted = Tally { input: 100, cache_write: 20, cache_read: 30, output: 40, ..Default::default() };
        let usage: Vec<_> = records.iter().filter(|record| record.tally == counted).collect();
        assert_eq!(usage.len(), 2);
        // No message id exists, so no business identity is invented.
        assert!(usage.iter().all(|record| record.deduplication_id.is_none()));
        // With only the fallback present nothing was excluded.
        assert!(notes.is_empty(), "{notes:?}");
        let record = usage[0];
        assert_eq!(record.model, "gpt-5");
        assert_eq!(record.session, "agent-7");
        assert_eq!(record.project.as_deref(), Some("Pulse"));
        // The log writes a naive local wall-clock string; its fields read back in
        // the same calendar the parser used.
        let expected = Local.with_ymd_and_hms(2026, 1, 2, 3, 5, 4).single().unwrap();
        assert_eq!(record.day, "2026-01-02");
        assert_eq!(record.hour, 3);
        assert_eq!(record.source_timestamp, Some(expected.timestamp()));
        let bare = records.iter().find(|record| record.unclassified_tokens == 500).unwrap();
        assert_eq!(bare.tally, Tally::default());
        let _ = fs::remove_dir_all(&home);
    }

    #[test]
    fn workbuddy_database_reports_one_aggregate_unclassified_quantity() {
        let home = home();
        let path = home.join(".workbuddy/workbuddy.db");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        let updated = 1_789_372_800_000i64;
        let db = Connection::open(&path).unwrap();
        db.execute_batch(
            "CREATE TABLE sessions (id TEXT PRIMARY KEY, cwd TEXT, model TEXT); \
             CREATE TABLE session_usage (session_id TEXT PRIMARY KEY, used INTEGER, size INTEGER, updated_at INTEGER, credit_json TEXT);",
        ).unwrap();
        db.execute("INSERT INTO sessions VALUES ('s1', '/work/pulse', 'gpt-5')", []).unwrap();
        db.execute("INSERT INTO session_usage VALUES ('s1', 1234, 0, ?1, NULL)", [updated]).unwrap();
        // A row with no usable numbers is not a record.
        db.execute("INSERT INTO sessions VALUES ('s2', '/work/other', NULL)", []).unwrap();
        db.execute("INSERT INTO session_usage VALUES ('s2', 0, 0, ?1, NULL)", [updated]).unwrap();
        drop(db);
        let (records, notes) = read_tree(&roots(&home, "workbuddy", &|_: &str| None), "workbuddy");
        assert_eq!(records.len(), 1, "{records:?}");
        let record = &records[0];
        assert_eq!(record.model, "gpt-5");
        assert_eq!(record.session, "s1");
        assert_eq!(record.project.as_deref(), Some("/work/pulse"));
        assert_eq!(record.tally, Tally::default());
        assert_eq!(record.unclassified_tokens, 1234);
        assert!(record.aggregate_timing);
        assert_eq!(record.deduplication_id.as_deref(), Some("workbuddy:s1:1789372800000"));
        assert!(notes.is_empty(), "{notes:?}");
        let _ = fs::remove_dir_all(&home);
    }

    #[test]
    fn message_text_is_never_serialized_and_cancellation_stops_the_read() {
        let home = home();
        let file = home.join(".codebuddy/projects/slug/secret.jsonl");
        let mut line: Value = serde_json::from_str(&transcript_line("line-1", Some("m1"), json!({
            "input_tokens": 10, "output_tokens": 2,
        }), 1_767_343_200, Some("completed"), "message")).unwrap();
        line["message"]["content"] = Value::from("SECRET PROMPT TEXT");
        write(&file, &line.to_string());
        let (records, _) = read(&file, "codebuddy", &AtomicBool::new(false)).unwrap();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].tally.input, 10);
        let serialized = serde_json::to_string(&records).unwrap();
        assert!(!serialized.contains("SECRET"));
        let cancelled = read(&file, "codebuddy", &AtomicBool::new(true));
        assert_eq!(cancelled.unwrap_err(), "cancelled");
        let _ = fs::remove_dir_all(&home);
    }
}
