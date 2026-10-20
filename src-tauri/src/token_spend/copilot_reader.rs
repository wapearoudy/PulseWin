//! Pulse 1.9 CopilotLogReader/OTEL/Desktop/VSCode (Apache-2.0): one client, three
//! unrelated stores, merged in a fixed order.
//!
//! `~/.copilot/otel/*.jsonl` is the authority for every session it names; the desktop
//! app's `data.db` (with its `session-state/<id>/events.jsonl` sidecar) is a lifetime
//! total; VS Code's `chatSessions` log is last. A later lane is filtered against the
//! earlier ones — a desktop session OTEL already named is dropped whole, and a VS Code
//! request is dropped when its own key or its `(session, instant)` is already present —
//! so one call counts once.
//!
//! **The lanes are never subtracted.** An OTEL span and a desktop lifetime are not
//! equal scopes: when a desktop row is larger than what OTEL recorded, the OTEL records
//! for that session are marked incomplete instead of a remainder being invented. The
//! same rule covers reasoning: the desktop store never declares it inside output, so it
//! is omitted and the read is flagged rather than added. VS Code's `promptTokens` holds
//! the cache with no cached figure beside it, so it is counted unclassified and never
//! priced. Nothing here turns a cost, a duration or a text length into a token count.
//!
//! `scan` is the entry point: OTEL correlation (a model or session often arrives on a
//! different line sharing the trace) and the lane merge both span files, so this client
//! cannot be read one file at a time. `read` remains the per-file seam for the JSONL
//! lanes and the SQLite lane.
#![allow(dead_code)] // The dispatcher calls `scan`; `read` is the per-file seam.

use super::measured_readers::{count, record, text, timestamp};
use super::*;
use rusqlite::{types::ValueRef, Connection, OpenFlags};
use std::collections::btree_map::Entry as MapEntry;

/// The scope each record came from. `:partial` says the read is a known subset — the
/// Rust port has no `isPartial` flag on `SpendRecord`, so the doubt travels as a scope
/// marker plus a note, and is never used to add or subtract a token.
const SCOPE_OTEL: &str = "copilot-otel";
const SCOPE_OTEL_PARTIAL: &str = "copilot-otel:partial";
const SCOPE_DESKTOP: &str = "copilot-desktop";
const SCOPE_DESKTOP_PARTIAL: &str = "copilot-desktop:partial";
const SCOPE_VSCODE: &str = "copilot-vscode";

/// Every root Copilot's records come from, in the order the lanes are collected.
///
/// A root that does not exist is still named, so the pane can show the store's real
/// locations and a store that appears later is picked up. No Copilot store has a
/// documented environment override; `%APPDATA%` only relocates the VS Code tree that
/// already sits under the default profile.
pub(super) fn roots(home: &Path, id: &str, env: &impl Fn(&str) -> Option<String>) -> Vec<PathBuf> {
    if id != "copilot" { return Vec::new(); }
    let mut roots = vec![
        home.join(".copilot/otel"),
        home.join(".copilot/data.db"),
        home.join(".copilot/session-state"),
        home.join("Library/Application Support/Code/User/workspaceStorage"),
        home.join(".config/Code/User/workspaceStorage"),
        home.join("AppData/Roaming/Code/User/workspaceStorage"),
    ];
    if let Some(appdata) = env("APPDATA").filter(|value| !value.trim().is_empty()) {
        roots.push(PathBuf::from(appdata.trim()).join("Code/User/workspaceStorage"));
    }
    roots
}

/// The two JSONL lanes only. The desktop database is read by `scan` from its named
/// root, not by the generic per-file walk, so a running database's WAL is never
/// mistaken for a settled fingerprint.
pub(super) fn candidate(path: &Path, id: &str) -> bool {
    if id != "copilot" { return false; }
    if !path.extension().is_some_and(|extension| extension.eq_ignore_ascii_case("jsonl")) { return false; }
    named_dir(path.parent(), "chatSessions") || named_dir(path.parent(), "otel")
        || path.ancestors().skip(1).take(4).any(|ancestor| named_dir(Some(ancestor), "otel"))
}

/// One file of one lane. `scan` is the complete client; this exists so a single file
/// can be exercised (and so a lane stays inspectable) without inventing a cross-file
/// result it cannot prove.
pub(super) fn read(path: &Path, _id: &str, cancel: &AtomicBool) -> Result<(Vec<SpendRecord>, Vec<String>), String> {
    check(cancel)?;
    let mut notes = Vec::new();
    let measured = if path.file_name().is_some_and(|name| name == "data.db") {
        desktop_database(path, cancel, &mut notes)?
    } else if named_dir(path.parent(), "chatSessions") {
        vscode_file(path, cancel, &mut notes)?
    } else {
        otel_lane(&[path.to_path_buf()], cancel, &mut notes)?
    };
    Ok((measured.into_iter().map(|entry| entry.record).collect(), notes))
}

/// A record plus what the boundary type cannot carry: whether the read is a known
/// subset, and the exact instant (milliseconds, not the record's seconds) that VS Code
/// is filtered on.
struct Measured { record: SpendRecord, partial: bool, instant_millis: i64 }

/// The whole client: three lanes, merged in priority order.
pub(super) fn scan(roots: &[PathBuf], cancel: &AtomicBool) -> Result<(Vec<SpendRecord>, Vec<String>), String> {
    check(cancel)?;
    let mut notes = Vec::new();
    let otel_roots: Vec<PathBuf> = roots.iter().filter(|root| named_dir(Some(root), "otel")).cloned().collect();
    let vscode_roots: Vec<PathBuf> = roots.iter().filter(|root| is_vscode_root(root)).cloned().collect();
    let desktop_roots: Vec<PathBuf> = roots.iter().filter(|root| root.file_name().is_some_and(|name| name == "data.db")).cloned().collect();

    // Lane one: OTEL is the authority for every session it names.
    let mut otel = otel_lane(&otel_roots, cancel, &mut notes)?;

    // Lane two is read *before* either lane is filtered: its rows are the lifetime
    // authority and are what says whether OTEL covered all of a session.
    let desktop_records = desktop_lane(&desktop_roots, cancel, &mut notes)?;
    if desktop_records.iter().any(|entry| entry.partial) {
        notes.push("Copilot：桌面端报告了推理 token，但没有声明它是否已包含在输出中；推理计数已略去，相关记录只是可证明的子集。".into());
    }

    // A desktop lifetime larger than what OTEL recorded means OTEL is a known subset of
    // that session. The scopes are not equal enough to subtract, so the difference is
    // stated, never turned into a record.
    let desktop_totals = totals(&desktop_records);
    let otel_totals = totals(&otel);
    let mut partly: HashSet<String> = HashSet::new();
    for (session, recorded) in &otel_totals {
        if desktop_totals.get(session).is_some_and(|lifetime| *lifetime > *recorded) { partly.insert(session.clone()); }
    }
    // A dropped desktop record that was itself a subset leaves a remainder no total can
    // show, so its doubt follows the OTEL records that displaced it.
    partly.extend(desktop_records.iter().filter(|entry| entry.partial).filter_map(|entry| session_of(&entry.record)));
    if !partly.is_empty() {
        for entry in &mut otel {
            if session_of(&entry.record).is_some_and(|session| partly.contains(&session)) {
                entry.record.source_scope = Some(SCOPE_OTEL_PARTIAL.into());
                entry.partial = true;
            }
        }
        notes.push("Copilot：桌面端会话的累计用量大于 OTEL 记录的范围；两者口径不同，未做相减，相关 OTEL 记录只覆盖了部分工作，计数可能不完整。".into());
    }

    // A desktop session already seen in OTEL is dropped whole; a record that named no
    // session can never be proven a duplicate and is always kept.
    let otel_sessions: HashSet<String> = otel.iter().filter_map(|entry| session_of(&entry.record)).collect();
    let desktop: Vec<Measured> = desktop_records.into_iter()
        .filter(|entry| session_of(&entry.record).map_or(true, |session| !otel_sessions.contains(&session)))
        .collect();

    // Lane three is filtered against everything accumulated so far, by its own key or
    // by the same session at the same instant.
    let mut accumulated = otel;
    accumulated.extend(desktop);
    let identities: HashSet<String> = accumulated.iter().filter_map(|entry| entry.record.deduplication_id.clone()).collect();
    let instants: HashSet<(String, i64)> = accumulated.iter()
        .filter_map(|entry| session_of(&entry.record).map(|session| (session, entry.instant_millis)))
        .collect();
    let mut vscode = vscode_lane(&vscode_roots, cancel, &mut notes)?;
    vscode.retain(|entry| {
        if let Some(id) = &entry.record.deduplication_id { if identities.contains(id) { return false; } }
        if let Some(session) = session_of(&entry.record) { if instants.contains(&(session, entry.instant_millis)) { return false; } }
        true
    });
    accumulated.extend(vscode);

    if accumulated.len() > MAX_RECORDS { return Err("Copilot：记录超过读取上限。".into()); }
    // A deterministic order, so two runs over the same stores agree.
    accumulated.sort_by(|left, right| left.instant_millis.cmp(&right.instant_millis)
        .then_with(|| left.record.deduplication_id.cmp(&right.record.deduplication_id)));
    check(cancel)?;
    notes.sort(); notes.dedup();
    Ok((accumulated.into_iter().map(|entry| entry.record).collect(), notes))
}

fn named_dir(path: Option<&Path>, name: &str) -> bool {
    path.and_then(|path| path.file_name()).and_then(|value| value.to_str()) == Some(name)
}

fn is_vscode_root(root: &Path) -> bool {
    named_dir(Some(root), "workspaceStorage") && named_dir(root.parent(), "User")
}

// MARK: - Lane one: the OTEL JSONL export

/// Four record kinds, ordered so a lower value outranks a higher one.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
enum Lane { ChatSpan, InferenceLog, AgentTurnLog, AgentSummarySpan }

impl Lane {
    const ALL: [Lane; 4] = [Lane::ChatSpan, Lane::InferenceLog, Lane::AgentTurnLog, Lane::AgentSummarySpan];
    fn rank(self) -> u8 { match self { Lane::ChatSpan => 0, Lane::InferenceLog => 1, Lane::AgentTurnLog => 2, Lane::AgentSummarySpan => 3 } }
}

