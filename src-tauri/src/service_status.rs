//! Official, anonymous service-status readings. Ported from Pulse 1.9's
//! ServiceStatus/OutageMemory (Apache-2.0, copyright 2026 qunqin24).
//! Missing readings are errors; uptime is always the publisher's own figure.
use chrono::{DateTime, Days, Local, NaiveDate, TimeZone, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    collections::{HashMap, HashSet},
    time::{Duration, Instant},
};
use tauri::{Manager, State};

const INTERVAL: Duration = Duration::from_secs(300);
const MAX_BYTES: usize = 8 * 1024 * 1024;
const OPENAI: &str = "https://status.openai.com/proxy/status.openai.com";
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Page {
    OpenAI,
    Claude,
    DeepSeek,
}
impl Page {
    fn key(self) -> &'static str {
        match self {
            Self::OpenAI => "codex",
            Self::Claude => "claude-code",
            Self::DeepSeek => "deepseek",
        }
    }
    pub fn from_provider(id: &str) -> Option<Self> {
        match id {
            "codex" => Some(Self::OpenAI),
            "claude-code" => Some(Self::Claude),
            "deepseek" => Some(Self::DeepSeek),
            _ => None,
        }
    }
    fn relevant(self, c: &Component) -> bool {
        match self {
            Self::OpenAI => true,
            Self::Claude => ["yyzkbfz2thpt", "k8w3r06qmzrp"].contains(&c.id.as_str()),
            Self::DeepSeek => c.name.contains("API"),
        }
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Health {
    Operational,
    Maintenance,
    Unrecognised,
    Degraded,
    PartialOutage,
    FullOutage,
}
impl Health {
    fn parse(s: &str) -> Self {
        match s {
            "operational" => Self::Operational,
            "degraded" | "degraded_performance" => Self::Degraded,
            "partial_outage" => Self::PartialOutage,
            "major_outage" | "full_outage" => Self::FullOutage,
            "maintenance" | "under_maintenance" => Self::Maintenance,
            _ => Self::Unrecognised,
        }
    }
    fn severity(self) -> u8 {
        match self {
            Self::Operational => 0,
            Self::Maintenance => 1,
            Self::Unrecognised => 2,
            Self::Degraded => 3,
            Self::PartialOutage => 4,
            Self::FullOutage => 5,
        }
    }
    fn outage(self) -> bool {
        self.severity() >= 3
    }
    fn title(self) -> &'static str {
        match self {
            Self::Operational => "运行正常",
            Self::Maintenance => "计划维护",
            Self::Unrecognised => "未知状态",
            Self::Degraded => "性能下降",
            Self::PartialOutage => "部分故障",
            Self::FullOutage => "全面故障",
        }
    }
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Day {
    date: String,
    state: Option<Health>,
    colour: Option<String>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Component {
    pub id: String,
    pub name: String,
    pub state: Health,
    days: Vec<Day>,
    uptime: Option<f64>,
}
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Reading {
    page: Page,
    components: Vec<Component>,
    checked_at: String,
    history_error: Option<String>,
}
fn component(id: &str, name: &str, state: Health) -> Component {
    Component {
        id: id.into(),
        name: name.into(),
        state,
        days: vec![],
        uptime: None,
    }
}
fn list<'a>(v: &'a Value, k: &str) -> &'a [Value] {
    v.get(k)
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or(&[])
}
fn text<'a>(v: &'a Value, k: &str) -> Option<&'a str> {
    v.get(k)?.as_str()
}
fn percent(v: Option<&Value>) -> Option<f64> {
    let n = v.and_then(|v| v.as_f64().or_else(|| v.as_str()?.parse().ok()))?;
    (n.is_finite() && (0.0..=100.0).contains(&n)).then_some(n)
}
fn stamp(v: Option<&Value>, seconds: bool) -> Option<i64> {
    let v = v?;
    if seconds {
        let f = v.as_f64()?;
        (f.is_finite() && f.abs() < 253_402_300_800.0).then_some(f as i64)
    } else {
        DateTime::parse_from_rfc3339(v.as_str()?)
            .ok()
            .map(|d| d.timestamp())
    }
}

