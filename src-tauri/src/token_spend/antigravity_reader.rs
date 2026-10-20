//! Pulse 1.9 AntigravityCLIReader (Apache-2.0): Antigravity CLI and Antigravity
//! IDE conversations, one SQLite database per conversation.
//!
//! The token data sits in protobuf blobs and no `.proto` ships with the client,
//! so this reader carries its own minimal wire decoder, written to the wire
//! rules and deliberately hostile-safe: a truncated buffer, an overlong varint,
//! a length that runs past the end, an unknown wire type or field number zero
//! ends the whole message instead of reading out of bounds.
//!
//! **Only fields whose meaning is established are counted.** Usage `#2` (fresh
//! input), `#5` (cache read), `#9` (output) and `#10` (thinking) are the counters
//! with an observed meaning; `#9 + #10` is the contract's total output, so
//! reasoning is folded into output exactly once. `#1` is a near-constant whose
//! "fixed system prompt" reading is reverse-engineered, not established, so it is
//! read by nothing here — not as input, not as unclassified work. A turn whose
//! real counters are all zero is dropped; nothing is estimated from text or cost.
//!
//! **Timestamps need evidence.** Only the explicit per-generation time and the
//! `steps` table's time are used; unknown bytes are never decoded into a time. A
//! turn without one falls back to the conversation's created-at and is marked
//! aggregate, because a session anchor is not the moment of the turn. A file
//! modification date is never used.
//!
//! **CLI and IDE are two clients, never one figure.** They write the same store
//! shape into sibling folders, so one reader serves both, but each folder belongs
//! to a different product and the two roots never overlap.

use super::measured_readers::record;
use super::*;
use rusqlite::{types::ValueRef, Connection, OpenFlags, Row};
use std::time::Instant;

/// `$GEMINI_CLI_HOME` else `~/.gemini`; then one folder per client. The IDE has
/// two spellings on disk — `antigravity` is what the current build writes,
/// `antigravity-ide` the other one seen beside it — and both are that client's.
pub(super) fn roots(home: &Path, id: &str, env: &impl Fn(&str) -> Option<String>) -> Vec<PathBuf> {
    let root = env("GEMINI_CLI_HOME")
        .filter(|value| !value.trim().is_empty())
        .map(|value| expand(home, value.trim()))
        .unwrap_or_else(|| home.join(".gemini"));
    match id {
        "antigravity-ide" => ["antigravity", "antigravity-ide"]
            .into_iter()
            .map(|folder| root.join(folder).join("conversations"))
            .collect(),
        "antigravity-cli" => vec![root.join("antigravity-cli").join("conversations")],
        _ => Vec::new(),
    }
}

pub(super) fn candidate(path: &Path, id: &str) -> bool {
    matches!(id, "antigravity-cli" | "antigravity-ide")
        && path.extension().and_then(|extension| extension.to_str()) == Some("db")
}

