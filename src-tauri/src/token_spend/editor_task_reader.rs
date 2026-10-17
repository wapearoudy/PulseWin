//! Pulse v1.9 VSCodeTaskLogReader, EditorLogSupport and ClineCLIReader
//! (Apache-2.0): Roo Code / Kilo Code / Cline VS Code task logs plus the Cline
//! CLI session store. A task's `ui_messages.json` is one snapshot of its
//! conversation, and every `api_req_started` entry is one real request carrying
//! the four counters as an inner JSON string; a rewritten snapshot replaces the
//! file's last read instead of being counted again.
use super::measured_readers::{count, document, record, text, timestamp};
use super::*;

const EDITORS:[&str;3]=["Code","Code - Insiders","VSCodium"];
const SERVERS:[&str;2]=[".vscode-server",".vscode-server-insiders"];
const ENV_OPEN:&str="<environment_details>";
const ENV_CLOSE:&str="</environment_details>";
const MAX_DOCUMENT_BYTES:u64=64*1024*1024;

fn extension(id:&str)->Option<&'static str>{match id{
    "roocode"=>Some("rooveterinaryinc.roo-cline"),"kilocode"=>Some("kilocode.kilo-code"),
    "cline"=>Some("saoudrizwan.claude-dev"),_=>None}}
fn label(id:&str)->&'static str{match id{"roocode"=>"Roo Code","kilocode"=>"Kilo Code","cline"=>"Cline",_=>"Editor"}}
/// `~/` and `~\` values expand under `home`; every other value stays as given.
fn expand(home:&Path,value:&str)->Option<PathBuf>{
    let value=value.trim(); if value.is_empty(){return None}
    if value=="~"{return Some(home.to_path_buf())}
    if let Some(rest)=value.strip_prefix("~/").or_else(||value.strip_prefix("~\\")){return Some(home.join(rest))}
    Some(PathBuf::from(value))
}
fn editor_roots(home:&Path,appdata:Option<&Path>,extension:&str)->Vec<PathBuf>{
    let mut roots=Vec::new();
    for editor in EDITORS{
        // Pulse's macOS spelling is `~/Library/Application Support`; on Windows
        // the same editor data directory is `%APPDATA%`.
        if let Some(appdata)=appdata{roots.push(appdata.join(editor).join("User/globalStorage").join(extension).join("tasks"));}
        roots.push(home.join(".config").join(editor).join("User/globalStorage").join(extension).join("tasks"));
    }
    for server in SERVERS{roots.push(home.join(server).join("data/User/globalStorage").join(extension).join("tasks"));}
    roots
}
/// The CLI's own precedence: first configured root wins; a blank value is not a
/// path. `~/.cline/data/sessions` is the CLI's documented fallback.
fn cli_roots(home:&Path,env:&impl Fn(&str)->Option<String>)->Vec<PathBuf>{
    if let Some(path)=env("CLINE_SESSION_DATA_DIR").as_deref().and_then(|v|expand(home,v)){return vec![path]}
    if let Some(path)=env("CLINE_DATA_DIR").as_deref().and_then(|v|expand(home,v)){return vec![path.join("sessions")]}
    if let Some(path)=env("CLINE_DIR").as_deref().and_then(|v|expand(home,v)){return vec![path.join("data/sessions")]}
    vec![home.join(".cline/data/sessions")]
}
pub(super) fn roots(home:&Path,id:&str,env:&impl Fn(&str)->Option<String>)->Vec<PathBuf>{
    let Some(extension)=extension(id) else{return Vec::new()};
    let appdata=env("APPDATA").filter(|v|!v.trim().is_empty()).and_then(|v|expand(home,&v));
    let mut roots=editor_roots(home,appdata.as_deref(),extension);
    // Cline drives both an editor extension and its own CLI; each store keeps
    // its own event identity, so both are read.
    if id=="cline"{roots.extend(cli_roots(home,env));}
    roots
}
pub(super) fn candidate(path:&Path,id:&str)->bool{
    let Some(name)=path.file_name().and_then(|n|n.to_str()) else{return false};
    match id{
        "roocode"|"kilocode"=>name=="ui_messages.json",
        "cline"=>name=="ui_messages.json"||name.ends_with(".messages.json"),
        _=>false,
    }
}
pub(super) fn read(path:&Path,id:&str,cancel:&AtomicBool)->Result<(Vec<SpendRecord>,Vec<String>),String>{
    check(cancel)?;
    let Some(name)=path.file_name().and_then(|n|n.to_str()) else{return Ok((Vec::new(),Vec::new()))};
    if name=="ui_messages.json"{return vscode_task(path,id,cancel)}
    if id=="cline"&&name.ends_with(".messages.json"){return cline_cli(path,id,cancel)}
    Ok((Vec::new(),Vec::new()))
}
fn usage_count(usage:&Value,key:&str)->u64{usage.get(key).map(count).unwrap_or(0)}