fn openai_summary(v: &Value) -> Result<Vec<Component>, String> {
    let groups = v
        .pointer("/summary/structure/items")
        .and_then(Value::as_array)
        .ok_or("无法读取 Codex 状态组件")?;
    let g = groups
        .iter()
        .filter_map(|v| v.get("group"))
        .find(|g| text(g, "name") == Some("Codex"))
        .ok_or("官方状态页未列出 Codex")?;
    let affected = list(&v["summary"], "affected_components");
    nonempty(
        list(g, "components")
            .iter()
            .filter(|c| c["hidden"] != true)
            .filter_map(|c| {
                let id = text(c, "component_id")?;
                let name = text(c, "name")?;
                let state = affected
                    .iter()
                    .find(|c| text(c, "component_id") == Some(id))
                    .map(|c| Health::parse(text(c, "status").unwrap_or("unknown")))
                    .unwrap_or(Health::Operational);
                Some(component(id, name, state))
            })
            .collect(),
    )
}
fn claude_summary(v: &Value) -> Result<Vec<Component>, String> {
    if !v["components"].is_array() {
        return Err("无法读取 Claude 状态页".into());
    }
    let mut rows = list(v, "components")
        .iter()
        .filter(|c| {
            c["group"] != true
                && !(c["only_show_if_degraded"] == true && text(c, "status") == Some("operational"))
        })
        .collect::<Vec<_>>();
    rows.sort_by_key(|c| c["position"].as_i64().unwrap_or(i64::MAX));
    nonempty(
        rows.into_iter()
            .filter_map(|c| {
                Some(component(
                    text(c, "id")?,
                    text(c, "name")?,
                    Health::parse(text(c, "status")?),
                ))
            })
            .collect(),
    )
}
fn nonempty(rows: Vec<Component>) -> Result<Vec<Component>, String> {
    if rows.is_empty() {
        Err("官方状态页没有可读取的组件".into())
    } else {
        Ok(rows)
    }
}

/// Decode only JSON string literals in Next's flight chunks, never execute HTML.
fn flashcat_data(html: &str) -> Result<Value, String> {
    let mut flight = String::new();
    for part in html.split("self.__next_f.push([1,").skip(1) {
        let part = part.trim_start();
        if !part.starts_with('"') {
            continue;
        }
        let mut escaped = false;
        let mut end = None;
        for (i, ch) in part.char_indices().skip(1) {
            if escaped {
                escaped = false;
                continue;
            }
            if ch == '\\' {
                escaped = true
            } else if ch == '"' {
                end = Some(i + 1);
                break;
            }
        }
        if let Some(end) = end {
            if let Ok(s) = serde_json::from_str::<String>(&part[..end]) {
                flight.push_str(&s)
            }
        }
    }
    fn collect(v: &Value, out: &mut serde_json::Map<String, Value>, depth: u8) {
        if depth > 64 {
            return;
        }
        if let Some(obj) = v.as_object() {
            if let Some(data) = obj.get("initialData").and_then(Value::as_object) {
                for (k, v) in data {
                    out.entry(k.clone()).or_insert_with(|| v.clone());
                }
            }
            for v in obj.values() {
                collect(v, out, depth + 1)
            }
        } else if let Some(a) = v.as_array() {
            for v in a {
                collect(v, out, depth + 1)
            }
        }
    }
    let mut merged = serde_json::Map::new();
    for line in flight.lines().filter(|l| l.contains("\"initialData\"")) {
        if let Some((_, json)) = line.split_once(':') {
            if let Ok(v) = serde_json::from_str::<Value>(json) {
                collect(&v, &mut merged, 0)
            }
        }
    }
    if !merged
        .get("page")
        .is_some_and(|v| v["components"].is_array())
    {
        return Err("无法识别 DeepSeek 官方状态页数据".into());
    }
    Ok(Value::Object(merged))
}
fn flashcat_summary(v: &Value, now: i64) -> Result<Vec<Component>, String> {
    let page = &v["page"];
    let mut groups: Vec<(i64, Vec<&Value>)> = vec![];
    let visible = list(page, "components")
        .iter()
        .filter(|v| v["hide_all"] != true)
        .collect::<Vec<_>>();
    let order = |v: &Value| v["order_id"].as_i64().unwrap_or(i64::MAX);
    for c in &visible {
        if text(c, "section_id").unwrap_or("").is_empty() {
            groups.push((order(c), vec![*c]))
        }
    }
    for section in list(page, "sections")
        .iter()
        .filter(|v| v["hide_all"] != true)
    {
        if let Some(id) = text(section, "section_id") {
            let mut members = visible
                .iter()
                .copied()
                .filter(|c| text(c, "section_id") == Some(id))
                .collect::<Vec<_>>();
            members.sort_by_key(|c| order(c));
            groups.push((order(section), members));
        }
    }
    groups.sort_by_key(|g| g.0);
    nonempty(
        groups
            .into_iter()
            .flat_map(|g| g.1)
            .filter_map(|c| {
                let id = text(c, "component_id")?;
                let name = text(c, "name")?;
                let open = list(v, "component_impacts")
                    .iter()
                    .filter(|i| {
                        text(i, "component_id") == Some(id)
                            && stamp(i.get("start_at_seconds"), true).is_some_and(|s| s <= now)
                            && stamp(i.get("end_at_seconds"), true).unwrap_or(i64::MAX) > now
                    })
                    .map(|i| Health::parse(text(i, "status").unwrap_or("unknown")))
                    .max_by_key(|s| s.severity());
                let stated = text(c, "status")
                    .filter(|s| !s.is_empty())
                    .map(Health::parse);
                Some(component(
                    id,
                    name,
                    stated.or(open).unwrap_or(Health::Operational),
                ))
            })
            .collect(),
    )
}