pub(super) fn read(
    path: &Path,
    id: &str,
    cancel: &AtomicBool,
) -> Result<(Vec<SpendRecord>, Vec<String>), String> {
    check(cancel)?;
    if !matches!(id, "antigravity-cli" | "antigravity-ide") {
        return Ok((Vec::new(), Vec::new()));
    }
    let label = label(id);
    let session = path
        .file_stem()
        .map(|stem| stem.to_string_lossy().into_owned())
        .unwrap_or_default();
    let db = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .map_err(|_| format!("{label}：会话数据库无法读取。"))?;
    let _ = db.busy_timeout(Duration::from_millis(150));
    let began = Instant::now();
    db.progress_handler(1000, Some(move || began.elapsed() > Duration::from_secs(15)));

    let anchor = trajectory(&db, cancel)?;
    let steps = step_timestamps(&db, cancel)?;
    let generations = generations(&db, cancel)?;

    // The models a label was seen with, and the file's single named model. Used
    // only to recover an identity the routing label threw away.
    let mut labels: HashMap<String, HashSet<String>> = HashMap::new();
    let mut models: HashSet<String> = HashSet::new();
    for generation in &generations {
        let Some(model) = generation.model.as_deref() else { continue };
        if routing_label(model) {
            continue;
        }
        models.insert(model.to_owned());
        if let Some(label) = generation.label.as_deref() {
            labels.entry(label.to_owned()).or_default().insert(model.to_owned());
        }
    }
    let sole = if models.len() == 1 { models.iter().next().cloned() } else { None };

    let mut seen: HashSet<String> = HashSet::new();
    let mut records = Vec::new();
    for generation in &generations {
        check(cancel)?;
        let Some(usage) = generation.usage.as_ref() else { continue };
        if usage.counted() == 0 {
            continue;
        }
        // A replayed response id is the same turn; the first one wins.
        if let Some(response) = usage.response_id.as_ref() {
            if !seen.insert(response.clone()) {
                continue;
            }
        }
        // An exact event time, or the session anchor as an aggregate.
        let event = generation
            .explicit_timestamp
            .or_else(|| usage.response_id.as_ref().and_then(|response| steps.by_response.get(response).copied()))
            .or_else(|| steps.by_index.get(&generation.index).copied());
        let Some(at) = event.or(anchor.created_at) else { continue };
        let model = resolved_model(generation, &labels, sole.as_deref());
        let mut spend = record(
            id,
            model,
            at,
            session.clone(),
            Tally {
                input: usage.fresh,
                output: usage.output.saturating_add(usage.reasoning),
                cache_read: usage.cache_read,
                ..Default::default()
            },
            0,
            Some(match usage.response_id.as_ref() {
                Some(response) => format!("antigravity:{session}:{response}"),
                None => format!("antigravity:{session}:{}", generation.index),
            }),
        );
        spend.project = anchor.workspace.clone();
        spend.aggregate_timing = event.is_none();
        records.push(spend);
        if records.len() >= MAX_RECORDS {
            return Err(format!("{label}：会话记录超过读取上限。"));
        }
    }
    let mut notes = Vec::new();
    if !records.is_empty() {
        // The Swift reader marks every record `isPartial`: excluding `#1` and
        // having no whole-usage total to reconcile the counted fields against
        // makes each record a proven subset. The port has no per-record flag, so
        // the read says it once instead of claiming a completeness it lacks.
        notes.push(format!(
            "{label}：只计入含义已确认的字段（#2、#5、#9、#10，推理已并入输出一次），没有可对账的整轮总量；已计数的只是可证明的子集，可能不完整。"
        ));
    }
    notes.sort();
    notes.dedup();
    check(cancel)?;
    Ok((records, notes))
}

fn label(id: &str) -> &'static str {
    if id == "antigravity-ide" { "Antigravity IDE" } else { "Antigravity CLI" }
}

/// `~`, `~/` and `~\` in an environment override, the way the crate's other
/// readers resolve one.
fn expand(home: &Path, value: &str) -> PathBuf {
    if value == "~" {
        return home.to_path_buf();
    }
    if let Some(rest) = value.strip_prefix("~/").or_else(|| value.strip_prefix("~\\")) {
        return home.join(rest);
    }
    PathBuf::from(value)
}

// MARK: - Tables

struct Anchor {
    created_at: Option<DateTime<Utc>>,
    workspace: Option<String>,
}

#[derive(Default)]
struct Steps {
    by_response: HashMap<String, DateTime<Utc>>,
    by_index: HashMap<i64, DateTime<Utc>>,
}

struct Usage {
    /// `#2`, newly-processed (non-cached) input. `#1` is deliberately absent: its
    /// meaning is not established, so it is no count at all.
    fresh: u64,
    cache_read: u64,
    output: u64,
    reasoning: u64,
    response_id: Option<String>,
}

impl Usage {
    fn counted(&self) -> u64 {
        self.fresh
            .saturating_add(self.cache_read)
            .saturating_add(self.output)
            .saturating_add(self.reasoning)
    }
}

struct Generation {
    index: i64,
    model: Option<String>,
    label: Option<String>,
    usage: Option<Usage>,
    explicit_timestamp: Option<DateTime<Utc>>,
}

fn trajectory(db: &Connection, cancel: &AtomicBool) -> Result<Anchor, String> {
    let mut anchor = Anchor { created_at: None, workspace: None };
    each(db, "SELECT data FROM trajectory_metadata_blob LIMIT 1", cancel, |row| {
        let Some(bytes) = blob(row, 0) else { return Ok(()) };
        let Some(message) = decode(&bytes) else { return Ok(()) };
        anchor.created_at = message.nested(2).and_then(|message| timestamp(&message));
        if let Some(uri) = message.nested(1).and_then(|folder| folder.string(1)) {
            anchor.workspace = path_from_uri(&uri);
        }
        Ok(())
    })?;
    Ok(anchor)
}

