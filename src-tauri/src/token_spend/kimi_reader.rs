//! Pulse v1.12 KimiCLIStore: legacy wire counters plus the Kimi Code store.
//!
//! `~/.kimi/sessions/<hash>/<uuid>/wire.jsonl` holds a `token_usage` block per
//! reply and never names the model. Kimi Code moved to
//! `~/.kimi-code/sessions/wd_<project>_<hash>/session_<uuid>/agents/<agent>/wire.jsonl`,
//! where every request is written three times and only the `usage.record` line
//! carries the counters, next to the provider-prefixed model and a millisecond
//! timestamp. A migrated request exists in both stores; the original folds them
//! by "same second and four counters", so the deduplication identity does too.
use super::measured_readers::{count, record, text, timestamp};
use super::*;
pub(super) fn candidate(path:&Path)->bool {path.file_name().is_some_and(|n|n=="wire.jsonl")}
fn session_of(path:&Path)->String{
    for ancestor in path.ancestors(){let name=ancestor.file_name().map(|n|n.to_string_lossy().into_owned()).unwrap_or_default();if name.starts_with("session_")||name.starts_with("wd_"){return name}}
    path.parent().unwrap_or(path).to_string_lossy().into_owned()
}
fn identity(at:DateTime<Utc>,tally:&Tally)->String{format!("kimi:{}:{}:{}:{}:{}",at.timestamp(),tally.input,tally.output,tally.cache_read,tally.cache_write)}
fn tally_of(usage:&Value,input:&str,output:&str,read:&str,write:&str)->Tally{
    Tally{input:count(&usage[input]),output:count(&usage[output]),cache_read:count(&usage[read]),cache_write:count(&usage[write]),replies_without_cache_fields:u64::from(usage.get(read).is_none()&&usage.get(write).is_none()),..Default::default()}
}
pub(super) fn read(path:&Path,cancel:&AtomicBool)->Result<(Vec<SpendRecord>,Vec<String>),String>{
    check(cancel)?;
    let mut reader=BufReader::new(File::open(path).map_err(|e|e.to_string())?);
    let session=session_of(path);
    let mut out=Vec::new();let mut notes=Vec::new();
    measured_readers::lines(&mut reader,cancel,&mut notes,|row,_ordinal|{
        let code=row["type"]=="usage.record";
        let (tally,at,model)=if code{
            let usage=&row["usage"];
            if !usage.is_object(){return Ok(())}
            let tally=tally_of(usage,"inputOther","output","inputCacheRead","inputCacheCreation");
            let Some(at)=timestamp(&row["time"],true)else{return Ok(())};
            let model=text(&row["model"]).map(|name|name.rsplit('/').next().unwrap_or(&name).to_owned()).unwrap_or_else(||"kimi (unnamed)".into());
            (tally,at,model)
        }else{
            let usage=&row["message"]["payload"]["token_usage"];
            if !usage.is_object(){return Ok(())}
            let tally=tally_of(usage,"input_other","output","input_cache_read","input_cache_creation");
            let Some(at)=aggregate_databases::dated(&row["timestamp"])else{return Ok(())};
            (tally,at,"kimi (unnamed)".to_owned())
        };
        if tally.total()==0{return Ok(())}
        if out.len()>=MAX_RECORDS{return Err("Kimi CLI：记录超过读取上限。".into())}
        // One identity across both stores: a migrated request keeps its second
        // and four counters, so the legacy copy and the Kimi Code copy fold.
        let id=identity(at,&tally);
        out.push(record("kimi",model,at,session.clone(),tally,0,Some(id)));Ok(())
    })?;Ok((out,notes))
}
