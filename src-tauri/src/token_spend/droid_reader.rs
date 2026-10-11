//! Factory session totals, following Pulse 1.9 DroidSessionReader (Apache-2.0).
//! No per-reply or hourly allocation is inferred from transcript text.
use super::measured_readers::{count, record, text, timestamp};
use super::*;
fn value(v: &Value) -> Option<u64> {
    v.as_u64().or_else(|| v.as_str()?.parse().ok())
}
fn decode(u: &Value) -> (Tally, u64, bool) {
    let i = count(&u["inputTokens"]);
    let o = count(&u["outputTokens"]);
    let k = count(&u["thinkingTokens"]);
    let cw = count(&u["cacheCreationTokens"]);
    let cr = count(&u["cacheReadTokens"]);
    let absent = u.get("cacheReadTokens").is_none() && u.get("cacheCreationTokens").is_none();
    if let Some(total) = ["totalTokens", "total", "total_tokens"]
        .iter()
        .find_map(|key| value(&u[*key]))
    {
        for (input, output) in [
            (i, o.saturating_add(k)),
            (i.saturating_sub(cr.saturating_add(cw)), o.saturating_add(k)),
            (i, o),
            (i.saturating_sub(cr.saturating_add(cw)), o),
        ] {
            let t = Tally {
                input,
                output,
                cache_write: cw,
                cache_read: cr,
                replies_without_cache_fields: u64::from(absent),
                ..Default::default()
            };
            if t.total() == total {
                return (t, 0, false);
            }
        }
        return (Tally::default(), total, false);
    }
    let output = if u.get("outputTokens").is_some() {
        o
    } else {
        k
    };
    if cw == 0 && cr == 0 {
        (
            Tally {
                input: i,
                output,
                replies_without_cache_fields: u64::from(absent),
                ..Default::default()
            },
            0,
            k > 0,
        )
    } else {
        (
            Tally {
                output,
                replies_without_cache_fields: 1,
                ..Default::default()
            },
            i,
            true,
        )
    }
}
fn normalize(v: &str) -> Option<String> {
    let v = v.strip_prefix("custom:").unwrap_or(v);
    let mut result = String::new();
    let mut bracket = false;
    for ch in v.chars() {
        match ch {
            '[' => bracket = true,
            ']' => bracket = false,
            _ if !bracket => result.push(ch),
            _ => {}
        }
    }
    text(&Value::String(result))
}
fn transcript_model(path: &Path, cancel: &AtomicBool) -> Result<Option<String>, String> {
    let Some(name) = path
        .file_name()
        .and_then(|s| s.to_str())
        .and_then(|s| s.strip_suffix(".settings.json"))
    else {
        return Ok(None);
    };
    let sibling = path.with_file_name(format!("{name}.jsonl"));
    let file = match fs::File::open(sibling) {
        Ok(f) => f,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(_) => return Err("Droid：模型所属会话无法读取。".into()),
    };
    let mut result = None;
    let mut notes = vec![];
    measured_readers::lines(&mut BufReader::new(file), cancel, &mut notes, |row, _| {
        if result.is_none() {
            let raw = row.to_string();
            if let Some((_, tail)) = raw.split_once("Model:") {
                let name = tail
                    .trim_start()
                    .split(|c: char| c.is_whitespace() || ['<', '"', '\\'].contains(&c))
                    .next()
                    .unwrap_or("");
                result = normalize(name)
            }
        }
        Ok(())
    })?;
    if !notes.is_empty() {
        return Err("Droid：会话模型信息不完整。".into());
    }
    Ok(result)
}
pub(super) fn parse(
    path: &Path,
    cancel: &AtomicBool,
) -> Result<(Vec<SpendRecord>, Vec<String>), String> {
    let root: Value = measured_readers::document(path, cancel)?;
    let usage = &root["tokenUsage"];
    if !usage.is_object() {
        return Ok((vec![], vec![]));
    }
    let (tally, bare, partial) = decode(usage);
    if tally.total() == 0 && bare == 0 {
        return Ok((vec![], vec![]));
    }
    let Some(at) = timestamp(&root["providerLockTimestamp"], false) else {
        return Ok((
            vec![],
            vec!["Droid：会话总量缺少有效时间，未以文件时间代替。".into()],
        ));
    };
    let reported = text(&root["model"]).and_then(|s| normalize(&s));
    let model = if reported.is_some() {
        reported
    } else {
        transcript_model(path, cancel)?
    };
    let model = model.or_else(|| {
        text(&root["providerLock"]).map(|s| {
            let low = s.to_lowercase();
            let p = if low.contains("anthropic") || low.contains("claude") {
                "claude"
            } else if low.contains("openai") || low.contains("gpt") {
                "gpt"
            } else if low.contains("google") || low.contains("gemini") {
                "gemini"
            } else if low.contains("xai") || low.contains("grok") {
                "grok"
            } else {
                &low
            };
            format!("{p}-unknown")
        })
    });
    let Some(model) = model else {
        return Ok((
            vec![],
            vec!["Droid：会话缺少可定位的模型，未猜测费用。".into()],
        ));
    };
    let session = path
        .file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .trim_end_matches(".settings.json")
        .to_owned();
    let mut r = record(
        "droid",
        model,
        at,
        session.clone(),
        tally,
        bare,
        Some(format!("droid:{session}")),
    );
    r.aggregate_timing = true;
    Ok((
        vec![r],
        if partial {
            vec!["Droid：未报告可证明缓存/思考关系的总量，仅展示能确认的部分计数。".into()]
        } else {
            vec![]
        },
    ))
}
#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn reported_totals_settle_cache_and_reasoning_overlap() {
        for (total, input, output) in [(160, 100, 30), (130, 70, 30), (150, 100, 20), (120, 70, 20)]
        {
            let (t, b, p) = decode(
                &json!({"inputTokens":100,"outputTokens":20,"thinkingTokens":10,"cacheReadTokens":20,"cacheCreationTokens":10,"totalTokens":total}),
            );
            assert_eq!((t.input, t.output, b, p), (input, output, 0, false))
        }
        let (t, b, p) = decode(&json!({"inputTokens":100,"cacheReadTokens":20,"totalTokens":999}));
        assert_eq!((t.total(), b, p), (0, 999, false))
    }
    #[test]
    fn absent_total_does_not_guess_cache_or_add_thinking_twice() {
        let (t, b, p) = decode(
            &json!({"inputTokens":100,"outputTokens":20,"thinkingTokens":10,"cacheReadTokens":40}),
        );
        assert_eq!((t.total(), b, p), (20, 100, true));
        let (t, b, p) = decode(&json!({"inputTokens":100,"outputTokens":20,"thinkingTokens":10}));
        assert_eq!((t.total(), b, p), (120, 0, true));
    }
    #[test]
    fn model_normalization_keeps_case_and_punctuation() {
        assert_eq!(
            normalize("custom:claude-opus-4.5[high]").as_deref(),
            Some("claude-opus-4.5")
        );
        assert_eq!(normalize("GPT-5.6").as_deref(), Some("GPT-5.6"))
    }
}
