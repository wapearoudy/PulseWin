//! Pi/omp/Kimchi and Amp formats, from Pulse 1.9's Group A readers.
use super::measured_readers::{count, record, text, timestamp};
use super::*;
pub(super) fn agent(id: &str) -> bool {
    matches!(id, "pi" | "omp" | "kimchi" | "amp" | "droid")
}
pub(super) fn roots(home: &Path, id: &str, env: &impl Fn(&str) -> Option<String>) -> Vec<PathBuf> {
    vec![match id {
        "pi" => home.join(".pi/agent/sessions"),
        "omp" => home.join(".omp/agent/sessions"),
        "kimchi" => env("KIMCHI_CODING_AGENT_DIR")
            .filter(|v| !v.trim().is_empty())
            .map(|v| {
                if let Some(tail) = v.strip_prefix("~/").or_else(|| v.strip_prefix("~\\")) {
                    home.join(tail)
                } else if v == "~" {
                    home.into()
                } else {
                    PathBuf::from(v)
                }
            })
            .map(|p| p.join("sessions"))
            .unwrap_or_else(|| home.join(".config/kimchi/harness/sessions")),
        "amp" => home.join(".local/share/amp/threads"),
        "droid" => home.join(".factory/sessions"),
        _ => return vec![],
    }]
}
pub(super) fn candidate(path: &Path, id: &str) -> bool {
    if id == "droid" {
        path.file_name()
            .is_some_and(|n| n.to_string_lossy().ends_with(".settings.json"))
    } else if id == "amp" {
        path.extension().is_some_and(|e| e == "json")
            && path
                .file_name()
                .is_some_and(|n| n.to_string_lossy().starts_with("T-"))
    } else {
        path.extension().is_some_and(|e| e == "jsonl")
    }
}
fn pi_usage(u: &Value) -> (Tally, u64) {
    let tally = Tally {
        input: count(&u["input"]),
        output: count(&u["output"]),
        cache_write: count(&u["cacheWrite"]),
        cache_read: count(&u["cacheRead"]),
        replies_without_cache_fields: u64::from(
            u.get("cacheRead").is_none() && u.get("cacheWrite").is_none(),
        ),
        ..Default::default()
    };
    let rest = count(&u["totalTokens"]).saturating_sub(tally.total());
    (tally, rest)
}
pub(super) fn parse(
    path: &Path,
    id: &str,
    cancel: &AtomicBool,
) -> Result<(Vec<SpendRecord>, Vec<String>), String> {
    if id == "droid" {
        return super::droid_reader::parse(path, cancel);
    }
    if id == "amp" {
        let root: Value = measured_readers::document(path, cancel)?;
        return Ok(amp(
            &root,
            path.file_stem()
                .unwrap_or_default()
                .to_string_lossy()
                .as_ref(),
        ));
    }
    let mut out = vec![];
    let mut notes = vec![];
    let mut header = None::<String>;
    let mut cwd = None;
    let mut invalid = false;
    let file = fs::File::open(path).map_err(|_| "会话文件无法读取。")?;
    let mut reader = BufReader::new(file);
    let mut own_notes = vec![];
    measured_readers::lines(&mut reader, cancel, &mut own_notes, |row, _| {
        if invalid {
            return Ok(());
        }
        if header.is_none() {
            if row["type"] == "title" {
                return Ok(());
            }
            if row["type"] != "session" || text(&row["id"]).is_none() {
                invalid = true;
                notes.push(format!("{id}：会话缺少有效 session 头，未归属用量。"));
                return Ok(());
            }
            header = text(&row["id"]);
            cwd = text(&row["cwd"]);
            return Ok(());
        }
        let m = &row["message"];
        if row["type"] != "message" || m["role"] != "assistant" || !m["usage"].is_object() {
            return Ok(());
        }
        let (tally, bare) = pi_usage(&m["usage"]);
        if tally.total() == 0 && bare == 0 {
            return Ok(());
        }
        let Some(at) =
            timestamp(&row["timestamp"], false).or_else(|| timestamp(&m["timestamp"], false))
        else {
            notes.push(format!("{id}：用量缺少有效时间。"));
            return Ok(());
        };
        let Some(model) = text(&m["model"]) else {
            notes.push(format!("{id}：用量缺少模型，未猜测费用。"));
            return Ok(());
        };
        let session = header.clone().unwrap();
        let identity = if id == "kimchi" {
            text(&row["id"]).map(|mid| format!("{id}:{session}:{mid}"))
        } else {
            Some(if let Some(rid) = text(&m["responseId"]) {
                format!("{id}:response:{rid}")
            } else {
                format!(
                    "{id}:message:{}:{}:{}:{}:{}:{}:{}:{}",
                    text(&row["id"]).unwrap_or_default(),
                    at.timestamp_millis(),
                    text(&m["provider"]).unwrap_or_else(|| if id == "omp" {
                        "omp".into()
                    } else {
                        String::new()
                    }),
                    model,
                    tally.input,
                    tally.output,
                    tally.cache_read,
                    tally.cache_write
                )
            })
        };
        let mut r = record(id, model, at, session, tally, bare, identity);
        r.project = cwd.clone();
        out.push(r);
        if out.len() >= MAX_RECORDS {
            return Err("会话超过记录上限。".into());
        }
        Ok(())
    })?;
    notes.extend(own_notes);
    if invalid {
        out.clear()
    }
    Ok((out, notes))
}
fn amp_tally(v: &Value, event: bool) -> Tally {
    Tally {
        input: count(&v[if event { "input" } else { "inputTokens" }]),
        output: count(&v[if event { "output" } else { "outputTokens" }]),
        cache_write: count(&v["cacheCreationInputTokens"]),
        cache_read: count(&v["cacheReadInputTokens"]),
        replies_without_cache_fields: u64::from(
            v.get("cacheReadInputTokens").is_none() && v.get("cacheCreationInputTokens").is_none(),
        ),
        ..Default::default()
    }
}
fn amp(root: &Value, stem: &str) -> (Vec<SpendRecord>, Vec<String>) {
    let session = text(&root["id"]).unwrap_or_else(|| stem.into());
    let created = timestamp(&root["created"], true);
    let mut out = vec![];
    let mut notes = vec![];
    let mut events = vec![];
    if let Some(rows) = root["usageLedger"]["events"].as_array() {
        for (i, row) in rows.iter().enumerate() {
            let tally = amp_tally(&row["tokens"], true);
            if tally.total() == 0 {
                continue;
            }
            if let Some(model) = text(&row["model"]) {
                events.push((i, row, model, tally, false))
            } else {
                notes.push("Amp：事件缺少模型。".into())
            }
        }
    }
    let mut unmatched = vec![];
    if let Some(rows) = root["messages"].as_array() {
        for (i, row) in rows.iter().enumerate() {
            if row["role"] != "assistant" || !row["usage"].is_object() {
                continue;
            }
            let tally = amp_tally(&row["usage"], false);
            if tally.total() == 0 {
                continue;
            }
            let Some(model) = text(&row["usage"]["model"]) else {
                notes.push("Amp：消息用量缺少模型。".into());
                continue;
            };
            let matched = row["messageId"]
                .as_u64()
                .and_then(|id| {
                    events.iter().position(|(_, e, _, _, used)| {
                        !*used && e["toMessageId"].as_u64() == Some(id)
                    })
                })
                .or_else(|| {
                    events.iter().position(|(_, _, m, t, used)| {
                        !*used
                            && m == &model
                            && [t.input, t.output, t.cache_write, t.cache_read]
                                == [
                                    tally.input,
                                    tally.output,
                                    tally.cache_write,
                                    tally.cache_read,
                                ]
                    })
                });
            if let Some(i) = matched {
                events[i].4 = true
            } else {
                unmatched.push((i, row, model, tally))
            }
        }
    }
    for (i, row, model, tally, _) in events {
        let stamped = timestamp(&row["timestamp"], false);
        let Some(at) = stamped.or(created) else {
            notes.push("Amp：事件缺少有效时间。".into());
            continue;
        };
        let suffix = row["toMessageId"]
            .as_u64()
            .or(row["fromMessageId"].as_u64())
            .map(|n| n.to_string())
            .unwrap_or_else(|| i.to_string());
        let mut r = record(
            "amp",
            model,
            at,
            session.clone(),
            tally,
            0,
            Some(format!("amp:{session}:event:{suffix}")),
        );
        r.aggregate_timing = stamped.is_none();
        out.push(r)
    }
    for (i, row, model, tally) in unmatched {
        let Some(at) = created else {
            notes.push("Amp：消息缺少报告时间，未均摊到小时。".into());
            continue;
        };
        let suffix = row["messageId"]
            .as_u64()
            .map(|n| n.to_string())
            .unwrap_or_else(|| i.to_string());
        let mut r = record(
            "amp",
            model,
            at,
            session.clone(),
            tally,
            0,
            Some(format!("amp:{session}:message:{suffix}")),
        );
        r.aggregate_timing = true;
        out.push(r)
    }
    (out, notes)
}
#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn pi_buckets_do_not_double_count_reasoning_and_carry_bare_remainder() {
        let (t, b) = pi_usage(
            &json!({"input":100,"output":20,"reasoning":10,"cacheRead":30,"cacheWrite":5,"totalTokens":160}),
        );
        assert_eq!(t.total(), 155);
        assert_eq!(t.output, 20);
        assert_eq!(b, 5);
        let (t, b) = pi_usage(&json!({"totalTokens":200}));
        assert_eq!(t.total(), 0);
        assert_eq!(b, 200);
    }
    #[test]
    fn amp_ledger_and_message_are_one_call_and_unmatched_messages_remain_aggregate() {
        let root = json!({"id":"T-thread","created":1790812800000i64,"messages":[{"role":"assistant","messageId":7,"usage":{"model":"m","inputTokens":100,"outputTokens":20}},{"role":"assistant","messageId":8,"usage":{"model":"m","inputTokens":50,"outputTokens":10}}],"usageLedger":{"events":[{"toMessageId":7,"timestamp":"2026-10-01T00:01:00Z","model":"m","tokens":{"input":100,"output":20}}]}});
        let (r, n) = amp(&root, "fallback");
        assert!(n.is_empty());
        assert_eq!(r.len(), 2);
        assert_eq!(r.iter().map(|r| r.tally.total()).sum::<u64>(), 180);
        assert!(!r[0].aggregate_timing);
        assert!(r[1].aggregate_timing);
        assert!(r[0]
            .deduplication_id
            .as_deref()
            .unwrap()
            .contains("event:7"));
    }
    #[test]
    fn amp_missing_time_does_not_use_file_time_and_equal_counts_match_without_ids() {
        let root = json!({"messages":[{"role":"assistant","usage":{"model":"m","inputTokens":100,"outputTokens":20}}],"usageLedger":{"events":[{"model":"m","tokens":{"input":100,"output":20}}]}});
        let (r, n) = amp(&root, "T-thread");
        assert!(r.is_empty());
        assert_eq!(n.len(), 1);
    }
    #[test]
    fn shared_pi_environment_is_not_counted_as_two_tools() {
        let env = |key: &str| {
            if key == "PI_CODING_AGENT_DIR" {
                Some("C:/shared".into())
            } else {
                None
            }
        };
        assert_eq!(
            roots(Path::new("C:/home"), "pi", &env),
            vec![PathBuf::from("C:/home/.pi/agent/sessions")]
        );
        assert_eq!(
            roots(Path::new("C:/home"), "omp", &env),
            vec![PathBuf::from("C:/home/.omp/agent/sessions")]
        );
    }
}
