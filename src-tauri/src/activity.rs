//! Local lifecycle evidence, ported from Pulse's AgentActivity.swift.
//! Bounded tail reads only; never parse prompt text for display or send it out.
use std::{collections::{HashMap, HashSet}, fs, io::{Read, Seek, SeekFrom}, path::{Path, PathBuf}};
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Wait { Model, Tool }
impl Wait { fn grace(self) -> i64 { match self { Self::Model => 90_000, Self::Tool => 300_000 } } }
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Verdict { Working(Wait, Option<i64>), Finished, Unknown }
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Activity {
    pub running: Vec<String>, pub last_write: Option<i64>, pub finished_at: HashMap<String,i64>,
    /// A working turn Pulse actually saw. Transient unreadable tails may stop
    /// the working mark, but cannot themselves end this witness.
    #[serde(skip)]
    witnessed_running: HashSet<String>,
}
#[derive(Debug, Clone, Default)]
pub struct Reading { pub working: bool, pub last_write: Option<i64>, pub finished: bool }
pub fn supported(id: &str) -> bool { matches!(id, "claude-code" | "codex" | "kiro" | "zai" | "glm-coding") }
fn stamp(v: &Value) -> Option<i64> { chrono::DateTime::parse_from_rfc3339(v["timestamp"].as_str()?).ok().map(|d| d.timestamp_millis()) }

pub fn verdict(text: &str, provider: &str) -> Verdict {
    let mut completed=HashSet::new(); let mut active=HashMap::new();
    for line in text.lines().rev() {
        let Ok(v)=serde_json::from_str::<Value>(line) else { continue };
        let at=stamp(&v);
        match provider {
            "claude-code" => match v["type"].as_str() {
                Some("assistant") => return match v["message"]["stop_reason"].as_str() {
                    Some("tool_use") => Verdict::Working(Wait::Tool,at),
                    None => Verdict::Working(Wait::Model,at), _ => Verdict::Finished,
                },
                Some("user") => {
                    let content=&v["message"]["content"];
                    let interrupted=content.as_str().is_some_and(|s|s.contains("[Request interrupted by user")) ||
                        content.as_array().is_some_and(|a|a.iter().any(|b|b["text"].as_str().is_some_and(|s|s.contains("[Request interrupted by user"))));
                    return if interrupted { Verdict::Finished } else { Verdict::Working(Wait::Model,at) };
                }, _ => {}
            },
            "codex" => match v["payload"]["type"].as_str() {
                Some("task_complete"|"turn_aborted") => return Verdict::Finished,
                Some("task_started"|"function_call_output"|"custom_tool_call_output"|"tool_search_output") => return Verdict::Working(Wait::Model,at),
                Some("function_call"|"custom_tool_call"|"local_shell_call"|"web_search_call"|"tool_search_call") => return Verdict::Working(Wait::Tool,at), _ => {}
            },
            "kiro" => {
                match v["payload"]["type"].as_str() {
                    Some("turn_end"|"pending_interaction") => return Verdict::Finished,
                    Some("turn_start"|"tool_result"|"interaction_resolved"|"assistant"|"sub_agent_complete") => return Verdict::Working(Wait::Model,at),
                    Some("tool_call"|"sub_agent_start") => return Verdict::Working(Wait::Tool,at), _ => {}
                }
                match v["kind"].as_str() {
                    Some("Prompt"|"ToolResults") => return Verdict::Working(Wait::Model,at),
                    Some("AssistantMessage") => return if v["data"]["content"].as_array().is_some_and(|a|a.iter().any(|b|b["kind"]=="toolUse")) { Verdict::Working(Wait::Tool,at) } else { Verdict::Finished }, _ => {}
                }
            },
            "zai" | "glm-coding" => {
                let Some(turn)=v["turnId"].as_str() else {continue};
                match v["event"].as_str() {
                    Some("turn.completed"|"turn.failed"|"turn.cancelled") => { completed.insert(turn.to_owned()); },
                    Some("tool.call.started") if !completed.contains(turn) => { active.entry(turn.to_owned()).or_insert((Wait::Tool,at)); },
                    Some("tool.call.completed"|"tool.call.failed"|"model.request.started"|"model.request.completed"|"model.request.failed") if !completed.contains(turn) => { active.entry(turn.to_owned()).or_insert((Wait::Model,at)); },
                    Some("turn.started") if !completed.contains(turn) => { let (wait,at)=active.get(turn).copied().unwrap_or((Wait::Model,at));return Verdict::Working(wait,at); }, _ => {}
                }
            }, _ => return Verdict::Unknown
        }
    }
    if provider=="zai" || provider=="glm-coding" {
        return active.into_iter().filter(|(k,_)|!completed.contains(k)).map(|(_,v)|v).max_by_key(|(_,at)|*at)
            .map_or(if completed.is_empty(){Verdict::Unknown}else{Verdict::Finished},|(wait,at)|Verdict::Working(wait,at));
    }
    Verdict::Unknown
}