fn calendar(now: DateTime<Local>, count: u64) -> Vec<(String, i64, i64)> {
    (0..count)
        .rev()
        .filter_map(|back| {
            let date = now.date_naive().checked_sub_days(Days::new(back))?;
            let next = date.checked_add_days(Days::new(1))?;
            // Earliest local midnight handles repeated DST boundaries; absent
            // midnight is omitted, never silently replaced with a 24-hour UTC day.
            let a = Local
                .from_local_datetime(&date.and_hms_opt(0, 0, 0)?)
                .earliest()?;
            let b = Local
                .from_local_datetime(&next.and_hms_opt(0, 0, 0)?)
                .earliest()?;
            Some((date.to_string(), a.timestamp(), b.timestamp()))
        })
        .collect()
}
fn impact_history(
    rows: &mut [Component],
    v: &Value,
    days: &[(String, i64, i64)],
    now: i64,
    seconds: bool,
) -> Result<(), String> {
    if !v["component_impacts"].is_array() || !v["component_uptimes"].is_array() {
        return Err("官方历史数据未能读取".into());
    }
    for c in rows {
        let uptime = list(v, "component_uptimes")
            .iter()
            .find(|u| text(u, "component_id") == Some(c.id.as_str()));
        let since = uptime.and_then(|u| {
            stamp(
                u.get(if seconds {
                    "available_since_seconds"
                } else {
                    "data_available_since"
                }),
                seconds,
            )
        });
        c.uptime = uptime.and_then(|u| percent(u.get("uptime")));
        let impacts = list(v, "component_impacts")
            .iter()
            .filter(|i| text(i, "component_id") == Some(c.id.as_str()))
            .filter_map(|i| {
                let start = stamp(
                    i.get(if seconds {
                        "start_at_seconds"
                    } else {
                        "start_at"
                    }),
                    seconds,
                )?;
                // A malformed non-null end is not an ongoing outage.
                let end_value = i.get(if seconds { "end_at_seconds" } else { "end_at" });
                let end = if end_value.is_none() || end_value == Some(&Value::Null) {
                    now
                } else {
                    stamp(end_value, seconds)?
                };
                (end > start).then_some((start, end, Health::parse(text(i, "status")?)))
            })
            .collect::<Vec<_>>();
        c.days = days
            .iter()
            .map(|(date, a, b)| Day {
                date: date.clone(),
                state: if since.is_some_and(|s| *b <= s) {
                    None
                } else {
                    Some(
                        impacts
                            .iter()
                            .filter(|(s, e, _)| *s < *b && *e > *a)
                            .map(|i| i.2)
                            .max_by_key(|s| s.severity())
                            .unwrap_or(Health::Operational),
                    )
                },
                colour: None,
            })
            .collect();
    }
    Ok(())
}
fn bar_colours(svg: &str) -> Vec<String> {
    svg.split("<rect")
        .skip(1)
        .filter_map(|s| {
            let tag = s.split_once('>')?.0;
            if !tag.contains("uptime-day") {
                return None;
            }
            let fill = tag.split_once("fill=\"#")?.1.get(..6)?;
            fill.bytes()
                .all(|b| b.is_ascii_hexdigit())
                .then(|| format!("#{fill}"))
        })
        .collect()
}
fn claude_history(rows: &mut [Component], v: &Value) -> Result<(), String> {
    if !v["timelines"].is_object() {
        return Err("Claude 官方历史数据未能读取".into());
    }
    let mut read = 0;
    for c in rows {
        let t = &v["timelines"][&c.id];
        if !t["days"].is_array() {
            continue;
        }
        let since = t.pointer("/component/startDate").and_then(Value::as_str);
        c.days = list(t, "days")
            .iter()
            .filter_map(|d| {
                let date = text(d, "date")?;
                NaiveDate::parse_from_str(date, "%Y-%m-%d").ok()?;
                Some(Day {
                    date: date.into(),
                    state: if since.is_some_and(|s| date < s) {
                        None
                    } else {
                        Some(
                            if d.pointer("/outages/m").and_then(Value::as_u64).unwrap_or(0) > 0 {
                                Health::FullOutage
                            } else if d.pointer("/outages/p").and_then(Value::as_u64).unwrap_or(0)
                                > 0
                            {
                                Health::PartialOutage
                            } else {
                                Health::Operational
                            },
                        )
                    },
                    colour: None,
                })
            })
            .collect();
        if c.days.is_empty() {
            continue;
        }
        read += 1;
        let colours = v["components"][&c.id]
            .as_str()
            .map(bar_colours)
            .unwrap_or_default();
        if colours.len() == c.days.len() {
            for (d, col) in c.days.iter_mut().zip(colours) {
                d.colour = Some(col)
            }
        }
        c.uptime = list(v, "values")
            .iter()
            .find(|u| text(u, "component") == Some(c.id.as_str()))
            .and_then(|u| percent(u.get("ninety")));
    }
    if read == 0 {
        Err("Claude 官方历史没有可读取的日期".into())
    } else {
        Ok(())
    }
}

