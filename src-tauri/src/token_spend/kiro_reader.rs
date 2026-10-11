//! Pulse 1.9 KiroReader (Apache-2.0). CLI's explicit per-turn counters only.
//! Never infer tokens from characters, IDE state, context percentages or dollars.
use super::measured_readers::{count, record, text};
use super::*;
pub(super) fn candidate(path: &Path) -> bool {
    path.extension().is_some_and(|s| s == "json")
}
pub(super) fn read(
    path: &Path,
    cancel: &AtomicBool,
) -> Result<(Vec<SpendRecord>, Vec<String>), String> {
    let header: Value = measured_readers::document(path, cancel)?;
    let Some(turns) =
        header["session_state"]["conversation_metadata"]["user_turn_metadatas"].as_array()
    else {
        return Ok((vec![], vec![]));
    };
    let session = text(&header["session_id"]).unwrap_or_else(|| {
        path.file_stem()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned()
    });
    let model = text(&header["session_state"]["rts_model_state"]["model_info"]["model_id"])
        .unwrap_or_else(|| "auto".into());
    let mut notes = vec![];
    let mut prompts = HashMap::new();
    let sidecar = path.with_extension("jsonl");
    if sidecar.is_file() {
        let file = File::open(&sidecar).map_err(|_| "Kiro：提示时间文件无法读取。")?;
        let mut reader = BufReader::new(file);
        measured_readers::lines(&mut reader, cancel, &mut notes, |row, _| {
            if row["kind"] == "Prompt" {
                if let (Some(id), Some(at)) = (
                    text(&row["data"]["message_id"]),
                    super::aggregate_databases::dated(&row["data"]["meta"]["timestamp"]),
                ) {
                    prompts.entry(id).and_modify(|old: &mut DateTime<Utc>| *old=(*old).min(at)).or_insert(at);
                }
            }
            Ok(())
        })?;
    }
    let mut out = vec![];
    for (index, turn) in turns.iter().enumerate() {
        check(cancel)?;
        if index >= MAX_RECORDS {
            return Err("Kiro：回合数超过读取上限。".into());
        }
        let tally = Tally {
            input: count(&turn["input_token_count"]),
            output: count(&turn["output_token_count"]),
            replies_without_cache_fields: 1,
            ..Default::default()
        };
        if tally.total() == 0 {
            continue;
        }
        let at = turn["message_ids"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|id| id.as_str().and_then(|id| prompts.get(id)).copied())
            .min()
            .or_else(|| super::aggregate_databases::dated(&turn["end_timestamp"]));
        let Some(at) = at else {
            notes.push("Kiro：明确计数缺少有效时间，未分配到日期。".into());
            continue;
        };
        let mut r = record(
            "kiro",
            model.clone(),
            at,
            session.clone(),
            tally,
            0,
            Some(format!("kiro:{session}:{index}")),
        );
        r.project = text(&header["cwd"]);
        out.push(r)
    }
    Ok((out, notes))
}
