//! Durable counter-only transcript history. Ordinary numbered parse caches
//! can be rebuilt; this journal cannot, so it has its own stable filename.
//! Keep only the last complete read of each file, after cross-file dedupe.
//! Missing files claim their counted reply IDs before live copies are read.
use super::*;

// A future change to persisted counters/shape must bump this and migrate older
// journals, rather than silently dropping fields on an older application's read.
const FORMAT:u32=2;
const MAX_BYTES:u64=128*1024*1024;
pub(super) fn file_key(path:&Path)->String{
    // FNV-1a, fixed across launches/compiler upgrades (DefaultHasher's
    // algorithm is not a persisted-format guarantee).
    let mut digest=0xcbf29ce484222325u64;
    for byte in path.to_string_lossy().as_bytes(){digest^=*byte as u64;digest=digest.wrapping_mul(0x100000001b3);}
    format!("{digest:016x}")
}
#[derive(Clone,Serialize,Deserialize)]
struct Entry { readings:u32, records:Vec<SpendRecord> }
#[derive(Default,Serialize,Deserialize)]
struct Journal { format:u32, files:BTreeMap<String,Entry> }
pub(super) struct History { path:PathBuf, previous:Option<Vec<u8>>, journal:Journal }

impl History {
    pub(super) fn load(cache:&Path, agent:&str, notes:&mut Vec<String>)->Option<Self> {
        let path=cache.join(format!("history-{agent}.json"));
        let previous=match fs::metadata(&path) {
            Err(e) if e.kind()==std::io::ErrorKind::NotFound=>None,
            Ok(m) if m.len()<=MAX_BYTES=>match fs::read(&path){Ok(v)=>Some(v),Err(_)=>{notes.push(format!("{agent}：历史文件无法读取，已保留原文件"));return None}},
            _=>{notes.push(format!("{agent}：历史文件无法读取或超过上限，已保留原文件"));return None}
        };
        let journal=if let Some(bytes)=&previous {
            match serde_json::from_slice::<Journal>(bytes) {
                Ok(mut v) if (1..=FORMAT).contains(&v.format) && v.files.values().all(|e| e.records.len()<=MAX_RECORDS && e.records.iter().all(|r|
                    r.tally.valid() && r.agent==agent && r.hour<24 && !r.model.trim().is_empty()
                    && chrono::NaiveDate::parse_from_str(&r.day,"%Y-%m-%d").is_ok()
                    && (r.tally.total()>0 || r.unclassified_tokens>0)))=>{if v.format==1{for e in v.files.values_mut(){for r in &mut e.records{migrate_timestamp(r);}}}v.format=FORMAT;v},
                _=>{notes.push(format!("{agent}：历史文件损坏或格式较新，已保留原文件；本次仅显示可读取的源记录"));return None}
            }
        } else { Journal{format:FORMAT,..Default::default()} };
        if agent=="codex"&&journal.files.values().any(|e|e.readings<8&&e.records.iter().any(|r|r.source_scope.is_none())){notes.push("Codex：早期历史缓存没有分叉来源标记；仍在的源文件会重新校正，已删除源文件的继承量无法核验。".into());}
        Some(Self{path,previous,journal})
    }

    pub(super) fn reconcile(&mut self, mut live:Vec<(String,Vec<SpendRecord>)>, stable:bool,
        identities:&mut HashSet<String>, cancel:&AtomicBool)->Result<(Vec<SpendRecord>,usize),String> {
        let paths:HashSet<_>=live.iter().map(|(p,_)|p.as_str()).collect();
        let mut out=Vec::new();
        // Kept records retain ownership even if a resumed/forked transcript
        // contains copies. A returned file is read live, replacing its old row.
        let mut kept=self.journal.files.iter().collect::<Vec<_>>();kept.sort_by_key(|(path,entry)|(entry.records.iter().any(is_codex_fork),path.as_str()));
        for (path,entry) in kept {
            if paths.contains(path.as_str()){continue}
            for record in &entry.records {
                check(cancel)?;
                if claim(record,identities){out.push(record.clone())}
            }
        }
        let retained=out.len();
        order_live(&mut live);
        for (path,records) in live {
            check(cancel)?;
            let counted:Vec<_>=records.into_iter().filter(|r|claim(r,identities)).collect();
            out.extend(counted.iter().cloned());
            if stable { self.journal.files.insert(path,Entry{readings:TRANSCRIPT_VERSION,records:counted}); }
        }
        Ok((out,retained))
    }