struct Ident { trace: Option<String>, span: Option<String> }

struct Candidate {
    lane: Lane,
    key: String,
    at: DateTime<Utc>,
    model: String,
    session: Option<String>,
    tally: Tally,
    unclassified: u64,
    trace: Option<String>,
    response: Option<String>,
    /// True when the record names no trace or response id, so no other lane can be
    /// checked against it. It is still counted — dropping it or folding it by an
    /// instant would both lose or duplicate real work — but the read says so.
    partial: bool,
}

impl Candidate {
    /// The same event seen twice is one event; a later copy can only add detail, so
    /// each bucket takes the larger of the two.
    fn merged(&self, other: &Candidate) -> Candidate {
        Candidate {
            lane: self.lane, key: self.key.clone(), at: self.at, model: self.model.clone(),
            session: self.session.clone(),
            tally: Tally {
                input: self.tally.input.max(other.tally.input),
                output: self.tally.output.max(other.tally.output),
                cache_write: self.tally.cache_write.max(other.tally.cache_write),
                cache_read: self.tally.cache_read.max(other.tally.cache_read),
                ..Default::default()
            },
            unclassified: self.unclassified.max(other.unclassified),
            trace: self.trace.clone(), response: self.response.clone(),
            partial: self.partial || other.partial,
        }
    }
}

/// The context one trace shares: the first stated model, session and response across
/// every line that carries that trace id.
#[derive(Default, Clone)]
struct TraceContext { model: Option<String>, session: Option<String>, response: Option<String> }

fn otel_lane(roots: &[PathBuf], cancel: &AtomicBool, notes: &mut Vec<String>) -> Result<Vec<Measured>, String> {
    let mut files = Vec::new();
    for root in roots { walk(root, cancel, notes, &|path| path.extension().is_some_and(|ext| ext.eq_ignore_ascii_case("jsonl")), &mut files)?; }
    files.sort(); files.dedup();
    let mut candidates: Vec<Candidate> = Vec::new();
    for file in files {
        check(cancel)?;
        let rows = jsonl(&file, cancel, notes)?;
        let context = trace_context(&rows);
        for (index, row) in rows.iter().enumerate() {
            if let Some(candidate) = candidate_of(row, index, &context, candidates.len()) { candidates.push(candidate); }
        }
    }
    let (kept, unnamed) = resolve(candidates)?;
    if unnamed {
        notes.push("Copilot：部分 OTEL 记录既无 trace id 也无 response id，无法与其他通道核对；已各自作为独立事件计入，计数可能不完整。".into());
    }
    Ok(kept)
}

fn attributes(row: &Value) -> &Value {
    static NULL: Value = Value::Null;
    row.get("attributes").filter(|value| value.is_object()).unwrap_or(&NULL)
}

fn identification(row: &Value) -> Ident {
    static NULL: Value = Value::Null;
    let nested = row.get("spanContext").filter(|value| value.is_object()).unwrap_or(&NULL);
    Ident {
        trace: valid_id(row.get("traceId").and_then(text).or_else(|| nested.get("traceId").and_then(text))),
        span: valid_id(row.get("spanId").and_then(text).or_else(|| nested.get("spanId").and_then(text))),
    }
}

/// The W3C non-recording sentinel — empty, or all zeroes — is absent.
fn valid_id(value: Option<String>) -> Option<String> {
    let value = value?;
    if value.is_empty() || value.bytes().all(|byte| byte == b'0') { return None; }
    Some(value)
}

fn lane_of(row: &Value) -> Option<Lane> {
    let attributes = attributes(row);
    let kind = row.get("type").and_then(text);
    let name = row.get("name").and_then(text);
    let operation = attributes.get("gen_ai.operation.name").and_then(text);
    let is_span = kind.as_deref() == Some("span") || (kind.is_none() && name.is_some() && has_span_shape(row));
    if !is_span {
        let event = attributes.get("event.name").and_then(text);
        if event.as_deref() == Some("gen_ai.client.inference.operation.details") || body_starts(row, "GenAI inference:") { return Some(Lane::InferenceLog); }
        if event.as_deref() == Some("copilot_chat.agent.turn") || body_starts(row, "copilot_chat.agent.turn") { return Some(Lane::AgentTurnLog); }
        return None;
    }
    if operation.as_deref() == Some("chat") || name.as_deref().is_some_and(|name| name.starts_with("chat ")) { return Some(Lane::ChatSpan); }
    if operation.as_deref() == Some("invoke_agent") || name.as_deref().is_some_and(|name| name.starts_with("invoke_agent ")) { return Some(Lane::AgentSummarySpan); }
    None
}

/// The shape a span has when its `type` is absent: a name plus any explicit span
/// identity, timing or kind. Inference and turn logs never carry a top-level name.
fn has_span_shape(row: &Value) -> bool {
    let identity = identification(row);
    if identity.trace.is_some() || identity.span.is_some() { return true; }
    ["startTime", "endTime", "duration", "kind"].iter().any(|key| row.get(*key).is_some())
}

fn body_starts(row: &Value, prefix: &str) -> bool {
    ["body", "_body"].iter().any(|key| row.get(*key).and_then(text).is_some_and(|body| body.starts_with(prefix)))
}

fn model_of(attributes: &Value) -> Option<String> {
    attributes.get("gen_ai.response.model").and_then(text).or_else(|| attributes.get("gen_ai.request.model").and_then(text))
}

/// The first stated session-ish id. `gen_ai.response.id` is last: it names a response,
/// and only stands in for a session when nothing better was written.
const SESSION_KEYS: [&str; 6] = [
    "gen_ai.conversation.id", "copilot_chat.session_id", "copilot_chat.chat_session_id",
    "session.id", "github.copilot.interaction_id", "gen_ai.response.id",
];

fn session_of_attributes(attributes: &Value) -> Option<String> {
    SESSION_KEYS.iter().find_map(|key| attributes.get(*key).and_then(text))
}

fn trace_context(rows: &[Value]) -> HashMap<String, TraceContext> {
    let mut contexts: HashMap<String, TraceContext> = HashMap::new();
    for row in rows {
        let Some(trace) = identification(row).trace else { continue };
        let attributes = attributes(row);
        let context = contexts.entry(trace).or_default();
        if context.model.is_none() { context.model = model_of(attributes); }
        if context.session.is_none() { context.session = session_of_attributes(attributes); }
        if context.response.is_none() { context.response = attributes.get("gen_ai.response.id").and_then(text); }
    }
    contexts
}

fn candidate_of(row: &Value, index: usize, context: &HashMap<String, TraceContext>, ordinal: usize) -> Option<Candidate> {
    let lane = lane_of(row)?;
    let attributes = attributes(row);
    let identity = identification(row);
    let shared = identity.trace.as_ref().and_then(|trace| context.get(trace));
    // A record with no usable time is not dated at all; the clock is never used.
    let at = row_timestamp(row).filter(|at| at.timestamp() > 0)?;
    let session = session_of_attributes(attributes).or_else(|| shared.and_then(|shared| shared.session.clone()));
    let response = attributes.get("gen_ai.response.id").and_then(text).or_else(|| shared.and_then(|shared| shared.response.clone()));
    let model = model_of(attributes).or_else(|| shared.and_then(|shared| shared.model.clone()))?;
    let (tally, unclassified) = counts(attributes);
    if tally.total() == 0 && unclassified == 0 { return None; }
    let turn = text_value(attributes.get("turn.index")).or_else(|| text_value(attributes.get("copilot_chat.turn.index")));
    let partial = identity.trace.is_none() && response.is_none();
    let key = dedup_key(lane, &identity, session.as_deref(), response.as_deref(), index, turn.as_deref(), ordinal);
    Some(Candidate { lane, key, at, model, session, tally, unclassified, trace: identity.trace, response, partial })
}

/// The product's own identity for a record, never a clock reading.
///
/// A trace + span, a session + span or a turn index proves two lines are one event; a
/// timestamp does not, because two different requests can share one. A record naming
/// none of them keeps its own ordinal, so over-counting two copies of one unnamed event
/// is preferred to deleting a different request that landed on the same instant.
fn dedup_key(lane: Lane, identity: &Ident, session: Option<&str>, response: Option<&str>, index: usize, turn: Option<&str>, ordinal: usize) -> String {
    match lane {
        Lane::ChatSpan | Lane::AgentSummarySpan => {
            if let (Some(trace), Some(span)) = (&identity.trace, &identity.span) { return format!("{trace}:{span}"); }
            if let (Some(session), Some(span)) = (session, &identity.span) { return format!("span:{session}:{span}"); }
            format!("span-unnamed:{ordinal}")
        }
        Lane::InferenceLog => {
            if let (Some(trace), Some(span)) = (&identity.trace, &identity.span) { return format!("log:{trace}:{span}"); }
            if let Some(response) = response { return format!("log-response:{response}"); }
            format!("log-unnamed:{ordinal}")
        }
        Lane::AgentTurnLog => {
            if let Some(trace) = &identity.trace {
                return format!("agent-turn:{trace}:{}", turn.map(str::to_owned).unwrap_or_else(|| format!("idx-{index}")));
            }
            format!("agent-turn-unnamed:{ordinal}")
        }
    }
}

/// The four disjoint kinds, plus a bare total that named no kind at all.
fn counts(attributes: &Value) -> (Tally, u64) {
    static NULL: Value = Value::Null;
    let input = count(attributes.get("gen_ai.usage.input_tokens").unwrap_or(&NULL));
    let output = count(attributes.get("gen_ai.usage.output_tokens").unwrap_or(&NULL));
    let cache_read = first_positive(attributes, &["gen_ai.usage.cache_read.input_tokens", "gen_ai.usage.cache_read_input_tokens"]);
    let cache_write = first_positive(attributes, &[
        "gen_ai.usage.cache_write.input_tokens", "gen_ai.usage.cache_creation.input_tokens",
        "gen_ai.usage.cache_write_input_tokens", "gen_ai.usage.cache_creation_input_tokens",
    ]);
    let reasoning = first_positive(attributes, &["gen_ai.usage.reasoning.output_tokens", "gen_ai.usage.reasoning_tokens"]);
    if input == 0 && output == 0 && cache_read == 0 && cache_write == 0 && reasoning == 0 {
        // A total with no split is real work Pulse cannot place: counted as
        // unclassified, never put into the input bucket.
        let total = first_positive(attributes, &["gen_ai.usage.total_tokens", "gen_ai.usage.total.tokens", "total_tokens"]);
        return (Tally::default(), total);
    }
    // Input includes the cache read; remove it once. Reasoning is a subset of output, so
    // it stays inside output and only stands in when output was not reported.
    let fresh = input.saturating_sub(cache_read.min(input));
    let folded_output = if output > 0 { output } else { reasoning };
    (Tally { input: fresh, cache_write, cache_read, output: folded_output, ..Default::default() }, 0)
}