async fn fetch(
    client: &reqwest::Client,
    url: &str,
    query: &[(&str, String)],
    html: bool,
) -> Result<Vec<u8>, String> {
    let mut response = client
        .get(url)
        .query(query)
        .header(
            "Accept",
            if html {
                "text/html"
            } else {
                "application/json"
            },
        )
        .timeout(Duration::from_secs(15))
        .send()
        .await
        .map_err(|_| "官方状态页请求失败".to_string())?;
    if !response.status().is_success() {
        return Err(format!(
            "官方状态页返回 HTTP {}",
            response.status().as_u16()
        ));
    }
    if response
        .content_length()
        .is_some_and(|n| n > MAX_BYTES as u64)
    {
        return Err("官方状态数据超过大小限制".into());
    }
    let mut data = vec![];
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_| "官方状态响应未完整读取")?
    {
        if data.len() + chunk.len() > MAX_BYTES {
            return Err("官方状态数据超过大小限制".into());
        }
        data.extend_from_slice(&chunk)
    }
    Ok(data)
}
async fn json(
    client: &reqwest::Client,
    url: &str,
    query: &[(&str, String)],
) -> Result<Value, String> {
    serde_json::from_slice(&fetch(client, url, query, false).await?)
        .map_err(|_| "官方状态页返回了无法识别的数据".into())
}
pub async fn read(client: &reqwest::Client, page: Page, history: bool) -> Result<Reading, String> {
    let now = Local::now();
    let mut history_error = None;
    let components = match page {
        Page::OpenAI => {
            let mut c = openai_summary(&json(client, OPENAI, &[]).await?)?;
            if history {
                let days = calendar(now, 91);
                if let (Some(first), Some(last)) = (days.first(), days.last()) {
                    let iso = |s| DateTime::from_timestamp(s, 0).unwrap().to_rfc3339();
                    let query = [("start_at", iso(first.1)), ("end_at", iso(last.2))];
                    let result =
                        match json(client, &format!("{OPENAI}/component_impacts"), &query).await {
                            Ok(v) => impact_history(&mut c, &v, &days, now.timestamp(), false),
                            Err(e) => Err(e),
                        };
                    history_error = result.err();
                }
            }
            c
        }
        Page::Claude => {
            let mut c = claude_summary(
                &json(client, "https://status.claude.com/api/v2/summary.json", &[]).await?,
            )?;
            if history {
                let query = [(
                    "components",
                    c.iter()
                        .map(|c| c.id.as_str())
                        .collect::<Vec<_>>()
                        .join(","),
                )];
                let result =
                    match json(client, "https://status.claude.com/uptime_showcase", &query).await {
                        Ok(v) => claude_history(&mut c, &v),
                        Err(e) => Err(e),
                    };
                history_error = result.err();
            }
            c
        }
        Page::DeepSeek => {
            let bytes = fetch(client, "https://status.deepseek.com/", &[], true).await?;
            let v = flashcat_data(&String::from_utf8_lossy(&bytes))?;
            let mut c = flashcat_summary(&v, now.timestamp())?;
            if history {
                history_error =
                    impact_history(&mut c, &v, &calendar(now, 90), now.timestamp(), true).err()
            }
            c
        }
    };
    Ok(Reading {
        page,
        components,
        checked_at: Utc::now().to_rfc3339(),
        history_error,
    })
}