    pub(super) fn save(self, cancel:&AtomicBool)->Result<(),String> {
        check(cancel)?;
        if self.journal.files.values().all(|entry|entry.records.is_empty()) && self.previous.is_none(){return Ok(())}
        let bytes=serde_json::to_vec(&self.journal).map_err(|e|e.to_string())?;
        if self.previous.as_deref()==Some(bytes.as_slice()){return Ok(())}
        check(cancel)?;
        // The existing compare-and-replace writer flushes then atomically
        // replaces on Windows; a damaged/newer journal is never passed here.
        crate::credential_store::atomic_write_bounded(&self.path,&bytes,self.previous.as_deref(),MAX_BYTES).map_err(|e|e.to_string())
    }
}

pub(super) fn claim(record:&SpendRecord,identities:&mut HashSet<String>)->bool {
    if let Some(scope)=record.source_scope.as_deref(){
        if let Some(total)=scope.strip_prefix("codex-fork:"){if identities.contains(&format!("codex-total:{total}")){return false}}
        if let Some(total)=scope.strip_prefix("codex-original:").or_else(||scope.strip_prefix("codex-fork:")){identities.insert(format!("codex-total:{total}"));}
    }
    record.deduplication_id.as_ref().map(|id|identities.insert(id.clone())).unwrap_or(true)
}
fn is_codex_fork(r:&SpendRecord)->bool{r.source_scope.as_deref().is_some_and(|s|s.starts_with("codex-fork:"))}
pub(super) fn order_live(live:&mut [(String,Vec<SpendRecord>)]){
    live.sort_by(|(a,ar),(b,br)| (ar.iter().any(is_codex_fork),ar.first().map(|r|(&r.day,r.hour)),a).cmp(&(br.iter().any(is_codex_fork),br.first().map(|r|(&r.day,r.hour)),b)));
}