fn first_positive(attributes: &Value, keys: &[&str]) -> u64 {
    for key in keys {
        let value = count(attributes.get(*key).unwrap_or(&Value::Null));
        if value > 0 { return value; }
    }
    0
}

fn text_value(value: Option<&Value>) -> Option<String> {
    let value = value?;
    if let Some(text) = text(value) { return Some(text); }
    whole_count(Some(value)).map(|number| number.to_string())
}

/// The first usable timing key, in the format's own order. A numeric `duration` is
/// taken as milliseconds and only consulted to back-calculate a start from an `endTime`.
fn row_timestamp(row: &Value) -> Option<DateTime<Utc>> {
    if let Some(value) = row.get("startTime") { if let Some(at) = timestamp(value, false) { return Some(at); } }
    if let Some(value) = row.get("endTime") {
        if let Some(end) = timestamp(value, false) {
            return Some(match duration_of(row) {
                Some(seconds) => end - chrono::Duration::milliseconds((seconds * 1000.).round() as i64),
                None => end,
            });
        }
    }
    for key in ["hrTime", "_hrTime"] {
        let Some(pair) = row.get(key).and_then(Value::as_array) else { continue };
        let Some(seconds) = pair.first().and_then(|value| whole_count(Some(value))) else { continue };
        let nanos = pair.get(1).and_then(|value| whole_count(Some(value))).unwrap_or(0);
        let millis = seconds as f64 * 1000. + nanos as f64 / 1_000_000.;
        if let Some(at) = millis_at(millis) { return Some(at); }
    }
    for key in ["time", "timestamp", "observedTimestamp"] {
        if let Some(value) = row.get(key) { if let Some(at) = timestamp(value, false) { return Some(at); } }
    }
    let nanos = row.get("timeUnixNano").and_then(|value| whole_count(Some(value)))?;
    millis_at(nanos as f64 / 1_000_000.)
}

fn duration_of(row: &Value) -> Option<f64> {
    let value = row.get("duration")?;
    if let Some(milliseconds) = whole_count(Some(value)) { return Some(milliseconds as f64 / 1000.); }
    if value.is_object() {
        let start = value.get("startTime").and_then(|value| timestamp(value, false))?;
        let end = value.get("endTime").and_then(|value| timestamp(value, false))?;
        return Some((end - start).num_milliseconds() as f64 / 1000.);
    }
    None
}

fn millis_at(millis: f64) -> Option<DateTime<Utc>> {
    if !millis.is_finite() || millis.abs() >= 9.2e18 { return None; }
    DateTime::from_timestamp_millis(millis.round() as i64)
}

/// Same-key copies collapse first, then a higher lane drops a lower one that shares a
/// trace or a response id. The coarse conversation id is deliberately not used: it
/// spans many turns.
fn resolve(candidates: Vec<Candidate>) -> Result<(Vec<Measured>, bool), String> {
    let mut by_key: BTreeMap<String, Candidate> = BTreeMap::new();
    for candidate in candidates {
        let key = candidate.key.clone();
        match by_key.entry(key) {
            MapEntry::Occupied(mut slot) => { let merged = slot.get().merged(&candidate); slot.insert(merged); }
            MapEntry::Vacant(slot) => { slot.insert(candidate); }
        }
    }
    let deduped: Vec<Candidate> = by_key.into_values().collect();
    let mut traces: HashMap<Lane, HashSet<String>> = HashMap::new();
    let mut responses: HashMap<Lane, HashSet<String>> = HashMap::new();
    for candidate in &deduped {
        if let Some(trace) = &candidate.trace { traces.entry(candidate.lane).or_default().insert(trace.clone()); }
        if let Some(response) = &candidate.response { responses.entry(candidate.lane).or_default().insert(response.clone()); }
    }
    let mut unnamed = false;
    let mut kept = Vec::new();
    for candidate in deduped {
        let suppressed = Lane::ALL.iter().any(|higher| higher.rank() < candidate.lane.rank() && (
            candidate.trace.as_ref().is_some_and(|trace| traces.get(higher).is_some_and(|set| set.contains(trace)))
            || candidate.response.as_ref().is_some_and(|response| responses.get(higher).is_some_and(|set| set.contains(response)))));
        if suppressed { continue; }
        if candidate.partial { unnamed = true; }
        let mut entry = record("copilot", candidate.model, candidate.at, candidate.session.unwrap_or_default(),
            candidate.tally, candidate.unclassified, Some(format!("copilot-otel:{}", candidate.key)));
        entry.source_scope = Some(if candidate.partial { SCOPE_OTEL_PARTIAL } else { SCOPE_OTEL }.into());
        kept.push(Measured { record: entry, partial: candidate.partial, instant_millis: candidate.at.timestamp_millis() });
    }
    Ok((kept, unnamed))
}

// MARK: - Lane two: the desktop database and its sidecar

fn desktop_lane(roots: &[PathBuf], cancel: &AtomicBool, notes: &mut Vec<String>) -> Result<Vec<Measured>, String> {
    let mut measured = Vec::new();
    for root in roots {
        check(cancel)?;
        if !root.is_file() { continue; }
        measured.extend(desktop_database(root, cancel, notes)?);
    }
    Ok(measured)
}

/// A database row: cumulative against its immutable `created_at`.
struct DesktopRow {
    id: String,
    title: Option<String>,
    model: Option<String>,
    input: u64,
    output: u64,
    cached: u64,
    reasoning: u64,
    created_at: Option<DateTime<Utc>>,
}

/// Five counts. `reasoning` is deliberately outside `counted`: the store never states it
/// is inside `output`, so it is never added to a bucket — a read that stated it is
/// flagged instead.
#[derive(Clone, Copy, Default)]
struct Counts { input: u64, output: u64, cache_read: u64, cache_write: u64, reasoning: u64 }

impl Counts {
    fn counted(&self) -> u64 { self.input.saturating_add(self.output).saturating_add(self.cache_read).saturating_add(self.cache_write) }
    fn omitted_reasoning(&self) -> bool { self.reasoning > 0 }
    fn add(&mut self, other: &Counts) {
        self.input = self.input.saturating_add(other.input);
        self.output = self.output.saturating_add(other.output);
        self.cache_read = self.cache_read.saturating_add(other.cache_read);
        self.cache_write = self.cache_write.saturating_add(other.cache_write);
        self.reasoning = self.reasoning.saturating_add(other.reasoning);
    }
}

fn desktop_database(database: &Path, cancel: &AtomicBool, notes: &mut Vec<String>) -> Result<Vec<Measured>, String> {
    check(cancel)?;
    let connection = match Connection::open_with_flags(database, OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX) {
        Ok(connection) => connection,
        Err(error) => { notes.push(format!("Copilot：桌面端数据库无法读取（{error}），计数可能不完整。")); return Ok(Vec::new()); }
    };
    let _ = connection.busy_timeout(Duration::from_millis(150));
    let present = match columns(&connection, "sessions") {
        Ok(present) => present,
        Err(error) => { notes.push(format!("Copilot：桌面端数据库结构无法读取（{error}），计数可能不完整。")); return Ok(Vec::new()); }
    };
    // A schema that does not match simply yields no rows.
    if present.is_empty() { return Ok(Vec::new()); }
    let names = ["id", "title", "model", "total_input_tokens", "total_output_tokens", "total_cached_tokens", "total_reasoning_tokens", "created_at"];
    let list = names.iter().map(|name| if present.contains(*name) { (*name).to_string() } else { "NULL".into() }).collect::<Vec<_>>().join(",");
    let mut statement = connection.prepare(&format!("SELECT {list} FROM sessions LIMIT {}", MAX_RECORDS + 1))
        .map_err(|error| format!("Copilot：桌面端数据库无法查询（{error}）。"))?;
    let mut cursor = statement.query([]).map_err(|error| format!("Copilot：桌面端数据库无法查询（{error}）。"))?;
    let mut rows: Vec<DesktopRow> = Vec::new();
    while let Some(row) = cursor.next().map_err(|error| format!("Copilot：桌面端数据库读取失败（{error}）。"))? {
        check(cancel)?;
        if rows.len() >= MAX_RECORDS { return Err("Copilot：桌面端记录超过读取上限。".into()); }
        let Some(id) = column_text(row, 0) else { continue };
        let input = column_count(row, 3);
        let output = column_count(row, 4);
        let cached = column_count(row, 5);
        let reasoning = column_count(row, 6);
        // A row with no tokens at all is not work.
        if input == 0 && output == 0 && cached == 0 && reasoning == 0 { continue; }
        rows.push(DesktopRow {
            id,
            title: column_text(row, 1),
            model: column_text(row, 2).and_then(|model| text(&Value::String(model))),
            input, output, cached, reasoning,
            created_at: column_text(row, 7).as_deref().and_then(flexible),
        });
    }
    drop(cursor); drop(statement);
    let sidecar = database.parent().unwrap_or(Path::new(".")).join("session-state");
    let mut measured = Vec::new();
    for row in rows {
        check(cancel)?;
        measured.extend(read_session(&row, &sidecar, cancel, notes)?);
    }
    Ok(measured)
}