fn step_timestamps(db: &Connection, cancel: &AtomicBool) -> Result<Steps, String> {
    let mut steps = Steps::default();
    each(db, "SELECT metadata FROM steps WHERE step_type = 15", cancel, |row| {
        let Some(bytes) = blob(row, 0) else { return Ok(()) };
        let Some(message) = decode(&bytes) else { return Ok(()) };
        let Some(at) = message.nested(1).and_then(|message| timestamp(&message)) else { return Ok(()) };
        if let Some(response) = message.nested(9).and_then(|message| message.string(11)) {
            steps.by_response.insert(response, at);
        }
        if let Some(index) = message.nested(20).and_then(|message| message.varint(3)) {
            if let Ok(index) = i64::try_from(index) {
                steps.by_index.insert(index, at);
            }
        }
        Ok(())
    })?;
    Ok(steps)
}

fn generations(db: &Connection, cancel: &AtomicBool) -> Result<Vec<Generation>, String> {
    let mut generations = Vec::new();
    each(db, "SELECT idx, data FROM gen_metadata ORDER BY idx", cancel, |row| {
        let Some(index) = column_int(row, 0) else { return Ok(()) };
        let Some(bytes) = blob(row, 1) else { return Ok(()) };
        let Some(message) = decode(&bytes) else { return Ok(()) };
        let Some(chat) = message.nested(1) else { return Ok(()) };
        let usage = chat.nested(4).map(|usage| Usage {
            fresh: usage.varint(2).unwrap_or(0),
            cache_read: usage.varint(5).unwrap_or(0),
            output: usage.varint(9).unwrap_or(0),
            reasoning: usage.varint(10).unwrap_or(0),
            response_id: usage.string(11),
        });
        generations.push(Generation {
            index,
            model: chat.string(19),
            label: chat.string(21),
            usage,
            explicit_timestamp: chat.nested(9).and_then(|wall| wall.nested(4)).and_then(|wall| timestamp(&wall)),
        });
        Ok(())
    })?;
    Ok(generations)
}

// MARK: - Fields

/// A `{#1 seconds, #2 nanos}` timestamp. A non-positive or absurd seconds value
/// is not a time.
fn timestamp(message: &WireMessage) -> Option<DateTime<Utc>> {
    let seconds = message.varint(1)?;
    if seconds == 0 || seconds >= 1_000_000_000_000 {
        return None;
    }
    let nanos = message.varint(2).unwrap_or(0);
    let seconds = i64::try_from(seconds).ok()?;
    let extra = i64::try_from(nanos / 1_000_000_000).ok()?;
    let nanos = u32::try_from(nanos % 1_000_000_000).ok()?;
    DateTime::from_timestamp(seconds.checked_add(extra)?, nanos)
}

/// A `file://` URI as a path, or the string unchanged when it is not one.
fn path_from_uri(uri: &str) -> Option<String> {
    let trimmed = uri.trim();
    if trimmed.is_empty() {
        return None;
    }
    let Some(rest) = trimmed.strip_prefix("file://") else { return Some(trimmed.to_owned()) };
    let rest = rest.strip_prefix("localhost").unwrap_or(rest);
    let path = percent_decode(rest);
    (!path.is_empty()).then_some(path)
}

fn percent_decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' && index + 2 < bytes.len() {
            let hex = std::str::from_utf8(&bytes[index + 1..index + 3]).ok();
            if let Some(byte) = hex.and_then(|hex| u8::from_str_radix(hex, 16).ok()) {
                out.push(byte);
                index += 3;
                continue;
            }
        }
        out.push(bytes[index]);
        index += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Ids that name a routing tier rather than a model: `gemini-default` (Flash) and
/// `gemini-pro-default` (Pro). Neither has a price, and the row's label says
/// which model ran (#90).
fn routing_label(model: &str) -> bool {
    matches!(model, "gemini-default" | "gemini-pro-default")
}