type Cache = HashMap<(Page, bool), (Instant, Result<Reading, String>)>;
#[derive(Default)]
pub struct StatusState {
    cache: tokio::sync::Mutex<Cache>,
    memory: std::sync::Mutex<OutageMemory>,
    wake: tokio::sync::Notify,
}
impl StatusState {
    async fn reading(
        &self,
        client: &reqwest::Client,
        page: Page,
        history: bool,
    ) -> Result<Reading, String> {
        // One in-flight public request per cache key. Failures are cached too;
        // opening a pane repeatedly cannot hammer a publisher or show stale green.
        let mut cache = self.cache.lock().await;
        if let Some((at, result)) = cache.get(&(page, history)) {
            if at.elapsed() < INTERVAL {
                return result.clone();
            }
        }
        if !history {
            if let Some((at, Ok(result))) = cache.get(&(page, true)) {
                if at.elapsed() < INTERVAL {
                    return Ok(result.clone());
                }
            }
        }
        let asked = Instant::now();
        let result = read(client, page, history).await;
        cache.insert((page, history), (asked, result.clone()));
        result
    }
}
#[tauri::command]
pub async fn get_service_status(
    state: State<'_, crate::AppState>,
    status: State<'_, StatusState>,
    provider: String,
) -> Result<Reading, String> {
    let prefs = state.preferences.lock().unwrap().clone();
    let base = prefs
        .accounts
        .iter()
        .find(|a| a.id == provider)
        .map(|a| a.provider.as_str())
        .unwrap_or(&provider);
    let page = Page::from_provider(base).ok_or("此服务没有受支持的官方状态页")?;
    let ctx = crate::providers::Ctx::current(&state.ctx, &prefs.network_proxy)
        .map_err(|_| "无法使用当前代理访问官方状态页")?;
    status.reading(&ctx.client, page, true).await
}