/// One session: the sidecar's snapshots become per-run increments, and the database row
/// is then the authority — a per-row budget caps what the sidecar may contribute, and
/// whatever the increments leave unexplained is emitted once at `created_at`.
fn read_session(row: &DesktopRow, sidecar_root: &Path, cancel: &AtomicBool, notes: &mut Vec<String>) -> Result<Vec<Measured>, String> {
    // A session id is a directory name here. One carrying a separator or naming a parent
    // is not a directory this reader will open.
    let events = if row.id.is_empty() || row.id.contains('/') || row.id.contains('\\') || row.id == "." || row.id == ".." {
        Vec::new()
    } else {
        let path = sidecar_root.join(&row.id).join("events.jsonl");
        if path.is_file() { jsonl(&path, cancel, notes)? } else { Vec::new() }
    };
    let mut running: HashMap<String, Counts> = HashMap::new();
    let mut applied = Counts::default();
    let mut tracked_model = row.model.clone();
    let mut workspace: Option<String> = None;
    let mut opened_with_start = false;
    let mut saw_first = false;
    let mut measured = Vec::new();
    for (index, event) in events.iter().enumerate() {
        check(cancel)?;
        let kind = event.get("type").and_then(text);
        if !saw_first { saw_first = true; opened_with_start = kind.as_deref() == Some("session.start"); }
        match kind.as_deref() {
            Some("session.start") => { if let Some(cwd) = workspace_of_event(event) { workspace = Some(cwd); } }
            Some("session.model_change") => {
                if let Some(next) = event.get("data").and_then(|data| data.get("newModel")).and_then(text) {
                    if next != "auto" { tracked_model = Some(next); }
                }
            }
            Some("session.shutdown") => {
                // A snapshot with no stated time cannot be placed; its tokens fall to
                // the remainder at `created_at` instead.
                let Some(at) = event_timestamp(event) else { continue };
                let data = event.get("data").filter(|value| value.is_object());
                let metrics = data.and_then(|data| data.get("modelMetrics")).filter(|value| value.is_object());
                let identity = event_identity(event, index);
                let mut trackers: Vec<String> = metrics.map(|metrics| metrics.as_object().map(|map| map.keys().cloned().collect()).unwrap_or_default()).unwrap_or_default();
                trackers.sort();
                for tracker in trackers {
                    let Some(entry) = metrics.and_then(|metrics| metrics.get(&tracker)) else { continue };
                    let Some(current) = usage_of(entry) else { continue };
                    let model = resolved_model(&tracker, data, tracked_model.as_deref(), row.model.as_deref());
                    let previous = running.get(&model).copied();
                    let delta = subtract(current, previous);
                    running.insert(model.clone(), current);
                    // The log did not open with a start, so its earliest snapshot is an
                    // unknown baseline rather than a delta.
                    if previous.is_none() && !opened_with_start { continue; }
                    let budgeted = budget(delta, applied, row);
                    applied.add(&budgeted);
                    if budgeted.counted() == 0 { continue; }
                    let mut entry = record("copilot", model.clone(), at, row.id.clone(), tally(budgeted), 0,
                        Some(format!("copilot-desktop:{}:shutdown:{}:{}", row.id, identity, model)));
                    entry.aggregate_timing = true;
                    entry.project = workspace.clone();
                    let partial = delta.omitted_reasoning();
                    entry.source_scope = Some(if partial { SCOPE_DESKTOP_PARTIAL } else { SCOPE_DESKTOP }.into());
                    measured.push(Measured { record: entry, partial, instant_millis: at.timestamp_millis() });
                }
            }
            _ => {}
        }
    }
    // Whatever the snapshots did not account for — a run that died before shutdown, a
    // session the CLI wrote, or a missing head — is emitted once at the row's creation.
    let remainder = residual(applied, row);
    if remainder.counted() > 0 {
        if let Some(created_at) = row.created_at {
            let model = row.model.clone().or_else(|| tracked_model.clone()).unwrap_or_else(|| "auto".into());
            let partial = remainder.omitted_reasoning();
            let mut entry = record("copilot", model, created_at, row.id.clone(), tally(remainder), 0,
                Some(format!("copilot-desktop:{}:row", row.id)));
            entry.aggregate_timing = true;
            entry.project = workspace.clone();
            entry.source_scope = Some(if partial { SCOPE_DESKTOP_PARTIAL } else { SCOPE_DESKTOP }.into());
            measured.push(Measured { record: entry, partial, instant_millis: created_at.timestamp_millis() });
        }
    }
    Ok(measured)
}

/// The tally for a run or remainder. Input is gross until here: both the row and the
/// sidecar hold the cached prefix, so the budget compares gross with gross and only the
/// record takes the cache out — otherwise the prefix is counted twice and priced once at
/// the full rate. A remainder has no write figure, so what was written stays in input.
fn tally(counts: Counts) -> Tally {
    Tally {
        input: counts.input.saturating_sub(counts.cache_read).saturating_sub(counts.cache_write),
        cache_write: counts.cache_write,
        cache_read: counts.cache_read,
        output: counts.output,
        ..Default::default()
    }
}

/// The sidecar's next increment, capped so it can never carry a row past its own lifetime
/// total. cache_write has no row column to compare against.
fn budget(delta: Counts, applied: Counts, row: &DesktopRow) -> Counts {
    Counts {
        input: delta.input.min(row.input.saturating_sub(applied.input)),
        output: delta.output.min(row.output.saturating_sub(applied.output)),
        cache_read: delta.cache_read.min(row.cached.saturating_sub(applied.cache_read)),
        cache_write: delta.cache_write,
        reasoning: delta.reasoning.min(row.reasoning.saturating_sub(applied.reasoning)),
    }
}

fn residual(applied: Counts, row: &DesktopRow) -> Counts {
    Counts {
        input: row.input.saturating_sub(applied.input),
        output: row.output.saturating_sub(applied.output),
        cache_read: row.cached.saturating_sub(applied.cache_read),
        cache_write: 0,
        reasoning: row.reasoning.saturating_sub(applied.reasoning),
    }
}

fn subtract(current: Counts, previous: Option<Counts>) -> Counts {
    let Some(previous) = previous else { return current };
    Counts {
        input: current.input.saturating_sub(previous.input),
        output: current.output.saturating_sub(previous.output),
        cache_read: current.cache_read.saturating_sub(previous.cache_read),
        cache_write: current.cache_write.saturating_sub(previous.cache_write),
        reasoning: current.reasoning.saturating_sub(previous.reasoning),
    }
}

fn workspace_of_event(event: &Value) -> Option<String> {
    event.get("data").and_then(|data| data.get("context")).and_then(|context| context.get("cwd")).and_then(text)
}

fn usage_of(entry: &Value) -> Option<Counts> {
    let usage = entry.get("usage").filter(|value| value.is_object())?;
    Some(Counts {
        input: clamped_count(usage.get("inputTokens")),
        output: clamped_count(usage.get("outputTokens")),
        cache_read: clamped_count(usage.get("cacheReadTokens")),
        cache_write: clamped_count(usage.get("cacheWriteTokens")),
        reasoning: clamped_count(usage.get("reasoningTokens")),
    })
}

/// A tracker key of `""` or `"auto"` defers to the run's current model, then to the last
/// change, then to the row.
fn resolved_model(tracker: &str, data: Option<&Value>, tracked: Option<&str>, row: Option<&str>) -> String {
    if !tracker.is_empty() && tracker != "auto" { return tracker.to_owned(); }
    if let Some(current) = data.and_then(|data| data.get("currentModel")).and_then(text) { return current; }
    if let Some(tracked) = tracked { return tracked.to_owned(); }
    if let Some(row) = row { return row.to_owned(); }
    "auto".into()
}

fn event_timestamp(event: &Value) -> Option<DateTime<Utc>> {
    if let Some(at) = flexible_value(event.get("timestamp")) { return Some(at); }
    flexible_value(event.get("data").and_then(|data| data.get("timestamp")))
}

/// The envelope's own id, else a stable hash of the event JSON, else its position. The
/// id is what keeps two runs' snapshots apart.
fn event_identity(event: &Value, index: usize) -> String {
    if let Some(id) = event.get("id").and_then(text) { return id; }
    if let Ok(bytes) = serde_json::to_vec(event) { return format!("{:016x}", digest(&bytes)); }
    format!("idx-{index}")
}

// MARK: - Lane three: VS Code's append/patch chat-session log

fn vscode_lane(roots: &[PathBuf], cancel: &AtomicBool, notes: &mut Vec<String>) -> Result<Vec<Measured>, String> {
    let mut files = Vec::new();
    for root in roots { walk(root, cancel, notes, &|path| path.extension().is_some_and(|ext| ext.eq_ignore_ascii_case("jsonl")) && named_dir(path.parent(), "chatSessions"), &mut files)?; }
    files.sort(); files.dedup();
    let mut measured = Vec::new();
    for file in files {
        check(cancel)?;
        measured.extend(vscode_file(&file, cancel, notes)?);
    }
    Ok(measured)
}

fn vscode_file(path: &Path, cancel: &AtomicBool, notes: &mut Vec<String>) -> Result<Vec<Measured>, String> {
    let session = path.file_stem().unwrap_or_default().to_string_lossy().into_owned();
    let workspace = vscode_workspace(path, cancel);
    let rows = jsonl(path, cancel, notes)?;
    let requests = reconstruct(&rows);
    let mut used: HashSet<String> = HashSet::new();
    let mut measured = Vec::new();
    let mut untimed = false;
    for request in requests {
        check(cancel)?;
        match vscode_record(&request, &session, workspace.as_deref()) {
            VscodeRequest::Record(entry) => {
                let id = unique_id(entry.record.deduplication_id.clone(), &mut used);
                let mut entry = entry;
                entry.record.deduplication_id = id;
                measured.push(entry);
            }
            VscodeRequest::Untimed => untimed = true,
            VscodeRequest::Skip => {}
        }
        if measured.len() >= MAX_RECORDS { return Err("Copilot：VS Code 记录超过读取上限。".into()); }
    }
    if untimed {
        notes.push("Copilot：部分 VS Code 请求没有时间戳，无法定位到真实时间，已跳过而非按纪元日期计入。".into());
    }
    if measured.iter().any(|entry| entry.record.unclassified_tokens > 0) {
        notes.push("Copilot：VS Code 的 promptTokens 含缓存且没有缓存计数，按未分类计入、不参与计价。".into());
    }
    Ok(measured)
}

enum VscodeRequest { Record(Measured), Untimed, Skip }

