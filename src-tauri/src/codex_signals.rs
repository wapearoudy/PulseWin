//! Pulse's local Codex evidence rules. Request-side signs never prove the model a server ran.
use serde::Serialize;
use serde_json::Value;
use std::{
    collections::{BTreeMap, HashMap, HashSet},
    fs,
    io::{BufRead, BufReader},
    path::{Path, PathBuf},
    sync::{Mutex, OnceLock},
};
#[derive(Clone, Default)]
struct Facts {
    session: String,
    responses: Vec<Response>,
    changes: Vec<Change>,
    last: Option<i64>,
    judged: bool,
}
#[derive(Clone)]
struct Response {
    date: i64,
    model: String,
    reasoning: u64,
}
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Change {
    kind: String,
    asked: String,
    ran: String,
    date: String,
    session: String,
    order: usize,
}
#[derive(Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Signals {
    models: Vec<Truncation>,
    changes: Vec<Change>,
    sessions: usize,
    judged_sessions: usize,
    notes: Vec<String>,
}
#[derive(Default, Serialize)]
#[serde(rename_all = "camelCase")]
struct Truncation {
    model: String,
    responses: usize,
    reached: usize,
    hits: usize,
    share: Option<f64>,
    measurable: bool,
    suspicious: bool,
}
#[derive(Default)]
struct Parser {
    facts: Facts,
    version: Option<(u32, u32, u32)>,
    model: Option<String>,
    totals: Option<Value>,
    applied: (Option<String>, Option<String>),
    at_start: (Option<String>, Option<String>),
    previous: (Option<String>, Option<String>),
    subturns: HashSet<String>,
    window: Option<(u64, Option<String>)>,
    applied_any: bool,
    own: bool,
    replay: Option<i64>,
}
fn own(model: &str) -> bool {
    model.contains("auto-review")
}
fn version(s: &str) -> Option<(u32, u32, u32)> {
    let v = s
        .split('-')
        .next()?
        .split('.')
        .map(str::parse)
        .collect::<Result<Vec<u32>, _>>()
        .ok()?;
    Some((*v.first()?, *v.get(1)?, *v.get(2)?))
}
fn string(v: &Value, key: &str) -> Option<String> {
    v[key].as_str().map(str::to_owned)
}
impl Parser {
    fn change(&mut self, kind: &str, asked: String, ran: String, date: i64) {
        self.facts.changes.push(Change {
            kind: kind.into(),
            asked,
            ran,
            date: chrono::DateTime::from_timestamp_millis(date)
                .unwrap()
                .to_rfc3339(),
            session: self.facts.session.clone(),
            order: self.facts.changes.len(),
        });
    }
    fn line(&mut self, v: &Value) {
        let p = &v["payload"];
        let date = v["timestamp"]
            .as_str()
            .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
            .map(|d| d.timestamp_millis());
        if let Some(t) = date {
            self.facts.last = Some(self.facts.last.unwrap_or(t).max(t))
        }
        let kind = v["type"].as_str().unwrap_or("");
        if kind != "session_meta" && self.replay.zip(date).is_some_and(|(until, t)| t <= until) {
            return;
        }
        match kind {
            "session_meta" => {
                self.window = None;
                if self.version.is_some() {
                    return;
                }
                self.version = p["cli_version"].as_str().and_then(version);
                self.facts.judged = self.version.is_some_and(|v| v >= (0, 144, 0));
                if let Some(id) = string(p, "id") {
                    self.facts.session = id
                }
                self.own = p["source"].is_object() || p["parent_thread_id"].is_string();
                if p["forked_from_id"].is_string() {
                    self.replay = date.map(|d| d + 2000)
                }
            }
            "turn_context" => {
                let ran = string(p, "model");
                if ran.is_some() {
                    self.model = ran.clone()
                }
                let Some(date) = date.filter(|_| self.facts.judged) else {
                    return;
                };
                if p["turn_id"]
                    .as_str()
                    .is_some_and(|id| self.subturns.contains(id))
                {
                    return;
                }
                let effort = string(p, "effort")
                    .or_else(|| string(&p["collaboration_mode"]["settings"], "reasoning_effort"));
                let asked = (
                    self.at_start.0.clone().or(self.previous.0.clone()),
                    self.at_start.1.clone().or(self.previous.1.clone()),
                );
                if let (Some(ran), Some(wanted)) = (&ran, asked.0) {
                    if ran != &wanted && !own(ran) && !own(&wanted) {
                        self.change("model", wanted, ran.clone(), date)
                    }
                }
                let ranks = ["none", "minimal", "low", "medium", "high", "xhigh"];
                if !ran.as_deref().is_some_and(own) {
                    if let (Some(have), Some(want)) = (&effort, asked.1) {
                        if ranks
                            .iter()
                            .position(|s| *s == have)
                            .zip(ranks.iter().position(|s| *s == want))
                            .is_some_and(|(h, w)| h < w)
                        {
                            self.change("effort", want, have.clone(), date)
                        }
                    }
                }
                self.previous = (
                    ran.or(self.previous.0.clone()),
                    effort.or(self.previous.1.clone()),
                );
            }
            "event_msg" => match p["type"].as_str().unwrap_or("") {
                "task_started" => {
                    if let (Some(turn), Some(root)) =
                        (p["turn_id"].as_str(), p["root_turn_id"].as_str())
                    {
                        if !root.is_empty() && root != turn {
                            self.subturns.insert(turn.into());
                        }
                    }
                    self.at_start = self.applied.clone()
                }
                "thread_settings_applied" => {
                    self.applied_any = true;
                    let settings = &p["thread_settings"];
                    self.applied = (
                        string(settings, "model").or(self.applied.0.clone()),
                        string(settings, "reasoning_effort").or(self.applied.1.clone()),
                    );
                }
                "token_count" => {
                    let info = &p["info"];
                    if !info.is_object() {
                        return;
                    }
                    if let Some(size) = info["model_context_window"].as_u64().filter(|n| *n > 0) {
                        if self.facts.judged {
                            if let (Some((old, model)), Some(date)) = (&self.window, date) {
                                if size < *old && model == &self.model {
                                    self.change(
                                        "contextWindow",
                                        old.to_string(),
                                        size.to_string(),
                                        date,
                                    )
                                }
                            }
                            self.window = Some((size, self.model.clone()));
                        }
                    }
                    if info["total_token_usage"].is_object() {
                        let usage = &info["total_token_usage"];
                        let totals = serde_json::json!([
                            usage["input_tokens"],
                            usage["output_tokens"],
                            usage["reasoning_output_tokens"],
                            usage["total_tokens"]
                        ]);
                        if self.totals.as_ref() == Some(&totals) {
                            return;
                        }
                        self.totals = Some(totals)
                    }
                    if let (Some(reasoning), Some(model), Some(date)) = (
                        info["last_token_usage"]["reasoning_output_tokens"]
                            .as_u64()
                            .filter(|n| *n > 0),
                        self.model.clone(),
                        date,
                    ) {
                        self.facts.responses.push(Response {
                            date,
                            model,
                            reasoning,
                        })
                    }
                }
                _ => {}
            },
            _ => {}
        }
    }
    fn finish(mut self) -> Facts {
        self.facts.judged = self.facts.judged && self.applied_any && !self.own;
        if !self.facts.judged {
            self.facts.changes.clear()
        }
        self.facts
    }
}
type Cache = HashMap<PathBuf, ((u64, std::time::SystemTime), Facts)>;
#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    fn line(kind: &str, p: Value, seconds: u32) -> Value {
        json!({"type":kind,"timestamp":format!("2026-10-01T00:00:{seconds:02}Z"),"payload":p})
    }
    fn meta(version: &str) -> Value {
        line(
            "session_meta",
            json!({"id":"session","cli_version":version}),
            0,
        )
    }
    fn context(model: &str, effort: &str, t: u32) -> Value {
        line(
            "turn_context",
            json!({"turn_id":"t","model":model,"effort":effort}),
            t,
        )
    }
    fn applied(model: &str, effort: &str, t: u32) -> Value {
        line(
            "event_msg",
            json!({"type":"thread_settings_applied","thread_settings":{"model":model,"reasoning_effort":effort}}),
            t,
        )
    }
    fn start(t: u32) -> Value {
        line(
            "event_msg",
            json!({"type":"task_started","turn_id":"t","root_turn_id":"t"}),
            t,
        )
    }
    fn count(reasoning: u64, total: u64, window: u64, t: u32) -> Value {
        line(
            "event_msg",
            json!({"type":"token_count","info":{"model_context_window":window,"last_token_usage":{"reasoning_output_tokens":reasoning},"total_token_usage":{"input_tokens":total,"output_tokens":total,"reasoning_output_tokens":total,"total_tokens":total}}}),
            t,
        )
    }
    fn parse(lines: Vec<Value>) -> Facts {
        let mut parser = Parser::default();
        for l in lines {
            parser.line(&l)
        }
        parser.finish()
    }
    #[test]
    fn cumulative_repeats_are_not_responses_and_old_sessions_are_not_judged() {
        let f = parse(vec![
            meta("0.120.0"),
            context("gpt-5.5", "high", 1),
            count(516, 100, 272000, 2),
            count(516, 100, 272000, 3),
            count(0, 150, 272000, 4),
            context("gpt-6-sol", "high", 5),
            count(700, 200, 272000, 6),
        ]);
        assert_eq!(f.responses.len(), 2);
        assert_eq!(f.responses[0].reasoning, 516);
        assert!(!f.judged);
        assert!(f.changes.is_empty());
    }
    #[test]
    fn model_effort_and_context_drops_require_new_user_setting_evidence() {
        let f = parse(vec![
            meta("0.146.0-alpha.3"),
            applied("gpt-6-astra", "high", 1),
            start(2),
            context("gpt-6-astra", "high", 2),
            count(600, 100, 272000, 3),
            context("gpt-6-astra", "low", 4),
            count(600, 200, 128000, 5),
            start(6),
            context("gpt-6-luna", "high", 6),
        ]);
        assert!(f.judged);
        assert_eq!(
            f.changes
                .iter()
                .map(|c| c.kind.as_str())
                .collect::<Vec<_>>(),
            vec!["effort", "contextWindow", "model"]
        );
        assert_eq!(f.changes[1].asked, "272000");
        assert_eq!(f.changes[1].ran, "128000");
    }
    #[test]
    fn settings_applied_mid_turn_wait_until_the_next_task() {
        let f = parse(vec![
            meta("0.146.0"),
            applied("gpt-6-sol", "medium", 0),
            start(1),
            context("gpt-6-sol", "medium", 1),
            applied("gpt-6-sol", "high", 2),
            context("gpt-6-sol", "medium", 3),
            start(4),
            context("gpt-6-sol", "high", 4),
        ]);
        assert!(f.judged);
        assert!(f.changes.is_empty())
    }
    #[test]
    fn no_settings_event_and_helper_headers_do_not_accuse_user_changes() {
        let mut header = meta("0.146.0");
        header["payload"]["source"] = json!({"subagent":"guardian"});
        for lines in [
            vec![
                meta("0.146.0"),
                start(1),
                context("gpt-6-sol", "high", 1),
                context("gpt-6-luna", "low", 2),
            ],
            vec![
                header,
                applied("gpt-6-sol", "high", 1),
                start(2),
                context("gpt-6-luna", "low", 2),
            ],
        ] {
            let f = parse(lines);
            assert!(!f.judged);
            assert!(f.changes.is_empty())
        }
    }
    #[test]
    fn fork_replayed_counts_within_two_seconds_are_excluded() {
        let mut header = meta("0.146.0");
        header["payload"]["forked_from_id"] = "parent".into();
        let f = parse(vec![
            header,
            context("gpt-6-sol", "high", 1),
            count(516, 100, 272000, 2),
            context("gpt-6-sol", "high", 3),
            count(700, 200, 272000, 4),
        ]);
        assert_eq!(f.responses.len(), 1);
        assert_eq!(f.responses[0].reasoning, 700);
        assert!(f.changes.is_empty())
    }
    #[test]
    fn another_root_turn_and_review_models_are_not_user_setting_changes() {
        let mut sub = start(3);
        sub["payload"]["root_turn_id"] = "parent".into();
        let f = parse(vec![
            meta("0.146.0"),
            applied("gpt-6-sol", "high", 1),
            start(2),
            context("gpt-6-sol", "high", 2),
            sub,
            context("gpt-6-luna", "low", 3),
            context("auto-review", "minimal", 4),
        ]);
        assert!(f.changes.is_empty());
    }
}
static REVISION: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
#[tauri::command]
pub fn cancel_codex_signal_read() {
    REVISION.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
}
static CACHE: OnceLock<Mutex<Cache>> = OnceLock::new();
static PURGE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
struct CacheAccess<'a>(std::sync::MutexGuard<'a, Cache>);
impl std::ops::Deref for CacheAccess<'_> {
    type Target = Cache;
    fn deref(&self) -> &Cache {
        &self.0
    }
}
impl std::ops::DerefMut for CacheAccess<'_> {
    fn deref_mut(&mut self) -> &mut Cache {
        &mut self.0
    }
}
impl Drop for CacheAccess<'_> {
    fn drop(&mut self) {
        if PURGE.swap(false, std::sync::atomic::Ordering::SeqCst) {
            self.0.clear()
        }
    }
}
pub fn clear() {
    PURGE.store(true, std::sync::atomic::Ordering::SeqCst);
    cancel_codex_signal_read();
    if let Some(c) = CACHE.get() {
        if let Ok(cache) = c.try_lock() {
            drop(CacheAccess(cache))
        }
    }
}
fn files(path: &Path, out: &mut Vec<PathBuf>, notes: &mut Vec<String>, depth: usize) {
    if depth > 16 || out.len() >= 10000 {
        notes.push("目录超过读取上限，分析可能不完整。".into());
        return;
    }
    let entries = match fs::read_dir(path) {
        Ok(e) => e,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return,
        Err(_) => {
            notes.push("部分会话目录无法读取。".into());
            return;
        }
    };
    for e in entries.flatten() {
        let Ok(kind) = e.file_type() else { continue };
        if kind.is_dir() {
            files(&e.path(), out, notes, depth + 1)
        } else if kind.is_file()
            && e.file_name().to_string_lossy().starts_with("rollout-")
            && e.path().extension().is_some_and(|x| x == "jsonl")
        {
            out.push(e.path());
        }
        if out.len() >= 10000 {
            break;
        }
    }
}
fn read_file(path: &Path, revision: u64) -> Result<Facts, String> {
    let file = fs::File::open(path).map_err(|_| "部分会话文件无法读取。")?;
    if file.metadata().map_err(|_| "会话信息读取失败。")?.len() > 256 * 1024 * 1024 {
        return Err("会话超过 256 MiB，分析可能不完整。".into());
    }
    let mut reader = BufReader::new(file);
    let mut parser = Parser::default();
    parser.facts.session = path
        .file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .into();
    let mut line = vec![];
    loop {
        if REVISION.load(std::sync::atomic::Ordering::SeqCst) != revision {
            return Err("分析已停止。".into());
        }
        line.clear();
        let mut over = false;
        loop {
            if REVISION.load(std::sync::atomic::Ordering::SeqCst) != revision {
                return Err("分析已停止。".into());
            }
            let buf = reader.fill_buf().map_err(|_| "会话读取失败。")?;
            if buf.is_empty() {
                break;
            }
            let n = buf
                .iter()
                .position(|b| *b == b'\n')
                .map(|n| n + 1)
                .unwrap_or(buf.len());
            let newline = buf[n - 1] == b'\n';
            if line.len() + n <= 8 * 1024 * 1024 && !over {
                line.extend_from_slice(&buf[..n])
            } else {
                over = true;
                line.clear()
            }
            reader.consume(n);
            if newline {
                break;
            }
        }
        if over {
            return Err("会话行超过 8 MiB，分析可能不完整。".into());
        }
        if line.is_empty() {
            break;
        }
        if ![
            b"session_meta".as_slice(),
            b"turn_context",
            b"thread_settings_applied",
            b"task_started",
            b"token_count",
        ]
        .iter()
        .any(|wanted| line.windows(wanted.len()).any(|w| w == *wanted))
        {
            continue;
        }
        if let Ok(v) = serde_json::from_slice::<Value>(&line) {
            parser.line(&v)
        } else {
            return Err("会话存在不完整行，未缓存本次分析。".into());
        }
        if parser.facts.responses.len() + parser.facts.changes.len() > 100000 {
            return Err("会话分析超过记录上限。".into());
        }
    }
    Ok(parser.finish())
}
#[tauri::command]
pub async fn get_codex_signals(span: String) -> Result<Signals, String> {
    let days = match span.as_str() {
        "month" => Some(30),
        "quarter" => Some(90),
        "all" => None,
        _ => return Err("请选择有效的分析范围。".into()),
    };
    if !crate::preferences::load().token_spend_enabled {
        return Err("请先开启本机 Token 用量记录读取。".into());
    }
    let revision = REVISION.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1;
    tokio::task::spawn_blocking(move || {
        let home = crate::credentials::home_dir().ok_or("无法定位用户目录。")?;
        let root = std::env::var_os("CODEX_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| home.join(".codex"));
        let mut signals = Signals::default();
        let mut paths = vec![];
        files(&root.join("sessions"), &mut paths, &mut signals.notes, 0);
        files(
            &root.join("archived_sessions"),
            &mut paths,
            &mut signals.notes,
            0,
        );
        paths.sort();
        let since = days.map(|d| chrono::Utc::now().timestamp_millis() - d * 86400000);
        let mut models: BTreeMap<String, Truncation> = BTreeMap::new();
        let mut sessions = HashSet::new();
        let mut cache = CacheAccess(CACHE.get_or_init(Default::default).lock().unwrap());
        if PURGE.swap(false, std::sync::atomic::Ordering::SeqCst) {
            cache.clear()
        }
        let existing: HashSet<_> = paths.iter().cloned().collect();
        cache.retain(|p, _| existing.contains(p));
        for path in paths {
            if REVISION.load(std::sync::atomic::Ordering::SeqCst) != revision {
                return Err("本机读取已关闭。".into());
            }
            let Ok(meta) = fs::metadata(&path) else {
                continue;
            };
            let Ok(modified) = meta.modified() else {
                continue;
            };
            let stamp = (meta.len(), modified);
            let facts = if let Some((_, f)) = cache.get(&path).filter(|(s, _)| *s == stamp) {
                f.clone()
            } else {
                match read_file(&path, revision) {
                    Ok(f) => {
                        if fs::metadata(&path)
                            .ok()
                            .and_then(|m| Some((m.len(), m.modified().ok()?)))
                            == Some(stamp)
                            && cache.len() < 512
                        {
                            cache.insert(path, (stamp, f.clone()));
                        }
                        f
                    }
                    Err(e) => {
                        signals.notes.push(e);
                        continue;
                    }
                }
            };
            if since.zip(facts.last).is_some_and(|(s, t)| t < s) || !sessions.insert(facts.session)
            {
                continue;
            }
            signals.sessions += 1;
            signals.judged_sessions += usize::from(facts.judged);
            for r in facts.responses {
                if own(&r.model) || since.is_some_and(|s| r.date < s) {
                    continue;
                }
                let m = models.entry(r.model.clone()).or_insert_with(|| Truncation {
                    model: r.model,
                    ..Default::default()
                });
                m.responses += 1;
                if r.reasoning >= 516 {
                    m.reached += 1;
                    if r.reasoning % 518 == 516 {
                        m.hits += 1
                    }
                }
            }
            signals
                .changes
                .extend(facts.changes.into_iter().filter(|c| {
                    since.map_or(true, |s| {
                        chrono::DateTime::parse_from_rfc3339(&c.date)
                            .is_ok_and(|d| d.timestamp_millis() >= s)
                    })
                }));
        }
        if REVISION.load(std::sync::atomic::Ordering::SeqCst) != revision {
            return Err("分析已停止。".into());
        }
        signals.models = models
            .into_values()
            .map(|mut m| {
                m.measurable = m.reached >= 20;
                m.share = (m.reached > 0).then(|| m.hits as f64 / m.reached as f64);
                m.suspicious = m.measurable && m.hits >= 5 && m.share.unwrap_or(0.) >= 0.05;
                m
            })
            .collect();
        signals
            .models
            .sort_by_key(|m| std::cmp::Reverse((m.suspicious, m.responses)));
        signals.changes.sort_by(|a, b| b.date.cmp(&a.date));
        signals.notes.sort();
        signals.notes.dedup();
        Ok(signals)
    })
    .await
    .map_err(|_| "本地分析任务失败。".to_owned())?
}