// MARK: - VS Code task logs

/// Only the fields one counted message needs; the conversation's prompts,
/// responses and images are discarded here rather than held in a record.
#[derive(Deserialize)]
struct TaskMessage{
    #[serde(rename="type")] kind:Option<Value>,
    say:Option<Value>,
    ts:Option<Value>,
    text:Option<Value>,
    #[serde(rename="modelInfo")] model_info:Option<Value>,
}
fn vscode_task(path:&Path,id:&str,cancel:&AtomicBool)->Result<(Vec<SpendRecord>,Vec<String>),String>{
    let entries:Vec<TaskMessage>=document(path,cancel)?;
    let session=path.parent().and_then(Path::file_name).map(|n|n.to_string_lossy().into_owned())
        .unwrap_or_else(||path.to_string_lossy().into_owned());
    let mut records=Vec::new(); let mut notes=Vec::new(); let mut corrupt=false;
    // The conversation history's last `<model>` tag is the fallback for a run
    // that predates `modelInfo`; loaded only when an entry really needs it.
    let mut details=None::<Option<String>>;
    for entry in &entries{
        check(cancel)?;
        if entry.kind.as_ref().and_then(text).as_deref()!=Some("say")
            || entry.say.as_ref().and_then(text).as_deref()!=Some("api_req_started"){continue}
        let Some(at)=entry.ts.as_ref().and_then(|v|timestamp(v,true)) else{continue};
        let Some(raw)=entry.text.as_ref().and_then(text) else{continue};
        let Some(usage)=serde_json::from_str::<Value>(&raw).ok().filter(Value::is_object) else{corrupt=true;continue};
        let tally=Tally{input:usage_count(&usage,"tokensIn"),cache_write:usage_count(&usage,"cacheWrites"),
            cache_read:usage_count(&usage,"cacheReads"),output:usage_count(&usage,"tokensOut"),..Default::default()};
        if tally.total()==0{continue}
        let model=entry.model_info.as_ref().and_then(Value::as_object).and_then(|m|m.get("modelId")).and_then(text)
            .or_else(||details.get_or_insert_with(||history_model(path,cancel)).clone());
        let Some(model)=model else{continue};
        if records.len()>=MAX_RECORDS{return Err(format!("{}：记录超过读取上限。",label(id)))}
        records.push(record(id,model,at,session.clone(),tally,0,None));
    }
    if corrupt{notes.push(format!("{}：一条 API 请求的计数无法解码，计数可能不完整",label(id)));}
    Ok((records,notes))
}
/// The `api_conversation_history.json` beside the task's messages. Only the
/// `<model>` tag inside `<environment_details>` blocks is used; an unclosed
/// block stops the scan, and a non-UTF-8 or oversized file yields no fallback.
fn history_model(path:&Path,cancel:&AtomicBool)->Option<String>{
    check(cancel).ok()?;
    let history=path.with_file_name("api_conversation_history.json");
    if fs::metadata(&history).ok()?.len()>MAX_DOCUMENT_BYTES{return None}
    let text=String::from_utf8(fs::read(&history).ok()?).ok()?;
    check(cancel).ok()?;
    let mut model=None; let mut rest=text.as_str();
    while let Some(start)=rest.find(ENV_OPEN){
        let after=&rest[start+ENV_OPEN.len()..];
        let Some(end)=after.find(ENV_CLOSE) else{break};
        if let Some(value)=last_tag(&after[..end],"model"){model=Some(value)}
        rest=&after[end+ENV_CLOSE.len()..];
    }
    model
}
fn last_tag(text:&str,tag:&str)->Option<String>{
    let open=format!("<{tag}>"); let close=format!("</{tag}>");
    let mut value=None; let mut rest=text;
    while let Some(start)=rest.find(&open){
        let after=&rest[start+open.len()..];
        let Some(end)=after.find(&close) else{break};
        let content=after[..end].trim();
        if !content.is_empty(){value=Some(content.to_owned())}
        rest=&after[end+close.len()..];
    }
    value
}

