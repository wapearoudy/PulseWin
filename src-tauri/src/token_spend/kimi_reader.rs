//! Pulse v1.9 KimiCLIStore (Apache-2.0): wire counters have no model name.
use super::measured_readers::{count, record};
use super::*;
pub(super) fn candidate(path:&Path)->bool {path.file_name().is_some_and(|n|n=="wire.jsonl")}
pub(super) fn read(path:&Path,cancel:&AtomicBool)->Result<(Vec<SpendRecord>,Vec<String>),String>{
    check(cancel)?;
    let mut reader=BufReader::new(File::open(path).map_err(|e|e.to_string())?);
    let session=path.parent().unwrap_or(path).to_string_lossy().into_owned();
    let mut out=Vec::new();let mut notes=Vec::new();
    measured_readers::lines(&mut reader,cancel,&mut notes,|row,ordinal|{
        let usage=&row["message"]["payload"]["token_usage"];
        if !usage.is_object(){return Ok(())}
        let tally=Tally{input:count(&usage["input_other"]),output:count(&usage["output"]),cache_read:count(&usage["input_cache_read"]),cache_write:count(&usage["input_cache_creation"]),replies_without_cache_fields:u64::from(usage.get("input_cache_read").is_none()&&usage.get("input_cache_creation").is_none()),..Default::default()};
        if tally.total()==0{return Ok(())}
        let Some(at)=aggregate_databases::dated(&row["timestamp"])else{return Ok(())};
        if out.len()>=MAX_RECORDS{return Err("Kimi CLI：记录超过读取上限。".into())}
        out.push(record("kimi","kimi (unnamed)".into(),at,session.clone(),tally,0,Some(format!("kimi:{session}:{ordinal}"))));Ok(())
    })?;Ok((out,notes))
}
