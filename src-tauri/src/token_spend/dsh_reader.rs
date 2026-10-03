//! DeepSeek Harness durable provider counters, never conversation text.
//! Pulse DSHUsageReader plus current Harness token-meter settlement semantics.
//! Bundled zstd handles concatenated checkpoint frames without external tools.
use super::*;
use std::io::Read;

const MAX_RAW: u64 = 64 * 1024 * 1024;
// Real long-running Harness logs exceed 64 MiB after decompression. Stream
// them rather than retaining a complete transcript; each line stays bounded.
const MAX_DECODED: usize = 512 * 1024 * 1024;
const MAX_SAFE: u64 = 9_007_199_254_740_991;

pub(super) fn roots(home: &Path, env: &impl Fn(&str) -> Option<String>) -> Vec<PathBuf> {
    let base = env("DSH_HOME").filter(|s| !s.trim().is_empty()).map(|s| {
        let s = s.trim();
        if s == "~" { home.to_path_buf() }
        else if let Some(rest) = s.strip_prefix("~/").or_else(|| s.strip_prefix("~\\")) { home.join(rest) }
        else { PathBuf::from(s) }
    }).unwrap_or_else(|| home.join(".dsh"));
    // Current desktop and CLI share this home. Never walk backups, profiles,
    // credentials, attachments or arbitrary export directories.
    vec![base.join("sessions")]
}
pub(super) fn candidate(path: &Path) -> bool {
    let name = path.file_name().unwrap_or_default().to_string_lossy();
    let name = name.strip_suffix(".zstd").unwrap_or(&name);
    if name == "session.jsonl" { return true; }
    name.strip_prefix("session.v").and_then(|n| n.strip_suffix(".jsonl"))
        .is_some_and(|version| !version.is_empty() && version.bytes().all(|c| c.is_ascii_digit()))
}
fn integer(value: &Value) -> Option<u64> { value.as_u64().filter(|n| *n <= MAX_SAFE) }
fn text(value: &Value) -> Option<&str> { value.as_str().map(str::trim).filter(|s| !s.is_empty()) }
fn tally(usage: &Value) -> Option<Tally> {
    let optional = |key| if usage.get(key).is_none() { Some(0) } else { integer(&usage[key]) };
    let result = Tally { input: integer(&usage["inputTokens"])?, output: integer(&usage["outputTokens"])?,
        cache_read: optional("cacheReadTokens")?, cache_write: optional("cacheWriteTokens")? };
    if optional("reasoningTokens")? > result.output { return None; }
    // Provider total includes the disjoint cache buckets. Reasoning is already
    // part of output. Reject inconsistent totals rather than fabricate costs.
    if usage.get("totalTokens").is_some() && integer(&usage["totalTokens"])? != result.total() { return None; }
    Some(result)
}
fn stream_usage(data: &Value) -> Option<&Value> {
    data["stream"].as_array()?.iter().rev().find_map(|record| {
        let chunk = if record["type"] == "chunk" { &record["chunk"] } else { record };
        (chunk["type"] == "usage").then_some(&chunk["usage"])
    })
}

pub(super) fn read(path: &Path, cancel: &AtomicBool) -> Result<(Vec<SpendRecord>, Vec<String>), String> {
    check(cancel)?;
    let file = File::open(path).map_err(|_| format!("DeepSeek Harness：无法读取 {}", path.display()))?;
    if file.metadata().map_err(|e| e.to_string())?.len() > MAX_RAW {
        return Err("DeepSeek Harness：文件超过 64 MiB，计数可能不完整".into());
    }
    let mut source = BufReader::with_capacity(64 * 1024, file.take(MAX_RAW + 1));
    let compressed = source.fill_buf().map_err(|e| e.to_string())?.starts_with(&[0x28, 0xb5, 0x2f, 0xfd]);
    let mut reader: Box<dyn BufRead> = if compressed {
        let mut decoder = zstd::stream::read::Decoder::with_buffer(source).map_err(|_| "DeepSeek Harness：无法初始化压缩解码器")?;
        decoder.window_log_max(26).map_err(|_| "DeepSeek Harness：无法限制压缩窗口")?;
        Box::new(BufReader::with_capacity(64 * 1024, decoder))
    } else { Box::new(source) };
    parse(&mut reader, path, cancel)
}

