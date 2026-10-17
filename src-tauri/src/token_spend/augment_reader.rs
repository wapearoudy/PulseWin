//! Pulse v1.9 AugmentUsageReader (Apache-2.0): `~/.augment/sessions/<id>.json`.
//! Only completed turns count; a streamed turn's response_nodes are cumulative,
//! so the last node that reported anything is the turn's own total, never a sum.
use super::measured_readers::{count, document, record, text, timestamp};
use super::*;

pub(super) fn roots(home:&Path,_id:&str,_env:&impl Fn(&str)->Option<String>)->Vec<PathBuf>{
    vec![home.join(".augment/sessions")]
}
pub(super) fn candidate(path:&Path,id:&str)->bool{
    id=="augment"&&path.extension().is_some_and(|extension|extension.eq_ignore_ascii_case("json"))
}
#[derive(Deserialize)]
struct Session{
    #[serde(rename="sessionId")] session_id:Option<Value>,
    #[serde(rename="agentState")] agent_state:Option<Value>,
    #[serde(rename="chatHistory",default)] chat_history:Option<Vec<Turn>>,
}
#[derive(Deserialize)]
struct Turn{
    completed:Option<Value>,
    #[serde(rename="finishedAt")] finished_at:Option<Value>,
    #[serde(rename="sequenceId")] sequence_id:Option<Value>,
    exchange:Option<Exchange>,
}
#[derive(Deserialize)]
struct Exchange{
    #[serde(rename="model_id")] model_id:Option<Value>,
    #[serde(rename="request_id")] request_id:Option<Value>,
    #[serde(rename="response_nodes",default)] response_nodes:Vec<Node>,
}
#[derive(Deserialize)]
struct Node{token_usage:Option<Value>}

fn usage_count(usage:&Value,key:&str)->u64{usage.get(key).map(count).unwrap_or(0)}
fn tally(usage:&Value)->Tally{Tally{input:usage_count(usage,"input_tokens"),
    cache_write:usage_count(usage,"cache_creation_input_tokens"),cache_read:usage_count(usage,"cache_read_input_tokens"),
    output:usage_count(usage,"output_tokens"),..Default::default()}}