fn vscode_record(request: &Value, session: &str, workspace: Option<&str>) -> VscodeRequest {
    let metadata = request.get("result").and_then(|result| result.get("metadata")).filter(|value| value.is_object());
    let resolved = metadata.and_then(|metadata| metadata.get("resolvedModel")).and_then(text);
    let model_id = request.get("modelId").and_then(text);
    // Not a Copilot request: an editor-served model with neither a resolved model nor
    // the product's own id prefix.
    if resolved.is_none() && !model_id.as_deref().is_some_and(|id| id.starts_with("copilot/")) { return VscodeRequest::Skip; }
    let model = resolved.or_else(|| stripped(model_id.as_deref())).unwrap_or_else(|| "auto".into());
    let prompt = whole_count(request.get("promptTokens"))
        .or_else(|| whole_count(metadata.and_then(|metadata| metadata.get("promptTokens"))))
        .unwrap_or(0);
    let completion = whole_count(request.get("completionTokens"))
        .or_else(|| whole_count(metadata.and_then(|metadata| metadata.get("outputTokens"))))
        .unwrap_or(0);
    let thinking = thinking_tokens(metadata);
    // Missing time is not a date: the format names no session or report date to borrow,
    // so an untimed request is skipped rather than placed at the epoch or on the clock.
    let Some(at) = request.get("timestamp").and_then(|value| timestamp(value, true))
        .or_else(|| metadata.and_then(|metadata| metadata.get("timestamp")).and_then(|value| timestamp(value, true)))
    else { return VscodeRequest::Untimed; };
    if at.timestamp() <= 0 { return VscodeRequest::Untimed; }
    let tally = Tally { output: completion.saturating_add(thinking), ..Default::default() };
    if tally.total() == 0 && prompt == 0 { return VscodeRequest::Skip; }
    let mut entry = record("copilot", model, at, session.to_owned(), tally, prompt,
        Some(format!("copilot-vscode:{session}:{}", at.timestamp_millis())));
    entry.project = workspace.map(str::to_owned);
    entry.source_scope = Some(SCOPE_VSCODE.into());
    VscodeRequest::Record(Measured { record: entry, partial: false, instant_millis: at.timestamp_millis() })
}

/// Keeps the format's own key when it is unique and separates a genuine collision with a
/// `#n` suffix, so two distinct requests that share a session and a millisecond are both
/// counted.
fn unique_id(base: Option<String>, used: &mut HashSet<String>) -> Option<String> {
    let base = base?;
    if used.insert(base.clone()) { return Some(base); }
    let mut suffix = 2;
    loop {
        let candidate = format!("{base}#{suffix}");
        if used.insert(candidate.clone()) { return Some(candidate); }
        suffix += 1;
    }
}

fn stripped(model_id: Option<&str>) -> Option<String> {
    let tail = model_id?.strip_prefix("copilot/")?;
    if tail.is_empty() { None } else { Some(tail.to_owned()) }
}

fn thinking_tokens(metadata: Option<&Value>) -> u64 {
    let Some(rounds) = metadata.and_then(|metadata| metadata.get("toolCallRounds")).and_then(Value::as_array) else { return 0; };
    rounds.iter().fold(0u64, |total, round| {
        let thinking = round.get("thinking").and_then(|thinking| thinking.get("tokens"));
        total.saturating_add(whole_count(thinking).unwrap_or(0))
    })
}

/// The workspace named by the sibling `workspace.json`.
fn vscode_workspace(file: &Path, cancel: &AtomicBool) -> Option<String> {
    let hash_directory = file.parent()?.parent()?;
    if check(cancel).is_err() { return None; }
    let bytes = fs::read(hash_directory.join("workspace.json")).ok()?;
    let object: Value = serde_json::from_slice(&bytes).ok()?;
    let uri = object.get("folder").and_then(text).or_else(|| object.get("workspace").and_then(text))?;
    Some(file_uri_path(&uri).unwrap_or(uri))
}

/// `file:///path` and `file://localhost/path` are paths; anything with another authority
/// stays the URI it was written as.
fn file_uri_path(uri: &str) -> Option<String> {
    let rest = uri.strip_prefix("file://")?;
    let (authority, path) = match rest.find('/') {
        Some(index) => (&rest[..index], &rest[index..]),
        None => ("", rest),
    };
    if !(authority.is_empty() || authority == "localhost") { return None; }
    Some(percent_decode(path))
}

fn percent_decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' && index + 2 < bytes.len() {
            let pair = std::str::from_utf8(&bytes[index + 1..index + 3]).ok();
            if let Some(byte) = pair.and_then(|pair| u8::from_str_radix(pair, 16).ok()) { out.push(byte); index += 3; continue; }
        }
        out.push(bytes[index]);
        index += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Replays the file's lines into the request array they describe: an early line carries
/// it, later lines append to it, and still later lines fill in a streamed response at a
/// nested path.
fn reconstruct(rows: &[Value]) -> Vec<Value> {
    let mut requests: Vec<Value> = Vec::new();
    for row in rows {
        let Some(kind) = whole_count(row.get("kind")) else { continue };
        match kind {
            0 => {
                let Some(appended) = row.get("v").and_then(|value| value.get("requests")).and_then(Value::as_array) else { continue };
                requests.extend(appended.iter().cloned());
            }
            1 => {
                // A patch addresses one path; only one into `requests` is a
                // usage-bearing write. An out-of-range index is dropped.
                let Some(path) = row.get("k").and_then(Value::as_array) else { continue };
                if path.len() < 2 { continue; }
                if path[0].as_str() != Some("requests") { continue; }
                let Some(index) = whole_count(Some(&path[1])).map(|index| index as usize) else { continue };
                if index >= requests.len() { continue; }
                let value = row.get("v").cloned().unwrap_or(Value::Null);
                if path.len() == 2 { requests[index] = value; }
                else { mutate(&mut requests[index], &path[2..], value); }
            }
            2 => {
                let Some(path) = row.get("k").and_then(Value::as_array) else { continue };
                if path.len() != 1 || path[0].as_str() != Some("requests") { continue; }
                let Some(appended) = row.get("v").and_then(Value::as_array) else { continue };
                requests.extend(appended.iter().cloned());
            }
            _ => {}
        }
    }
    requests.into_iter().filter(Value::is_object).collect()
}

/// Writes `value` at a nested path, creating the containers the path names. A scalar, a
/// missing key or an out-of-range index is not a path into anything, so the write is
/// dropped.
fn mutate(container: &mut Value, path: &[Value], value: Value) {
    let Some(head) = path.first() else { *container = value; return; };
    let rest = &path[1..];
    match container {
        Value::Array(items) => {
            let Some(index) = whole_count(Some(head)).map(|index| index as usize) else { return };
            if index >= items.len() { return; }
            if rest.is_empty() { items[index] = value; } else { mutate(&mut items[index], rest, value); }
        }
        Value::Object(map) => {
            let Some(key) = head.as_str() else { return };
            if rest.is_empty() { map.insert(key.to_owned(), value); return; }
            let child = map.entry(key.to_owned()).or_insert_with(|| {
                if whole_count(Some(&rest[0])).is_some() { Value::Array(Vec::new()) } else { Value::Object(Default::default()) }
            });
            mutate(child, rest, value);
        }
        _ => {}
    }
}

// MARK: - Shared

fn jsonl(path: &Path, cancel: &AtomicBool, notes: &mut Vec<String>) -> Result<Vec<Value>, String> {
    let file = File::open(path).map_err(|error| format!("Copilot：日志文件无法读取（{error}）。"))?;
    let mut reader = BufReader::with_capacity(64 * 1024, file);
    let mut rows: Vec<Value> = Vec::new();
    measured_readers::lines(&mut reader, cancel, notes, |row, _| {
        if row.is_object() {
            if rows.len() >= MAX_RECORDS { return Err("Copilot：记录超过读取上限。".into()); }
            rows.push(row);
        }
        Ok(())
    })?;
    Ok(rows)
}

fn walk(root: &Path, cancel: &AtomicBool, notes: &mut Vec<String>, keep: &dyn Fn(&Path) -> bool, out: &mut Vec<PathBuf>) -> Result<(), String> {
    let mut pending = vec![(root.to_path_buf(), 0usize)];
    let mut visited = 0usize;
    while let Some((path, depth)) = pending.pop() {
        check(cancel)?;
        visited += 1;
        if visited > 100_000 || out.len() >= MAX_FILES { notes.push("Copilot：目录或文件数超过读取上限，计数可能不完整。".into()); break; }
        let meta = match fs::symlink_metadata(&path) {
            Ok(meta) => meta,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => { notes.push(format!("Copilot：无法读取路径 {}：{error}", path.display())); continue; }
        };
        if meta.file_type().is_symlink() { continue; }
        if meta.is_file() { if keep(&path) { out.push(path); } continue; }
        if !meta.is_dir() { continue; }
        if depth >= 32 { notes.push(format!("Copilot：目录深度超过读取上限：{}", path.display())); continue; }
        match fs::read_dir(&path) {
            Ok(entries) => for entry in entries {
                check(cancel)?;
                match entry { Ok(entry) => pending.push((entry.path(), depth + 1)), Err(error) => notes.push(format!("Copilot：目录条目读取失败：{error}")) }
            },
            Err(error) => notes.push(format!("Copilot：无法读取目录 {}：{error}", path.display())),
        }
    }
    Ok(())
}

fn session_of(record: &SpendRecord) -> Option<String> {
    let session = record.session.trim();
    if session.is_empty() { None } else { Some(session.to_owned()) }
}