/// The model identity, preferring the machine id, then a sibling row's id
/// recovered through the display label, then a verified label table, then the
/// file's single model, then the routing label itself.
///
/// **The label table comes before the file's single model** (#90): a Pro row in a
/// file whose only named model is a Flash one would otherwise be priced as that
/// Flash. **No vendor default is invented** — a file naming no model at all falls
/// back to `unknown`, an unpriced name, rather than to a concrete product model
/// that would be priced at the wrong rate.
fn resolved_model(
    generation: &Generation,
    labels: &HashMap<String, HashSet<String>>,
    sole: Option<&str>,
) -> String {
    if let Some(model) = generation.model.as_deref() {
        if !routing_label(model) {
            return model.to_owned();
        }
    }
    if let Some(label) = generation.label.as_deref() {
        if let Some(models) = labels.get(label) {
            if models.len() == 1 {
                if let Some(only) = models.iter().next() {
                    return only.clone();
                }
            }
        }
        if let Some(mapped) = label_table(label) {
            return mapped.to_owned();
        }
    }
    if let Some(sole) = sole {
        return sole.to_owned();
    }
    generation.model.clone().unwrap_or_else(|| "unknown".into())
}

/// Labels whose model identity is verified. Server-supplied and localisable, so an
/// unknown label is never guessed at.
fn label_table(label: &str) -> Option<&'static str> {
    Some(match label {
        "Gemini 3.5 Flash (Low)" => "gemini-3.5-flash-extra-low",
        "Gemini 3.5 Flash (Medium)" => "gemini-3.5-flash-medium",
        "Gemini 3.5 Flash (High)" => "gemini-3.5-flash-high",
        "Gemini 3.1 Pro (High)" => "gemini-3.1-pro-preview-high",
        _ => return None,
    })
}

// MARK: - SQLite

