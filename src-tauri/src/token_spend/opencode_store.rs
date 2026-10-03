//! Read-only OpenCode/Kilo databases (Pulse OpenCodeStore) and MiMo Code's
//! compatible stores (MicodeReader). Message bodies and vendor-written cost
//! are never retained. Live WAL commits invalidate the normalized-record cache.
use super::*;
use chrono::Timelike;
use rusqlite::{Connection, OpenFlags};
use std::time::Instant;

pub(super) fn agent(id: &str) -> bool { matches!(id,"opencode"|"kilo"|"micode") }
pub(super) fn is_database(path: &Path) -> bool { path.extension().is_some_and(|ext|ext=="db") }
pub(super) fn candidate(path: &Path, agent: &str) -> bool {
    let name=path.file_name().unwrap_or_default().to_string_lossy();
    match agent { "opencode"=>name=="opencode.db", "kilo"=>name=="kilo.db", "micode"=>name.starts_with("mimocode") && name.ends_with(".db"), _=>false }
}
pub(super) fn roots(home: &Path, agent: &str, env: &impl Fn(&str)->Option<String>) -> Vec<PathBuf> {
    let data=env("XDG_DATA_HOME").filter(|s| !s.trim().is_empty()).map(PathBuf::from).filter(|p|p.is_absolute()).unwrap_or_else(||home.join(".local/share"));
    match agent {
        "opencode"=>vec![data.join("opencode/opencode.db")],
        "kilo"=>vec![data.join("kilo/kilo.db")],
        "micode"=>{
            let mut paths=vec![data.join("mimocode"),home.join("Library/Application Support/orca/mimocode-hooks/shared/data")];
            if let Some(appdata)=env("APPDATA").filter(|s|!s.trim().is_empty()) { paths.push(PathBuf::from(appdata).join("orca/mimocode-hooks/shared/data")); }
            paths
        },
        _=>Vec::new()
    }
}
fn columns(conn: &Connection, table: &str) -> Result<HashSet<String>,String> {
    // The only callers supply our constant table names, never a file's input.
    let mut statement=conn.prepare(&format!("PRAGMA table_info({table})")).map_err(|_|"无法读取数据库结构".to_string())?;
    let rows=statement.query_map([],|row|row.get::<_,String>(1)).map_err(|_|"无法读取数据库结构".to_string())?;
    rows.collect::<rusqlite::Result<HashSet<_>>>().map_err(|_|"无法读取数据库结构".into())
}
fn text(value: &Value) -> Option<&str> { value.as_str().map(str::trim).filter(|s| !s.is_empty()) }
fn count(value: &Value) -> Option<u64> {
    if value.is_null() { return Some(0); }
    value.as_u64().or_else(||value.as_str().and_then(|s|s.trim().parse().ok()))
        .or_else(||value.as_f64().filter(|v|v.is_finite() && *v>=0. && v.fract()==0. && *v<i64::MAX as f64).map(|v|v as u64))
        .filter(|v|*v<=i64::MAX as u64)
}
fn at(value: &Value) -> Option<DateTime<Utc>> {
    let value=value.as_f64().or_else(||value.as_str().and_then(|s|s.parse().ok()))?;
    if !value.is_finite() || value<=0. {return None;}
    let seconds=if value>1e11 {value/1000.}else{value};
    DateTime::from_timestamp(seconds.floor() as i64,((seconds.fract()*1e9).round() as u32).min(999_999_999))
}
pub(super) fn read(path: &Path, agent: &str, cancel: &AtomicBool) -> Result<(Vec<SpendRecord>,Vec<String>),String> {
    check(cancel)?;
    let conn=Connection::open_with_flags(path,OpenFlags::SQLITE_OPEN_READ_ONLY|OpenFlags::SQLITE_OPEN_NO_MUTEX)
        .map_err(|_|format!("{agent}：无法只读打开数据库 {}",path.display()))?;
    conn.busy_timeout(Duration::from_millis(150)).map_err(|_|format!("{agent}：无法设置只读等待时限"))?;
    let deadline=Instant::now()+Duration::from_secs(15);
    conn.progress_handler(10_000,Some(move||Instant::now()>=deadline));
    let schema=columns(&conn,"message")?;
    if !["id","session_id","data"].iter().all(|key|schema.contains(*key)) {return Err(format!("{agent}：不支持的 message 表结构，未编造用量"));}
    let sessions=columns(&conn,"session")?;
    let join=["id","directory"].iter().all(|key|sessions.contains(*key));
    let sql=if join {
        "SELECT m.id,m.session_id,CASE WHEN length(m.data)<=?1 THEN m.data ELSE NULL END,s.directory FROM message m LEFT JOIN session s ON s.id=m.session_id"
    }else{
        "SELECT id,session_id,CASE WHEN length(data)<=?1 THEN data ELSE NULL END,NULL FROM message"
    };
    let mut query=conn.prepare(sql).map_err(|_|format!("{agent}：无法读取消息表"))?;
    let mut rows=query.query([MAX_LINE as i64]).map_err(|_|format!("{agent}：数据库正忙或无法读取消息"))?;
    let mut records=Vec::new(); let mut notes=Vec::new(); let mut seen=HashSet::new(); let mut visited=0usize;
    loop {
        check(cancel)?;
        let row=match rows.next() { Ok(Some(row))=>row,Ok(None)=>break,Err(_)=>{notes.push(format!("{agent}：数据库读取中断，计数可能不完整"));break;} };
        visited+=1;
        if visited>MAX_RECORDS*4 || records.len()>=MAX_RECORDS { notes.push(format!("{agent}：达到数据库读取上限，计数可能不完整"));break; }
        let Some((id,session,data))=row.get::<_,String>(0).ok().zip(row.get::<_,String>(1).ok()).zip(row.get::<_,String>(2).ok()).map(|((id,session),data)|(id,session,data)) else {
            notes.push(format!("{agent}：消息超过大小限制或字段无效，计数可能不完整"));continue;
        };
        if data.len()>MAX_LINE {notes.push(format!("{agent}：消息超过大小限制，计数可能不完整"));continue;}
        let payload: Value=match serde_json::from_str(&data) {Ok(v)=>v,Err(_)=>{notes.push(format!("{agent}：消息 JSON 损坏，计数可能不完整"));continue;}};
        if payload["role"]!="assistant" {continue;}
        let tokens=&payload["tokens"];
        if !tokens.is_object() {continue;}
        let fields=[&tokens["input"],&tokens["output"],&tokens["reasoning"],&tokens["cache"]["write"],&tokens["cache"]["read"]];
        let counts=fields.map(count);
        if counts.iter().any(Option::is_none) {notes.push(format!("{agent}：Token 计数无效，计数可能不完整"));continue;}
        let values=counts.map(Option::unwrap);
        let named=fields.iter().any(|v|!v.is_null());
        let tally=Tally{input:values[0],output:values[1].saturating_add(values[2]),cache_write:values[3],cache_read:values[4]};
        let unclassified=if named {0}else{count(&tokens["total"]).unwrap_or(0)};
        if tally.total()==0 && unclassified==0 {continue;}
        let Some((model,time))=text(&payload["modelID"]).zip(at(&payload["time"]["created"])) else {
            notes.push(format!("{agent}：实测计数缺少有效模型或时间，计数可能不完整"));continue;
        };
        let sid=if agent=="micode" {text(&payload["sessionID"]).or_else(||text(&payload["session_id"])).unwrap_or(&session)}else{&session};
        if sid.trim().is_empty() || id.trim().is_empty() {notes.push(format!("{agent}：会话或消息身份无效，计数可能不完整"));continue;}
        let identity=if agent=="micode" {text(&payload["id"]).map(|id|format!("micode:embedded:{id}")).unwrap_or_else(||format!("micode:{}:{id}",path.display()))}else{format!("{agent}:message:{id}")};
        if !seen.insert(identity.clone()) {continue;}
        let local=time.with_timezone(&Local);
        let project=if agent=="micode" {text(&payload["path"]["root"]).map(str::to_owned).or_else(||row.get::<_,Option<String>>(3).ok().flatten())}else{row.get::<_,Option<String>>(3).ok().flatten()};
        records.push(SpendRecord{agent:agent.into(),model:model.into(),day:local.format("%Y-%m-%d").to_string(),hour:local.hour(),
            session:sid.into(),project,tally,unclassified_tokens:unclassified,deduplication_id:Some(identity),aggregate_timing:false,
            source_timestamp:Some(time.timestamp()),source_scope:None,cost:None,cost_breakdown:None,model_name:None});
    }
    check(cancel)?; notes.sort();notes.dedup();Ok((records,notes))
}