// MARK: - Cline CLI sessions

/// The transcript keeps its whole conversation; only these fields are read.
#[derive(Deserialize)]
struct ClineSession{
    #[serde(rename="sessionId")] session_id:Option<Value>,
    #[serde(default)] messages:Vec<ClineMessage>,
}
#[derive(Deserialize)]
struct ClineMessage{
    role:Option<Value>,
    metrics:Option<Value>,
    ts:Option<Value>,
    id:Option<Value>,
    #[serde(rename="modelInfo")] model_info:Option<Value>,
}
/// One directory per session: `<session>.messages.json` plus a
/// `<session>.json` manifest that may carry model, session id and title.
fn cline_cli(path:&Path,id:&str,cancel:&AtomicBool)->Result<(Vec<SpendRecord>,Vec<String>),String>{
    let root:ClineSession=document(path,cancel)?;
    let Some(stem)=path.file_name().and_then(|n|n.to_str()).and_then(|n|n.strip_suffix(".messages.json")) else{
        return Ok((Vec::new(),Vec::new()))
    };
    let manifest=read_json(&path.with_file_name(format!("{stem}.json")));
    let manifest_model=manifest.as_ref().and_then(|m|m.get("model")).and_then(text);
    let session=root.session_id.as_ref().and_then(text)
        .or_else(||manifest.as_ref().and_then(|m|m.get("sessionId")).and_then(text))
        .unwrap_or_else(||stem.to_owned());
    let mut records=Vec::new();
    for (index,message) in root.messages.iter().enumerate(){
        check(cancel)?;
        if message.role.as_ref().and_then(text).as_deref()!=Some("assistant"){continue}
        let Some(metrics)=message.metrics.as_ref().filter(|v|v.is_object()) else{continue};
        let Some(at)=message.ts.as_ref().and_then(|v|timestamp(v,true)) else{continue};
        let cache_read=usage_count(metrics,"cacheReadTokens"); let cache_write=usage_count(metrics,"cacheWriteTokens");
        // The store's `inputTokens` is cache-inclusive: the fresh input is what
        // remains after both cache counts, clamped rather than negative.
        let tally=Tally{input:usage_count(metrics,"inputTokens").saturating_sub(cache_read).saturating_sub(cache_write),
            cache_write,cache_read,output:usage_count(metrics,"outputTokens"),..Default::default()};
        if tally.total()==0{continue}
        let model=message.model_info.as_ref().and_then(Value::as_object).and_then(|m|m.get("id")).and_then(text)
            .or_else(||manifest_model.clone());
        let Some(model)=model else{continue};
        // The store's own message id folds replayed snapshots; a metadata-less
        // message still has its line position to tell it from its neighbours.
        let message_id=message.id.as_ref().and_then(text).unwrap_or_else(||format!("line{index}"));
        if records.len()>=MAX_RECORDS{return Err(format!("{}：记录超过读取上限。",label(id)))}
        records.push(record(id,model,at,session.clone(),tally,0,Some(format!("cline:{session}:{message_id}"))));
    }
    Ok((records,Vec::new()))
}
fn read_json(path:&Path)->Option<Value>{
    if fs::metadata(path).ok()?.len()>MAX_DOCUMENT_BYTES{return None}
    serde_json::from_slice(&fs::read(path).ok()?).ok()
}