/// Every record's counted total, grouped by the session it named. Only decides whether a
/// dropped desktop session left work behind, so each addition saturates rather than
/// trapping on a hostile count.
fn totals(records: &[Measured]) -> HashMap<String, u64> {
    let mut totals: HashMap<String, u64> = HashMap::new();
    for entry in records {
        let Some(session) = session_of(&entry.record) else { continue };
        let tally = &entry.record.tally;
        let total = totals.entry(session).or_default();
        *total = total
            .saturating_add(tally.input).saturating_add(tally.output)
            .saturating_add(tally.cache_write).saturating_add(tally.cache_read)
            .saturating_add(entry.record.unclassified_tokens);
    }
    totals
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

/// `DatabaseReaderSupport.clampedCount`: a positive number truncated, anything else zero.
fn clamped_count(value: Option<&Value>) -> u64 {
    let Some(number) = value.and_then(|value| value.as_f64().filter(|number| number.is_finite())) else { return 0 };
    if number <= 0.0 { return 0; }
    let truncated = number.trunc();
    if truncated >= 9_223_372_036_854_775_808.0 { return 0; }
    truncated as u64
}

/// `DatabaseReaderSupport.flexible`: a numeric epoch (seconds or milliseconds, decided by
/// magnitude) or an ISO 8601 string. Zero and negatives are nil — an unset column is not
/// 1970.
fn flexible(raw: &str) -> Option<DateTime<Utc>> {
    let raw = raw.trim();
    if raw.is_empty() { return None; }
    if let Ok(number) = raw.parse::<f64>() { return epoch(number); }
    timestamp(&Value::String(raw.to_owned()), false)
}

fn flexible_value(value: Option<&Value>) -> Option<DateTime<Utc>> {
    let value = value?;
    if let Some(raw) = value.as_str() { return flexible(raw); }
    epoch(value.as_f64()?)
}

fn epoch(number: f64) -> Option<DateTime<Utc>> {
    if !number.is_finite() || number <= 0.0 { return None; }
    let seconds = if number < 1e12 { number } else { number / 1000. };
    if !seconds.is_finite() || seconds >= 9.2e15 { return None; }
    DateTime::from_timestamp_millis((seconds * 1000.).round() as i64)
}

/// FNV-1a 64-bit. `DefaultHasher` is not a persisted-format guarantee, so a fallback
/// identity that must fold with itself across launches uses this instead.
fn digest(bytes: &[u8]) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in bytes { hash ^= *byte as u64; hash = hash.wrapping_mul(0x0000_0100_0000_01b3); }
    hash
}

fn columns(connection: &Connection, table: &str) -> Result<HashSet<String>, String> {
    let mut statement = connection.prepare(&format!("PRAGMA table_info({table})")).map_err(|error| error.to_string())?;
    let mut cursor = statement.query([]).map_err(|error| error.to_string())?;
    let mut names = HashSet::new();
    while let Some(row) = cursor.next().map_err(|error| error.to_string())? {
        if let Ok(name) = row.get::<_, String>(1) { names.insert(name); }
    }
    Ok(names)
}

fn column_text(row: &rusqlite::Row, index: usize) -> Option<String> {
    match row.get_ref(index).ok()? {
        ValueRef::Null => None,
        ValueRef::Integer(number) => Some(number.to_string()),
        ValueRef::Real(number) => Some(number.to_string()),
        ValueRef::Text(bytes) => Some(String::from_utf8_lossy(bytes).into_owned()),
        ValueRef::Blob(_) => None,
    }
}

