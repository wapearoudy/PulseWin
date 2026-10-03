//! Real on-disk readers ported from Pulse's GeminiSessionReader and Group E.
//! Captured sources require an existing export/sync/capture. No authentication,
//! client execution, native-store guessing or vendor-reported cost is used here.
use super::*;
use chrono::{NaiveDate, NaiveDateTime, TimeZone};
use serde::de::DeserializeOwned;
use std::io::Read;

const MAX_DOCUMENT: usize = 64 * 1024 * 1024;
pub(super) fn captured(agent: &str) -> bool { matches!(agent, "cursor" | "antigravity" | "hindsight" | "mcode") }
pub(super) fn roots(home: &Path, agent: &str, environment: &impl Fn(&str) -> Option<String>) -> Vec<PathBuf> {
    let cache = environment("TOKSCALE_CONFIG_DIR").filter(|v| !v.trim().is_empty()).map(|v| PathBuf::from(v.trim())).unwrap_or_else(|| home.join(".config/tokscale"));
    let mut roots = vec![match agent {
        "cursor" => cache.join("cursor-cache"),
        "antigravity" => cache.join("antigravity-cache/sessions"),
        "mcode" => cache.join("headless/mcode"),
        "hindsight" => environment("HINDSIGHT_HOME").filter(|v| !v.trim().is_empty()).map(|v| PathBuf::from(v.trim())).unwrap_or_else(|| home.join(".hindsight")).join("usage"),
        _ => return Vec::new(),
    }];
    // Windows adaptation of Pulse's own UsageImports drop folder, not a guessed
    // product-native path. The pane displays these actual roots to the user.
    if let Some(appdata) = environment("APPDATA").filter(|v| !v.trim().is_empty()) { roots.push(PathBuf::from(appdata).join("PulseWin/UsageImports").join(agent)); }
    roots
}
pub(super) fn candidate(path: &Path, agent: &str) -> bool {
    let extension = path.extension().unwrap_or_default().to_string_lossy().to_ascii_lowercase();
    match agent {
        "gemini" => extension == "jsonl" || (extension == "json" && (
            path.file_name().unwrap_or_default().to_string_lossy().starts_with("session-") || gemini_chat_path(path))),
        "cursor" => matches!(extension.as_str(), "json" | "csv") && !path.file_stem().unwrap_or_default().to_string_lossy().to_ascii_lowercase().starts_with("usage.backup"),
        _ => extension == "jsonl",
    }
}
fn gemini_chat_path(path: &Path) -> bool {
    let pieces: Vec<_> = path.components().map(|v| v.as_os_str().to_string_lossy()).collect();
    pieces.len() >= 4 && pieces[pieces.len()-2] == "chats" && pieces[pieces.len()-4] == "tmp"
}
fn text(value: &Value) -> Option<String> { value.as_str().map(str::trim).filter(|s| !s.is_empty()).map(str::to_owned) }
fn count_value(value: &Value) -> Option<u64> {
    if let Some(raw) = value.as_str() { return raw.trim().parse::<u64>().ok().filter(|v| *v <= i64::MAX as u64); }
    if let Some(raw) = value.as_u64() { return (raw <= i64::MAX as u64).then_some(raw); }
    value.as_f64().filter(|v| v.is_finite() && *v >= 0. && v.fract() == 0. && *v < 9_223_372_036_854_775_808.).map(|v| v as u64)
}
fn count(value: &Value) -> u64 { count_value(value).unwrap_or(0) }
fn timestamp(value: &Value, milliseconds: bool) -> Option<DateTime<Utc>> {
    if let Some(iso) = value.as_str().and_then(|s| DateTime::parse_from_rfc3339(s).ok()) { return Some(iso.with_timezone(&Utc)); }
    let number = value.as_f64().or_else(|| value.as_str().and_then(|s| s.trim().parse::<f64>().ok()))?;
    if !number.is_finite() { return None; }
    let millis = if milliseconds { number } else { number * 1000. };
    if millis >= i64::MAX as f64 || millis <= i64::MIN as f64 { return None; }
    DateTime::from_timestamp_millis(millis as i64)
}
fn record(agent: &str, model: String, at: DateTime<Utc>, session: String, tally: Tally, unclassified: u64, id: Option<String>) -> SpendRecord {
    let local = at.with_timezone(&Local);
    SpendRecord { agent: agent.into(), model, day: local.format("%Y-%m-%d").to_string(), hour: chrono::Timelike::hour(&local),
        session, project: None, tally, unclassified_tokens: unclassified, deduplication_id: id,
        aggregate_timing: false, source_timestamp: Some(at.timestamp_millis()), source_scope: None,
        cost: None, cost_breakdown: None, model_name: None }
}
fn bounded_bytes(path: &Path, cancel: &AtomicBool) -> Result<Vec<u8>, String> {
    let mut file = File::open(path).map_err(|e| format!("无法读取 {}：{e}", path.display()))?;
    let mut bytes = Vec::new(); let mut buffer = [0; 64 * 1024];
    loop {
        check(cancel)?; let read = file.read(&mut buffer).map_err(|e| e.to_string())?;
        if read == 0 { break; }
        if bytes.len() + read > MAX_DOCUMENT { return Err(format!("{} 超过 64 MiB；未读取，计数可能不完整", path.display())); }
        bytes.extend_from_slice(&buffer[..read]);
    }
    check(cancel)?; Ok(bytes)
}
fn document<T: DeserializeOwned>(path: &Path, cancel: &AtomicBool) -> Result<T, String> {
    let bytes = bounded_bytes(path, cancel)?;
    let data = bytes.strip_prefix(&[0xef, 0xbb, 0xbf]).unwrap_or(&bytes);
    let decoded = serde_json::from_slice(data).map_err(|e| format!("无法解码 {}：{e}；计数可能不完整", path.display()))?;
    check(cancel)?; Ok(decoded)
}
/// Bounded, cancellable JSONL. Bad/oversize lines do not hide subsequent usage.
fn lines(reader: &mut impl BufRead, cancel: &AtomicBool, notes: &mut Vec<String>, mut visit: impl FnMut(Value, usize) -> Result<(), String>) -> Result<(), String> {
    let mut ordinal = 0;
    loop {
        check(cancel)?; let mut line = Vec::new(); let mut over = false; let mut eof = false;
        loop {
            check(cancel)?; let chunk = reader.fill_buf().map_err(|e| e.to_string())?;
            if chunk.is_empty() { eof = true; break; }
            let length = chunk.iter().position(|c| *c == b'\n').map(|i| i+1).unwrap_or(chunk.len());
            let end = chunk[length-1] == b'\n';
            if !over { if line.len() + length <= MAX_LINE { line.extend_from_slice(&chunk[..length]); } else { over = true; line.clear(); } }
            reader.consume(length); if end { break; }
        }
        ordinal += 1;
        if over { notes.push("一条捕获记录超过 8 MiB，计数可能不完整".into()); }
        let line = line.strip_prefix(&[0xef, 0xbb, 0xbf]).unwrap_or(&line);
        if !line.iter().all(u8::is_ascii_whitespace) {
            match serde_json::from_slice(line) {
                Ok(value) => visit(value, ordinal)?,
                Err(_) => notes.push("无法解码一条捕获/会话记录，计数可能不完整".into()),
            }
        }
        if eof { break; }
    }
    check(cancel)
}
pub(super) fn parse_file(path: &Path, agent: &str, cancel: &AtomicBool) -> Result<(Vec<SpendRecord>, Vec<String>), String> {
    check(cancel)?;
    if agent == "gemini" && path.extension().is_some_and(|s| s.eq_ignore_ascii_case("json")) { return gemini_session(path, cancel); }
    if agent == "cursor" { return cursor_file(path, cancel); }
    let file = File::open(path).map_err(|e| format!("无法读取 {}：{e}", path.display()))?;
    let mut reader = BufReader::with_capacity(64 * 1024, file);
    let stem = path.file_stem().unwrap_or_default().to_string_lossy();
    match agent {
        "gemini" => gemini_headless(&mut reader, &stem, cancel),
        "antigravity" => antigravity(&mut reader, cancel),
        "hindsight" => hindsight(&mut reader, cancel),
        "mcode" => mcode(&mut reader, cancel),
        _ => Err("未实现的 reader".into()),
    }
}
// Selective deserialization discards conversation text/title fields outright.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct GeminiSession {
    session_id: Option<Value>, #[serde(default)] messages: Vec<GeminiMessage>,
    #[serde(rename = "session_id")] alternate_session: Option<Value>,
}
#[derive(Deserialize)]
struct GeminiMessage { #[serde(rename = "type")] kind: Option<Value>, id: Option<Value>, model: Option<Value>, timestamp: Option<Value>, created_at: Option<Value>, tokens: Option<Value> }
fn gemini_session(path: &Path, cancel: &AtomicBool) -> Result<(Vec<SpendRecord>, Vec<String>), String> {
    let root: GeminiSession = document(path, cancel)?; let mut records = Vec::new(); let mut notes = Vec::new();
    let session = root.session_id.as_ref().and_then(text).or_else(|| root.alternate_session.as_ref().and_then(text)).unwrap_or_else(|| path.file_stem().unwrap_or_default().to_string_lossy().into_owned());
    for message in root.messages {
        check(cancel)?; if message.kind.as_ref().and_then(Value::as_str) != Some("gemini") { continue; }
        let Some(tokens) = message.tokens else { continue; };
        let (tally, unclassified) = gemini_usage(&tokens, false, false);
        if tally.total() == 0 && unclassified == 0 { continue; }
        let model = message.model.as_ref().and_then(text);
        let at = message.timestamp.as_ref().and_then(|v| timestamp(v, false)).or_else(|| message.created_at.as_ref().and_then(|v| timestamp(v,false)));
        if let (Some(model), Some(at)) = (model, at) {
            let id = message.id.as_ref().and_then(text).map(|id| format!("gemini:session:{session}:{id}"));
            records.push(record("gemini", model, at, session.clone(), tally, unclassified, id));
        } else { notes.push("Gemini CLI：实测计数缺少有效模型或时间，计数可能不完整".into()); }
        if records.len() >= MAX_RECORDS { notes.push("Gemini CLI：达到记录上限，计数可能不完整".into()); break; }
    }
    check(cancel)?; Ok((records, notes))
}
fn gemini_usage(value: &Value, headless: bool, wrapper: bool) -> (Tally, u64) {
    fn first<'a>(v: &'a Value, keys: &[&'a str]) -> (u64, Option<&'a str>) {
        keys.iter().find(|key| !v[**key].is_null()).map(|key| (count(&v[*key]), Some(*key))).unwrap_or((0, None))
    }
    let (input, key) = first(value, &["input","prompt","input_tokens","prompt_tokens","promptTokenCount"]);
    let (output, _) = first(value, &["output","candidates","output_tokens","completion_tokens","candidatesTokenCount"]);
    let (cached, _) = first(value, &["cached","cached_tokens","cachedContentTokenCount"]);
    let (reasoning, _) = first(value, &["thoughts","reasoning","thoughts_tokens"]);
    let (tool, _) = first(value, &["tool","tool_tokens"]);
    let (total, total_key) = first(value, &["total","totalTokenCount","total_tokens"]);
    let named = ["input","prompt","input_tokens","prompt_tokens","promptTokenCount","output","candidates","output_tokens","completion_tokens","candidatesTokenCount","cached","cached_tokens","cachedContentTokenCount","thoughts","reasoning","thoughts_tokens","tool","tool_tokens"].iter().any(|key| !value[*key].is_null());
    if !named { return (Tally::default(), total); }
    let output = output.saturating_add(reasoning); let raw = if headless { input } else { input.saturating_add(tool) };
    let included = if headless { wrapper || key.is_some_and(|k| k != "input") } else if total_key.is_some() {
        total == raw.saturating_add(output) && total != raw.saturating_add(output).saturating_add(cached)
    } else { key.is_some_and(|k| k != "input") };
    (Tally { input: if included { raw.saturating_sub(cached) } else { raw }, output, cache_write: 0, cache_read: cached }, 0)
}
fn gemini_headless(reader: &mut impl BufRead, stem: &str, cancel: &AtomicBool) -> Result<(Vec<SpendRecord>, Vec<String>), String> {
    let mut records = Vec::new(); let mut io_notes = Vec::new(); let mut notes = Vec::new();
    let mut current_model = None; let mut current_session = None; let mut positions: HashMap<String, usize> = HashMap::new();
    lines(reader, cancel, &mut io_notes, |line, ordinal| {
        if line["type"] == "init" {
            if let Some(model) = text(&line["model"]) { current_model = Some(model); }
            if let Some(session) = text(&line["session_id"]).or_else(|| text(&line["sessionId"])) { current_session = Some(session); }
            return Ok(());
        }
        let sid = text(&line["session_id"]).or_else(|| text(&line["sessionId"])).or_else(|| current_session.clone()).unwrap_or_else(|| stem.into());
        let id = text(&line["id"]); let at = timestamp(&line["timestamp"], false).or_else(|| timestamp(&line["created_at"], false));
        let mut submit = |usage: &Value, model: Option<String>, at: Option<DateTime<Utc>>, identity: Option<String>, fallback: String, wrapper: bool| {
            let (tally, unclassified) = gemini_usage(usage, true, wrapper);
            if tally.total() == 0 && unclassified == 0 { return; }
            if let (Some(model), Some(at)) = (model, at) {
                let dedup = identity.as_ref().map(|id| format!("gemini:line:{id}")).unwrap_or(fallback);
                let entry = record("gemini", model, at, sid.clone(), tally, unclassified, Some(dedup));
                if let Some(position) = identity.as_ref().and_then(|id| positions.get(id)).copied() { records[position] = entry; }
                else { if let Some(identity) = identity { positions.insert(identity, records.len()); } records.push(entry); }
            } else { notes.push("Gemini CLI：实测计数缺少有效模型或时间，计数可能不完整".into()); }
        };
        if line["tokens"].is_object() {
            submit(&line["tokens"], text(&line["model"]).or_else(|| current_model.clone()), at, id, format!("gemini:headless:{stem}:{ordinal}"), true);
        } else {
            let stats = if line["stats"].is_object() { &line["stats"] } else { &line["result"]["stats"] };
            if let Some(models) = stats["models"].as_object() {
                for (model, usage) in models { check(cancel)?; submit(usage, Some(model.clone()), timestamp(&usage["timestamp"], false).or(at), id.as_ref().map(|id| format!("{id}:{model}")), format!("gemini:headless:{stem}:{ordinal}:{model}"), false); }
            } else if stats.is_object() {
                submit(stats, text(&stats["model"]).or_else(|| current_model.clone()), timestamp(&stats["timestamp"], false).or(at), id, format!("gemini:headless:{stem}:{ordinal}"), false);
            }
        }
        if records.len() >= MAX_RECORDS { return Err("Gemini CLI 达到记录上限，计数可能不完整".into()); }
        Ok(())
    })?;
    notes.extend(io_notes); Ok((records, notes))
}
fn antigravity(reader: &mut impl BufRead, cancel: &AtomicBool) -> Result<(Vec<SpendRecord>, Vec<String>), String> {
    let mut records = Vec::new(); let mut notes = Vec::new(); let mut io_notes = Vec::new(); let mut model = None;
    lines(reader, cancel, &mut io_notes, |line, _| {
        if line["type"] == "session_meta" { model = text(&line["modelId"]); return Ok(()); }
        if line["type"] != "usage" { return Ok(()); }
        let tally = Tally { input: count(&line["input"]), output: count(&line["output"]), cache_read: count(&line["cacheRead"]), cache_write: count(&line["cacheWrite"]) };
        if tally.total() == 0 { return Ok(()); }
        let named = text(&line["modelId"]).or_else(|| model.clone()).filter(|s| !s.to_ascii_lowercase().starts_with("model_placeholder_"));
        if let (Some(named), Some(sid), Some(at)) = (named, text(&line["sessionId"]), timestamp(&line["timestamp"], true).filter(|at| at.timestamp_millis() > 0)) {
            if count(&line["reasoning"]) > 0 { notes.push("Antigravity IDE：reasoning 与输出是否重叠不明；仅计入明确的四类计数，可能不完整".into()); }
            let id = text(&line["responseId"]).map(|id| format!("antigravity:{id}"));
            records.push(record("antigravity", named, at, sid, tally, 0, id));
        } else { notes.push("Antigravity IDE：用量缺少有效模型、会话或时间，计数可能不完整".into()); }
        if records.len() >= MAX_RECORDS { return Err("Antigravity IDE 达到记录上限".into()); } Ok(())
    })?;
    notes.extend(io_notes); Ok((records, notes))
}
fn hindsight(reader: &mut impl BufRead, cancel: &AtomicBool) -> Result<(Vec<SpendRecord>, Vec<String>), String> {
    let mut records = Vec::new(); let mut io_notes = Vec::new(); let mut notes = Vec::new();
    lines(reader, cancel, &mut io_notes, |line, _| {
        if count_value(&line["total_tokens"]) == Some(0) { return Ok(()); }
        let tally = Tally { input: count(&line["input_tokens"]), output: count(&line["output_tokens"]), cache_read: count(&line["cached_tokens"]), cache_write: 0 };
        if tally.input == 0 && tally.output == 0 { return Ok(()); }
        if let (Some(id), Some(model), Some(at)) = (text(&line["id"]), text(&line["model"]), timestamp(&line["started_at"], false)) {
            let sid = text(&line["trace_id"]).unwrap_or_else(|| id.clone());
            let mut entry = record("hindsight", model, at, sid, tally, 0, Some(format!("hindsight:{id}")));
            entry.project = text(&line["bank"]); records.push(entry);
        } else { notes.push("Hindsight：用量缺少 id、模型或有效 started_at，计数可能不完整".into()); }
        if records.len() >= MAX_RECORDS { return Err("Hindsight 达到记录上限".into()); } Ok(())
    })?;
    notes.extend(io_notes); Ok((records, notes))
}
struct MessageUsage { tally: Tally, unclassified: u64, at: Option<DateTime<Utc>> }
fn mcode(reader: &mut impl BufRead, cancel: &AtomicBool) -> Result<(Vec<SpendRecord>, Vec<String>), String> {
    let mut buffered: BTreeMap<String, Vec<MessageUsage>> = BTreeMap::new(); let mut positions: HashMap<(String,String),usize> = HashMap::new();
    let mut results: HashMap<String,(String,String)> = HashMap::new(); let mut io_notes = Vec::new(); let mut notes = Vec::new(); let mut messages = 0;
    lines(reader, cancel, &mut io_notes, |line, _| {
        if line["type"] == "message" {
            let message = &line["message"];
            if message["role"] != "assistant" { return Ok(()); }
            let Some(turn) = text(&message["turnId"]) else { return Ok(()); };
            let usage = &message["usage"]; if !usage.is_object() { return Ok(()); }
            let tally = Tally { input: count(&usage["inputTokens"]), output: count(&usage["outputTokens"]), cache_write: count(&usage["cacheWriteTokens"]), cache_read: count(&usage["cacheReadTokens"]) };
            let unclassified = if tally.total() == 0 { count(&usage["totalTokens"]) } else { 0 };
            if tally.total() == 0 && unclassified == 0 { return Ok(()); }
            let raw_time = message["timestamp"].as_f64().or_else(|| message["timestamp"].as_str().and_then(|s| s.parse::<f64>().ok()));
            let at = raw_time.filter(|v| *v > 0.).and_then(|v| timestamp(&Value::from(v), v >= 10_000_000_000.));
            let identity = text(&message["id"]).or_else(|| text(&message["messageId"])).or_else(|| text(&message["responseId"]));
            let entry = MessageUsage { tally, unclassified, at }; let entries = buffered.entry(turn.clone()).or_default();
            if let Some(position) = identity.as_ref().and_then(|id| positions.get(&(turn.clone(), id.clone()))).copied() { entries[position] = entry; }
            else { if let Some(id) = identity { positions.insert((turn,id), entries.len()); } entries.push(entry); messages += 1; }
        } else if line["type"] == "exec.result" {
            if let (Some(sid), Some(turn), Some(_provider), Some(model)) = (text(&line["sessionId"]), text(&line["turnId"]), text(&line["model"]["providerId"]), text(&line["model"]["modelId"])) { results.insert(turn,(sid,model)); }
        }
        if messages >= MAX_RECORDS { return Err("MCode 达到记录上限".into()); } Ok(())
    })?;
    let mut records = Vec::new();
    for (turn, entries) in buffered {
        check(cancel)?; let Some((sid,model)) = results.get(&turn) else { notes.push("MCode：捕获缺少对应 exec.result 模型，用量未计入".into()); continue; };
        for (index, entry) in entries.into_iter().enumerate() {
            check(cancel)?; let Some(at) = entry.at else { notes.push("MCode：用量缺少有效时间，计数可能不完整".into()); continue; };
            let t = &entry.tally;
            let id = format!("mcode:{sid}:{turn}:{index}:{}:{}:{}:{}:{}", t.input,t.output,t.cache_read,t.cache_write,entry.unclassified);
            records.push(record("mcode", model.clone(), at, sid.clone(), entry.tally, entry.unclassified, Some(id)));
        }
    }
    notes.extend(io_notes); Ok((records, notes))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CursorExport { usage_events_display: Option<Vec<CursorEvent>> }
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CursorEvent { model: Option<Value>, timestamp: Option<Value>, conversation_id: Option<Value>, token_usage: Option<Value> }
fn cursor_scope(path: &Path) -> String {
    let stem = path.file_stem().unwrap_or_default().to_string_lossy(); let mut parts = stem.split('.');
    if !parts.next().is_some_and(|s| s.eq_ignore_ascii_case("usage")) { return "import".into(); }
    let account = parts.next().map(|s| {
        let mut result = String::new(); let mut dash = false;
        for c in s.chars() { if c.is_ascii_alphanumeric() { if dash && !result.is_empty() { result.push('-'); } result.push(c.to_ascii_lowercase()); dash = false; } else { dash = true; } }
        result
    }).filter(|s| !s.is_empty()).unwrap_or_else(|| "active".into());
    format!("account:{account}")
}
fn cursor_file(path: &Path, cancel: &AtomicBool) -> Result<(Vec<SpendRecord>, Vec<String>), String> {
    let scope = cursor_scope(path); let account = scope.strip_prefix("account:").unwrap_or("unscoped");
    let mut records = Vec::new(); let mut notes = Vec::new();
    if path.extension().is_some_and(|s| s.eq_ignore_ascii_case("csv")) {
        let bytes = bounded_bytes(path, cancel)?; let rows = csv_rows(std::str::from_utf8(bytes.strip_prefix(&[0xef,0xbb,0xbf]).unwrap_or(&bytes)).map_err(|_| "Cursor CSV 编码无效")?, cancel)?;
        let header_index = rows.iter().position(|r| r.iter().any(|s| s.trim().eq_ignore_ascii_case("date")) && r.iter().any(|s| s.trim().eq_ignore_ascii_case("model")));
        let Some(header_index) = header_index else { return Ok((records,notes)); };
        let header: Vec<_> = rows[header_index].iter().map(|s| s.trim().to_ascii_lowercase()).collect();
        let required = ["date","model","input (w/o cache write)","input (w/ cache write)","cache read","output tokens"];
        let Some(columns) = required.iter().map(|name| header.iter().position(|s| s == name)).collect::<Option<Vec<_>>>() else { return Ok((records,notes)); };
        let cloud = header.iter().position(|s| s == "cloud agent id");
        let widest = *columns.iter().max().unwrap();
        for row in rows.into_iter().skip(header_index+1) {
            check(cancel)?; if row.iter().all(|s| s.trim().is_empty()) { continue; }
            if row.len() <= widest { notes.push("Cursor CSV：一条记录列数不足，计数可能不完整".into()); continue; }
            let at = csv_date(row[columns[0]].trim()); let model = row[columns[1]].trim();
            let cell = |index: usize| count(&Value::String(row[columns[index]].replace(',', "")));
            let tally = Tally { input: cell(2), cache_write: cell(3), cache_read: cell(4), output: cell(5) };
            if tally.total() == 0 { continue; }
            if let Some(at) = at.filter(|at| !model.is_empty() && at.timestamp_millis() > 0) {
                let sid = cloud.and_then(|i| row.get(i)).filter(|s| !s.trim().is_empty()).map(|s| format!("cursor:{account}:cloud:{}", s.trim())).unwrap_or_default();
                let mut entry = record("cursor", model.into(), at, sid, tally,0,None); entry.aggregate_timing = true; entry.source_scope = Some(scope.clone()); records.push(entry);
            } else { notes.push("Cursor CSV：用量缺少模型或有效日期，计数可能不完整".into()); }
            if records.len() >= MAX_RECORDS { return Err("Cursor CSV 达到记录上限".into()); }
        }
    } else {
        let root: CursorExport = document(path, cancel)?;
        for event in root.usage_events_display.unwrap_or_default() {
            check(cancel)?; let usage = event.token_usage.unwrap_or(Value::Null);
            let tally = Tally { input: count(&usage["inputTokens"]), output: count(&usage["outputTokens"]), cache_write: count(&usage["cacheWriteTokens"]), cache_read: count(&usage["cacheReadTokens"]) };
            if tally.total() == 0 { continue; }
            if let (Some(model),Some(at)) = (event.model.as_ref().and_then(text), event.timestamp.as_ref().and_then(|v| timestamp(v,true)).filter(|at| at.timestamp_millis() > 0)) {
                let sid = event.conversation_id.as_ref().and_then(text).map(|s| format!("cursor:{account}:{s}")).unwrap_or_default();
                let mut entry = record("cursor",model,at,sid,tally,0,None); entry.source_scope = Some(scope.clone()); records.push(entry);
            } else { notes.push("Cursor：用量缺少模型或有效时间，计数可能不完整".into()); }
            if records.len() >= MAX_RECORDS { return Err("Cursor 达到记录上限".into()); }
        }
    }
    check(cancel)?; Ok((records,notes))
}
fn csv_date(raw: &str) -> Option<DateTime<Utc>> {
    if let Some(at) = timestamp(&Value::String(raw.into()), false) { return Some(at); }
    if let Ok(at) = DateTime::parse_from_str(raw,"%Y-%m-%d %H:%M:%S%z") { return Some(at.with_timezone(&Utc)); }
    let local = NaiveDateTime::parse_from_str(raw,"%Y-%m-%d %H:%M:%S").ok().or_else(|| NaiveDate::parse_from_str(raw,"%Y-%m-%d").ok().and_then(|v| v.and_hms_opt(0,0,0)))?;
    Local.from_local_datetime(&local).earliest().map(|v| v.with_timezone(&Utc))
}
/// RFC4180 quotes/newlines/escaped quotes; neither position nor Total Tokens is
/// used to infer a bucket. Limits apply to the whole document and every cell.
fn csv_rows(input: &str, cancel: &AtomicBool) -> Result<Vec<Vec<String>>,String> {
    let mut rows = Vec::new(); let mut row = Vec::new(); let mut cell = String::new(); let mut quote = false; let mut chars = input.chars().peekable(); let mut size = 0;
    while let Some(c) = chars.next() {
        size += 1; if size % 4096 == 0 { check(cancel)?; }
        match c {
            '"' if quote && chars.peek() == Some(&'"') => { chars.next(); cell.push('"'); }
            '"' => quote = !quote,
            ',' if !quote => { row.push(std::mem::take(&mut cell)); }
            '\n' if !quote => { row.push(std::mem::take(&mut cell)); rows.push(std::mem::take(&mut row)); }
            '\r' if !quote => {},
            c => cell.push(c),
        }
        if cell.len() > MAX_LINE || row.len() > 256 || rows.len() > MAX_RECORDS { return Err("Cursor CSV 超过读取上限，计数可能不完整".into()); }
    }
    if quote { return Err("Cursor CSV 引号未闭合，计数可能不完整".into()); }
    if !cell.is_empty() || !row.is_empty() { row.push(cell); rows.push(row); }
    check(cancel)?; Ok(rows)
}
pub(super) fn reconcile_cursor(files: Vec<Vec<SpendRecord>>, notes: &mut Vec<String>, cancel: &AtomicBool) -> Result<Vec<SpendRecord>,String> {
    let mut scopes: BTreeMap<String,(Vec<Vec<SpendRecord>>,Vec<Vec<SpendRecord>>)> = BTreeMap::new();
    for file in files {
        check(cancel)?; let Some(first) = file.first() else { continue; };
        let lanes = scopes.entry(first.source_scope.clone().unwrap_or_else(|| "import".into())).or_default();
        if first.aggregate_timing { lanes.1.push(file); } else { lanes.0.push(file); }
    }
    let mut output = Vec::new();
    for (scope,(json,csv)) in scopes {
        check(cancel)?; let (mut json, json_overlap) = multiset(json,cancel)?; let (csv,csv_overlap) = multiset(csv,cancel)?;
        let mut partial = json_overlap || csv_overlap || scope == "import";
        let range = json.iter().filter_map(|r|r.source_timestamp).min().zip(json.iter().filter_map(|r|r.source_timestamp).max());
        if let Some((start,end)) = range {
            for record in csv { check(cancel)?; let Some(at) = record.source_timestamp else { continue; };
                if at < start || at > end { json.push(record); } else { partial = true; }
            }
            output.extend(json);
        } else { output.extend(csv); }
        if partial { notes.push(format!("Cursor（{scope}）：导出未声明账号或存在无法确认的重叠；按每个文件实测重复次数的最大值合并，JSON 时间范围内优先 JSON，计数可能不完整")); }
    }
    check(cancel)?; Ok(output)
}
fn multiset(files: Vec<Vec<SpendRecord>>, cancel: &AtomicBool) -> Result<(Vec<SpendRecord>,bool),String> {
    let mut order = Vec::new(); let mut representative = HashMap::new(); let mut maximum: HashMap<String,usize> = HashMap::new(); let mut totals: HashMap<String,usize> = HashMap::new();
    for file in files {
        check(cancel)?; let mut counts: HashMap<String,usize> = HashMap::new();
        for record in file { check(cancel)?;
            let key = serde_json::to_string(&(&record.session,record.source_timestamp,&record.model,record.tally.input,record.tally.output,record.tally.cache_write,record.tally.cache_read)).map_err(|e| e.to_string())?;
            *counts.entry(key.clone()).or_default() += 1;
            if !representative.contains_key(&key) { order.push(key.clone()); representative.insert(key,record); }
        }
        for (key,count) in counts { let max = maximum.entry(key.clone()).or_default(); *max = (*max).max(count); *totals.entry(key).or_default() += count; }
    }
    let mut records = Vec::new(); let mut overlap = false;
    for key in order { check(cancel)?; let count = maximum[&key]; if totals[&key] > count { overlap = true; }
        for _ in 0..count { records.push(representative[&key].clone()); }
    }
    Ok((records,overlap))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::io::Cursor;
    struct Fixture { root: PathBuf }
    impl Fixture {
        fn new() -> Self {
            let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../test-results").join(format!("reader-fixture-{}-{}",std::process::id(),NEXT_ID.fetch_add(1,Ordering::Relaxed)));
            fs::create_dir_all(root.join("logs")).unwrap(); Self { root }
        }
        fn write(&self, name: &str, bytes: impl AsRef<[u8]>) -> PathBuf {
            let file = self.root.join("logs").join(name); fs::create_dir_all(file.parent().unwrap()).unwrap(); fs::write(&file,bytes).unwrap(); file
        }
        fn scan(&self, agent: &str) -> SpendSnapshot {
            scan_roots(&self.root.join("cache"), &Prices::new(), &AtomicBool::new(false), |_,_| {}, |id| if id == agent { vec![self.root.join("logs")] } else { vec![] }).unwrap()
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let base = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../test-results").canonicalize();
            if let (Ok(base),Ok(target)) = (base,self.root.canonicalize()) { if target.starts_with(base) { let _ = fs::remove_dir_all(target); } }
        }
    }
    fn encoded(lines: Vec<Value>) -> String { lines.iter().map(Value::to_string).collect::<Vec<_>>().join("\n") }
    fn gemini_message(id: &str) -> Value {
        json!({"type":"gemini","id":id,"model":"gemini-test","timestamp":"2026-10-03T12:00:00Z","tokens":{"prompt":100,"candidates":20,"cached":30,"thoughts":5,"tool":10,"total":135},"content":"SECRET GEMINI BODY","thoughts":"SECRET THOUGHT BODY"})
    }
    fn cursor_event(input: u64, conversation: Option<&str>) -> Value {
        json!({"model":"cursor-test","timestamp":1791028800000i64,"conversationId":conversation,"tokenUsage":{"inputTokens":input,"cacheWriteTokens":2,"cacheReadTokens":3,"outputTokens":4},"chargedCents":999999,"totalCents":999999})
    }
    #[test] fn gemini_real_roots_and_three_shape_candidate_rules() {
        let home = Path::new("fixture-home");
        assert_eq!(super::super::roots(home,"gemini", |_| None),vec![home.join(".gemini/tmp")]);
        assert_eq!(super::super::roots(home,"gemini", |k| (k == "GEMINI_CLI_HOME").then(|| "~/custom".into())),vec![home.join("custom/tmp")]);
        assert!(candidate(Path::new("tmp/workspace/chats/arbitrary.json"),"gemini"));
        assert!(candidate(Path::new("old/session-uuid.json"),"gemini"));
        assert!(candidate(Path::new("headless.jsonl"),"gemini"));
        assert!(!candidate(Path::new("tmp/workspace/config.json"),"gemini"));
        assert!(!candidate(Path::new("usage.backup.account.json"),"cursor"));
    }
    #[test] fn capture_roots_are_documented_cache_and_windows_explicit_imports() {
        let home = Path::new("fixture-home");
        let env = |key: &str| match key { "APPDATA" => Some("fixture-appdata".into()), "TOKSCALE_CONFIG_DIR" => Some("fixture-cache".into()), "HINDSIGHT_HOME" => Some("fixture-hindsight".into()), _ => None };
        assert_eq!(roots(home,"cursor",&env),vec![PathBuf::from("fixture-cache/cursor-cache"),PathBuf::from("fixture-appdata/PulseWin/UsageImports/cursor")]);
        assert_eq!(roots(home,"antigravity",&env)[0],PathBuf::from("fixture-cache/antigravity-cache/sessions"));
        assert_eq!(roots(home,"mcode",&env)[0],PathBuf::from("fixture-cache/headless/mcode"));
        assert_eq!(roots(home,"hindsight",&env)[0],PathBuf::from("fixture-hindsight/usage"));
        assert!(roots(home,"warp",&env).is_empty());
        assert_eq!(AGENTS.len(),11);
    }
    #[test] fn gemini_session_cache_overlap_tool_and_reasoning_are_disjoint() {
        let fixture = Fixture::new();
        let file = fixture.write("tmp/work/chats/new-name.json",json!({"sessionId":"real-session","messages":[{"type":"user","content":"SECRET USER BODY"},gemini_message("a")]}).to_string());
        let (records,notes) = parse_file(&file,"gemini",&AtomicBool::new(false)).unwrap();
        assert!(notes.is_empty()); assert_eq!(records.len(),1);
        assert_eq!(records[0].tally,Tally { input:80,output:25,cache_read:30,cache_write:0 });
        assert_eq!(records[0].session,"real-session"); assert!(!serde_json::to_string(&records).unwrap().contains("SECRET"));
    }
    #[test] fn gemini_session_without_total_prompt_and_net_input_differ() {
        assert_eq!(gemini_usage(&json!({"prompt":100,"cached":30,"candidates":20}),false,false).0.input,70);
        assert_eq!(gemini_usage(&json!({"input":100,"cached":30,"output":20}),false,false).0.input,100);
        assert_eq!(gemini_usage(&json!({"total":77}),false,false),(Tally::default(),77));
        assert_eq!(gemini_usage(&json!({"input":100,"tool":10,"output":20,"cached":30,"total":160}),false,false).0.input,110);
    }
    #[test] fn measured_counter_conversion_never_turns_fractions_bools_or_overflow_into_tokens() {
        for value in [json!(1.5),json!(-1),json!(true),json!("1.5"),json!("NaN"),json!(u64::MAX)] { assert_eq!(count_value(&value),None); }
        assert_eq!(count_value(&json!(12.0)),Some(12)); assert_eq!(count_value(&json!(" 12 ")),Some(12));
    }
    #[test] fn invalid_json_metadata_does_not_hide_valid_later_usage_or_time_fallback() {
        let fixture = Fixture::new(); let mut message = gemini_message("valid");
        message["timestamp"] = json!("invalid"); message["created_at"] = json!("2026-10-03T12:00:00Z");
        let file = fixture.write("session-fallback.json",json!({"sessionId":" ","session_id":" stated-session ","messages":[{"type":"gemini","model":42,"tokens":{"input":10},"timestamp":"2026-10-03T12:00:00Z"},message]}).to_string());
        let (records,notes) = parse_file(&file,"gemini",&AtomicBool::new(false)).unwrap();
        assert_eq!(records.len(),1); assert_eq!(records[0].session,"stated-session"); assert!(!notes.is_empty());
        let mut bad = cursor_event(99,None); bad["model"] = json!(42);
        let file = fixture.write("usage.valid.json",json!({"usageEventsDisplay":[bad,cursor_event(10,None)]}).to_string());
        let (records,notes) = cursor_file(&file,&AtomicBool::new(false)).unwrap();
        assert_eq!(records.len(),1); assert_eq!(records[0].tally.input,10); assert!(!notes.is_empty());
    }
    #[test] fn gemini_headless_init_stats_models_and_id_restatements() {
        let rows = vec![json!({"type":"init","model":"gemini-init","session_id":"headless-session"}),
            json!({"id":"request","timestamp":"2026-10-03T12:00:00Z","tokens":{"input":100,"output":20,"cached":30}}),
            json!({"id":"request","timestamp":"2026-10-03T12:00:00Z","tokens":{"input":140,"output":25,"cached":40}}),
            json!({"id":"stats","timestamp":"2026-10-03T12:01:00Z","result":{"stats":{"models":{"another-model":{"input":10,"output":2,"cached":3},"bare-model":{"total":17}}}}})];
        let (records,notes) = gemini_headless(&mut Cursor::new(encoded(rows)),"capture",&AtomicBool::new(false)).unwrap();
        assert!(notes.is_empty()); assert_eq!(records.len(),3);
        assert_eq!(records[0].tally,Tally { input:100,output:25,cache_read:40,cache_write:0 });
        assert!(records.iter().any(|r|r.model=="another-model" && r.tally.input==10));
        assert!(records.iter().any(|r|r.model=="bare-model" && r.unclassified_tokens==17 && r.tally.total()==0));
        assert!(records.iter().all(|r|r.session=="headless-session"));
    }
    #[test] fn gemini_cross_file_mirrors_cache_and_reprice_safely() {
        let fixture = Fixture::new(); let bytes = json!({"sessionId":"same-session","messages":[gemini_message("same")]}).to_string();
        fixture.write("session-a.json",&bytes); fixture.write("session-b.json",&bytes);
        let first = fixture.scan("gemini"); assert_eq!(first.records.len(),1); assert_eq!(first.records[0].tally.total(),135);
        let second = fixture.scan("gemini"); assert_eq!(second.sources[3].cached_files,2); assert_eq!(second.records.len(),1);
        assert_eq!(second.sources[3].origin,"native"); assert_eq!(second.sources[3].status,"counted");
        let prices = parse_prices(&json!({"google":{"models":{"gemini-test":{"cost":{"input":1,"output":2,"cache_read":0.1}}}}}));
        let priced = scan_roots(&fixture.root.join("cache"),&prices,&AtomicBool::new(false),|_,_| {},|id| if id=="gemini" {vec![fixture.root.join("logs")]} else {vec![]}).unwrap();
        assert!((priced.records[0].cost.unwrap()-0.000133).abs()<1e-10);
        for entry in fs::read_dir(fixture.root.join("cache")).unwrap() { assert!(!fs::read_to_string(entry.unwrap().path()).unwrap().contains("SECRET")); }
    }
    #[test] fn cursor_json_scope_multiset_preserves_in_file_multiplicity() {
        let fixture = Fixture::new(); let a = cursor_event(10,None); let b = cursor_event(20,Some("conversation"));
        fixture.write("usage.alice.json",json!({"usageEventsDisplay":[a.clone(),a.clone()]}).to_string());
        fixture.write("usage.alice.more.json",json!({"usageEventsDisplay":[a.clone(),b.clone()]}).to_string());
        let result = fixture.scan("cursor");
        assert_eq!(result.records.len(),3); assert_eq!(result.records.iter().map(|r|r.tally.total()).sum::<u64>(),67);
        assert_eq!(result.records.iter().filter(|r|r.session.is_empty()).count(),2);
        assert!(result.records.iter().any(|r|r.session=="cursor:alice:conversation"));
        assert!(!result.notes.is_empty()); assert_eq!(result.sources[4].origin,"export");
        let again = fixture.scan("cursor"); assert_eq!(again.records.len(),3); assert_eq!(again.sources[4].cached_files,2);
    }
    #[test] fn cursor_distinct_accounts_and_import_names_have_correct_scope() {
        let fixture = Fixture::new(); let export = json!({"usageEventsDisplay":[cursor_event(10,Some("same"))]}).to_string();
        fixture.write("usage.alice.json",&export); fixture.write("usage.bob.json",&export);
        assert_eq!(fixture.scan("cursor").records.len(),2);
        let imported = Fixture::new(); imported.write("export-a.json",&export); imported.write("export-b.json",&export);
        let result = imported.scan("cursor"); assert_eq!(result.records.len(),1); assert!(result.notes.iter().any(|s|s.contains("import")));
    }
    #[test] fn cursor_csv_proves_headers_quotes_independent_buckets_and_aggregate_timing() {
        let fixture = Fixture::new();
        let csv = "\u{feff}Cost,Output Tokens,Date,Input (w/ Cache Write),Model,Cache Read,Input (w/o Cache Write),Cloud Agent ID,Total Tokens\r\n999,20,2026-10-03,30,\"model,one\",40,\"1,000\",cloud-1,999999\r\n";
        let file = fixture.write("usage.alice.csv",csv);
        let (records,notes) = cursor_file(&file,&AtomicBool::new(false)).unwrap();
        assert!(notes.is_empty()); assert_eq!(records.len(),1); assert_eq!(records[0].model,"model,one");
        assert_eq!(records[0].tally,Tally { input:1000,output:20,cache_write:30,cache_read:40 });
        assert!(records[0].aggregate_timing); assert_eq!(records[0].day,"2026-10-03"); assert_eq!(records[0].session,"cursor:alice:cloud:cloud-1");
        assert!(records[0].cost.is_none());
        let unrelated = fixture.write("unknown.csv","Date,Model,Total Tokens\n2026-10-03,m,999\n");
        assert!(cursor_file(&unrelated,&AtomicBool::new(false)).unwrap().0.is_empty());
    }
    #[test] fn cursor_json_is_authoritative_for_csv_overlap_but_empty_json_does_not_hide_csv() {
        let fixture = Fixture::new();
        fixture.write("usage.alice.json",json!({"usageEventsDisplay":[cursor_event(10,None)]}).to_string());
        fixture.write("usage.alice.csv","Date,Model,Input (w/o Cache Write),Input (w/ Cache Write),Cache Read,Output Tokens\n2026-10-03T12:00:00Z,cursor-test,100,2,3,4\n");
        let result = fixture.scan("cursor"); assert_eq!(result.records.len(),1); assert_eq!(result.records[0].tally.input,10);
        assert!(!result.notes.is_empty()); assert!(!result.records[0].aggregate_timing);
        fixture.write("usage.alice.json",json!({"usageEventsDisplay":[]}).to_string());
        let changed = fixture.scan("cursor"); assert_eq!(changed.records.len(),1); assert_eq!(changed.records[0].tally.input,100); assert!(changed.records[0].aggregate_timing);
    }
    #[test] fn antigravity_fallback_partial_reasoning_and_placeholder_skip() {
        let rows = vec![json!({"type":"session_meta","modelId":"real-model"}),
            json!({"type":"usage","sessionId":"s","timestamp":1791028800000i64,"input":10,"output":2,"cacheRead":3,"cacheWrite":4,"reasoning":5,"responseId":"r"}),
            json!({"type":"usage","modelId":"MODEL_PLACEHOLDER_123","sessionId":"s","timestamp":1791028800000i64,"input":99})];
        let (records,notes) = antigravity(&mut Cursor::new(encoded(rows)),&AtomicBool::new(false)).unwrap();
        assert_eq!(records.len(),1); assert_eq!(records[0].model,"real-model"); assert_eq!(records[0].tally.output,2); assert_eq!(records[0].tally.total(),19);
        assert!(notes.iter().any(|s|s.contains("reasoning"))); assert_eq!(records[0].deduplication_id.as_deref(),Some("antigravity:r"));
    }
    #[test] fn hindsight_requires_stated_id_and_preserves_output_cache_and_bank_only() {
        let rows = vec![json!({"id":"request","model":"hindsight-model","started_at":"2026-10-03T12:00:00Z","input_tokens":10,"output_tokens":2,"cached_tokens":3,"reasoning_tokens":999,"trace_id":"trace","bank":"bank","prompt":"SECRET PROMPT","completion":"SECRET COMPLETION"}),
            json!({"model":"missing-id","started_at":"2026-10-03T12:00:00Z","input_tokens":999}),
            json!({"id":"zero-total","model":"m","started_at":"2026-10-03T12:00:00Z","total_tokens":0,"input_tokens":999})];
        let (records,notes) = hindsight(&mut Cursor::new(encoded(rows)),&AtomicBool::new(false)).unwrap();
        assert_eq!(records.len(),1); assert_eq!(records[0].tally.total(),15); assert_eq!(records[0].session,"trace"); assert_eq!(records[0].project.as_deref(),Some("bank"));
        assert!(!notes.is_empty()); assert!(!serde_json::to_string(&records).unwrap().contains("SECRET"));
    }
    fn mcode_message(id: Option<&str>, usage: Value) -> Value { json!({"type":"message","message":{"role":"assistant","turnId":"turn","id":id,"timestamp":1791028800,"usage":usage,"content":"SECRET MCODE BODY"}}) }
    fn mcode_result() -> Value { json!({"type":"exec.result","sessionId":"session","turnId":"turn","model":{"providerId":"minimax","modelId":"mcode-model"}}) }
    #[test] fn mcode_turn_buffer_replaces_same_identity_but_preserves_equal_distinct_messages() {
        let rows = vec![mcode_message(Some("same"),json!({"inputTokens":10,"outputTokens":2})),mcode_message(Some("same"),json!({"inputTokens":20,"outputTokens":3})),mcode_message(None,json!({"inputTokens":20,"outputTokens":3})),mcode_message(None,json!({"inputTokens":20,"outputTokens":3})),mcode_result()];
        let (records,notes) = mcode(&mut Cursor::new(encoded(rows)),&AtomicBool::new(false)).unwrap();
        assert!(notes.is_empty()); assert_eq!(records.len(),3); assert_eq!(records.iter().map(|r|r.tally.total()).sum::<u64>(),69);
        assert!(records.iter().all(|r|r.model=="mcode-model" && r.session=="session"));
        assert!(!serde_json::to_string(&records).unwrap().contains("SECRET"));
    }
    #[test] fn mcode_requires_result_and_keeps_bare_total_unclassified() {
        let row = mcode_message(None,json!({"totalTokens":91}));
        let (none,notes) = mcode(&mut Cursor::new(encoded(vec![row.clone()])),&AtomicBool::new(false)).unwrap();
        assert!(none.is_empty()); assert!(!notes.is_empty());
        let (records,notes) = mcode(&mut Cursor::new(encoded(vec![row,mcode_result()])),&AtomicBool::new(false)).unwrap();
        assert!(notes.is_empty()); assert_eq!(records[0].tally.total(),0); assert_eq!(records[0].unclassified_tokens,91); assert!(records[0].cost.is_none());
    }
    #[test] fn captured_jsonl_replay_dedup_survives_cache() {
        let fixture = Fixture::new(); let row = json!({"id":"request","model":"m","started_at":"2026-10-03T12:00:00Z","input_tokens":10,"output_tokens":2}).to_string();
        fixture.write("ledger-a.jsonl",&row); fixture.write("ledger-b.jsonl",&row);
        assert_eq!(fixture.scan("hindsight").records.len(),1);
        let again = fixture.scan("hindsight"); assert_eq!(again.records.len(),1); assert_eq!(again.sources[6].cached_files,2);
        let mcode_fixture = Fixture::new(); let bytes = encoded(vec![mcode_message(Some("a"),json!({"inputTokens":10})),mcode_result()]);
        mcode_fixture.write("capture-a.jsonl",&bytes); mcode_fixture.write("capture-b.jsonl",&bytes);
        assert_eq!(mcode_fixture.scan("mcode").records.len(),1); assert_eq!(mcode_fixture.scan("mcode").sources[7].cached_files,2);
    }
    #[test] fn bad_line_and_bom_do_not_hide_later_mcode_turn_and_partial_is_not_cached() {
        let fixture = Fixture::new(); let bytes = format!("\u{feff}{}\n{{malformed}}\n{}",mcode_message(Some("a"),json!({"inputTokens":10})),mcode_result());
        fixture.write("capture.jsonl",bytes); let scan = fixture.scan("mcode");
        assert_eq!(scan.records.len(),1); assert!(!scan.notes.is_empty()); assert!(!fixture.root.join("cache").exists());
    }
    #[test] fn cancellation_after_first_parsed_line_discards_all_counts() {
        struct Cancel<'a> { inner: Cursor<Vec<u8>>, cancel: &'a AtomicBool, lines: usize }
        impl Read for Cancel<'_> { fn read(&mut self,buf:&mut [u8])->std::io::Result<usize> { self.inner.read(buf) } }
        impl BufRead for Cancel<'_> {
            fn fill_buf(&mut self)->std::io::Result<&[u8]> { self.inner.fill_buf() }
            fn consume(&mut self,count:usize) { self.inner.consume(count); self.lines+=1; if self.lines==2 {self.cancel.store(true,Ordering::Release);} }
        }
        let cancel = AtomicBool::new(false);
        let bytes = encoded(vec![json!({"type":"init","model":"m","session_id":"s"}),json!({"timestamp":"2026-10-03T12:00:00Z","tokens":{"input":10}})]).into_bytes();
        let mut reader = Cancel { inner:Cursor::new(bytes), cancel:&cancel, lines:0 };
        assert_eq!(gemini_headless(&mut reader,"fixture",&cancel).unwrap_err(),"cancelled");
        assert_eq!(reader.lines,2);
    }
    #[test] fn oversize_captured_line_is_bounded_and_later_usage_remains_visible() {
        let mut bytes = vec![b'x';MAX_LINE+1]; bytes.push(b'\n');
        bytes.extend(encoded(vec![mcode_message(Some("a"),json!({"inputTokens":10})),mcode_result()]).as_bytes());
        let (records,notes) = mcode(&mut BufReader::with_capacity(64,Cursor::new(bytes)),&AtomicBool::new(false)).unwrap();
        assert_eq!(records.len(),1); assert!(notes.iter().any(|s|s.contains("8 MiB")));
    }
}