/// Stores read as whole ledgers can delete individual rows. Keep the largest
/// complete tally per UTC quarter-hour/model (or local day for aggregates),
/// adding only the part beyond the live tally. Never invent session metadata.
#[derive(Serialize,Deserialize)]
struct Marks { format:u32, readings:u32, offset:i32, records:Vec<SpendRecord> }
pub(super) struct StoreHistory { path:PathBuf, previous:Option<Vec<u8>>, marks:Marks }
impl StoreHistory {
    pub(super) fn load(cache:&Path,agent:&str,notes:&mut Vec<String>)->Option<Self>{
        let path=cache.join(format!("history-store-{agent}.json"));
        let previous=match fs::metadata(&path){
            Err(e) if e.kind()==std::io::ErrorKind::NotFound=>None,
            Ok(m) if m.len()<=MAX_BYTES=>fs::read(&path).ok(),
            _=>{notes.push(format!("{agent}：历史文件无法读取或超过上限，已保留原文件"));return None}
        };
        if path.exists()&&previous.is_none(){notes.push(format!("{agent}：历史文件无法读取，已保留原文件"));return None}
        let marks=if let Some(bytes)=&previous {
            match serde_json::from_slice::<Marks>(bytes){
                Ok(mut m) if (1..=FORMAT).contains(&m.format) && m.records.len()<=MAX_RECORDS && (-86400..=86400).contains(&m.offset) && m.records.iter().all(|r|
                    r.tally.valid() && r.agent==agent && r.hour<24 && !r.model.trim().is_empty()
                    && chrono::NaiveDate::parse_from_str(&r.day,"%Y-%m-%d").is_ok()
                    && (r.tally.total()>0 || r.unclassified_tokens>0))=>{if m.format==1{for r in &mut m.records{migrate_timestamp(r);}}m.format=FORMAT;m},
                _=>{notes.push(format!("{agent}：历史文件损坏或格式较新，已保留原文件；本次仅显示源记录"));return None}
            }
        }else{Marks{format:FORMAT,readings:TRANSCRIPT_VERSION,offset:Local::now().offset().local_minus_utc(),records:vec![]}};
        Some(Self{path,previous,marks})
    }
    pub(super) fn reconcile(&mut self,live:&mut Vec<SpendRecord>,stable:bool,cancel:&AtomicBool)->Result<usize,String>{
        let now_offset=Local::now().offset().local_minus_utc();
        let days:HashSet<_>=live.iter().map(|r|r.day.clone()).collect();
        let mut old=BTreeMap::new();
        let before=live.len();
        for mut r in self.marks.records.clone(){
            check(cancel)?;
            let moved=now_offset!=self.marks.offset;
            if let Some(ts)=r.source_timestamp{
                if let Some(date)=DateTime::from_timestamp(ts,0){let date=date.with_timezone(&Local);r.day=date.format("%Y-%m-%d").to_string();r.hour=chrono::Timelike::hour(&date);}
            }else if moved{
                // A local aggregate has no instant. Map its old noon to the
                // new zone and suppress overlap on neighbouring live days.
                let noon=chrono::NaiveDate::parse_from_str(&r.day,"%Y-%m-%d").unwrap().and_hms_opt(12,0,0).unwrap();
                let ts=noon.and_utc().timestamp()-self.marks.offset as i64;
                let date=DateTime::from_timestamp(ts,0).unwrap().with_timezone(&Local);
                r.day=date.format("%Y-%m-%d").to_string();
                r.aggregate_timing=true;r.hour=0;
                if (-1..=1).any(|delta|days.contains(&(date+chrono::Duration::days(delta)).format("%Y-%m-%d").to_string())){continue}
            }
            // Reader fixes can legitimately lower or reclassify counters.
            // Old marks stand only for a day the new reader sees nothing on.
            if self.marks.readings!=TRANSCRIPT_VERSION&&days.contains(&r.day){continue}
            old.insert(mark_key(&r),r);
        }
        let current=totals(live);
        let mut next=old.clone();
        for (key,r) in &current{
            let held=next.entry(key.clone()).or_insert_with(||r.clone());
            highest(&mut held.tally,&r.tally);held.unclassified_tokens=held.unclassified_tokens.max(r.unclassified_tokens);
        }
        for (key,held) in &old {
            check(cancel)?;
            let mut added=held.clone();let empty=Tally::default();let base=current.get(key);
            let tally=base.map(|r|&r.tally).unwrap_or(&empty);
            added.tally=held.tally.removing(tally);
            added.unclassified_tokens=held.unclassified_tokens.saturating_sub(base.map(|r|r.unclassified_tokens).unwrap_or(0));
            if added.tally.total()>0||added.unclassified_tokens>0{live.push(added)}
        }
        if stable{self.marks=Marks{format:FORMAT,readings:TRANSCRIPT_VERSION,offset:now_offset,records:next.into_values().collect()};}
        Ok(live.len()-before)
    }
    pub(super) fn save(self,cancel:&AtomicBool)->Result<(),String>{
        check(cancel)?;
        if self.marks.records.is_empty()&&self.previous.is_none(){return Ok(())}
        let bytes=serde_json::to_vec(&self.marks).map_err(|e|e.to_string())?;
        if self.previous.as_deref()==Some(bytes.as_slice()){return Ok(())}
        check(cancel)?;
        crate::credential_store::atomic_write_bounded(&self.path,&bytes,self.previous.as_deref(),MAX_BYTES).map_err(|e|e.to_string())
    }
}
fn mark_key(r:&SpendRecord)->(String,String){
    let time=if !r.aggregate_timing {r.source_timestamp.map(|t|format!("utc:{}",t.div_euclid(900))).unwrap_or_else(||format!("hour:{}:{}",r.day,r.hour))}else{format!("day:{}",r.day)};
    (time,r.model.clone())
}
fn highest(a:&mut Tally,b:&Tally){a.input=a.input.max(b.input);a.output=a.output.max(b.output);a.cache_write=a.cache_write.max(b.cache_write);a.cache_read=a.cache_read.max(b.cache_read);a.cache_write_1h=a.cache_write_1h.max(b.cache_write_1h).min(a.cache_write);}
fn migrate_timestamp(r:&mut SpendRecord){
    // Format 1 mixed seconds (transcript/OpenCode) and milliseconds (exports,
    // Gemini/Harness). Format 2 and the native contract use epoch seconds.
    if matches!(r.agent.as_str(),"gemini"|"cursor"|"antigravity"|"hindsight"|"mcode"|"dsh"){
        if let Some(ts)=r.source_timestamp{r.source_timestamp=Some(ts.div_euclid(1000));}
    }
}
fn totals(records:&[SpendRecord])->BTreeMap<(String,String),SpendRecord>{
    let mut sums=BTreeMap::new();
    for r in records{
        let sum=sums.entry(mark_key(r)).or_insert_with(||{
            let mut r=r.clone();r.session.clear();r.project=None;r.deduplication_id=None;r.source_scope=Some("retained-store-total".into());
            r.cost=None;r.cache_savings=None;r.cost_breakdown=None;r.model_name=None;r.tally=Tally::default();r.unclassified_tokens=0;r
        });
        sum.tally.add(&r.tally);sum.unclassified_tokens=sum.unclassified_tokens.saturating_add(r.unclassified_tokens);
    }
    sums
}