#[cfg(test)]
mod tests{
    use super::*;
    use serde_json::json;
    struct Temp(PathBuf);
    impl Temp{
        fn new(tag:&str)->Self{
            // The app's own test convention: a workspace-local `test-results`
            // tree, because the system temp directory is not writable from the
            // sandbox the library tests run in.
            let root=PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../test-results")
                .join(format!("reader-fixture-{tag}-{}-{}",std::process::id(),NEXT_ID.fetch_add(1,Ordering::Relaxed)));
            fs::create_dir_all(&root).unwrap(); Self(root)
        }
        fn path(&self,parts:&str)->PathBuf{self.0.join(parts)}
        fn write(&self,parts:&str,bytes:impl AsRef<[u8]>)->PathBuf{
            let file=self.path(parts); fs::create_dir_all(file.parent().unwrap()).unwrap(); fs::write(&file,bytes).unwrap(); file
        }
    }
    impl Drop for Temp{fn drop(&mut self){let _=fs::remove_dir_all(&self.0);}}
    fn msgs(entries:Value)->String{entries.to_string()}
    fn task_entry(ts:Value,text:impl std::fmt::Display)->Value{json!({"type":"say","say":"api_req_started","ts":ts,"text":text.to_string()})}

    #[test] fn roots_cover_editor_layouts_and_cline_cli_precedence(){
        let home=Path::new("fixture-home");
        let none=|_:&str|None::<String>;
        let roots=super::roots(home,"roocode",&none);
        assert_eq!(roots.len(),5); // 3 editors' .config roots + 2 remote servers
        assert!(roots.contains(&home.join(".config/Code/User/globalStorage/rooveterinaryinc.roo-cline/tasks")));
        assert!(roots.contains(&home.join(".config/VSCodium/User/globalStorage/rooveterinaryinc.roo-cline/tasks")));
        assert!(roots.contains(&home.join(".vscode-server/data/User/globalStorage/rooveterinaryinc.roo-cline/tasks")));
        assert!(roots.contains(&home.join(".vscode-server-insiders/data/User/globalStorage/rooveterinaryinc.roo-cline/tasks")));
        let appdata=|k:&str|match k{"APPDATA"=>Some("fixture-appdata".into()),_=>None::<String>};
        let windows_roots=super::roots(home,"kilocode",&appdata);
        assert_eq!(windows_roots.len(),8); // APPDATA variants + .config + servers
        assert!(windows_roots.contains(&PathBuf::from("fixture-appdata/Code/User/globalStorage/kilocode.kilo-code/tasks")));
        assert!(windows_roots.contains(&PathBuf::from("fixture-appdata/Code - Insiders/User/globalStorage/kilocode.kilo-code/tasks")));
        assert!(super::roots(home,"unknown",&none).is_empty());
        assert!(super::roots(home,"augment",&none).is_empty()); // owned by another module
        // Cline adds the CLI store; the first configured CLI root wins.
        assert_eq!(super::roots(home,"cline",&none).last(),Some(&home.join(".cline/data/sessions")));
        let cli_session=|k:&str|match k{"CLINE_SESSION_DATA_DIR"=>Some("~/custom-sessions".into()),"CLINE_DATA_DIR"=>Some("ignored".into()),_=>None::<String>};
        assert_eq!(super::roots(home,"cline",&cli_session).last(),Some(&home.join("custom-sessions")));
        let cli_data=|k:&str|match k{"CLINE_DATA_DIR"=>Some("fixture-data".into()),"CLINE_DIR"=>Some("ignored".into()),_=>None::<String>};
        assert_eq!(super::roots(home,"cline",&cli_data).last(),Some(&PathBuf::from("fixture-data/sessions")));
        let cli_dir=|k:&str|match k{"CLINE_DIR"=>Some("fixture-cline".into()),_=>None::<String>};
        assert_eq!(super::roots(home,"cline",&cli_dir).last(),Some(&PathBuf::from("fixture-cline/data/sessions")));
        let blank=|k:&str|match k{"CLINE_SESSION_DATA_DIR"=>Some("   ".into()),_=>None::<String>};
        assert_eq!(super::roots(home,"cline",&blank).last(),Some(&home.join(".cline/data/sessions")));
    }
    #[test] fn candidate_accepts_only_read_data_files(){
        assert!(candidate(Path::new("Code/User/globalStorage/rooveterinaryinc.roo-cline/tasks/t/ui_messages.json"),"roocode"));
        assert!(candidate(Path::new("Code/User/globalStorage/kilocode.kilo-code/tasks/t/ui_messages.json"),"kilocode"));
        assert!(!candidate(Path::new("tasks/t/api_conversation_history.json"),"roocode"));
        assert!(!candidate(Path::new("sessions/s/s.messages.json"),"roocode"));
        assert!(candidate(Path::new("sessions/s/s.messages.json"),"cline"));
        assert!(candidate(Path::new("Code/User/globalStorage/saoudrizwan.claude-dev/tasks/t/ui_messages.json"),"cline"));
        assert!(!candidate(Path::new("tasks/t/ui_messages.json"),"unknown"));
        assert!(!candidate(Path::new("tasks/t/ui_messages.json"),"augment"));
    }
    #[test] fn vscode_task_maps_reported_counters_and_history_model(){
        let fixture=Temp::new("vscode-task");
        let history=json!([
            {"role":"user","content":[{"type":"text","text":"<environment_details>\n<model>old-model</model>\n<slug>roo</slug>\n</environment_details>"}]},
            {"role":"user","content":[{"type":"text","text":"<environment_details>\n<model>history-model</model>\n</environment_details>"}]},
        ]);
        let entries=json!([
            {"type":"user","text":"SECRET PROMPT BODY"},
            task_entry(json!(1791028800000i64),json!({"tokensIn":100,"tokensOut":20,"cacheWrites":3,"cacheReads":4,"cost":9.9}).to_string()),
            {"type":"say","say":"api_req_started","ts":"1791028900000","modelInfo":{"modelId":"entry-model"},
                "text":json!({"tokensIn":10,"tokensOut":2}).to_string(),"content":"SECRET CONTENT"},
            task_entry(json!(1791029000000i64),json!({"tokensIn":0,"tokensOut":0}).to_string()),
            task_entry(json!(1791029100000i64),json!({"total":99,"tokens":5}).to_string()),
            task_entry(json!(1791029200000i64),"not-json"),
            {"type":"ask","say":"api_req_started","ts":1791029250000i64,"text":json!({"tokensIn":50}).to_string()},
            task_entry(json!(1791029300000i64),json!({"tokensIn":7,"tokensOut":1}).to_string()),
        ]);
        let file=fixture.write("Code/User/globalStorage/rooveterinaryinc.roo-cline/tasks/session-a/ui_messages.json",msgs(entries));
        fixture.write("Code/User/globalStorage/rooveterinaryinc.roo-cline/tasks/session-a/api_conversation_history.json",history.to_string());
        let (records,notes)=read(&file,"roocode",&AtomicBool::new(false)).unwrap();
        assert_eq!(records.len(),3);
        assert_eq!(records[0].tally,Tally{input:100,output:20,cache_write:3,cache_read:4,..Default::default()});
        assert_eq!(records[0].model,"history-model");
        assert_eq!(records[0].session,"session-a"); assert_eq!(records[0].agent,"roocode");
        assert_eq!(records[1].tally.total(),12); assert_eq!(records[1].model,"entry-model");
        assert_eq!(records[2].tally.total(),8); assert_eq!(records[2].model,"history-model");
        // The VS Code log has no product event id: one file read is one count.
        assert!(records.iter().all(|r|r.deduplication_id.is_none()&&!r.aggregate_timing));
        assert_eq!(notes.len(),1); assert!(notes[0].contains("无法解码"));
        assert!(!serde_json::to_string(&records).unwrap().contains("SECRET"));
        // No history file and no modelInfo: real usage without provable
        // attribution is dropped, never guessed.
        let bare=fixture.write("Code/User/globalStorage/rooveterinaryinc.roo-cline/tasks/session-b/ui_messages.json",
            msgs(json!([task_entry(json!(1791028800000i64),json!({"tokensIn":9,"tokensOut":1}).to_string())])));
        let (records,notes)=read(&bare,"roocode",&AtomicBool::new(false)).unwrap();
        assert!(records.is_empty()); assert!(notes.is_empty());
    }
    #[test] fn vscode_task_snapshot_is_replaced_not_accumulated_across_scans(){
        let fixture=Temp::new("vscode-cache");
        let entry=|ms:u64,tokens:Value|json!({"type":"say","say":"api_req_started","ts":ms,
            "modelInfo":{"modelId":"m"},"text":tokens.to_string()});
        let file=fixture.write("logs/Code/User/globalStorage/rooveterinaryinc.roo-cline/tasks/t1/ui_messages.json",
            msgs(json!([entry(1791028800000,json!({"tokensIn":10,"tokensOut":2}))])));
        let _=file;
        let scan=|cache:&Path|super::super::scan_roots(cache,&Prices::new(),&AtomicBool::new(false),|_,_|{},|id|{
            if id=="roocode"{vec![fixture.path("logs")]}else{vec![]}
        }).unwrap();
        let cache=fixture.path("cache");
        assert_eq!(scan(&cache).records.len(),1);
        // The task log is rewritten in place; the second scan must still see one
        // request, not the old record plus the new one.
        fixture.write("logs/Code/User/globalStorage/rooveterinaryinc.roo-cline/tasks/t1/ui_messages.json",
            msgs(json!([entry(1791028800000,json!({"tokensIn":10,"tokensOut":2})),
                entry(1791028900000,json!({"tokensIn":30,"tokensOut":4}))])));
        let again=scan(&cache);
        assert_eq!(again.records.len(),2);
        assert_eq!(again.records.iter().map(|r|r.tally.total()).sum::<u64>(),46);
        assert_eq!(again.sources.iter().find(|s|s.id=="roocode").unwrap().cached_files,0);
    }
    #[test] fn cline_cli_is_cache_inclusive_has_manifest_fallback_and_message_ids(){
        let fixture=Temp::new("cline-cli");
        let transcript=json!({"sessionId":"shared-session","messages":[
            {"role":"user","content":"SECRET USER BODY"},
            {"id":"m1","role":"assistant","ts":1791028800000i64,"modelInfo":{"id":"cli-model"},
                "metrics":{"inputTokens":100,"cacheReadTokens":4,"cacheWriteTokens":2,"outputTokens":20},"content":"SECRET ASSISTANT BODY"},
            {"id":"m1","role":"assistant","ts":1791028800000i64,"modelInfo":{"id":"cli-model"},
                "metrics":{"inputTokens":100,"cacheReadTokens":4,"cacheWriteTokens":2,"outputTokens":20}},
            {"id":"m2","role":"assistant","ts":"1791028900000",
                "metrics":{"inputTokens":5,"cacheReadTokens":0,"cacheWriteTokens":0,"outputTokens":1}},
            {"role":"assistant","ts":1791029000000i64,"metrics":{"inputTokens":0,"outputTokens":0}}
        ]});
        let manifest=json!({"sessionId":"shared-session","model":"manifest-model","workspace_root":"C:/work/project","metadata":{"title":"SECRET TITLE"}});
        let file=fixture.write("logs/a/a.messages.json",transcript.to_string());
        fixture.write("logs/a/a.json",manifest.to_string());
        let (records,notes)=read(&file,"cline",&AtomicBool::new(false)).unwrap();
        assert!(notes.is_empty()); assert_eq!(records.len(),3);
        assert_eq!(records[0].tally,Tally{input:94,output:20,cache_write:2,cache_read:4,..Default::default()});
        assert_eq!(records[0].model,"cli-model"); assert_eq!(records[0].session,"shared-session");
        assert_eq!(records[0].deduplication_id.as_deref(),Some("cline:shared-session:m1"));
        assert_eq!(records[1].deduplication_id,records[0].deduplication_id); // replay keeps one id
        assert_eq!(records[2].model,"manifest-model"); // manifest fallback
        assert_eq!(records[2].deduplication_id.as_deref(),Some("cline:shared-session:m2"));
        assert!(!serde_json::to_string(&records).unwrap().contains("SECRET"));
        // A replayed assistant snapshot is kept here (the same message id), and
        // the scan's global identity claim folds the two copies to one.
        assert_eq!(records.iter().map(|r|r.tally.total()).sum::<u64>(),246);
    }
    #[test] fn cline_cli_copied_sessions_collapse_by_message_identity(){
        let fixture=Temp::new("cline-dedup");
        let transcript=json!({"sessionId":"shared-session","messages":[
            {"id":"m1","role":"assistant","ts":1791028800000i64,"modelInfo":{"id":"m"},
                "metrics":{"inputTokens":10,"cacheReadTokens":2,"cacheWriteTokens":3,"outputTokens":4}},
            {"id":"m2","role":"assistant","ts":1791028900000i64,"modelInfo":{"id":"m"},
                "metrics":{"inputTokens":1,"cacheReadTokens":0,"cacheWriteTokens":0,"outputTokens":2}}
        ]});
        let manifest=json!({"model":"m"});
        fixture.write("logs/a/a.messages.json",transcript.to_string()); fixture.write("logs/a/a.json",manifest.to_string());
        fixture.write("logs/b/b.messages.json",transcript.to_string()); fixture.write("logs/b/b.json",manifest.to_string());
        let scan=|cache:&Path|super::super::scan_roots(cache,&Prices::new(),&AtomicBool::new(false),|_,_|{},|id|{
            if id=="cline"{vec![fixture.path("logs")]}else{vec![]}
        }).unwrap();
        let cache=fixture.path("cache");
        let first=scan(&cache);
        assert_eq!(first.records.len(),2); // m1 and m2 collapse across the two copies
        assert_eq!(first.records.iter().map(|r|r.tally.total()).sum::<u64>(),17);
        let second=scan(&cache);
        assert_eq!(second.records.len(),2);
        assert_eq!(second.sources.iter().find(|s|s.id=="cline").unwrap().cached_files,2);
    }
    #[test] fn malformed_task_log_and_cancellation_surface_as_errors(){
        let fixture=Temp::new("editor-error");
        let file=fixture.write("tasks/t/ui_messages.json","{not a task log");
        assert!(read(&file,"roocode",&AtomicBool::new(false)).unwrap_err().contains("无法解码"));
        let good=fixture.write("tasks/u/ui_messages.json",msgs(json!([task_entry(json!(1791028800000i64),
            json!({"tokensIn":1,"tokensOut":1,"modelInfo":{"modelId":"m"}}).to_string())])));
        let cancel=AtomicBool::new(true);
        assert_eq!(read(&good,"roocode",&cancel).unwrap_err(),"cancelled");
    }
}