/// The last response node whose `token_usage` reports anything: streamed nodes
/// are cumulative snapshots of one turn, so summing them would multiply it.
fn last_usage(nodes:&[Node])->Option<&Value>{
    for node in nodes.iter().rev(){
        let Some(usage)=node.token_usage.as_ref().filter(|value|value.is_object()) else{continue};
        if tally(usage).total()>0{return Some(usage)}
    }
    None
}
pub(super) fn read(path:&Path,id:&str,cancel:&AtomicBool)->Result<(Vec<SpendRecord>,Vec<String>),String>{
    check(cancel)?;
    let session:Session=document(path,cancel)?;
    let session_id=session.session_id.as_ref().and_then(text)
        .unwrap_or_else(||path.file_stem().unwrap_or_default().to_string_lossy().into_owned());
    let agent_model=session.agent_state.as_ref().and_then(Value::as_object).and_then(|state|state.get("modelId")).and_then(text);
    // No chat history is not a partial read: there is nothing to count yet.
    let Some(turns)=session.chat_history else{return Ok((Vec::new(),Vec::new()))};
    let mut records=Vec::new();
    for (index,turn) in turns.iter().enumerate(){
        check(cancel)?;
        // An aborted or in-progress turn can carry a partial streamed total;
        // only a completed turn is a usage event.
        if turn.completed.as_ref().and_then(Value::as_bool)!=Some(true){continue}
        let Some(exchange)=turn.exchange.as_ref() else{continue};
        // `finishedAt` is the turn's only time; the epoch is "not set", never a
        // real hour.
        let Some(at)=turn.finished_at.as_ref().and_then(|value|timestamp(value,false)).filter(|at|at.timestamp()>0) else{continue};
        let Some(usage)=last_usage(&exchange.response_nodes) else{continue};
        let tally=tally(usage); if tally.total()==0{continue}
        let model=exchange.model_id.as_ref().and_then(text).or_else(||agent_model.clone());
        let Some(model)=model else{continue};
        let identity=exchange.request_id.as_ref().and_then(text).or_else(||turn.sequence_id.as_ref().and_then(text))
            .unwrap_or_else(||index.to_string());
        if records.len()>=MAX_RECORDS{return Err("Augment：记录超过读取上限。".into())}
        records.push(record(id,model,at,session_id.clone(),tally,0,Some(format!("augment:{session_id}:{identity}"))));
    }
    Ok((records,Vec::new()))
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
        fn sessions(&self)->PathBuf{self.0.join(".augment/sessions")}
        fn write(&self,name:&str,value:&Value)->PathBuf{
            let file=self.sessions().join(name); fs::create_dir_all(file.parent().unwrap()).unwrap(); fs::write(&file,value.to_string()).unwrap(); file
        }
    }
    impl Drop for Temp{fn drop(&mut self){let _=fs::remove_dir_all(&self.0);}}

    #[test] fn roots_and_candidate_are_the_documented_store(){
        let home=Path::new("fixture-home");
        assert_eq!(super::roots(home,"augment",&|_|None::<String>),vec![home.join(".augment/sessions")]);
        assert!(candidate(Path::new(".augment/sessions/session-a.json"),"augment"));
        assert!(candidate(Path::new(".augment/sessions/UPPER.JSON"),"augment"));
        assert!(!candidate(Path::new(".augment/sessions/session-a.jsonl"),"augment"));
        assert!(!candidate(Path::new(".augment/sessions/session-a.json"),"roocode"));
    }
    #[test] fn completed_turns_take_the_last_streamed_usage_once(){
        let fixture=Temp::new("augment-map");
        let session=json!({"sessionId":"aug-session","agentState":{"modelId":"agent-model"},"chatHistory":[
            {"completed":false,"finishedAt":"2026-10-03T13:00:00Z","exchange":{"model_id":"skipped-model","response_nodes":[{"token_usage":{"input_tokens":999}}]}},
            {"completed":true,"finishedAt":"2026-10-03T12:00:00Z","exchange":{"model_id":"exchange-model","request_id":"req-1","response_nodes":[
                {"token_usage":{"input_tokens":999,"output_tokens":999}},
                {"token_usage":{"input_tokens":100,"cache_read_input_tokens":10,"cache_creation_input_tokens":5,"output_tokens":20},"content":"SECRET TOKEN BODY"}],
                "content":"SECRET EXCHANGE BODY"}},
            {"completed":true,"finishedAt":1791029400,"sequenceId":"seq-7","exchange":{"response_nodes":[{"token_usage":{"input_tokens":7,"output_tokens":1}}]}},
            {"completed":true,"finishedAt":0,"exchange":{"model_id":"m","response_nodes":[{"token_usage":{"input_tokens":5}}]}},
            {"completed":true,"finishedAt":"2026-10-03T12:00:00Z","exchange":{"model_id":"m"}},
            {"completed":true,"finishedAt":"2026-10-03T12:00:00Z","exchange":{"model_id":"m","response_nodes":[{"token_usage":{"total_tokens":999}}]}},
            {"completed":true,"finishedAt":"2026-10-03T12:00:00Z"},
            {"completed":"true","finishedAt":"2026-10-03T12:00:00Z","exchange":{"model_id":"m","response_nodes":[{"token_usage":{"input_tokens":5}}]}}
        ]});
        let file=fixture.write("session-a.json",&session);
        let (records,notes)=read(&file,"augment",&AtomicBool::new(false)).unwrap();
        assert!(notes.is_empty()); assert_eq!(records.len(),2);
        assert_eq!(records[0].tally,Tally{input:100,output:20,cache_write:5,cache_read:10,..Default::default()});
        assert_eq!(records[0].model,"exchange-model"); assert_eq!(records[0].session,"aug-session");
        assert_eq!(records[0].deduplication_id.as_deref(),Some("augment:aug-session:req-1"));
        assert_eq!(records[0].source_timestamp,Some(1791028800));
        assert_eq!(records[1].model,"agent-model"); assert_eq!(records[1].tally.total(),8);
        assert_eq!(records[1].deduplication_id.as_deref(),Some("augment:aug-session:seq-7"));
        assert!(!serde_json::to_string(&records).unwrap().contains("SECRET"));
        // A session with no embedded id falls back to the file stem; a turn with
        // no provable model is skipped whole rather than attributed.
        let anonymous=fixture.write("no-agent.json",&json!({"chatHistory":[{"completed":true,"finishedAt":1791028800,
            "exchange":{"request_id":"r","response_nodes":[{"token_usage":{"input_tokens":9}}]}}]}));
        assert!(read(&anonymous,"augment",&AtomicBool::new(false)).unwrap().0.is_empty());
        let named=fixture.write("no-agent-2.json",&json!({"chatHistory":[{"completed":true,"finishedAt":1791028800,
            "exchange":{"model_id":"stem-model","request_id":"r","response_nodes":[{"token_usage":{"input_tokens":9}}]}}]}));
        let records=read(&named,"augment",&AtomicBool::new(false)).unwrap().0;
        assert_eq!(records[0].session,"no-agent-2"); assert_eq!(records[0].model,"stem-model");
    }
    #[test] fn copied_session_files_collapse_on_the_embedded_identity(){
        let fixture=Temp::new("augment-dedup");
        let session=json!({"sessionId":"shared-session","chatHistory":[
            {"completed":true,"finishedAt":1791028800,"exchange":{"model_id":"m","request_id":"req-1","response_nodes":[{"token_usage":{"input_tokens":10,"output_tokens":2}}]}}
        ]});
        fixture.write("session-copy.json",&session); fixture.write("session.json",&session);
        let scan=|cache:&Path|super::super::scan_roots(cache,&Prices::new(),&AtomicBool::new(false),|_,_|{},|id|{
            if id=="augment"{vec![fixture.sessions()]}else{vec![]}
        }).unwrap();
        let cache=fixture.0.join("cache");
        let first=scan(&cache);
        assert_eq!(first.records.len(),1); // the same request in a copied file counts once
        assert_eq!(first.records[0].tally.total(),12);
        let second=scan(&cache);
        assert_eq!(second.records.len(),1);
        assert_eq!(second.sources.iter().find(|s|s.id=="augment").unwrap().cached_files,2);
    }
    #[test] fn malformed_session_and_cancellation_surface_as_errors(){
        let fixture=Temp::new("augment-error");
        fs::create_dir_all(fixture.sessions()).unwrap();
        let file=fixture.sessions().join("broken.json"); fs::write(&file,"{not json").unwrap();
        assert!(read(&file,"augment",&AtomicBool::new(false)).unwrap_err().contains("无法解码"));
        let cancel=AtomicBool::new(true);
        assert_eq!(read(&file,"augment",&cancel).unwrap_err(),"cancelled");
    }
}