fn tail(path: &Path, limit: u64) -> Option<String> {
    let mut file=fs::File::open(path).ok()?;
    let start=file.metadata().ok()?.len().saturating_sub(limit);
    file.seek(SeekFrom::Start(start)).ok()?;
    let mut bytes=Vec::new();file.take(limit).read_to_end(&mut bytes).ok()?;
    let text=String::from_utf8_lossy(&bytes);
    Some(if start>0 {text.split_once('\n').map_or("",|(_,t)|t).to_owned()} else {text.into_owned()})
}
fn root(home: &Path, id: &str) -> Option<PathBuf> {
    match id {
        "claude-code" => Some(std::env::var_os("CLAUDE_CONFIG_DIR").map(PathBuf::from).unwrap_or_else(||home.join(".claude")).join("projects")),
        "codex" => Some(std::env::var_os("CODEX_HOME").map(PathBuf::from).unwrap_or_else(||home.join(".codex")).join("sessions")),
        "kiro" => Some(home.join(".kiro/sessions")),
        "zai" | "glm-coding" => {
            let v:Value=serde_json::from_slice(&fs::read(home.join(".zcode/cli/config.json")).ok()?).ok()?;
            let provider=v["model"]["main"].as_str()?.split('/').next()?;
            let url=reqwest::Url::parse(v["provider"][provider]["options"]["baseURL"].as_str()?).ok()?;
            let host=url.host_str()?;
            let matches=if id=="zai" {host=="api.z.ai"||host.ends_with(".z.ai")} else {host=="open.bigmodel.cn"||host.ends_with(".bigmodel.cn")};
            matches.then(||home.join(".zcode/cli/log"))
        },_=>None
    }
}
fn files(root:PathBuf,id:&str,cancelled:&impl Fn()->bool)->Vec<(PathBuf,i64)> {
    let mut dirs=vec![root];let mut found=vec![];
    while let Some(dir)=dirs.pop() {
        if cancelled(){break}
        let Ok(entries)=fs::read_dir(dir) else {continue};
        for entry in entries.flatten() {
            if cancelled(){return found}
            let Ok(kind)=entry.file_type() else {continue};
            if kind.is_symlink(){continue}
            let path=entry.path();
            if kind.is_dir(){dirs.push(path);continue}
            if path.extension().and_then(|s|s.to_str())!=Some("jsonl"){continue}
            if id=="kiro" && path.file_name().and_then(|s|s.to_str())!=Some("messages.jsonl") && path.parent().and_then(|p|p.file_name()).and_then(|s|s.to_str())!=Some("cli"){continue}
            if let Ok(m)=entry.metadata(){if m.len()>0 {if let Ok(time)=m.modified(){if let Ok(time)=time.duration_since(std::time::UNIX_EPOCH){found.push((path,time.as_millis() as i64))}}}}
        }
    }
    found.sort_by_key(|(_,time)|std::cmp::Reverse(*time));found
}
pub fn scan(home:&Path,providers:&[String],now:i64,cancelled:impl Fn()->bool)->HashMap<String,Reading> {
    let mut out=HashMap::new();
    for id in providers.iter().filter(|id|supported(id)) {
        if cancelled(){break}
        let mut reading=Reading::default();
        let mut evidence=ScanEvidence::default();
        if let Some(root)=root(home,id) {
            let files=files(root,id,&cancelled);reading.last_write=files.first().map(|(_,at)|*at);
            let zcode=id=="zai"||id=="glm-coding";
            if zcode {reading.last_write=files.first().and_then(|(p,mtime)|tail(p,2*1024*1024).and_then(|s|s.lines().rev().filter_map(|l|serde_json::from_str::<Value>(l).ok()).find(|v|v["turnId"].is_string()&&v["event"].as_str().is_some_and(|e|["turn.","tool.","model."].iter().any(|p|e.starts_with(p)))).and_then(|v|stamp(&v).or(Some(*mtime)))));}
            for (path,modified) in files.iter().filter(|(_,at)|now-at<=300_000) {
                if cancelled(){return out}
                let Some(text)=tail(path,if zcode{2*1024*1024}else{128*1024}) else {evidence.uncertain=true;continue};
                let verdict=verdict(&text,id);
                evidence.observe(verdict);
                reading.working=is_reading_working(verdict,*modified,now,zcode);
                if reading.working {break}
            }
        }
        reading.finished=evidence.finished&&!evidence.uncertain&&!reading.working;
        out.insert(id.clone(),reading);
    }
    out
}
#[derive(Default)]
struct ScanEvidence { finished:bool, uncertain:bool }
impl ScanEvidence {
    fn observe(&mut self,verdict:Verdict){
        match verdict {Verdict::Finished=>self.finished=true,Verdict::Working(..)|Verdict::Unknown=>self.uncertain=true}
    }
}
fn is_working(verdict:Verdict,modified:i64,now:i64)->bool {
    match verdict {Verdict::Working(wait,at)=>now-at.unwrap_or(modified)<=wait.grace(),Verdict::Finished=>false,Verdict::Unknown=>now-modified<=30_000}
}
fn is_reading_working(verdict:Verdict,modified:i64,now:i64,lifecycle_only:bool)->bool {
    // ZCode's heartbeat writes are deliberately not activity. A route whose
    // format has explicit turn ids cannot fall back to filesystem freshness.
    if lifecycle_only&&verdict==Verdict::Unknown {false}else{is_working(verdict,modified,now)}
}
impl Activity {
    pub fn record(&mut self,states:HashMap<String,Reading>,now:i64) {
        let mut active:Vec<_>=states.iter().filter(|(_,s)|s.working).map(|(id,_)|id.clone()).collect();active.sort();
        // Removing a provider, failing to read a tail, or a grace timer
        // expiring is not a completion event. Only explicit lifecycle evidence
        // can consume a turn this process previously saw working.
        self.witnessed_running.retain(|id|states.contains_key(id));
        for (id,reading) in &states {
            if reading.working {self.witnessed_running.insert(id.clone());}
            else if reading.finished&&self.witnessed_running.remove(id){self.finished_at.insert(id.clone(),now);}
        }
        self.running=active;self.last_write=states.values().filter_map(|s|s.last_write).max();
    }
}