#[derive(Default, Clone, Serialize, Deserialize)]
#[serde(default)]
struct OutageMemory {
    announced: HashMap<Page, HashMap<String, Health>>,
}
impl OutageMemory {
    fn changes(&mut self, page: Page, components: &[Component]) -> Vec<(String, String)> {
        let before = self.announced.remove(&page).unwrap_or_default();
        let mut next = HashMap::new();
        let mut notices = vec![];
        for c in components.iter().filter(|c| page.relevant(c)) {
            let old = before.get(&c.id).copied();
            if c.state.outage() {
                if old.map_or(true, |s| c.state.severity() > s.severity()) {
                    notices.push((
                        "服务故障".into(),
                        format!("{}：{}", c.name, c.state.title()),
                    ))
                }
                next.insert(c.id.clone(), c.state);
            } else if c.state == Health::Operational {
                if old.is_some() {
                    notices.push(("服务已恢复".into(), format!("{}：运行正常", c.name)))
                }
            } else if let Some(old) = old {
                next.insert(c.id.clone(), old);
            }
        }
        if !next.is_empty() {
            self.announced.insert(page, next);
        }
        notices
    }
    fn keep(&mut self, pages: &HashSet<Page>) {
        self.announced.retain(|p, _| pages.contains(p));
    }
    fn load() -> Self {
        crate::settings::directory()
            .and_then(|d| std::fs::read(d.join("status-alerts.json")).ok())
            .filter(|b| b.len() < 1024 * 1024)
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default()
    }
    fn save(&self) -> Result<(), String> {
        let d = crate::settings::directory().ok_or("无法保存服务通知记录")?;
        std::fs::create_dir_all(&d).map_err(|e| e.to_string())?;
        let path = d.join("status-alerts.json");
        let old = std::fs::read(&path).ok();
        let bytes = serde_json::to_vec(self).map_err(|e| e.to_string())?;
        crate::credential_store::atomic_write_bounded(&path, &bytes, old.as_deref(), 1024 * 1024)
            .map_err(|e| e.to_string())
    }
}
pub fn preferences_changed(app: &tauri::AppHandle) {
    let state = app.state::<crate::AppState>();
    let prefs = state.preferences.lock().unwrap().clone();
    let pages = if prefs.alerts.on_service_outage {
        prefs
            .enabled_providers
            .iter()
            .filter_map(|id| {
                let base = prefs
                    .accounts
                    .iter()
                    .find(|a| &a.id == id)
                    .map(|a| a.provider.as_str())
                    .unwrap_or(id);
                Page::from_provider(base)
            })
            .collect()
    } else {
        HashSet::new()
    };
    let status = app.state::<StatusState>();
    let mut memory = status.memory.lock().unwrap();
    memory.keep(&pages);
    let _ = memory.save();
    status.wake.notify_one();
}
pub fn watch(app: tauri::AppHandle) {
    *app.state::<StatusState>().memory.lock().unwrap() = OutageMemory::load();
    tauri::async_runtime::spawn(async move {
        loop {
            let state = app.state::<crate::AppState>();
            let status = app.state::<StatusState>();
            let prefs = state.preferences.lock().unwrap().clone();
            let pages = if prefs.alerts.on_service_outage {
                prefs
                    .enabled_providers
                    .iter()
                    .filter_map(|id| {
                        let base = prefs
                            .accounts
                            .iter()
                            .find(|a| &a.id == id)
                            .map(|a| a.provider.as_str())
                            .unwrap_or(id);
                        Page::from_provider(base)
                    })
                    .collect::<HashSet<_>>()
            } else {
                HashSet::new()
            };
            {
                let mut memory = status.memory.lock().unwrap();
                memory.keep(&pages);
                let _ = memory.save();
            }
            for page in pages {
                let ctx = match crate::providers::Ctx::current(&state.ctx, &prefs.network_proxy) {
                    Ok(c) => c,
                    Err(_) => continue,
                };
                if let Ok(reading) = status.reading(&ctx.client, page, false).await {
                    // Recheck after network await: disabling a provider/notification
                    // must prevent an already-running read from sending anything.
                    let current = state.preferences.lock().unwrap().clone();
                    let watching = current.alerts.on_service_outage
                        && current.enabled_providers.iter().any(|id| {
                            let base = current
                                .accounts
                                .iter()
                                .find(|a| &a.id == id)
                                .map(|a| a.provider.as_str())
                                .unwrap_or(id);
                            Page::from_provider(base) == Some(page)
                        });
                    if !watching {
                        continue;
                    }
                    let mut memory = status.memory.lock().unwrap();
                    let old = memory.clone();
                    let notices = memory.changes(page, &reading.components);
                    use tauri_plugin_notification::NotificationExt;
                    let mut delivered = true;
                    for (title, body) in notices {
                        if app
                            .notification()
                            .builder()
                            .title(format!("Pulse · {} · {title}", page.key()))
                            .body(body)
                            .show()
                            .is_err()
                        {
                            delivered = false
                        }
                    }
                    if !delivered {
                        *memory = old;
                    }
                    let _ = memory.save();
                }
            }
            tokio::select! {_=tokio::time::sleep(INTERVAL)=>{},_=status.wake.notified()=>{}}
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    fn fixture(name: &str) -> Value {
        serde_json::from_str(match name {
            "openai" => {
                include_str!("../tests/fixtures/service-status/openai-status-operational.json")
            }
            "incident" => {
                include_str!("../tests/fixtures/service-status/openai-status-incident.json")
            }
            "impacts" => {
                include_str!("../tests/fixtures/service-status/openai-status-impacts.json")
            }
            "claude" => include_str!("../tests/fixtures/service-status/claude-status-summary.json"),
            _ => include_str!("../tests/fixtures/service-status/claude-status-uptime.json"),
        })
        .unwrap()
    }
    fn deepseek() -> Value {
        flashcat_data(include_str!(
            "../tests/fixtures/service-status/deepseek-status-page.html"
        ))
        .unwrap()
    }
    fn days(count: i64) -> Vec<(String, i64, i64)> {
        let offset = chrono::FixedOffset::east_opt(28800).unwrap();
        let date = NaiveDate::from_ymd_opt(2026, 10, 4).unwrap();
        (0..count)
            .rev()
            .map(|i| {
                let d = date - chrono::Duration::days(i);
                let a = offset
                    .from_local_datetime(&d.and_hms_opt(0, 0, 0).unwrap())
                    .unwrap()
                    .timestamp();
                (d.to_string(), a, a + 86400)
            })
            .collect()
    }
    fn bars(c: &Component) -> String {
        c.days
            .iter()
            .map(|d| match d.state {
                None => '-',
                Some(Health::Operational) => '0',
                Some(Health::Degraded) => '1',
                Some(Health::PartialOutage) => '2',
                Some(Health::FullOutage) => '3',
                Some(Health::Maintenance) => 'm',
                _ => '?',
            })
            .collect()
    }
    #[test]
    fn codex_visible_group_and_affected_states() {
        let c = openai_summary(&fixture("openai")).unwrap();
        assert_eq!(
            c.iter().map(|c| c.name.as_str()).collect::<Vec<_>>(),
            ["Codex Web", "Codex API", "CLI", "VS Code extension"]
        );
        assert!(c.iter().all(|c| c.state == Health::Operational));
        let c = openai_summary(&fixture("incident")).unwrap();
        assert_eq!(
            c.iter().map(|c| c.state).collect::<Vec<_>>(),
            [
                Health::Degraded,
                Health::Operational,
                Health::FullOutage,
                Health::Unrecognised
            ]
        );
        assert!(openai_summary(&serde_json::json!({})).is_err());
    }
    #[test]
    fn openai_history_matches_every_original_bar_and_official_uptime() {
        let mut c = openai_summary(&fixture("openai")).unwrap();
        impact_history(&mut c, &fixture("impacts"), &days(91), 1791096000, false).unwrap();
        assert_eq!(bars(&c[0]),"0000000000000010011100000000000000000000000000000000000000011000000000000000000000300010000");
        assert_eq!(bars(&c[1]),"0000000000000000011100000000000000000000000000100000000000011000000000000000000000300010000");
        assert_eq!(bars(&c[2]),"0000000000001010011100000000000000000000000000000000000000011000000000000000000000300010000");
        assert_eq!(bars(&c[3]), bars(&c[0]));
        assert!(c.iter().all(|c| c.uptime == Some(99.95)));
        assert_eq!(c[0].days.len(), 91);
    }
    #[test]
    fn claude_colours_and_unknown_history_preserve_current() {
        let mut c = claude_summary(&fixture("claude")).unwrap();
        assert_eq!(c.len(), 6);
        claude_history(&mut c, &fixture("uptime")).unwrap();
        let code = c.iter().find(|c| c.id == "yyzkbfz2thpt").unwrap();
        assert_eq!(code.days.len(), 90);
        assert_eq!(code.days[0].state, Some(Health::PartialOutage));
        assert_eq!(code.days[0].colour.as_deref(), Some("#e75f36"));
        assert_eq!(code.days[1].colour.as_deref(), Some("#76ad2a"));
        for c in &c {
            for d in &c.days {
                assert_eq!(
                    d.colour.as_deref() == Some("#76ad2a"),
                    d.state == Some(Health::Operational)
                )
            }
        }
        let mut current = claude_summary(&fixture("claude")).unwrap();
        assert!(claude_history(&mut current, &serde_json::json!({})).is_err());
        assert!(current.iter().all(|c| c.days.is_empty()));
    }
    #[test]
    fn deepseek_flight_data_component_order_and_every_original_bar() {
        let v = deepseek();
        let now = DateTime::parse_from_rfc3339("2026-10-04T16:00:00+08:00")
            .unwrap()
            .timestamp();
        let mut c = flashcat_summary(&v, now).unwrap();
        assert_eq!(c.len(), 5);
        assert_eq!(c.iter().filter(|c| Page::DeepSeek.relevant(c)).count(), 2);
        impact_history(&mut c, &v, &days(90), now, true).unwrap();
        assert_eq!(c.iter().map(bars).collect::<Vec<_>>(),[
            "000000000000000001000101100120020000001000010000000000000300000000000010100000220000000100",
            "000000000000000001000100200020020010000000010120000000000000000000000020000100220000000100",
            "000000000000000200000000100000000000000000000000000000000000000000000020000100220000000100",
            "---------------000000000000000000000000000000000000000000000000000000000000000000000000000",
            "---------------000000020000000000000003000200000000000000000000000000000000000000000000000"]);
        assert_eq!(
            c.iter().map(|c| c.uptime).collect::<Vec<_>>(),
            [
                Some(99.92),
                Some(99.69),
                Some(99.68),
                Some(100.),
                Some(99.91)
            ]
        );
        assert!(flashcat_data("<html>unknown redesign</html>").is_err());
    }
    #[test]
    fn partial_colours_invalid_dates_and_before_data_are_not_fabricated() {
        let mut c = vec![component("x", "X", Health::Degraded)];
        let v = serde_json::json!({"timelines":{"x":{"component":{"startDate":"2026-10-02"},"days":[{"date":"2026-10-01"},{"date":"2026-10-02","outages":{"m":60}},{"date":"invalid"},{"date":"2026-10-03","outages":{"p":60}}]}},"components":{"x":"<rect fill=\"#000000\" class=\"uptime-day\"/>"},"values":[{"component":"x","ninety":101}]});
        claude_history(&mut c, &v).unwrap();
        assert_eq!(bars(&c[0]), "-32");
        assert!(c[0].days.iter().all(|d| d.colour.is_none()));
        assert_eq!(c[0].uptime, None);
        assert_eq!(c[0].state, Health::Degraded);
    }
    #[test]
    fn malformed_unrelated_components_do_not_drop_valid_rows() {
        let v = serde_json::json!({"components":[{"id":"bad","name":null},{"id":"group","name":"Group","status":"operational","group":true},{"id":"quiet","name":"Quiet","status":"operational","only_show_if_degraded":true},{"id":"a","name":"API","status":"new-state","position":2}]});
        let c = claude_summary(&v).unwrap();
        assert_eq!(c.len(), 1);
        assert_eq!(c[0].state, Health::Unrecognised);
    }
    #[test]
    fn outages_deduplicate_escalate_deescalate_recover_and_survive_serialization() {
        let mut m = OutageMemory::default();
        let c = |s| vec![component("a", "API", s)];
        assert!(m.changes(Page::OpenAI, &c(Health::Operational)).is_empty());
        assert_eq!(m.changes(Page::OpenAI, &c(Health::Degraded)).len(), 1);
        let serialized = serde_json::to_string(&m).unwrap();
        let mut m: OutageMemory = serde_json::from_str(&serialized).unwrap();
        assert!(m.changes(Page::OpenAI, &c(Health::Degraded)).is_empty());
        assert_eq!(m.changes(Page::OpenAI, &c(Health::FullOutage)).len(), 1);
        assert!(m
            .changes(Page::OpenAI, &c(Health::PartialOutage))
            .is_empty());
        assert_eq!(m.changes(Page::OpenAI, &c(Health::FullOutage)).len(), 1);
        assert!(m.changes(Page::OpenAI, &c(Health::Maintenance)).is_empty());
        assert!(m.changes(Page::OpenAI, &c(Health::Unrecognised)).is_empty());
        assert_eq!(m.changes(Page::OpenAI, &c(Health::Operational)).len(), 1);
        assert!(m.changes(Page::OpenAI, &c(Health::Operational)).is_empty());
    }
    #[test]
    fn disappearing_disabled_and_unrelated_components_never_claim_recovery() {
        let mut m = OutageMemory::default();
        let api = component("yyzkbfz2thpt", "Claude Code", Health::Degraded);
        let chat = component("chat", "claude.ai", Health::FullOutage);
        assert_eq!(m.changes(Page::Claude, &[api.clone(), chat]).len(), 1);
        assert!(m.changes(Page::Claude, &[]).is_empty());
        let mut healthy = api.clone();
        healthy.state = Health::Operational;
        assert!(m.changes(Page::Claude, &[healthy]).is_empty());
        assert_eq!(m.changes(Page::Claude, &[api.clone()]).len(), 1);
        m.keep(&HashSet::new());
        assert_eq!(m.changes(Page::Claude, &[api]).len(), 1);
    }
    #[test]
    fn history_at_exact_boundaries_and_since_are_calendar_intersections() {
        let mut c = vec![component("x", "X", Health::Operational)];
        let v = serde_json::json!({"component_impacts":[{"component_id":"x","status":"full_outage","start_at_seconds":10,"end_at_seconds":20},{"component_id":"x","status":"partial_outage","start_at_seconds":30,"end_at_seconds":null},{"component_id":"x","status":"full_outage","start_at_seconds":15,"end_at_seconds":"broken"}],"component_uptimes":[{"component_id":"x","available_since_seconds":10,"uptime":99.95}]});
        impact_history(
            &mut c,
            &v,
            &[
                ("a".into(), 0, 10),
                ("b".into(), 10, 20),
                ("c".into(), 20, 30),
                ("d".into(), 30, 40),
            ],
            35,
            true,
        )
        .unwrap();
        assert_eq!(bars(&c[0]), "-302");
        assert_eq!(c[0].uptime, Some(99.95));
    }
}