#[cfg(test)] mod tests {
    use super::*;
    use serde_json::json;
    struct Fixture { dir:PathBuf }
    impl Fixture {
        fn new()->Self {
            let base=PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../.test-data/sqlite");
            let dir=base.join(format!("{}-{}",std::process::id(),NEXT_ID.fetch_add(1,Ordering::Relaxed)));
            fs::create_dir_all(&dir).unwrap(); Self{dir}
        }
        fn db(&self,name:&str)->Connection {
            let conn=Connection::open(self.dir.join(name)).unwrap();
            conn.execute_batch("CREATE TABLE message(id TEXT PRIMARY KEY,session_id TEXT,data TEXT); CREATE TABLE session(id TEXT PRIMARY KEY,directory TEXT); INSERT INTO session VALUES('s','C:/project');").unwrap();conn
        }
        fn scan(&self,agent:&str,prices:&Prices)->SpendSnapshot {
            scan_roots(&self.dir.join("cache"),prices,&AtomicBool::new(false),|_,_|{},|id|if id==agent {vec![self.dir.clone()]}else{vec![]}).unwrap()
        }
    }
    impl Drop for Fixture {fn drop(&mut self){
        let base=PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../.test-data/sqlite").canonicalize();
        if let (Ok(base),Ok(dir))=(base,self.dir.canonicalize()){if dir.starts_with(base){let _=fs::remove_dir_all(dir);}}
    }}
    fn payload()->Value {json!({"role":"assistant","modelID":"model-only-on-plan","time":{"created":1_780_000_000_000u64},
        "tokens":{"input":100,"output":20,"reasoning":5,"cache":{"write":7,"read":30}},"cost":999,"content":"SENSITIVE TEXT NEVER RETAIN"})}
    fn insert(conn:&Connection,id:&str,payload:&Value) {conn.execute("INSERT INTO message VALUES(?1,'s',?2)",(id,payload.to_string())).unwrap();}
    #[test] fn production_dispatch_reads_exclusive_counts_and_never_changes_database() {
        let f=Fixture::new();let conn=f.db("opencode.db");insert(&conn,"m1",&payload());
        insert(&conn,"u1",&json!({"role":"user","tokens":{"input":999}}));drop(conn);
        let before=fs::read(f.dir.join("opencode.db")).unwrap();let scan=f.scan("opencode",&Prices::new());
        assert!(scan.notes.is_empty());assert_eq!(scan.records.len(),1);
        assert_eq!(scan.records[0].tally,Tally{input:100,output:25,cache_write:7,cache_read:30});
        assert_eq!(scan.records[0].project.as_deref(),Some("C:/project"));assert!(scan.records[0].cost.is_none());
        assert_eq!(fs::read(f.dir.join("opencode.db")).unwrap(),before);
        let normalized=serde_json::to_string(&scan).unwrap();assert!(!normalized.contains("SENSITIVE"));assert!(!normalized.contains("999"));
        assert_eq!(f.scan("opencode",&Prices::new()).sources.iter().find(|s|s.id=="opencode").unwrap().cached_files,1);
    }
    #[test] fn live_wal_commit_invalidates_cache_without_main_file_change() {
        let f=Fixture::new();let conn=f.db("opencode.db");conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA wal_autocheckpoint=0;").unwrap();
        insert(&conn,"m1",&payload());let before=fs::read(f.dir.join("opencode.db")).unwrap();
        let first=f.scan("opencode",&Prices::new());assert_eq!(first.records.len(),1);
        let reused=f.scan("opencode",&Prices::new());assert_eq!(reused.sources.iter().find(|s|s.id=="opencode").unwrap().cached_files,1);
        insert(&conn,"m2",&payload());assert_eq!(fs::read(f.dir.join("opencode.db")).unwrap(),before);
        let next=f.scan("opencode",&Prices::new());assert_eq!(next.records.len(),2);
        assert_eq!(next.sources.iter().find(|s|s.id=="opencode").unwrap().cached_files,0);
    }
    #[test] fn plan_prices_are_isolated_and_first_party_always_wins() {
        let prices=parse_prices(&json!({"opencode-go":{"models":{"model-only-on-plan":{"cost":{"input":2,"output":8}}}},
            "kilo":{"models":{"model-only-on-plan":{"cost":{"input":4,"output":16}}}}}));
        assert!(price_for_agent("model-only-on-plan","claude",&prices).is_none());
        for (agent,name,expected) in [("opencode","opencode.db",0.000474),("kilo","kilo.db",0.000948)] {
            let f=Fixture::new();let conn=f.db(name);insert(&conn,"m1",&payload());drop(conn);
            let scan=f.scan(agent,&prices);assert_eq!(scan.records.len(),1);assert!((scan.records[0].cost.unwrap()-expected).abs()<1e-10);
        }
        let mut primary=prices;primary.insert("model-only-on-plan".into(),Price{input:1.,output:3.,cache_read:None,cache_write:None,name:None});
        assert_eq!(price_for_agent("model-only-on-plan","opencode",&primary).unwrap().input,1.);
    }
    #[test] fn micode_mirrors_fold_by_embedded_identity_and_support_both_epochs() {
        let f=Fixture::new();let mut row=payload();row["id"]=json!("shared");row["time"]["created"]=json!(1_780_000_000.125);row["path"]=json!({"root":"D:/override"});
        for name in ["mimocode.db","mimocode-beta.db"] {let conn=f.db(name);insert(&conn,"row-1",&row);}
        let scan=f.scan("micode",&Prices::new());assert!(scan.notes.is_empty());assert_eq!(scan.records.len(),1);
        assert_eq!(scan.records[0].project.as_deref(),Some("D:/override"));assert_eq!(scan.records[0].source_timestamp,Some(1_780_000_000));
        assert!(at(&json!(1_780_000_000_125u64)).unwrap().timestamp_subsec_millis()==125);
    }
    #[test] fn invalid_records_are_partial_and_cannot_enter_cache() {
        let f=Fixture::new();let conn=f.db("opencode.db");insert(&conn,"m1",&payload());let mut invalid=payload();invalid["tokens"]["input"]=json!(true);insert(&conn,"bad",&invalid);drop(conn);
        let scan=f.scan("opencode",&Prices::new());assert_eq!(scan.records.len(),1);assert!(!scan.notes.is_empty());assert!(!f.dir.join("cache").exists());
    }
    #[test] fn missing_database_is_not_created_and_cancelled_read_is_empty() {
        let f=Fixture::new();let missing=f.dir.join("missing.db");assert!(read(&missing,"opencode",&AtomicBool::new(false)).is_err());assert!(!missing.exists());
        let conn=f.db("opencode.db");insert(&conn,"m1",&payload());drop(conn);
        assert!(read(&f.dir.join("opencode.db"),"opencode",&AtomicBool::new(true)).is_err());
    }
    #[test] fn optional_session_table_does_not_block_micode_reading() {
        let f=Fixture::new();let conn=f.db("mimocode.db");conn.execute_batch("DROP TABLE session").unwrap();insert(&conn,"m1",&payload());drop(conn);
        let scan=f.scan("micode",&Prices::new());assert!(scan.notes.is_empty());assert_eq!(scan.records.len(),1);assert!(scan.records[0].project.is_none());
    }
}