fn parse(reader: &mut dyn BufRead, path: &Path, cancel: &AtomicBool) -> Result<(Vec<SpendRecord>, Vec<String>), String> {
    let mut notes = Vec::new();
    let mut session = path.parent().and_then(Path::file_name).unwrap_or_default().to_string_lossy().to_string();
    let mut project = None; let mut header_model = String::new(); let mut header_provider = String::new();
    let mut seed_cut = 0; let mut seeded = false; let mut has_cut = false; let mut has_header = false;
    let mut records: Vec<(u64, SpendRecord)> = Vec::new();
    let mut last: Option<(u64, u64, usize)> = None;
    let mut decoded = 0usize;
    loop {
        check(cancel)?;
        let mut line = Vec::new(); let mut oversize = false; let mut ended = false; let mut eof = false;
        loop {
            check(cancel)?;
            let chunk = match reader.fill_buf() {
                Ok(chunk) => chunk,
                Err(error) if error.kind() == std::io::ErrorKind::UnexpectedEof => {
                    notes.push("DeepSeek Harness：压缩尾帧尚未写完；仅统计完整记录，请稍后重新扫描".into()); eof = true; break;
                }
                Err(_) => return Err("DeepSeek Harness：压缩或文件读取失败，未计入损坏文件".into()),
            };
            if chunk.is_empty() { eof = true; break; }
            let size = chunk.iter().position(|b| *b == b'\n').map(|n| n + 1).unwrap_or(chunk.len());
            decoded += size;
            if decoded > MAX_DECODED { return Err("DeepSeek Harness：解压后超过 512 MiB，计数可能不完整".into()); }
            ended = chunk[size - 1] == b'\n';
            if !oversize {
                if line.len() + size <= MAX_LINE { line.extend_from_slice(&chunk[..size]); }
                else { line.clear(); oversize = true; }
            }
            reader.consume(size);
            if ended { break; }
        }
        if oversize { notes.push("DeepSeek Harness：一条记录超过 8 MiB，计数可能不完整".into()); }
        if line.is_empty() { if eof { break; } continue; }
        // Ignore unrelated transcript lines before JSON parsing. No raw message
        // or stream body survives this iteration or enters our normalized cache.
        if ![b"\"session\"".as_slice(), b"\"request/header\"", b"\"session/end-seed\"", b"\"llm/retry-started\"",
             b"\"assistant/message\"", b"\"assistant/attempt\"", b"\"compaction/summary\""].iter()
            .any(|key| line.windows(key.len()).any(|window| window == *key)) { if eof { break; } continue; }
        let event: Value = match serde_json::from_slice(&line) {
            Ok(event) => event,
            Err(_) => { notes.push(if eof && !ended { "DeepSeek Harness：尾部记录尚未写完，请稍后重新扫描" } else { "DeepSeek Harness：无法解码一条记录，计数可能不完整" }.into()); if eof { break; } continue; }
        };
        let typ = event["type"].as_str().unwrap_or(""); let data = &event["data"];
        if typ == "session" {
            if has_header { return Err("DeepSeek Harness：会话头重复，未计入文件".into()); }
            has_header = true;
            if !integer(&event["version"]).is_some_and(|v| v <= 4) { return Err("DeepSeek Harness：尚不支持此会话格式版本".into()); }
            if let Some(id) = text(&event["id"]) { session = id.into(); }
            project = text(&event["cwd"]).map(str::to_owned);
            has_cut = event.get("seedLength").is_some();
            seed_cut = if has_cut { integer(&event["seedLength"]).ok_or("DeepSeek Harness：继承长度无效")? } else { 0 };
            seeded = event["isSeeded"] == true;
            continue;
        }
        if !has_header { return Err("DeepSeek Harness：缺少会话头，未计入文件".into()); }
        if typ == "request/header" {
            header_model = text(&data["header"]["config"]["model"]).unwrap_or("").into();
            header_provider = text(&data["header"]["config"]["provider"]).unwrap_or("").into();
            continue;
        }
        if typ == "session/end-seed" {
            if data["inherited"] == true {
                if !seeded { return Err("DeepSeek Harness：分叉标记与会话头不一致".into()); }
                seed_cut = integer(&event["seq"]).ok_or("DeepSeek Harness：分叉边界缺少有效序号")?;
                has_cut = true;
            }
            last = None; continue;
        }
        let step_key = integer(&data["turn"]).zip(integer(&data["step"]));
        if typ == "llm/retry-started" {
            if last.is_some_and(|(turn, step, _)| Some((turn, step)) == step_key) { last = None; }
            continue;
        }
        let assistant = matches!(typ, "assistant/message" | "assistant/attempt");
        if !assistant && typ != "compaction/summary" { continue; }
        let usage = if typ == "assistant/attempt" { stream_usage(data) } else { data.get("usage").or_else(|| stream_usage(data)) };
        let Some(usage) = usage else { continue; };
        let Some(tally) = tally(usage) else { notes.push("DeepSeek Harness：用量分类无效或总量不一致，计数可能不完整".into()); continue; };
        let seq = integer(&event["seq"]);
        let timestamp = integer(&event["time"]).and_then(|ms| DateTime::from_timestamp_millis(ms as i64));
        let source = &data["message"]["source"];
        let model = text(&source["replayState"]["response"]["responseModel"]).or_else(|| text(&source["model"]))
            .or_else(|| text(&data["model"])).or_else(|| (!header_model.is_empty()).then_some(header_model.as_str()));
        let provider = text(&source["provider"]).or_else(|| text(&data["provider"])).unwrap_or(&header_provider);
        let (Some(seq), Some(at), Some(model)) = (seq, timestamp, model) else {
            if tally.total() > 0 { notes.push("DeepSeek Harness：实测用量缺少有效模型、时间或序号，计数可能不完整".into()); } continue;
        };
        if assistant && step_key.is_none() { notes.push("DeepSeek Harness：请求缺少 turn/step，计数可能不完整".into()); continue; }
        // Message/compaction IDs survive physical log migrations. The fallback
        // uses call coordinates plus timestamp, NOT physical sequence numbers.
        let identity = if let Some(id) = text(&data["message"]["id"]) { format!("msg:{id}") }
            else if let Some(id) = text(&data["compactionId"]) { format!("summary:{id}") }
            else if let Some((turn, step)) = step_key { format!("attempt:{turn}:{step}") }
            else { format!("summary:seq:{seq}") };
        let id = format!("dsh:{identity}:{}:{provider}:{model}:{}:{}:{}:{}", at.timestamp_millis(), tally.input, tally.output, tally.cache_read, tally.cache_write);
        let local = at.with_timezone(&Local);
        let record = SpendRecord { agent: "dsh".into(), model: model.into(), day: local.format("%Y-%m-%d").to_string(),
            hour: chrono::Timelike::hour(&local), session: session.clone(), project: project.clone(), tally,
            unclassified_tokens: 0, deduplication_id: Some(id), aggregate_timing: false, source_timestamp: Some(at.timestamp_millis()),
            source_scope: None, cost: None, cost_breakdown: None, model_name: None };
        let replacement = if assistant { last.filter(|(turn, step, _)| Some((*turn, *step)) == step_key).map(|(_, _, i)| i) } else { None };
        let index = if let Some(index) = replacement { records[index] = (seq, record); index }
            else { records.push((seq, record)); records.len() - 1 };
        if assistant { let (turn, step) = step_key.unwrap(); last = Some((turn, step, index)); }
        if records.len() >= MAX_RECORDS { return Err("DeepSeek Harness：达到记录上限，计数可能不完整".into()); }
        if eof { break; }
    }
    check(cancel)?;
    if !has_header { return Err("DeepSeek Harness：缺少会话头，未计入文件".into()); }
    if seeded && !has_cut { return Err("DeepSeek Harness：分叉会话缺少继承边界，未计入以避免重复统计".into()); }
    let mut seen = HashSet::new();
    let records = records.into_iter().filter(|(seq, record)| *seq >= seed_cut && record.tally.total() > 0
        && seen.insert(record.deduplication_id.clone())).map(|(_, record)| record).collect();
    notes.sort(); notes.dedup(); Ok((records, notes))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::io::Cursor;

    fn header(version: u64) -> Value { json!({"type":"session","version":version,"id":"fixture-session","cwd":"E:/synthetic-project"}) }
    fn message(seq: u64) -> Value { json!({"type":"assistant/message","seq":seq,"time":1_790_000_000_000u64+seq,
        "data":{"turn":1,"step":seq,"message":{"id":format!("message-{seq}"),"source":{"provider":"deepseek","model":"deepseek-chat"}},
        "usage":{"inputTokens":100,"outputTokens":40,"cacheReadTokens":200,"cacheWriteTokens":10,"reasoningTokens":30,"totalTokens":350}}}) }
    fn bytes(events: &[Value]) -> Vec<u8> { events.iter().map(|e| format!("{e}\n")).collect::<String>().into_bytes() }
    fn parse_events(events: &[Value]) -> (Vec<SpendRecord>, Vec<String>) {
        parse(&mut Cursor::new(bytes(events)),Path::new("fixture/session.jsonl"),&AtomicBool::new(false)).unwrap()
    }
    struct Fixture { dir: PathBuf }
    impl Fixture {
        fn new() -> Self {
            let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../.test-data/dsh-fixtures").join(format!("pulsewin-dsh-{}-{}",std::process::id(),NEXT_ID.fetch_add(1,Ordering::Relaxed)));
            fs::create_dir_all(&dir).unwrap(); Self { dir }
        }
        fn file(&self, name: &str, content: &[u8]) -> PathBuf { let path = self.dir.join(name); fs::write(&path,content).unwrap(); path }
        fn scan(&self, prices: &Prices) -> SpendSnapshot {
            scan_roots(&self.dir.join("cache"),prices,&AtomicBool::new(false),|_,_|{},|agent|if agent=="dsh" {vec![self.dir.clone()]}else{vec![]}).unwrap()
        }
    }
    impl Drop for Fixture { fn drop(&mut self) { let _=fs::remove_dir_all(&self.dir); } }

    #[test] fn roots_follow_shared_home_and_never_walk_profiles_or_backups() {
        let home=Path::new("E:/user");
        assert_eq!(roots(home,&|_|None),vec![home.join(".dsh/sessions")]);
        assert_eq!(roots(home,&|_|Some("  ".into())),roots(home,&|_|None));
        assert_eq!(roots(home,&|_|Some("~/harness".into())),vec![home.join("harness/sessions")]);
        assert_eq!(roots(home,&|_|Some("D:/custom".into())),vec![PathBuf::from("D:/custom/sessions")]);
    }
    #[test] fn only_native_session_filenames_are_candidates() {
        for name in ["session.jsonl","session.jsonl.zstd","session.v3.jsonl","session.v4.jsonl.zstd"] {assert!(candidate(Path::new(name)));}
        for name in ["session.jsonl.backup","session.vx.jsonl","session.v.jsonl","credentials.jsonl","capture.jsonl.zstd"] {assert!(!candidate(Path::new(name)));}
    }
    #[test] fn all_known_generations_keep_disjoint_cache_and_inclusive_reasoning() {
        for version in 0..=4 {
            let (records,notes)=parse_events(&[header(version),message(1)]);
            assert!(notes.is_empty());assert_eq!(records.len(),1);assert_eq!(records[0].tally.total(),350);
            assert_eq!(records[0].tally.output,40);assert_eq!(records[0].tally.input,100);assert_eq!(records[0].project.as_deref(),Some("E:/synthetic-project"));
            let cache=serde_json::to_string(&records).unwrap();assert!(!cache.contains("reasoningTokens"));assert!(!cache.contains("message-1\"}"));
        }
    }
    #[test] fn response_model_wins_then_source_then_request_header() {
        let mut call=message(1);call["data"]["message"]["source"]["replayState"]=json!({"response":{"responseModel":"response-model"}});
        assert_eq!(parse_events(&[header(4),call.clone()]).0[0].model,"response-model");
        call["data"]["message"]["source"]=Value::Null;
        let request=json!({"type":"request/header","seq":0,"data":{"header":{"config":{"model":"header-model","provider":"deepseek"}}}});
        assert_eq!(parse_events(&[header(4),request,call]).0[0].model,"header-model");
    }
    #[test] fn compact_stream_uses_last_sample_and_final_settlement_replaces_it() {
        let mut attempt=message(1);attempt["type"]=json!("assistant/attempt");attempt["data"]["usage"]=Value::Null;
        attempt["data"]["stream"]=json!([{"type":"chunk","chunk":{"type":"usage","usage":{"inputTokens":10,"outputTokens":5}}},
            {"type":"chunk","chunk":{"type":"usage","usage":{"inputTokens":20,"outputTokens":7}}}]);
        let mut final_call=message(2);final_call["data"]["step"]=json!(1);
        let (records,notes)=parse_events(&[header(4),attempt,final_call]);assert!(notes.is_empty());assert_eq!(records.len(),1);assert_eq!(records[0].tally.total(),350);
    }
    #[test] fn retry_closes_slot_and_failed_reported_attempt_is_billed_separately() {
        let mut attempt=message(1);attempt["type"]=json!("assistant/attempt");attempt["data"]["stream"]=json!([{"type":"chunk","chunk":{"type":"usage","usage":{"inputTokens":20,"outputTokens":7}}}]);
        let retry=json!({"type":"llm/retry-started","data":{"turn":1,"step":1}});
        let mut call=message(2);call["data"]["step"]=json!(1);
        let (records,notes)=parse_events(&[header(4),attempt,retry,call]);assert!(notes.is_empty());assert_eq!(records.len(),2);assert_eq!(records.iter().map(|r|r.tally.total()).sum::<u64>(),377);
    }
    #[test] fn old_seed_length_excludes_inherited_prefix() {
        let mut h=header(0);h["seedLength"]=json!(2);
        let (records,notes)=parse_events(&[h,message(1),message(2)]);assert!(notes.is_empty());assert_eq!(records.len(),1);assert!(records[0].deduplication_id.as_ref().unwrap().contains("message-2"));
    }
    #[test] fn last_tagged_marker_is_fork_cut_and_ordinary_resume_is_not_a_cut() {
        let mut h=header(4);h["isSeeded"]=json!(true);
        let marker=|seq,inherited|json!({"type":"session/end-seed","seq":seq,"data":if inherited {json!({"inherited":true})}else{json!({})}});
        let (records,notes)=parse_events(&[h,message(1),marker(2,true),message(3),marker(4,true),message(5),marker(6,false),message(7)]);
        assert!(notes.is_empty());assert_eq!(records.len(),2);assert!(records.iter().all(|r|r.source_timestamp.unwrap()>=1_790_000_000_005));
    }
    #[test] fn ambiguous_seed_and_future_version_fail_instead_of_double_counting() {
        let mut h=header(4);h["isSeeded"]=json!(true);
        for events in [vec![h,message(1)],vec![header(5),message(1)],vec![message(1)]] {
            assert!(parse(&mut Cursor::new(bytes(&events)),Path::new("fixture/session.jsonl"),&AtomicBool::new(false)).is_err());
        }
    }
    #[test] fn compaction_counts_actual_usage_not_shadowed_context() {
        let summary=json!({"type":"compaction/summary","seq":2,"time":1_790_000_000_002u64,"data":{"compactionId":"cmp-1","model":"summary-model",
            "usage":{"inputTokens":30,"outputTokens":2},"shadowedTokenCount":999999}});
        let context=json!({"type":"request/context","data":{"contextWindow":999999}});
        let (records,notes)=parse_events(&[header(4),message(1),summary,context]);assert!(notes.is_empty());assert_eq!(records.len(),2);
        assert_eq!(records.iter().map(|r|r.tally.total()).sum::<u64>(),382);
    }
    #[test] fn invalid_counters_models_and_times_are_explicit_partial_results() {
        for (key,value) in [("inputTokens",json!(-1)),("outputTokens",json!(1.5)),("cacheReadTokens",json!(true)),("reasoningTokens",json!(41)),("totalTokens",json!(1))] {
            let mut bad=message(2);bad["data"]["usage"][key]=value;
            let (records,notes)=parse_events(&[header(4),message(1),bad]);assert_eq!(records.len(),1);assert!(!notes.is_empty());
        }
        let mut bad=message(2);bad["time"]=json!("invalid");
        assert!(!parse_events(&[header(4),bad]).1.is_empty());
        let mut missing=message(2);missing["data"]["message"]["source"]=Value::Null;
        assert!(!parse_events(&[header(4),missing]).1.is_empty());
    }
    #[test] fn optional_cache_counters_can_be_missing_but_input_output_are_required() {
        assert_eq!(tally(&json!({"inputTokens":10,"outputTokens":5})).unwrap().total(),15);
        assert!(tally(&json!({"totalTokens":20})).is_none());
        assert!(tally(&json!({"inputTokens":MAX_SAFE+1,"outputTokens":0})).is_none());
    }
    #[test] fn plain_and_concatenated_zstd_are_detected_by_magic_not_suffix() {
        let f=Fixture::new();let events=vec![header(4),message(1),message(2)];
        let packed=events.iter().flat_map(|e|zstd::stream::encode_all(Cursor::new(bytes(&[e.clone()])),1).unwrap()).collect::<Vec<_>>();
        for (name,data) in [("session.v4.jsonl",packed),("session.jsonl.zstd",bytes(&events))] {
            let path=f.file(name,&data);let before=fs::read(&path).unwrap();
            let (records,notes)=read(&path,&AtomicBool::new(false)).unwrap();assert_eq!(records.len(),2);assert!(notes.is_empty());assert_eq!(fs::read(&path).unwrap(),before);
        }
    }
    #[test] fn torn_compressed_tail_keeps_complete_checkpoint_records_without_cache() {
        let f=Fixture::new();let mut content=zstd::stream::encode_all(Cursor::new(bytes(&[header(4),message(1)])),1).unwrap();
        let next=zstd::stream::encode_all(Cursor::new(bytes(&[message(2)])),1).unwrap();content.extend_from_slice(&next[..next.len()/2]);
        let path=f.file("session.v4.jsonl.zstd",&content);
        let (records,notes)=read(&path,&AtomicBool::new(false)).unwrap();assert_eq!(records.len(),1);assert!(!notes.is_empty());
        let first=f.scan(&Prices::new());let second=f.scan(&Prices::new());assert_eq!(first.records.len(),1);assert_eq!(second.sources.last().unwrap().cached_files,0);
    }
    #[test] fn malformed_line_is_not_cached_and_does_not_hide_later_records() {
        let f=Fixture::new();let mut content=bytes(&[header(4),message(1)]);content.extend_from_slice(b"{\"type\":\"assistant/message\",oops}\n");content.extend(bytes(&[message(2)]));
        f.file("session.jsonl",&content);let scan=f.scan(&Prices::new());assert_eq!(scan.records.len(),2);assert!(!scan.notes.is_empty());assert_eq!(f.scan(&Prices::new()).sources.last().unwrap().cached_files,0);
    }
    #[test] fn migration_copies_and_appended_records_reconcile_on_cache_hits() {
        let f=Fixture::new();let mut migrated=message(1);migrated["seq"]=json!(900);
        f.file("session.jsonl",&bytes(&[header(0),message(1)]));f.file("session.v4.jsonl.zstd",&zstd::stream::encode_all(Cursor::new(bytes(&[header(4),migrated])),1).unwrap());
        let prices=parse_prices(&json!({"deepseek":{"models":{"deepseek-chat":{"cost":{"input":1,"output":2,"cache_read":0.1,"cache_write":1}}}}}));
        let first=f.scan(&prices);assert!(first.notes.is_empty());assert_eq!(first.records.len(),1);assert!((first.records[0].cost.unwrap()-0.00021).abs()<1e-10);
        assert_eq!(f.scan(&prices).sources.last().unwrap().cached_files,2);
        f.file("session.jsonl",&bytes(&[header(0),message(1),message(2)]));
        let next=f.scan(&prices);assert_eq!(next.records.len(),2);assert_eq!(next.sources.last().unwrap().cached_files,1);
    }
    #[test] fn cancelling_precedes_io_and_no_content_enters_normalized_cache() {
        assert!(read(Path::new("missing/session.jsonl"),&AtomicBool::new(true)).is_err());
        let f=Fixture::new();let mut call=message(1);call["data"]["message"]["content"]=json!("PRIVATE_SENTINEL");call["data"]["stream"]=json!([{"type":"text","text":"PRIVATE_SENTINEL"}]);
        f.file("session.jsonl",&bytes(&[header(4),call]));let snapshot=f.scan(&Prices::new());assert_eq!(snapshot.records.len(),1);
        for entry in fs::read_dir(f.dir.join("cache")).unwrap() {assert!(!String::from_utf8(fs::read(entry.unwrap().path()).unwrap()).unwrap().contains("PRIVATE_SENTINEL"));}
    }
    #[test] fn oversized_raw_file_and_corrupt_frame_fail_with_visible_explanations() {
        let f=Fixture::new();let path=f.file("session.jsonl.zstd",&[0x28,0xb5,0x2f,0xfd,0xff,0xff,0xff,0xff]);
        assert!(read(&path,&AtomicBool::new(false)).is_err());
        File::create(&path).unwrap().set_len(MAX_RAW+1).unwrap();
        assert!(read(&path,&AtomicBool::new(false)).unwrap_err().contains("64 MiB"));
    }
    #[test] fn oversize_record_is_skipped_without_hiding_a_later_measured_call() {
        let mut content=bytes(&[header(4)]);content.extend(vec![b'x';MAX_LINE+1]);content.push(b'\n');content.extend(bytes(&[message(1)]));
        let (records,notes)=parse(&mut Cursor::new(content),Path::new("fixture/session.jsonl"),&AtomicBool::new(false)).unwrap();
        assert_eq!(records.len(),1);assert!(notes.iter().any(|note|note.contains("8 MiB")));
    }
    #[test] fn a_zero_final_sample_replaces_a_nonzero_interim_sample() {
        let mut zero=message(2);zero["data"]["step"]=json!(1);zero["data"]["usage"]=json!({"inputTokens":0,"outputTokens":0});
        let (records,notes)=parse_events(&[header(4),message(1),zero]);assert!(records.is_empty());assert!(notes.is_empty());
    }
    #[test] #[ignore = "explicit opt-in read-only verification of real Harness sessions"]
    fn verify_local_harness_read_only() {
        let home=PathBuf::from(std::env::var("PULSEWIN_DSH_VERIFY_HOME").expect("explicit verification home required"));
        let result=PathBuf::from(std::env::var("PULSEWIN_DSH_VERIFY_RESULT").expect("explicit private result required"));
        let f=Fixture::new();let root=home.join("sessions");let cancel=AtomicBool::new(false);let mut files=Vec::new();
        enumerate(&root,"dsh",&mut files,&cancel,&mut Vec::new()).unwrap();
        let before=files.iter().map(|p|(p.clone(),hash(fs::read(p).unwrap()))).collect::<Vec<_>>();
        let snapshot=scan_roots(&f.dir.join("cache"),&Prices::new(),&cancel,|_,_|{},|agent|if agent=="dsh" {vec![root.clone()]}else{vec![]}).unwrap();
        let again=scan_roots(&f.dir.join("cache"),&Prices::new(),&cancel,|_,_|{},|agent|if agent=="dsh" {vec![root.clone()]}else{vec![]}).unwrap();
        assert!(before.iter().all(|(p,digest)|hash(fs::read(p).unwrap())==*digest));
        let mut tally=Tally::default();let mut second=Tally::default();for r in &snapshot.records {tally.add(&r.tally);}for r in &again.records {second.add(&r.tally);}assert_eq!(tally,second);
        // Private verification artifact contains counters only, no user paths,
        // session IDs, projects, messages, credentials, or source transcript.
        fs::write(result,serde_json::to_vec_pretty(&json!({"files":files.len(),"records":snapshot.records.len(),"tally":tally,
            "notes":snapshot.notes,"cachedFiles":again.sources.last().unwrap().cached_files,"readOnlyVerified":true})).unwrap()).unwrap();
        assert!(!snapshot.records.is_empty());
    }
}