fn column_count(row: &rusqlite::Row, index: usize) -> u64 {
    match row.get_ref(index) {
        Ok(ValueRef::Integer(number)) if number > 0 => number as u64,
        Ok(ValueRef::Real(number)) if number.is_finite() && number > 0.0 => number.trunc() as u64,
        _ => 0,
    }
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
        // Sandboxed test binaries can be denied writes outside the workspace.
        let fallback = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target").join("test-scratch").join(format!("pulsewin-{suffix}"));
        fs::create_dir_all(&fallback).unwrap_or_else(|error| panic!("create_dir_all {}: {error:?}", fallback.display()));
        fallback
    }

    fn home() -> PathBuf { temp_dir("copilot-home") }

    fn write(path: &Path, body: &str) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, body).unwrap();
    }

    fn encoded(rows: &[Value]) -> String { rows.iter().map(Value::to_string).collect::<Vec<_>>().join("\n") }

    fn otel(home: &Path, rows: &[Value]) { write(&home.join(".copilot/otel/monitoring.jsonl"), &encoded(rows)); }

    fn records(home: &Path) -> Vec<SpendRecord> {
        scan(&roots(home, "copilot", &|_: &str| None), &AtomicBool::new(false)).unwrap().0
    }

    fn find<'a>(records: &'a [SpendRecord], id: &str) -> Option<&'a SpendRecord> {
        records.iter().find(|record| record.deduplication_id.as_deref() == Some(id))
    }

    fn at(iso: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(iso).unwrap().with_timezone(&Utc)
    }

    /// One desktop database with the `sessions` schema and whatever rows the test hands
    /// it, plus a sidecar event log per session that names one.
    fn desktop(home: &Path, rows: &[(&str, Option<&str>, Option<&str>, i64, i64, i64, i64, &str)], events: &[(&str, Vec<Value>)]) {
        let database = home.join(".copilot/data.db");
        fs::create_dir_all(database.parent().unwrap()).unwrap();
        let connection = Connection::open(&database).unwrap();
        connection.execute_batch("CREATE TABLE sessions (id TEXT, title TEXT, model TEXT, total_input_tokens INTEGER, total_output_tokens INTEGER, total_cached_tokens INTEGER, total_reasoning_tokens INTEGER, total_nano_aiu INTEGER, created_at TEXT)").unwrap();
        for (id, title, model, input, output, cached, reasoning, created) in rows {
            let quote = |value: Option<&str>| value.map(|value| format!("'{value}'")).unwrap_or_else(|| "NULL".into());
            connection.execute(&format!("INSERT INTO sessions VALUES ('{id}', {}, {}, {input}, {output}, {cached}, {reasoning}, 0, '{created}')", quote(*title), quote(*model)), []).unwrap();
        }
        drop(connection);
        for (id, lines) in events {
            write(&home.join(format!(".copilot/session-state/{id}/events.jsonl")), &encoded(lines));
        }
    }

    #[test]
    fn roots_name_all_three_stores_and_the_windows_profile() {
        let home = PathBuf::from("/Users/test");
        let all = roots(&home, "copilot", &|_: &str| None);
        assert_eq!(all, vec![
            PathBuf::from("/Users/test/.copilot/otel"),
            PathBuf::from("/Users/test/.copilot/data.db"),
            PathBuf::from("/Users/test/.copilot/session-state"),
            PathBuf::from("/Users/test/Library/Application Support/Code/User/workspaceStorage"),
            PathBuf::from("/Users/test/.config/Code/User/workspaceStorage"),
            PathBuf::from("/Users/test/AppData/Roaming/Code/User/workspaceStorage"),
        ]);
        let windows = roots(&home, "copilot", &|key: &str| (key == "APPDATA").then(|| "/win/roaming".to_string()));
        assert_eq!(windows.last(), Some(&PathBuf::from("/win/roaming/Code/User/workspaceStorage")));
        assert!(roots(&home, "trae", &|_: &str| None).is_empty());
        assert!(roots(&home, "copilot", &|_: &str| None).iter().all(|root| root.starts_with("/Users/test")));
    }

    #[test]
    fn candidate_accepts_the_two_jsonl_lanes_only() {
        assert!(candidate(Path::new("/h/.copilot/otel/a.jsonl"), "copilot"));
        assert!(candidate(Path::new("/h/Library/Application Support/Code/User/workspaceStorage/hash1/chatSessions/s.jsonl"), "copilot"));
        // The database is read from its named root, never by the per-file walk.
        assert!(!candidate(Path::new("/h/.copilot/data.db"), "copilot"));
        assert!(!candidate(Path::new("/h/.copilot/otel/a.json"), "copilot"));
        assert!(!candidate(Path::new("/h/.copilot/otel/a.jsonl"), "trae"));
    }

    #[test]
    fn otel_counts_a_chat_span_once_drops_lower_lanes_and_keeps_a_bare_total() {
        let home = home();
        let chat = json!({"type":"span","name":"chat gpt-4o","traceId":"T1","spanId":"S1","startTime":"2026-09-01T10:00:00Z",
            "attributes":{"gen_ai.operation.name":"chat","gen_ai.response.model":"gpt-4o","gen_ai.response.id":"R1",
                "gen_ai.conversation.id":"C1","gen_ai.agent.id":"github.copilot.default",
                "gen_ai.usage.input_tokens":1000,"gen_ai.usage.output_tokens":200,
                "gen_ai.usage.cache_read.input_tokens":400,"gen_ai.usage.cache_write.input_tokens":50}});
        let inference = json!({"time":"2026-09-01T10:00:00Z",
            "attributes":{"event.name":"gen_ai.client.inference.operation.details","gen_ai.response.id":"R1",
                "gen_ai.conversation.id":"C1","gen_ai.usage.input_tokens":1000,"gen_ai.usage.output_tokens":200}});
        let summary = json!({"type":"span","name":"invoke_agent gpt-4o","traceId":"T2","spanId":"S2","startTime":"2026-09-01T11:00:00Z",
            "attributes":{"gen_ai.operation.name":"invoke_agent","gen_ai.response.model":"gpt-4o",
                "gen_ai.conversation.id":"C2","gen_ai.usage.input_tokens":500,"gen_ai.usage.output_tokens":100}});
        let chat_t2 = json!({"type":"span","name":"chat gpt-4o","traceId":"T2","spanId":"S3","startTime":"2026-09-01T11:00:00Z",
            "attributes":{"gen_ai.operation.name":"chat","gen_ai.response.model":"gpt-4o",
                "gen_ai.conversation.id":"C2","gen_ai.usage.input_tokens":300,"gen_ai.usage.output_tokens":50}});
        let bare = json!({"type":"span","name":"chat x","traceId":"T3","spanId":"S4","startTime":"2026-09-01T12:00:00Z",
            "attributes":{"gen_ai.operation.name":"chat","gen_ai.response.model":"gpt-4o",
                "gen_ai.conversation.id":"C3","gen_ai.usage.total_tokens":777}});
        // No usable time and a negative count: skipped without trapping.
        let invalid = json!({"type":"span","name":"chat gpt-4o","traceId":"T4","spanId":"S5","startTime":"not-a-date",
            "attributes":{"gen_ai.operation.name":"chat","gen_ai.response.model":"gpt-4o",
                "gen_ai.usage.input_tokens":-5,"gen_ai.usage.output_tokens":10}});
        let tokenless = json!({"type":"span","name":"chat gpt-4o","traceId":"T7","spanId":"S7","startTime":"2026-09-01T14:00:00Z",
            "attributes":{"gen_ai.operation.name":"chat","gen_ai.response.model":"gpt-4o","gen_ai.conversation.id":"C7"}});
        otel(&home, &[chat.clone(), chat, inference, summary, chat_t2, bare, invalid, tokenless]);
        let records = records(&home);
        assert_eq!(records.len(), 3, "{records:?}");
        let span = find(&records, "copilot-otel:T1:S1").unwrap();
        assert_eq!(span.model, "gpt-4o");
        assert_eq!(span.source_timestamp, Some(at("2026-09-01T10:00:00Z").timestamp()));
        assert_eq!(span.session, "C1");
        assert!(!span.aggregate_timing);
        // Input counted the cache read; it is removed once.
        assert_eq!(span.tally, Tally { input: 600, cache_write: 50, cache_read: 400, output: 200, ..Default::default() });
        assert_eq!(find(&records, "copilot-otel:T2:S3").unwrap().tally.total(), 350);
        let total = find(&records, "copilot-otel:T3:S4").unwrap();
        assert_eq!(total.tally.total(), 0);
        assert_eq!(total.unclassified_tokens, 777);
        // The agent summary and the token-less span produced nothing.
        assert!(find(&records, "copilot-otel:T2:S2").is_none());
        assert!(!records.iter().any(|record| record.session == "C7"));
        let _ = fs::remove_dir_all(&home);
    }

    #[test]
    fn otel_never_folds_two_unnamed_records_by_a_shared_instant_but_folds_a_replay() {
        let home = home();
        let unnamed = |model: &str| json!({"type":"span","name":format!("chat {model}"),"startTime":"2026-09-01T10:00:00Z",
            "attributes":{"gen_ai.operation.name":"chat","gen_ai.response.model":model,
                "gen_ai.usage.input_tokens":100,"gen_ai.usage.output_tokens":10}});
        write(&home.join(".copilot/otel/a.jsonl"), &encoded(&[unnamed("gpt-4o")]));
        write(&home.join(".copilot/otel/b.jsonl"), &encoded(&[unnamed("gpt-4o")]));
        let (first, notes) = scan(&roots(&home, "copilot", &|_: &str| None), &AtomicBool::new(false)).unwrap();
        let anonymous: Vec<_> = first.iter().filter(|record| record.deduplication_id.as_deref().is_some_and(|id| id.contains("span-unnamed"))).collect();
        // Two different requests: both counted, both flagged.
        assert_eq!(anonymous.len(), 2, "{first:?}");
        assert_eq!(anonymous.iter().map(|record| record.tally.input).sum::<u64>(), 200);
        assert!(anonymous.iter().all(|record| record.source_scope.as_deref() == Some(SCOPE_OTEL_PARTIAL)));
        assert!(notes.iter().any(|note| note.contains("既无 trace id 也无 response id")), "{notes:?}");
        // An explicit trace + span written into two files is one event.
        let replay = json!({"type":"span","name":"chat gpt-4o","traceId":"TR","spanId":"SP","startTime":"2026-09-01T11:00:00Z",
            "attributes":{"gen_ai.operation.name":"chat","gen_ai.response.model":"gpt-4o",
                "gen_ai.usage.input_tokens":40,"gen_ai.usage.output_tokens":4}});
        write(&home.join(".copilot/otel/c.jsonl"), &encoded(&[replay.clone()]));
        write(&home.join(".copilot/otel/d.jsonl"), &encoded(&[replay]));
        let (second, _) = scan(&roots(&home, "copilot", &|_: &str| None), &AtomicBool::new(false)).unwrap();
        assert_eq!(second.iter().filter(|record| record.deduplication_id.as_deref() == Some("copilot-otel:TR:SP")).count(), 1);
        let _ = fs::remove_dir_all(&home);
    }

    #[test]
    fn desktop_snapshots_are_differenced_and_a_missing_head_is_a_baseline() {
        let home = home();
        let desk1 = vec![
            json!({"type":"session.start","timestamp":"2026-09-02T08:00:00Z","data":{"context":{"cwd":"/Users/me/Code/Pulse"}}}),
            json!({"type":"session.model_change","timestamp":"2026-09-02T08:05:00Z","data":{"newModel":"gpt-4o"}}),
            json!({"type":"session.shutdown","id":"e1","timestamp":"2026-09-02T09:00:00Z","data":{"currentModel":"gpt-4o","modelMetrics":{
                "":{"usage":{"inputTokens":400,"outputTokens":80,"cacheReadTokens":120,"cacheWriteTokens":10,"reasoningTokens":20}}}}}),
            json!({"type":"session.shutdown","id":"e2","timestamp":"2026-09-03T10:00:00Z","data":{"currentModel":"gpt-4o","modelMetrics":{
                "gpt-4o":{"usage":{"inputTokens":1000,"outputTokens":200,"cacheReadTokens":300,"cacheWriteTokens":25,"reasoningTokens":50}}}}}),
        ];
        // No session.start: the first snapshot is an unknown baseline.
        let desk2 = vec![
            json!({"type":"session.model_change","timestamp":"2026-09-03T08:05:00Z","data":{"newModel":"gpt-4o"}}),
            json!({"type":"session.shutdown","id":"f1","timestamp":"2026-09-03T09:00:00Z","data":{"currentModel":"gpt-4o","modelMetrics":{
                "gpt-4o":{"usage":{"inputTokens":500,"outputTokens":60,"cacheReadTokens":0,"cacheWriteTokens":0,"reasoningTokens":0}}}}}),
            json!({"type":"session.shutdown","id":"f2","timestamp":"2026-09-03T10:00:00Z","data":{"currentModel":"gpt-4o","modelMetrics":{
                "gpt-4o":{"usage":{"inputTokens":800,"outputTokens":100,"cacheReadTokens":0,"cacheWriteTokens":0,"reasoningTokens":0}}}}}),
        ];
        desktop(&home, &[
            ("desk1", Some("Ring"), Some("gpt-4o"), 1000, 200, 300, 50, "2026-09-02T08:00:00Z"),
            ("desk2", None, Some("gpt-4o"), 800, 100, 0, 0, "2026-09-03T08:00:00Z"),
            // Recorded by the CLI: a row with no sidecar at all.
            ("desk3", None, Some("gpt-4o"), 250, 25, 0, 0, "2026-09-04T08:00:00Z"),
            ("desk4", None, None, 0, 0, 0, 0, "2026-09-05T08:00:00Z"),
        ], &[("desk1", desk1), ("desk2", desk2)]);
        let records = records(&home);
        assert_eq!(records.len(), 5, "{records:?}");
        let run1 = find(&records, "copilot-desktop:desk1:shutdown:e1:gpt-4o").unwrap();
        assert_eq!(run1.source_timestamp, Some(at("2026-09-02T09:00:00Z").timestamp()));
        // 400 sent, 120 read and 10 written, 270 fresh; output whole, reasoning not added.
        assert_eq!(run1.tally, Tally { input: 270, cache_write: 10, cache_read: 120, output: 80, ..Default::default() });
        assert_eq!(run1.unclassified_tokens, 0);
        assert!(run1.aggregate_timing);
        assert_eq!(run1.session, "desk1");
        assert_eq!(run1.project.as_deref(), Some("/Users/me/Code/Pulse"));
        assert_eq!(run1.source_scope.as_deref(), Some(SCOPE_DESKTOP_PARTIAL));
        // The second snapshot is the first minus the baseline, on its own day.
        let run2 = find(&records, "copilot-desktop:desk1:shutdown:e2:gpt-4o").unwrap();
        assert_eq!(run2.tally, Tally { input: 405, cache_write: 15, cache_read: 180, output: 120, ..Default::default() });
        // The headless session's first snapshot is not an increment.
        assert!(find(&records, "copilot-desktop:desk2:shutdown:f1:gpt-4o").is_none());
        let baseline = find(&records, "copilot-desktop:desk2:row").unwrap();
        assert_eq!(baseline.tally, Tally { input: 500, output: 60, ..Default::default() });
        let after = find(&records, "copilot-desktop:desk2:shutdown:f2:gpt-4o").unwrap();
        assert_eq!(after.tally, Tally { input: 300, output: 40, ..Default::default() });
        // A session with no sidecar is its row's lifetime total, once.
        let cli = find(&records, "copilot-desktop:desk3:row").unwrap();
        assert_eq!(cli.tally, Tally { input: 250, output: 25, ..Default::default() });
        assert!(!records.iter().any(|record| record.session == "desk4"));
        assert_eq!(records.iter().filter(|record| record.deduplication_id.as_deref().is_some_and(|id| id.contains(":desk1:"))).map(|record| record.tally.cache_write).sum::<u64>(), 25);
        let _ = fs::remove_dir_all(&home);
    }

    #[test]
    fn desktop_reasoning_is_omitted_and_flagged_not_added() {
        let home = home();
        // An older row with no reasoning column; the sidecar still reports it. The flag
        // must read the sidecar's own snapshot, not the zero the row budgeted it to.
        desktop(&home, &[("s", None, Some("gpt-4o"), 300, 60, 0, 0, "2026-09-03T08:00:00Z")], &[("s", vec![
            json!({"type":"session.start","timestamp":"2026-09-03T08:00:00Z","data":{"context":{"cwd":"/work"}}}),
            json!({"type":"session.shutdown","id":"e1","timestamp":"2026-09-03T09:00:00Z","data":{"currentModel":"gpt-4o","modelMetrics":{
                "gpt-4o":{"usage":{"inputTokens":300,"outputTokens":60,"cacheReadTokens":0,"cacheWriteTokens":0,"reasoningTokens":25}}}}}),
        ])]);
        let (records, notes) = scan(&roots(&home, "copilot", &|_: &str| None), &AtomicBool::new(false)).unwrap();
        let run = find(&records, "copilot-desktop:s:shutdown:e1:gpt-4o").unwrap();
        // 60, not 85: the 25 reasoning tokens are not added to output.
        assert_eq!(run.tally, Tally { input: 300, output: 60, ..Default::default() });
        assert_eq!(run.unclassified_tokens, 0);
        assert_eq!(run.source_scope.as_deref(), Some(SCOPE_DESKTOP_PARTIAL));
        assert!(notes.iter().any(|note| note.contains("推理 token")), "{notes:?}");
        let _ = fs::remove_dir_all(&home);
    }

    #[test]
    fn vscode_replays_the_append_log_folds_thinking_and_skips_untimed_requests() {
        let home = home();
        let hash_directory = home.join("Library/Application Support/Code/User/workspaceStorage/hash1");
        write(&hash_directory.join("workspace.json"), r#"{"folder":"file:///Users/me/Code/Pulse"}"#);
        let first = json!({"promptTokens":100,"completionTokens":20,"timestamp":1_788_267_600_000i64,
            "modelId":"copilot/gpt-4o","result":{"metadata":{"toolCallRounds":[{"thinking":{"tokens":5}}]}}});
        let second = json!({"promptTokens":200,"completionTokens":40,"timestamp":1_788_267_700_000i64,"modelId":"copilot/gpt-4o"});
        let untimed = json!({"promptTokens":30,"completionTokens":4,"modelId":"copilot/gpt-4o"});
        let untimed_two = json!({"promptTokens":70,"completionTokens":8,"modelId":"copilot/gpt-4o"});
        let foreign = json!({"promptTokens":999,"completionTokens":999,"timestamp":1_788_267_800_000i64,"modelId":"other/model"});
        let tokenless = json!({"timestamp":1_788_267_900_000i64,"modelId":"copilot/gpt-4o"});
        write(&hash_directory.join("chatSessions/vscode-session-1.jsonl"), &encoded(&[
            json!({"kind":0,"v":{"requests":[first]}}),
            json!({"kind":1,"k":["requests",0,"result","metadata","resolvedModel"],"v":"gpt-4o"}),
            json!({"kind":1,"k":["requests",0,"response"],"v":{"text":"streamed"}}),
            json!({"kind":2,"k":["requests"],"v":[second, untimed, untimed_two, foreign, tokenless]}),
            json!({"kind":1,"k":["requests",5,"x"],"v":1}),
        ]));
        let (records, notes) = scan(&roots(&home, "copilot", &|_: &str| None), &AtomicBool::new(false)).unwrap();
        assert_eq!(records.len(), 2, "{records:?}");
        let first_record = find(&records, "copilot-vscode:vscode-session-1:1788267600000").unwrap();
        assert_eq!(first_record.model, "gpt-4o");
        // The prompt figure holds the cache with no split beside it.
        assert_eq!(first_record.tally, Tally { output: 25, ..Default::default() });
        assert_eq!(first_record.unclassified_tokens, 100);
        assert_eq!(first_record.session, "vscode-session-1");
        assert_eq!(first_record.project.as_deref(), Some("/Users/me/Code/Pulse"));
        assert!(!first_record.aggregate_timing);
        assert_eq!(first_record.source_timestamp, Some(1_788_267_600));
        assert_eq!(find(&records, "copilot-vscode:vscode-session-1:1788267700000").unwrap().unclassified_tokens, 200);
        // No request was dated at the epoch, and neither untimed request counted.
        assert!(!records.iter().any(|record| record.source_timestamp == Some(0)));
        assert!(!records.iter().any(|record| record.unclassified_tokens == 30 || record.unclassified_tokens == 70));
        assert!(!records.iter().any(|record| record.unclassified_tokens == 999));
        assert!(notes.iter().any(|note| note.contains("没有时间戳")), "{notes:?}");
        assert!(notes.iter().any(|note| note.contains("promptTokens 含缓存")), "{notes:?}");
        let _ = fs::remove_dir_all(&home);
    }

    #[test]
    fn two_vscode_requests_sharing_a_session_and_instant_are_both_kept() {
        let home = home();
        let hash_directory = home.join("Library/Application Support/Code/User/workspaceStorage/hash1");
        let request = json!({"promptTokens":100,"completionTokens":10,"timestamp":1_788_267_600_000i64,"modelId":"copilot/gpt-4o"});
        write(&hash_directory.join("chatSessions/same.jsonl"), &encoded(&[json!({"kind":0,"v":{"requests":[request.clone(), request]}})]));
        let records = records(&home);
        assert_eq!(records.len(), 2, "{records:?}");
        assert_eq!(records.iter().map(|record| record.unclassified_tokens).sum::<u64>(), 200);
        assert_eq!(records.iter().map(|record| record.tally.output).sum::<u64>(), 20);
        // Distinct identities even though the format's own key collides.
        let ids: HashSet<_> = records.iter().filter_map(|record| record.deduplication_id.clone()).collect();
        assert_eq!(ids.len(), 2);
        let _ = fs::remove_dir_all(&home);
    }

    #[test]
    fn a_session_otel_named_is_dropped_from_desktop_and_a_matching_vscode_instant_is_dropped() {
        let home = home();
        let shared = json!({"type":"span","name":"chat gpt-4o","traceId":"T9","spanId":"S9","startTime":"2026-09-01T10:00:00Z",
            "attributes":{"gen_ai.operation.name":"chat","copilot_chat.session_id":"desk-shared","gen_ai.response.model":"gpt-4o",
                "gen_ai.usage.input_tokens":100,"gen_ai.usage.output_tokens":10}});
        let editor = json!({"type":"span","name":"chat gpt-4o","traceId":"T6","spanId":"S6","startTime":"2026-09-01T13:00:00Z",
            "attributes":{"gen_ai.operation.name":"chat","copilot_chat.session_id":"vscode-session-1","gen_ai.response.model":"gpt-4o",
                "gen_ai.usage.input_tokens":111,"gen_ai.usage.output_tokens":22}});
        otel(&home, &[shared, editor]);
        desktop(&home, &[
            ("desk-shared", None, Some("gpt-4o"), 500, 50, 0, 0, "2026-09-01T09:00:00Z"),
            ("desk-alone", None, Some("gpt-4o"), 200, 20, 0, 0, "2026-09-02T09:00:00Z"),
        ], &[]);
        let hash_directory = home.join("Library/Application Support/Code/User/workspaceStorage/hash1");
        write(&hash_directory.join("chatSessions/vscode-session-1.jsonl"), &encoded(&[json!({"kind":0,"v":{"requests":[
            json!({"promptTokens":111,"completionTokens":22,"timestamp":1_788_267_600_000i64,"modelId":"copilot/gpt-4o"}),
            json!({"promptTokens":200,"completionTokens":40,"timestamp":1_788_267_700_000i64,"modelId":"copilot/gpt-4o"}),
        ]}})]));
        let records = records(&home);
        // The desktop lane's copy of the session OTEL named is gone; the other is kept.
        assert!(!records.iter().any(|record| record.deduplication_id.as_deref().is_some_and(|id| id.starts_with("copilot-desktop:desk-shared"))));
        let alone = find(&records, "copilot-desktop:desk-alone:row").unwrap();
        assert_eq!(alone.source_scope.as_deref(), Some(SCOPE_DESKTOP));
        // The VS Code request at the OTEL instant is gone; the later one stays.
        assert!(find(&records, "copilot-vscode:vscode-session-1:1788267600000").is_none());
        assert!(find(&records, "copilot-vscode:vscode-session-1:1788267700000").is_some());
        // OTEL keeps its own record; the desktop lifetime (550) is larger than OTEL's
        // 110, so the uncovered part is stated rather than zeroed.
        assert_eq!(find(&records, "copilot-otel:T9:S9").unwrap().source_scope.as_deref(), Some(SCOPE_OTEL_PARTIAL));
        assert_eq!(find(&records, "copilot-otel:T6:S6").unwrap().source_scope.as_deref(), Some(SCOPE_OTEL));
        let _ = fs::remove_dir_all(&home);
    }

    #[test]
    fn no_prompt_text_or_secret_reaches_a_record() {
        let home = home();
        // Every lane can carry message bodies beside its counters. The boundary keeps
        // the four buckets and the declared names, and nothing else.
        otel(&home, &[json!({"type":"span","name":"chat gpt-4o","traceId":"T","spanId":"S","startTime":"2026-09-01T10:00:00Z",
            "attributes":{"gen_ai.operation.name":"chat","gen_ai.response.model":"gpt-4o",
                "gen_ai.usage.input_tokens":10,"gen_ai.usage.output_tokens":2},
            "body":"SECRET OTEL BODY","prompt":"SECRET OTEL PROMPT","api_key":"SECRET OTEL KEY"})]);
        write(&home.join("Library/Application Support/Code/User/workspaceStorage/h/chatSessions/s.jsonl"),
            &encoded(&[json!({"kind":0,"v":{"requests":[{"promptTokens":5,"completionTokens":1,
                "timestamp":1_788_267_600_000i64,"modelId":"copilot/gpt-4o",
                "message":"SECRET CHAT MESSAGE","response":{"text":"SECRET VSCODE RESPONSE"}}]}})]));
        let records = records(&home);
        assert!(!records.is_empty(), "{records:?}");
        let serialized = serde_json::to_string(&records).unwrap();
        for secret in ["SECRET OTEL BODY", "SECRET OTEL PROMPT", "SECRET OTEL KEY", "SECRET CHAT MESSAGE", "SECRET VSCODE RESPONSE"] {
            assert!(!serialized.contains(secret), "{secret} leaked into {serialized}");
        }
        let _ = fs::remove_dir_all(&home);
    }

    #[test]
    fn cancellation_and_missing_stores_are_safe() {
        let home = home();
        otel(&home, &[json!({"type":"span","name":"chat gpt-4o","traceId":"T","spanId":"S","startTime":"2026-09-01T10:00:00Z",
            "attributes":{"gen_ai.operation.name":"chat","gen_ai.response.model":"gpt-4o","gen_ai.usage.input_tokens":10}})].to_vec());
        assert_eq!(scan(&roots(&home, "copilot", &|_: &str| None), &AtomicBool::new(true)).unwrap_err(), "cancelled");
        // No store at all is an empty read, not an error.
        let empty = temp_dir("copilot-empty");
        assert!(scan(&roots(&empty, "copilot", &|_: &str| None), &AtomicBool::new(false)).unwrap().0.is_empty());
        let serialized = serde_json::to_string(&records(&home)).unwrap();
        assert!(!serialized.contains("streamed"));
        let _ = fs::remove_dir_all(&home);
        let _ = fs::remove_dir_all(&empty);
    }
}
