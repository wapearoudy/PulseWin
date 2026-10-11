//! Prompt-cache expiry from logged Claude tiers or Codex's documented minimum.
use serde::Serialize;
use serde_json::Value;
use std::{
    collections::HashMap,
    fs,
    io::{Read, Seek, SeekFrom},
    path::{Path, PathBuf},
};
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Session {
    id: String,
    title: String,
    project: Option<String>,
    last_request: String,
    expires_at: String,
    lifetime: u32,
    is_minimum: bool,
}
#[derive(Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Reading {
    live: Vec<Session>,
    last_lapsed: Option<Session>,
    notes: Vec<String>,
}
fn timestamp(v: &Value) -> Option<i64> {
    v["timestamp"]
        .as_str()
        .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
        .map(|d| d.timestamp())
}
fn lifetime(model: &str) -> bool {
    if !model.starts_with("gpt-") || model.contains("auto-review") {
        return false;
    }
    let digits = model[4..]
        .chars()
        .take_while(|c| c.is_ascii_digit() || *c == '.')
        .collect::<String>();
    let mut parts = digits.split('.');
    let Some(major) = parts.next().and_then(|s| s.parse::<u32>().ok()) else {
        return false;
    };
    let minor = parts
        .next()
        .and_then(|s| s.parse::<u32>().ok())
        .unwrap_or(0);
    major > 5 || major == 5 && minor >= 6
}
fn input(p: &Value) -> bool {
    p["role"] == "user"
        || p["type"]
            .as_str()
            .is_some_and(|s| s.ends_with("_call_output"))
}
fn codex_usage(v: &Value) -> &Value {
    if v["type"] == "token_usage_record" {
        &v["payload"]["usage"]
    } else {
        &v["payload"]["info"]["last_token_usage"]
    }
}
fn codex_model(v: &Value) -> Option<&str> {
    if v["type"] == "turn_context" {
        v["payload"]["model"].as_str()
    } else if v["type"] == "session_meta" {
        v["payload"]["base_instructions"]["provenance"]["model"].as_str()
    } else {
        None
    }
}
fn lapse(lines: &[Value], provider: &str) -> Option<(i64, u32, bool)> {
    if provider == "codex" {
        let i = lines.iter().rposition(|v| {
            let p = &v["payload"];
            (p["type"] == "token_count" || v["type"] == "token_usage_record")
                && ["cached_input_tokens", "cache_write_input_tokens"]
                    .iter()
                    .any(|k| codex_usage(v)[*k].as_u64().unwrap_or(0) > 0)
        })?;
        let answered = timestamp(&lines[i])?;
        let model = lines[..i].iter().rev().find_map(codex_model)?;
        if !lifetime(model) {
            return None;
        }
        let sent = lines[..i]
            .iter()
            .rev()
            .find(|v| {
                (v["type"] == "response_item" && input(&v["payload"]))
                    || (v["type"] == "event_msg"
                        && ["user_message", "task_started"]
                            .contains(&v["payload"]["type"].as_str().unwrap_or("")))
            })
            .and_then(timestamp)
            .filter(|t| answered - *t >= 0 && answered - *t <= 1200)
            .unwrap_or(answered);
        return Some((sent, 1800, true));
    }
    let entries = lines
        .iter()
        .filter(|v| v["isSidechain"] != true)
        .collect::<Vec<_>>();
    let ids: HashMap<&str, &Value> = entries
        .iter()
        .filter_map(|v| v["uuid"].as_str().map(|id| (id, *v)))
        .collect();
    let mut sent = None;
    for v in entries.into_iter().rev() {
        let usage = &v["message"]["usage"];
        if v["type"] != "assistant" || !usage.is_object() {
            continue;
        }
        if sent.is_none() {
            if usage["cache_read_input_tokens"].as_u64().unwrap_or(0) == 0
                && usage["cache_creation_input_tokens"].as_u64().unwrap_or(0) == 0
            {
                continue;
            }
            let Some(answered) = timestamp(v) else {
                continue;
            };
            let mut parent = v["parentUuid"].as_str();
            let mut request = None;
            for _ in 0..64 {
                let Some(entry) = parent.and_then(|id| ids.get(id)) else {
                    break;
                };
                if entry["type"] != "assistant" {
                    request =
                        timestamp(entry).filter(|t| answered - *t >= 0 && answered - *t <= 1200);
                    break;
                }
                parent = entry["parentUuid"].as_str()
            }
            sent = Some(request.unwrap_or(answered));
        }
        if usage["cache_creation"]["ephemeral_1h_input_tokens"]
            .as_u64()
            .unwrap_or(0)
            > 0
        {
            return Some((sent?, 3600, false));
        }
        if usage["cache_creation"]["ephemeral_5m_input_tokens"]
            .as_u64()
            .unwrap_or(0)
            > 0
        {
            return Some((sent?, 300, false));
        }
    }
    None
}
fn text(v: &Value) -> Option<String> {
    if let Some(s) = v.as_str() {
        return Some(s.into());
    }
    v.as_array()?
        .iter()
        .filter(|v| v["type"] == "text" || v["type"] == "input_text")
        .find_map(|v| v["text"].as_str().map(str::to_owned))
}
fn name(
    head: &[Value],
    tail: &[Value],
    fallback: &str,
    provider: &str,
) -> (String, Option<String>) {
    let custom = tail
        .iter()
        .rev()
        .chain(head.iter().rev())
        .find_map(|v| v["customTitle"].as_str());
    let opening = head.iter().find_map(|v| {
        if provider == "codex" && v["type"] == "response_item" && v["payload"]["role"] == "user" {
            text(&v["payload"]["content"])
        } else if provider != "codex" && v["type"] == "user" && v["isSidechain"] != true {
            text(&v["message"]["content"])
        } else {
            None
        }
    });
    let title = custom
        .map(str::to_owned)
        .or(opening)
        .map(|s| {
            s.lines()
                .find(|s| !s.trim().is_empty())
                .unwrap_or("")
                .trim()
                .chars()
                .take(80)
                .collect::<String>()
        })
        .filter(|s| !s.is_empty() && !s.starts_with('<'))
        .unwrap_or_else(|| fallback.into());
    let project = head
        .iter()
        .chain(tail.iter())
        .find_map(|v| v["cwd"].as_str().or(v["payload"]["cwd"].as_str()))
        .and_then(|s| s.trim_end_matches(['/', '\\']).rsplit(['/', '\\']).next())
        .filter(|s| !s.is_empty())
        .map(str::to_owned);
    (title, project)
}
fn session(path: &Path, provider: &str) -> Option<Session> {
    let mut file = fs::File::open(path).ok()?;
    let size = file.metadata().ok()?.len();
    let offset = size.saturating_sub(512 * 1024);
    file.seek(SeekFrom::Start(offset)).ok()?;
    let mut tail = vec![];
    Read::by_ref(&mut file)
        .take(512 * 1024)
        .read_to_end(&mut tail)
        .ok()?;
    let parse = |b: &[u8]| {
        b.split(|b| *b == b'\n')
            .filter_map(|l| serde_json::from_slice::<Value>(l).ok())
            .collect::<Vec<_>>()
    };
    let tail = parse(&tail);
    let (last, lifetime, minimum) = lapse(&tail, provider)?;
    file.seek(SeekFrom::Start(0)).ok()?;
    let mut head = vec![];
    file.take(64 * 1024).read_to_end(&mut head).ok()?;
    let head = parse(&head);
    let fallback = path.file_stem()?.to_string_lossy().to_string();
    let (title, project) = name(&head, &tail, &fallback, provider);
    let id = head
        .iter()
        .find_map(|v| v["payload"]["id"].as_str().or(v["sessionId"].as_str()))
        .unwrap_or(&fallback)
        .to_owned();
    Some(Session {
        id,
        title,
        project,
        last_request: chrono::DateTime::from_timestamp(last, 0)?.to_rfc3339(),
        expires_at: chrono::DateTime::from_timestamp(last + i64::from(lifetime), 0)?.to_rfc3339(),
        lifetime,
        is_minimum: minimum,
    })
}
fn enumerate(
    root: &Path,
    provider: &str,
    depth: usize,
    out: &mut Vec<(PathBuf, i64)>,
    notes: &mut Vec<String>,
) {
    if depth > 16 || out.len() >= 10000 {
        notes.push("部分目录超过读取上限。".into());
        return;
    }
    let entries = match fs::read_dir(root) {
        Ok(e) => e,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return,
        Err(_) => {
            notes.push("部分会话目录无法读取。".into());
            return;
        }
    };
    for e in entries.flatten() {
        let Ok(kind) = e.file_type() else { continue };
        if kind.is_dir() && (provider == "codex" || depth == 0) {
            enumerate(&e.path(), provider, depth + 1, out, notes)
        } else if kind.is_file() && e.path().extension().is_some_and(|s| s == "jsonl") {
            if let Some(modified) = e
                .metadata()
                .ok()
                .and_then(|m| m.modified().ok())
                .and_then(|d| d.duration_since(std::time::UNIX_EPOCH).ok())
            {
                out.push((e.path(), modified.as_secs() as i64));
            }
        }
        if out.len() >= 10000 {
            break;
        }
    }
}
#[tauri::command]
pub async fn get_prompt_cache(provider: String) -> Result<Reading, String> {
    if !["codex", "claude-code"].contains(&provider.as_str()) {
        return Err("此服务没有可计时的缓存记录。".into());
    }
    if !crate::preferences::load().token_spend_enabled {
        return Err("本机记录读取未开启。".into());
    }
    tokio::task::spawn_blocking(move || {
        let home = crate::credentials::home_dir().ok_or("无法定位用户目录。")?;
        let root = if provider == "codex" {
            std::env::var_os("CODEX_HOME")
                .map(PathBuf::from)
                .unwrap_or_else(|| home.join(".codex"))
                .join("sessions")
        } else {
            std::env::var_os("CLAUDE_CONFIG_DIR")
                .map(PathBuf::from)
                .unwrap_or_else(|| home.join(".claude"))
                .join("projects")
        };
        let now = chrono::Utc::now().timestamp();
        let horizon = if provider == "codex" { 1800 } else { 3600 };
        let mut reading = Reading::default();
        let mut files = vec![];
        enumerate(&root, &provider, 0, &mut files, &mut reading.notes);
        files.sort_by_key(|(_, m)| std::cmp::Reverse(*m));
        let newest = files.first().map(|(p, _)| p.clone());
        let mut ids = std::collections::HashSet::new();
        for (path, mtime) in files {
            if !crate::preferences::load().token_spend_enabled {
                return Err("本机记录读取已关闭。".into());
            }
            if now - mtime > horizon + 60 && Some(&path) != newest.as_ref() {
                continue;
            }
            let Some(s) = session(&path, &provider) else {
                continue;
            };
            if !ids.insert(s.id.clone()) {
                continue;
            }
            let expires = chrono::DateTime::parse_from_rfc3339(&s.expires_at)
                .map(|d| d.timestamp())
                .unwrap_or(0);
            let last = expires - i64::from(s.lifetime);
            if last > now + 60 {
                reading
                    .notes
                    .push("会话时钟晚于当前时间，未推断缓存。".into());
                continue;
            }
            if expires > now {
                reading.live.push(s)
            } else if reading.last_lapsed.is_none() {
                reading.last_lapsed = Some(s)
            }
        }
        reading.live.sort_by(|a, b| a.expires_at.cmp(&b.expires_at));
        if !reading.live.is_empty() {
            reading.last_lapsed = None
        }
        reading.notes.sort();
        reading.notes.dedup();
        Ok(reading)
    })
    .await
    .map_err(|_| "缓存计时读取失败。".to_owned())?
}
#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn codex_only_times_documented_models() {
        for m in ["gpt-5.6", "gpt-5.6-sol", "gpt-6-astra", "gpt-10"] {
            assert!(lifetime(m))
        }
        for m in ["gpt-5.5", "gpt-5.3-codex", "o3", "auto-review", "gpt-x"] {
            assert!(!lifetime(m))
        }
    }
    #[test]
    fn claude_cache_read_reuses_last_recorded_tier_and_follows_parent_request() {
        let lines = vec![
            json!({"type":"user","uuid":"user","timestamp":"2026-10-01T00:00:00Z"}),
            json!({"type":"assistant","uuid":"reply","parentUuid":"user","timestamp":"2026-10-01T00:00:10Z","message":{"usage":{"cache_creation_input_tokens":10,"cache_creation":{"ephemeral_1h_input_tokens":10}}}}),
            json!({"type":"user","uuid":"next","timestamp":"2026-10-01T00:01:00Z"}),
            json!({"type":"assistant","parentUuid":"next","timestamp":"2026-10-01T00:01:10Z","message":{"usage":{"cache_read_input_tokens":10}}}),
        ];
        let l = lapse(&lines, "claude-code").unwrap();
        assert_eq!(l.1, 3600);
        assert_eq!(l.0, timestamp(&lines[2]).unwrap());
        assert!(!l.2);
    }
    #[test]
    fn missing_tier_is_unknown_and_sidechains_do_not_time_main_session() {
        let v = json!({"type":"assistant","timestamp":"2026-10-01T00:00:10Z","message":{"usage":{"cache_read_input_tokens":10}}});
        assert!(lapse(&[v.clone()], "claude-code").is_none());
        let mut v = v;
        v["isSidechain"] = true.into();
        v["message"]["usage"]["cache_creation"] = json!({"ephemeral_5m_input_tokens":1});
        assert!(lapse(&[v], "claude-code").is_none());
    }
    #[test]
    fn codex_uses_request_time_and_thirty_minimum_only_when_cache_touched() {
        let lines = vec![
            json!({"type":"turn_context","payload":{"model":"gpt-6-sol"}}),
            json!({"type":"response_item","timestamp":"2026-10-01T00:00:00Z","payload":{"role":"user"}}),
            json!({"type":"event_msg","timestamp":"2026-10-01T00:00:10Z","payload":{"type":"token_count","info":{"last_token_usage":{"cached_input_tokens":10}}}}),
        ];
        assert_eq!(
            lapse(&lines, "codex"),
            Some((timestamp(&lines[1]).unwrap(), 1800, true))
        );
        let mut lines = lines;
        lines[0]["payload"]["model"] = "gpt-5.5".into();
        assert!(lapse(&lines, "codex").is_none());
    }
}