/// AgentSQLite.each: a statement that will not prepare yields no rows and no
/// error, so a schema mismatch reads as "no rows" rather than a failure.
fn each(
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

/// AgentSQLite.data: NULL is nil, and a blob's bytes are the payload.
fn blob(row: &Row, index: usize) -> Option<Vec<u8>> {
    match row.get_ref(index).ok()? {
        ValueRef::Null => None,
        ValueRef::Blob(bytes) | ValueRef::Text(bytes) => Some(bytes.to_vec()),
        _ => None,
    }
}

/// DatabaseReaderSupport.intColumn: a non-negative integer, or nil.
fn column_int(row: &Row, index: usize) -> Option<i64> {
    match row.get_ref(index).ok()? {
        ValueRef::Integer(number) => (number >= 0).then_some(number),
        ValueRef::Real(number) => (number >= 0.0 && number.fract() == 0.0 && number < 9.0e18)
            .then_some(number as i64),
        ValueRef::Text(bytes) => std::str::from_utf8(bytes).ok()?.trim().parse::<i64>().ok().filter(|number| *number >= 0),
        _ => None,
    }
}

// MARK: - Wire format

/// A minimal, bounds-checked protobuf wire decoder.
///
/// Only the wire format itself is implemented — tags, the three scalar wire types
/// and length-delimited bytes — because the message schemas differ per client and
/// none of their `.proto` files ship. Unknown field numbers are kept; an
/// unsupported wire type (the group types) or any malformed byte fails the whole
/// message rather than returning a partial one, so a caller never reads a
/// truncated field as a value.
#[derive(Clone, Debug, PartialEq, Eq)]
enum WireValue {
    Varint(u64),
    Fixed64(u64),
    Length(Vec<u8>),
    Fixed32(u32),
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct WireMessage {
    fields: BTreeMap<u64, Vec<WireValue>>,
}

impl WireMessage {
    /// A length-delimited field decoded as a nested message, or nil when it is
    /// another wire type or not a valid message.
    fn nested(&self, number: u64) -> Option<WireMessage> {
        decode(self.bytes(number)?)
    }

    fn first(&self, number: u64) -> Option<&WireValue> {
        self.fields.get(&number)?.first()
    }

    fn varint(&self, number: u64) -> Option<u64> {
        match self.first(number)? {
            WireValue::Varint(value) => Some(*value),
            _ => None,
        }
    }

    fn bytes(&self, number: u64) -> Option<&[u8]> {
        match self.first(number)? {
            WireValue::Length(value) => Some(value),
            _ => None,
        }
    }

    /// A length-delimited field as a UTF-8 string, trimmed, nil when empty.
    fn string(&self, number: u64) -> Option<String> {
        let text = std::str::from_utf8(self.bytes(number)?).ok()?.trim();
        (!text.is_empty()).then(|| text.to_owned())
    }

    fn append(&mut self, value: WireValue, number: u64) {
        self.fields.entry(number).or_default().push(value);
    }
}

fn decode(bytes: &[u8]) -> Option<WireMessage> {
    let mut reader = ByteReader { bytes, index: 0 };
    let mut message = WireMessage::default();
    while reader.index < bytes.len() {
        let (number, value) = reader.next()?;
        message.append(value, number);
    }
    Some(message)
}

/// A forward cursor over a buffer. Every read is bounds-checked and every failure
/// returns nil rather than panicking.
struct ByteReader<'a> {
    bytes: &'a [u8],
    index: usize,
}

impl<'a> ByteReader<'a> {
    fn next(&mut self) -> Option<(u64, WireValue)> {
        let tag = self.varint()?;
        let number = tag >> 3;
        // Field zero is not a field.
        if number == 0 {
            return None;
        }
        match tag & 7 {
            0 => Some((number, WireValue::Varint(self.varint()?))),
            1 => Some((number, WireValue::Fixed64(self.fixed64()?))),
            2 => {
                let length = self.varint()?;
                let remaining = self.bytes.len().checked_sub(self.index)?;
                if length > remaining as u64 {
                    return None;
                }
                let end = self.index + length as usize;
                let slice = self.bytes[self.index..end].to_vec();
                self.index = end;
                Some((number, WireValue::Length(slice)))
            }
            5 => Some((number, WireValue::Fixed32(self.fixed32()?))),
            // Wire types 3 and 4 are the deprecated groups.
            _ => None,
        }
    }

    /// A base-128 varint, refused past ten bytes or when the tenth would overflow.
    fn varint(&mut self) -> Option<u64> {
        let mut result = 0u64;
        let mut shift = 0u32;
        for _ in 0..10 {
            let byte = *self.bytes.get(self.index)?;
            self.index += 1;
            if shift == 63 && byte > 1 {
                return None;
            }
            result |= u64::from(byte & 0x7F) << shift;
            if byte & 0x80 == 0 {
                return Some(result);
            }
            shift += 7;
        }
        None
    }

    fn fixed64(&mut self) -> Option<u64> {
        let slice = self.bytes.get(self.index..self.index.checked_add(8)?)?;
        let mut value = 0u64;
        for (offset, byte) in slice.iter().enumerate() {
            value |= u64::from(*byte) << (8 * offset);
        }
        self.index += 8;
        Some(value)
    }

    fn fixed32(&mut self) -> Option<u32> {
        let slice = self.bytes.get(self.index..self.index.checked_add(4)?)?;
        let mut value = 0u32;
        for (offset, byte) in slice.iter().enumerate() {
            value |= u32::from(*byte) << (8 * offset);
        }
        self.index += 4;
        Some(value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
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

    fn field_varint(number: u64, value: u64) -> Vec<u8> {
        let mut out = tag(number, 0);
        out.extend(varint(value));
        out
    }

    fn field_bytes(number: u64, value: &[u8]) -> Vec<u8> {
        let mut out = tag(number, 2);
        out.extend(varint(value.len() as u64));
        out.extend_from_slice(value);
        out
    }

    fn field_string(number: u64, value: &str) -> Vec<u8> {
        field_bytes(number, value.as_bytes())
    }

    fn field_fixed64(number: u64, value: u64) -> Vec<u8> {
        let mut out = tag(number, 1);
        out.extend_from_slice(&value.to_le_bytes());
        out
    }

    fn tag(number: u64, wire: u64) -> Vec<u8> {
        varint((number << 3) | wire)
    }

    fn varint(mut value: u64) -> Vec<u8> {
        let mut out = Vec::new();
        loop {
            let byte = (value & 0x7F) as u8;
            value >>= 7;
            if value == 0 {
                out.push(byte);
                return out;
            }
            out.push(byte | 0x80);
        }
    }

    fn timestamp_bytes(seconds: u64) -> Vec<u8> {
        field_varint(1, seconds)
    }

    /// One conversation database with the three tables the reader selects from.
    fn database(home: &Path, client: &str, stem: &str, trajectory: &[u8], generations: &[(i64, Vec<u8>)]) -> PathBuf {
        let folder = if client == "antigravity-ide" { "antigravity" } else { client };
        let path = home.join(".gemini").join(folder).join("conversations").join(format!("{stem}.db"));
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        let db = Connection::open(&path).unwrap();
        db.execute_batch(
            "CREATE TABLE trajectory_metadata_blob (data BLOB); \
             CREATE TABLE gen_metadata (idx INTEGER, data BLOB); \
             CREATE TABLE steps (metadata BLOB, step_type INTEGER);",
        ).unwrap();
        db.execute("INSERT INTO trajectory_metadata_blob VALUES (?1)", [trajectory]).unwrap();
        for (index, data) in generations {
            db.execute("INSERT INTO gen_metadata VALUES (?1, ?2)", rusqlite::params![index, data]).unwrap();
        }
        drop(db);
        path
    }

    /// A generation whose usage is under `chat.#4`.
    fn generation(chat: &[u8]) -> Vec<u8> {
        field_bytes(1, chat)
    }

    #[test]
    fn roots_keep_the_two_antigravity_clients_disjoint() {
        let home = PathBuf::from("/home");
        let cli = roots(&home, "antigravity-cli", &|_: &str| None);
        assert_eq!(cli, vec![home.join(".gemini/antigravity-cli/conversations")]);
        let ide = roots(&home, "antigravity-ide", &|_: &str| None);
        assert!(ide.contains(&home.join(".gemini/antigravity/conversations")), "{ide:?}");
        assert!(ide.contains(&home.join(".gemini/antigravity-ide/conversations")), "{ide:?}");
        assert!(!ide.contains(&home.join(".gemini/antigravity-cli/conversations")), "{ide:?}");
        // The two clients never read one folder, so their figures are never pooled.
        assert!(cli.iter().all(|root| !ide.contains(root)));
        // An override moves both clients' roots, and only those.
        let overridden = |client: &str| roots(&home, client, &|key: &str| (key == "GEMINI_CLI_HOME").then(|| "/elsewhere".to_string()));
        for client in ["antigravity-cli", "antigravity-ide"] {
            let found = overridden(client);
            assert!(!found.is_empty(), "{client}");
            assert!(found.iter().all(|root| root.starts_with("/elsewhere/")), "{client}: {found:?}");
        }
        // The export id is a different source and has no root here.
        assert!(roots(&home, "antigravity", &|_: &str| None).is_empty());
        // An unknown client reads nothing rather than defaulting to a product.
        assert!(roots(&home, "unknown", &|_: &str| None).is_empty());
    }

    #[test]
    fn wire_decoder_keeps_known_fields_and_rejects_malformed_bytes() {
        let mut valid = field_varint(1, 5);
        valid.extend(field_string(2, "hi"));
        valid.extend(field_fixed64(3, 0x0102_0304_0506_0708));
        let message = decode(&valid).expect("valid message");
        assert_eq!(message.varint(1), Some(5));
        assert_eq!(message.string(2).as_deref(), Some("hi"));
        assert_eq!(message.first(3), Some(&WireValue::Fixed64(0x0102_0304_0506_0708)));
        // An unknown field number is kept, not guessed at.
        assert_eq!(decode(&field_varint(500, 9)).unwrap().varint(500), Some(9));
        // A truncated varint, an overlong varint, a length past the end, a group
        // wire type and field zero each fail the whole message.
        assert!(decode(&[0x08, 0x80]).is_none());
        let mut overlong = vec![0x08];
        overlong.extend(std::iter::repeat(0x80).take(11));
        overlong.push(0x01);
        assert!(decode(&overlong).is_none());
        let mut past_end = tag(1, 2);
        past_end.extend_from_slice(&[0x0A, 0x01]);
        assert!(decode(&past_end).is_none());
        assert!(decode(&tag(1, 3)).is_none());
        assert!(decode(&[0x00]).is_none());
        // A nested field that is not a valid message is nil, never partial.
        let nested = field_bytes(1, &[0x08, 0x80]);
        assert!(decode(&nested).unwrap().nested(1).is_none());
    }

    #[test]
    fn conversation_counts_the_established_fields_once_and_dedups_by_response_id() {
        let home = temp_dir("antigravity-counts");
        let trajectory = {
            let mut bytes = field_bytes(2, &timestamp_bytes(1_789_372_800));
            bytes.extend(field_bytes(1, &field_string(1, "file:///Users/me/Code/Pulse")));
            bytes
        };
        let mut usage = field_varint(1, 1132);
        usage.extend(field_varint(2, 100));
        usage.extend(field_varint(5, 30));
        usage.extend(field_varint(9, 10));
        usage.extend(field_varint(10, 5));
        usage.extend(field_string(11, "resp-1"));
        let mut chat = field_string(19, "priced");
        chat.extend(field_bytes(4, &usage));
        chat.extend(field_bytes(9, &field_bytes(4, &timestamp_bytes(1_789_372_900))));
        let path = database(&home, "antigravity-cli", "conv-a", &trajectory, &[(0, generation(&chat))]);

        let (records, notes) = read(&path, "antigravity-cli", &AtomicBool::new(false)).unwrap();
        assert_eq!(records.len(), 1, "{records:?}");
        let record = &records[0];
        // #2 input, #5 cache read, #9 + #10 output counted once; #1 is not a count.
        assert_eq!(record.tally, Tally { input: 100, cache_read: 30, output: 15, ..Default::default() });
        assert_eq!(record.tally.total(), 145);
        assert_eq!(record.model, "priced");
        assert_eq!(record.session, "conv-a");
        assert_eq!(record.project.as_deref(), Some("/Users/me/Code/Pulse"));
        assert_eq!(record.source_timestamp, Some(1_789_372_900));
        assert_eq!(record.deduplication_id.as_deref(), Some("antigravity:conv-a:resp-1"));
        assert!(!record.aggregate_timing);
        // Every record is a proven subset, and the read says so.
        assert!(notes.iter().any(|note| note.contains("可证明的子集")), "{notes:?}");
        let _ = fs::remove_dir_all(&home);
    }

    #[test]
    fn pro_routing_label_is_resolved_before_the_files_single_model() {
        let home = temp_dir("antigravity-label");
        let trajectory = field_bytes(2, &timestamp_bytes(1_789_372_800));
        let flash = {
            let mut chat = field_string(19, "gemini-3.6-flash");
            chat.extend(field_string(21, "Gemini 3.6 Flash (High)"));
            let mut usage = field_varint(2, 100);
            usage.extend(field_string(11, "resp-f"));
            chat.extend(field_bytes(4, &usage));
            generation(&chat)
        };
        let pro = {
            // A routing tier, not a model: the label says which model ran.
            let mut chat = field_string(19, "gemini-pro-default");
            chat.extend(field_string(21, "Gemini 3.1 Pro (High)"));
            let mut usage = field_varint(2, 100);
            usage.extend(field_string(11, "resp-p"));
            chat.extend(field_bytes(4, &usage));
            generation(&chat)
        };
        let path = database(&home, "antigravity-cli", "conv-p", &trajectory, &[(0, flash), (1, pro)]);
        let (records, _) = read(&path, "antigravity-cli", &AtomicBool::new(false)).unwrap();
        assert_eq!(records.len(), 2, "{records:?}");
        let flash = records.iter().find(|record| record.deduplication_id.as_deref() == Some("antigravity:conv-p:resp-f")).unwrap();
        assert_eq!(flash.model, "gemini-3.6-flash");
        let pro = records.iter().find(|record| record.deduplication_id.as_deref() == Some("antigravity:conv-p:resp-p")).unwrap();
        // The verified label wins over the file's only named model, so a Pro row
        // is not priced as a Flash one.
        assert_eq!(pro.model, "gemini-3.1-pro-preview-high");
        let _ = fs::remove_dir_all(&home);
    }

    #[test]
    fn fields_with_no_established_meaning_are_never_counted() {
        let home = temp_dir("antigravity-unknown");
        let trajectory = field_bytes(2, &timestamp_bytes(1_789_372_800));
        let empty = {
            let mut usage = field_varint(1, 1132);
            usage.extend(field_varint(20, 999_999));
            usage.extend(field_varint(2, 0));
            usage.extend(field_string(11, "resp-3"));
            generation(&field_bytes(4, &usage))
        };
        let counted = {
            let mut usage = field_varint(1, 1132);
            usage.extend(field_varint(20, 999_999));
            usage.extend(field_varint(2, 100));
            usage.extend(field_string(11, "resp-4"));
            generation(&field_bytes(4, &usage))
        };
        let path = database(&home, "antigravity-cli", "conv-c", &trajectory, &[(0, empty), (1, counted)]);
        let (records, _) = read(&path, "antigravity-cli", &AtomicBool::new(false)).unwrap();
        // A turn whose real counters are all zero is dropped, and neither #1 nor
        // #20 is promoted into a count or into unclassified work.
        assert_eq!(records.len(), 1, "{records:?}");
        assert_eq!(records[0].tally, Tally { input: 100, ..Default::default() });
        assert_eq!(records[0].tally.total(), 100);
        assert_eq!(records[0].unclassified_tokens, 0);
        let _ = fs::remove_dir_all(&home);
    }

    #[test]
    fn a_turn_without_an_event_time_anchors_on_the_session_and_is_aggregate() {
        let home = temp_dir("antigravity-anchor");
        let trajectory = field_bytes(2, &timestamp_bytes(1_789_372_800));
        let mut usage = field_varint(2, 100);
        usage.extend(field_string(11, "resp-2"));
        let path = database(&home, "antigravity-cli", "conv-b", &trajectory, &[(0, generation(&field_bytes(4, &usage)))]);
        let (records, _) = read(&path, "antigravity-cli", &AtomicBool::new(false)).unwrap();
        assert_eq!(records.len(), 1, "{records:?}");
        assert_eq!(records[0].source_timestamp, Some(1_789_372_800));
        assert_eq!(records[0].tally, Tally { input: 100, ..Default::default() });
        // A session anchor is not the moment of the turn.
        assert!(records[0].aggregate_timing);
        let _ = fs::remove_dir_all(&home);
    }

    #[test]
    fn ide_conversations_are_read_from_their_own_folder_and_nothing_private_is_serialized() {
        let home = temp_dir("antigravity-ide");
        let trajectory = {
            let mut bytes = field_bytes(2, &timestamp_bytes(1_789_372_800));
            bytes.extend(field_bytes(1, &field_string(1, "file:///Users/me/Code/Pulse")));
            bytes
        };
        let mut usage = field_varint(2, 100);
        usage.extend(field_string(11, "resp-1"));
        let mut chat = field_string(19, "gemini-3.6-flash");
        // A field the reader does not read: prompt text stays out of the record.
        chat.extend(field_string(30, "SECRET PROMPT TEXT"));
        chat.extend(field_bytes(4, &usage));
        let path = database(&home, "antigravity-ide", "conv-i", &trajectory, &[(0, generation(&chat))]);
        assert!(candidate(&path, "antigravity-ide"));
        // The two clients share one store shape, so the same file is a candidate
        // for both; what keeps their figures apart is the root each one scans.
        assert!(candidate(&path, "antigravity-cli"));
        let ide_roots = roots(&home, "antigravity-ide", &|_: &str| None);
        let cli_roots = roots(&home, "antigravity-cli", &|_: &str| None);
        assert!(ide_roots.iter().any(|root| path.starts_with(root)), "{ide_roots:?}");
        assert!(cli_roots.iter().all(|root| !path.starts_with(root)), "{cli_roots:?}");
        assert!(!candidate(&path.with_extension("db-shm"), "antigravity-ide"));
        let (records, notes) = read(&path, "antigravity-ide", &AtomicBool::new(false)).unwrap();
        assert_eq!(records.len(), 1, "{records:?}");
        assert_eq!(records[0].agent, "antigravity-ide");
        assert!(notes.iter().any(|note| note.starts_with("Antigravity IDE")), "{notes:?}");
        let serialized = serde_json::to_string(&records).unwrap();
        assert!(!serialized.contains("SECRET"));
        let cancelled = read(&path, "antigravity-ide", &AtomicBool::new(true));
        assert_eq!(cancelled.unwrap_err(), "cancelled");
        let _ = fs::remove_dir_all(&home);
    }
}