#[cfg(test)] mod tests {
    use super::*;
    #[test] fn codex_lifecycle_skips_bookkeeping_and_distinguishes_tools(){
        assert_eq!(verdict("{\"payload\":{\"type\":\"task_started\"}}\n{\"type\":\"session_meta\"}","codex"),Verdict::Working(Wait::Model,None));
        for kind in ["function_call","custom_tool_call","web_search_call"]{assert_eq!(verdict(&format!(r#"{{"payload":{{"type":"{kind}"}}}}"#),"codex"),Verdict::Working(Wait::Tool,None));}
        assert_eq!(verdict("{\"payload\":{\"type\":\"task_complete\"}}\n{\"type\":\"session_meta\"}","codex"),Verdict::Finished);
        assert_eq!(verdict(r#"{"payload":{"type":"turn_aborted"}}"#,"codex"),Verdict::Finished);
    }
    #[test] fn claude_interrupt_is_not_a_new_prompt(){
        for content in [serde_json::json!("[Request interrupted by user for tool use]"),serde_json::json!([{"type":"text","text":"[Request interrupted by user]"}])]{
            assert_eq!(verdict(&serde_json::json!({"type":"user","message":{"content":content}}).to_string(),"claude-code"),Verdict::Finished);
        }
        assert_eq!(verdict(r#"{"type":"assistant","message":{"stop_reason":"tool_use"}}"#,"claude-code"),Verdict::Working(Wait::Tool,None));
        assert_eq!(verdict(r#"{"type":"assistant","message":{"stop_reason":"end_turn"}}"#,"claude-code"),Verdict::Finished);
    }
    #[test] fn kiro_waiting_for_person_is_idle(){assert_eq!(verdict(r#"{"payload":{"type":"pending_interaction"}}"#,"kiro"),Verdict::Finished);}
    #[test] fn zcode_overlapping_turns_and_heartbeats(){
        let text="{\"event\":\"turn.started\",\"turnId\":\"a\"}\n{\"event\":\"tool.call.started\",\"turnId\":\"a\"}\n{\"event\":\"turn.completed\",\"turnId\":\"b\"}\n{\"event\":\"heartbeat\"}";
        assert_eq!(verdict(text,"zai"),Verdict::Working(Wait::Tool,None));
        assert_eq!(verdict("{\"event\":\"heartbeat\"}","zai"),Verdict::Unknown);
    }
    #[test] fn bookkeeping_mtime_cannot_extend_a_dead_turn(){
        assert!(!is_working(Verdict::Working(Wait::Model,Some(0)),200_000,200_000));
        assert!(is_working(Verdict::Working(Wait::Tool,Some(0)),200_000,200_000));
        assert!(!is_working(Verdict::Working(Wait::Tool,Some(0)),400_000,400_000));
        assert!(!is_working(Verdict::Unknown,0,31_000));
    }
    #[test] fn finish_requires_a_witness_and_stop_clears_it(){
        let mut a=Activity::default();a.record(HashMap::from([("codex".into(),Reading::default())]),10);assert!(a.finished_at.is_empty());
        a.record(HashMap::from([("codex".into(),Reading{working:true,last_write:Some(20),finished:false})]),20);
        a.record(HashMap::from([("codex".into(),Reading{finished:true,..Reading::default()})]),30);assert_eq!(a.finished_at["codex"],30);
        a=Activity::default();a.record(HashMap::new(),40);assert!(a.finished_at.is_empty());
    }
    #[test] fn missing_unknown_and_expired_work_are_not_finished(){
        for stopped in [HashMap::new(),HashMap::from([("codex".into(),Reading::default())])]{
            let mut a=Activity::default();a.record(HashMap::from([("codex".into(),Reading{working:true,..Reading::default()})]),10);
            a.record(stopped,20);assert!(a.running.is_empty());assert!(a.finished_at.is_empty());
        }
        let mut evidence=ScanEvidence::default();evidence.observe(Verdict::Working(Wait::Model,Some(0)));
        assert!(!is_working(Verdict::Working(Wait::Model,Some(0)),100_000,100_000));assert!(!evidence.finished);assert!(evidence.uncertain);
    }
    #[test] fn completion_after_a_transient_read_failure_is_witnessed_once(){
        let mut a=Activity::default();a.record(HashMap::from([("codex".into(),Reading{working:true,..Reading::default()})]),10);
        a.record(HashMap::from([("codex".into(),Reading::default())]),20);assert!(a.finished_at.is_empty());
        a.record(HashMap::from([("codex".into(),Reading{finished:true,..Reading::default()})]),30);assert_eq!(a.finished_at["codex"],30);
        a.record(HashMap::from([("codex".into(),Reading{finished:true,..Reading::default()})]),40);assert_eq!(a.finished_at["codex"],30);
    }
    #[test] fn disabling_a_provider_discards_its_pending_finish_witness(){
        let mut a=Activity::default();a.record(HashMap::from([("codex".into(),Reading{working:true,..Reading::default()})]),10);
        a.record(HashMap::new(),20);a.record(HashMap::from([("codex".into(),Reading{finished:true,..Reading::default()})]),30);
        assert!(a.finished_at.is_empty());
    }
    #[test] fn finished_readings_on_startup_and_unresolved_parallel_turns_do_not_celebrate(){
        let mut a=Activity::default();a.record(HashMap::from([("codex".into(),Reading{finished:true,..Reading::default()})]),10);assert!(a.finished_at.is_empty());
        let mut evidence=ScanEvidence::default();evidence.observe(Verdict::Finished);evidence.observe(Verdict::Working(Wait::Tool,Some(0)));
        assert!(evidence.finished);assert!(evidence.uncertain);
        let mut unknown=ScanEvidence::default();unknown.observe(Verdict::Unknown);unknown.observe(Verdict::Finished);assert!(unknown.uncertain);
    }
    #[test] fn zcode_empty_or_heartbeat_only_tails_are_unknown_not_completion(){
        for text in ["",r#"{"event":"heartbeat"}"#]{assert_eq!(verdict(text,"zai"),Verdict::Unknown);assert_eq!(verdict(text,"glm-coding"),Verdict::Unknown)}
        assert_eq!(verdict(r#"{"event":"turn.completed","turnId":"a"}"#,"zai"),Verdict::Finished);
        assert!(!is_reading_working(Verdict::Unknown,100_000,100_000,true));
        assert!(is_reading_working(Verdict::Working(Wait::Model,Some(100_000)),100_000,100_000,true));
    }
    #[test] fn deselected_and_cancelled_scans_are_empty(){
        assert!(scan(Path::new("."),&[],0,||false).is_empty());
        assert!(scan(Path::new("."),&["codex".into()],0,||true).is_empty());
        assert!(scan(Path::new("."),&["copilot".into()],0,||false).is_empty());
    }
}
