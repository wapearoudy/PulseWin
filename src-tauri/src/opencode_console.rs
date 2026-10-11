//! Pulse 1.9 OpenCodeConsole/History/Workspace (Apache-2.0), separate from Go keys.
use crate::console_pager::{self, Item, Page, Range, Reply};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{collections::BTreeMap, sync::Mutex, time::Duration};
use tauri::{Emitter, Manager, State, WebviewUrl, WebviewWindow, WebviewWindowBuilder};
const ORIGIN: &str = "https://opencode.ai";
const SLOT: &str = "opencode-console";
const LABEL: &str = "opencode-console-login";
const SPAN: i64 = 31 * 86400000;
static LOGIN: Mutex<u64> = Mutex::new(0);
static READ: crate::console_reads::ReadGate = crate::console_reads::ReadGate::new();
pub(crate) fn cancel_read() {
    READ.cancel()
}
static MEMORY: tokio::sync::Mutex<Option<Memory>> = tokio::sync::Mutex::const_new(None);
fn path(name: &str) -> Option<std::path::PathBuf> {
    crate::settings::directory().map(|p| p.join(format!("{name}.json")))
}
fn keep_cookie(header: &str) -> Option<String> {
    if header.len() > 16384 || header.chars().any(char::is_control) {
        return None;
    }
    let mut kept = BTreeMap::new();
    for part in header.split(';') {
        let Some((name, value)) = part.trim().split_once('=') else {
            continue;
        };
        if matches!(name, "__Host-console_session" | "auth") {
            kept.insert(name, value);
        }
    }
    if !kept
        .get("__Host-console_session")
        .is_some_and(|s| !s.is_empty())
    {
        return None;
    }
    Some(
        kept.into_iter()
            .map(|(k, v)| format!("{k}={v}"))
            .collect::<Vec<_>>()
            .join("; "),
    )
}
fn cookie() -> Option<String> {
    let value = crate::credential_store::read_at(&path(SLOT)?, SLOT).ok()??;
    keep_cookie(value["cookie"].as_str()?)
}
fn local(window: &WebviewWindow) -> Result<(), String> {
    if window.label() == "settings" {
        Ok(())
    } else {
        Err("请在设置窗口操作控制台登录。".into())
    }
}
#[derive(Serialize)]
pub struct Connection {
    connected: bool,
}
#[tauri::command]
pub fn get_opencode_console_connection() -> Connection {
    Connection {
        connected: cookie().is_some(),
    }
}
#[tauri::command]
pub async fn begin_opencode_console_login(
    app: tauri::AppHandle,
    window: WebviewWindow,
) -> Result<(), String> {
    local(&window)?;
    *LOGIN.lock().unwrap() += 1;
    if let Some(login) = app.get_webview_window(LABEL) {
        login.show().map_err(|_| "无法显示登录窗口。")?;
        login.set_focus().map_err(|_| "无法切换至登录窗口。")?;
        return Ok(());
    }
    WebviewWindowBuilder::new(
        &app,
        LABEL,
        WebviewUrl::External(format!("{ORIGIN}/console").parse().unwrap()),
    )
    .title("OpenCode Go · 登录控制台")
    .inner_size(980., 720.)
    .incognito(true)
    .on_navigation(|url| {
        url.scheme() == "https"
            && url.port().is_none()
            && url.host_str().is_some_and(|host| {
                matches!(
                    host,
                    "opencode.ai" | "auth.opencode.ai" | "github.com" | "accounts.google.com"
                )
            })
    })
    .build()
    .map_err(|_| "无法打开 OpenCode 登录窗口。")?;
    Ok(())
}
#[tauri::command]
pub async fn finish_opencode_console_login(
    app: tauri::AppHandle,
    window: WebviewWindow,
) -> Result<Connection, String> {
    local(&window)?;
    let generation = *LOGIN.lock().unwrap();
    let login = app
        .get_webview_window(LABEL)
        .ok_or("请先打开控制台并登录。")?;
    let url = login.url().map_err(|_| "无法读取登录页面。")?;
    if url.scheme() != "https" || url.host_str() != Some("opencode.ai") {
        return Err("请返回 OpenCode 官方控制台。".into());
    }
    let cookies = login
        .cookies_for_url(format!("{ORIGIN}/console").parse().unwrap())
        .map_err(|_| "无法读取控制台登录状态。")?;
    let header = cookies
        .iter()
        .filter(|c| matches!(c.name(), "__Host-console_session" | "auth"))
        .map(|c| format!("{}={}", c.name(), c.value()))
        .collect::<Vec<_>>()
        .join("; ");
    let cookie = keep_cookie(&header).ok_or("尚未完成控制台登录，请登录后再连接。")?;
    let mut current = LOGIN.lock().unwrap();
    if generation != *current {
        return Err("登录操作已变更，请重新连接。".into());
    }
    crate::credential_store::write_document(
        &path(SLOT).ok_or("无法定位登录保存目录。")?,
        SLOT,
        &json!({"cookie":cookie}),
    )
    .map_err(|_| "无法加密保存控制台登录。")?;
    *current += 1;
    cancel_read();
    let _ = login.close();
    Ok(Connection { connected: true })
}
#[tauri::command]
pub fn remove_opencode_console_login(window: WebviewWindow) -> Result<Connection, String> {
    local(&window)?;
    *LOGIN.lock().unwrap() += 1;
    cancel_read();
    if let Some(path) = path(SLOT) {
        match std::fs::remove_file(path) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(_) => return Err("无法移除控制台登录。".into()),
        }
    }
    if let Ok(mut memory) = MEMORY.try_lock() {
        *memory = None;
    }
    Ok(Connection { connected: false })
}
#[derive(Clone, Debug)]
enum Failure {
    SignedOut,
    Network,
}
async fn get(
    ctx: &crate::providers::Ctx,
    route: &str,
    query: &[(String, String)],
    cookie: &str,
    org: Option<&str>,
) -> Result<Value, Failure> {
    let future = async {
        let mut request = ctx
            .gateway_client
            .get(format!("{ORIGIN}{route}"))
            .query(query)
            .header("Cookie", cookie)
            .header("Accept", "application/json")
            .timeout(Duration::from_secs(20));
        if let Some(org) = org {
            request = request.header("x-org-id", org)
        }
        let mut response = request.send().await.map_err(|_| Failure::Network)?;
        if matches!(response.status().as_u16(), 401 | 403) {
            return Err(Failure::SignedOut);
        }
        if !response.status().is_success()
            || response
                .content_length()
                .is_some_and(|n| n > 8 * 1024 * 1024)
        {
            return Err(Failure::Network);
        }
        let mut bytes = vec![];
        while let Some(chunk) = response.chunk().await.map_err(|_| Failure::Network)? {
            if bytes.len() + chunk.len() > 8 * 1024 * 1024 {
                return Err(Failure::Network);
            }
            bytes.extend_from_slice(&chunk);
        }
        if bytes.iter().take(64).any(|c| *c == b'<') {
            return Err(Failure::SignedOut);
        }
        serde_json::from_slice(&bytes).map_err(|_| Failure::Network)
    };
    tokio::select! {r=future=>r,_=ctx.cancelled.cancelled()=>Err(Failure::Network)}
}
#[derive(Clone, Serialize, Deserialize)]
struct Workspace {
    id: String,
    name: Option<String>,
}
fn workspace_list(value: Value) -> Result<Vec<Workspace>, Failure> {
    let list: Vec<Workspace> = serde_json::from_value(value).map_err(|_| Failure::Network)?;
    Ok(list
        .into_iter()
        .filter(|w| !w.id.is_empty() && w.id.len() <= 256 && !w.id.chars().any(char::is_control))
        .take(100)
        .collect())
}
async fn workspace(
    ctx: &crate::providers::Ctx,
    cookie: &str,
) -> Result<(Workspace, bool), Failure> {
    let list = workspace_list(get(ctx, "/console/api/orgs", &[], cookie, None).await?)?;
    let first = list.first().ok_or(Failure::Network)?.clone();
    if list.len() == 1 {
        return Ok((first, true));
    }
    for org in list {
        match get(ctx, "/console/api/go/status", &[], cookie, Some(&org.id)).await {
            Ok(v) if v["access"].is_object() => return Ok((org, true)),
            Err(Failure::SignedOut) => return Err(Failure::SignedOut),
            _ => {}
        }
    }
    Ok((first, false))
}
#[derive(Clone, Default, Serialize, Deserialize)]
struct Saved {
    org: String,
    items: Vec<Item>,
    complete: bool,
    pending: Vec<Range>,
    checked_through: Option<i64>,
    #[serde(default)]
    truncated: bool,
}
struct Memory {
    generation: u64,
    context: usize,
    workspace: Workspace,
    workspace_proven: bool,
    saved: Saved,
    previous: Option<Vec<u8>>,
}
fn restore(org: &str) -> (Saved, Option<Vec<u8>>) {
    let default = Saved {
        org: org.into(),
        ..Default::default()
    };
    let Some(path) = path("opencode-console-log") else {
        return (default, None);
    };
    let Ok(file) = std::fs::File::open(path) else {
        return (default, None);
    };
    use std::io::Read;
    let mut bytes = vec![];
    if file
        .take(32 * 1024 * 1024 + 1)
        .read_to_end(&mut bytes)
        .is_err()
        || bytes.len() > 32 * 1024 * 1024
    {
        return (default, None);
    }
    let parsed = serde_json::from_slice::<Saved>(&bytes).ok().filter(|s| {
        s.items.len() <= 100000
            && s.items.iter().all(Item::valid)
            && s.pending.len() <= 1000
            && s.pending.iter().all(|r| r.since > 0 && r.until >= r.since)
    });
    match parsed {
        Some(saved) => (if saved.org == org { saved } else { default }, Some(bytes)),
        None => (default, None),
    }
}

fn ranges(saved: &Saved, now: i64) -> Vec<Range> {
    let horizon = now - SPAN;
    if saved.truncated || (!saved.complete && saved.pending.is_empty()) {
        return vec![Range {
            since: horizon,
            until: now,
        }];
    }
    let mut result = saved
        .pending
        .iter()
        .filter_map(|r| {
            let range = Range {
                since: r.since.max(horizon),
                until: r.until.min(now),
            };
            (range.since <= range.until).then_some(range)
        })
        .collect::<Vec<_>>();
    let latest = saved
        .checked_through
        .or_else(|| saved.items.iter().filter_map(Item::timestamp).max())
        .unwrap_or(horizon);
    result.insert(
        0,
        Range {
            since: horizon.max(latest.min(now) - 900000),
            until: now,
        },
    );
    result
}
#[derive(Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Day {
    date: String,
    input: String,
    output: String,
    cache_read: String,
    cache_write: String,
    tokens: String,
    cost: Option<f64>,
    unpriced_tokens: String,
    models: BTreeMap<String, String>,
}
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct History {
    workspace: String,
    workspace_name: Option<String>,
    days: Vec<Day>,
    checked_at: Option<String>,
    partial: bool,
    notes: Vec<String>,
}
fn history(
    saved: &Saved,
    workspace: &Workspace,
    now: chrono::DateTime<chrono::Local>,
    mut notes: Vec<String>,
) -> History {
    let mut days = BTreeMap::<String, Vec<&Item>>::new();
    for i in 0..=31 {
        days.insert(
            (now.date_naive() - chrono::Duration::days(i)).to_string(),
            vec![],
        );
    }
    for item in saved
        .items
        .iter()
        .filter(|i| i.product.as_deref() == Some("go"))
    {
        if let Some(at) = item
            .timestamp()
            .and_then(chrono::DateTime::from_timestamp_millis)
            .filter(|d| d.timestamp_millis() <= now.timestamp_millis())
        {
            let day = at.with_timezone(&chrono::Local).date_naive().to_string();
            if let Some(rows) = days.get_mut(&day) {
                rows.push(item)
            }
        }
    }
    let days = days
        .into_iter()
        .map(|(date, items)| {
            let mut kinds = [0u128; 4];
            let mut unpriced = 0u128;
            let mut cost = 0.;
            let mut priced = items.is_empty() && saved.complete;
            let mut models = BTreeMap::<String, u128>::new();
            for item in items {
                let tally = [
                    item.input_tokens.unwrap_or(0),
                    item.output_tokens.unwrap_or(0),
                    item.cache_read_tokens.unwrap_or(0),
                    item.cache_write_tokens.unwrap_or(0),
                ];
                let tokens = tally.iter().map(|n| u128::from(*n)).sum::<u128>();
                for (i, n) in tally.iter().enumerate() {
                    kinds[i] += u128::from(*n);
                }
                if let Some(value) = item.cost {
                    cost += value;
                    priced = true
                } else {
                    unpriced += tokens
                }
                if let Some(model) = item.model.as_ref() {
                    *models.entry(model.clone()).or_default() += tokens;
                }
            }
            Day {
                date,
                input: kinds[0].to_string(),
                output: kinds[1].to_string(),
                cache_read: kinds[2].to_string(),
                cache_write: kinds[3].to_string(),
                tokens: kinds.iter().sum::<u128>().to_string(),
                cost: priced.then_some(cost),
                unpriced_tokens: unpriced.to_string(),
                models: models
                    .into_iter()
                    .map(|(m, n)| (m, n.to_string()))
                    .collect(),
            }
        })
        .collect();
    if !saved.complete {
        notes.push("请求日志读取尚未完整，已保留未读区间，下次继续。".into())
    }
    notes.sort();
    notes.dedup();
    History {
        workspace: workspace.id.clone(),
        workspace_name: workspace.name.clone(),
        days,
        checked_at: saved
            .checked_through
            .and_then(chrono::DateTime::from_timestamp_millis)
            .map(|d| d.to_rfc3339()),
        partial: !saved.complete,
        notes,
    }
}
fn enabled(state: &crate::AppState) -> bool {
    state
        .preferences
        .lock()
        .unwrap()
        .enabled_providers
        .iter()
        .any(|p| p == "opencode-go")
}
#[tauri::command]
pub async fn get_opencode_console_history(
    app: tauri::AppHandle,
    state: State<'_, crate::AppState>,
    force: Option<bool>,
) -> Result<History, String> {
    let cancel = READ.request();
    tokio::select! {r=read_opencode_history(app,state,force)=>r,_=cancel.cancelled()=>Err("控制台读取已取消。".into())}
}
async fn read_opencode_history(
    app: tauri::AppHandle,
    state: State<'_, crate::AppState>,
    force: Option<bool>,
) -> Result<History, String> {
    if !enabled(&state) {
        return Err("请先启用 OpenCode Go 账号。".into());
    }
    let cookie = cookie().ok_or("请先连接 OpenCode 控制台。")?;
    let generation = *LOGIN.lock().unwrap();
    let ctx = state.ctx.read().unwrap().clone();
    let context = std::sync::Arc::as_ptr(&ctx) as usize;
    let mut memory = MEMORY.lock().await;
    if !enabled(&state) || self::cookie().as_deref() != Some(&cookie) {
        return Err("控制台读取已取消。".into());
    }
    let existing = memory
        .as_ref()
        .filter(|m| m.generation == generation && m.context == context && m.workspace_proven)
        .map(|m| m.workspace.clone());
    let (workspace, proven) = if let Some(w) = existing {
        (w, true)
    } else {
        tokio::time::timeout(Duration::from_secs(30), workspace(&ctx, &cookie))
            .await
            .unwrap_or(Err(Failure::Network))
            .map_err(|e| match e {
                Failure::SignedOut => "控制台登录已失效，请重新登录。",
                Failure::Network => "无法读取控制台工作区，请检查网络或代理。",
            })?
    };
    let new_scope = memory.as_ref().is_none_or(|m| {
        m.generation != generation || m.context != context || m.workspace.id != workspace.id
    });
    if new_scope {
        let (saved, previous) = restore(&workspace.id);
        *memory = Some(Memory {
            generation,
            context,
            workspace: workspace.clone(),
            workspace_proven: proven,
            saved,
            previous,
        })
    }
    let current = memory.as_mut().unwrap();
    current.workspace_proven = proven;
    let now = chrono::Local::now();
    let stamp = now.timestamp_millis();
    if !force.unwrap_or(false)
        && current.saved.complete
        && current
            .saved
            .checked_through
            .is_some_and(|t| stamp >= t && stamp - t < 60000)
    {
        return Ok(history(&current.saved, &workspace, now, vec![]));
    }
    let todo = ranges(&current.saved, stamp);
    let original = todo.clone();
    let worker_ctx = ctx.clone();
    let worker_cookie = cookie.clone();
    let org = workspace.id.clone();
    let progress = Mutex::new(&mut current.saved);
    let progress_app = app.clone();
    let progress_workspace = workspace.clone();
    let progress_cookie = cookie.clone();
    let worker_app = app.clone();
    let read = tokio::time::timeout(
        Duration::from_secs(120),
        console_pager::read_with_progress(
            todo,
            200,
            Duration::from_secs(1),
            move |range, cursor| {
                let ctx = worker_ctx.clone();
                let cookie = worker_cookie.clone();
                let org = org.clone();
                let app = worker_app.clone();
                async move {
                    if *LOGIN.lock().unwrap() != generation
                        || !enabled(&app.state::<crate::AppState>())
                    {
                        return Reply::Failed;
                    }
                    let mut query = vec![
                        ("since".into(), range.since.to_string()),
                        ("until".into(), range.until.to_string()),
                        ("category".into(), "inference".into()),
                        ("limit".into(), "100".into()),
                    ];
                    if let Some(cursor) = cursor {
                        query.push(("cursor".into(), cursor))
                    }
                    match get(
                        &ctx,
                        "/console/api/request-logs",
                        &query,
                        &cookie,
                        Some(&org),
                    )
                    .await
                    {
                        Ok(value) => serde_json::from_value::<Page>(value)
                            .map(Reply::Page)
                            .unwrap_or(Reply::Failed),
                        Err(Failure::SignedOut) => Reply::SignedOut,
                        Err(Failure::Network) => Reply::Failed,
                    }
                }
            },
            |arrived| {
                if *LOGIN.lock().unwrap() != generation
                    || !enabled(&progress_app.state::<crate::AppState>())
                    || self::cookie().as_deref() != Some(&progress_cookie)
                {
                    return;
                }
                let mut saved = progress.lock().unwrap();
                let mut by_id = saved
                    .items
                    .drain(..)
                    .map(|item| (item.id.clone(), item))
                    .collect::<BTreeMap<_, _>>();
                for item in arrived {
                    by_id.insert(item.id.clone(), item.clone());
                }
                saved.items = by_id.into_values().collect();
                saved.complete = false;
                let _ = progress_app.emit(
                    "opencode-console-history",
                    history(&saved, &progress_workspace, chrono::Local::now(), vec![]),
                );
            },
        ),
    )
    .await;
    drop(progress);
    let mut notes = vec![];
    let read = match read {
        Ok(read) => read,
        Err(_) => {
            notes.push("读取达到时间上限，未读区间已保留。".into());
            console_pager::Read {
                items: vec![],
                pending: original,
                signed_out: false,
            }
        }
    };
    if !enabled(&state)
        || self::cookie().as_deref() != Some(&cookie)
        || *LOGIN.lock().unwrap() != generation
    {
        *memory = None;
        return Err("控制台读取已取消。".into());
    }
    if read.signed_out {
        *memory = None;
        return Err("控制台登录已失效，请重新登录。".into());
    }
    let mut items = current
        .saved
        .items
        .drain(..)
        .map(|i| (i.id.clone(), i))
        .collect::<BTreeMap<_, _>>();
    for item in &read.items {
        items.insert(item.id.clone(), item.clone());
    }
    let horizon = stamp - SPAN - 86400000;
    let mut retained = items
        .into_values()
        .filter(|i| i.timestamp().is_some_and(|t| t >= horizon))
        .collect::<Vec<_>>();
    let capped = retained.len() > 100000;
    retained.sort_by(|a, b| {
        b.timestamp()
            .cmp(&a.timestamp())
            .then_with(|| a.id.cmp(&b.id))
    });
    retained.truncate(100000);
    current.saved.items = retained;
    current.saved.complete = read.complete() && !capped;
    current.saved.truncated = capped || (current.saved.truncated && !read.complete());
    current.saved.pending = if capped {
        vec![Range {
            since: stamp - SPAN,
            until: stamp,
        }]
    } else {
        read.pending
    };
    if capped {
        notes.push("请求计数超过保存上限，结果可能不完整；保留最近记录，后续继续读取。".into())
    }
    current.saved.checked_through = Some(stamp);
    if let Some(path) = path("opencode-console-log") {
        let bytes = serde_json::to_vec(&current.saved).map_err(|_| "无法整理请求计数。")?;
        if bytes.len() > 32 * 1024 * 1024
            || crate::credential_store::atomic_write_bounded(
                &path,
                &bytes,
                current.previous.as_deref(),
                32 * 1024 * 1024,
            )
            .is_err()
        {
            notes.push("请求计数未能保存到本机，下次需要重新读取。".into());
        } else {
            current.previous = Some(bytes);
        }
    }
    if !current.saved.complete && current.saved.items.is_empty() {
        return Err("控制台未返回可用请求日志，请重试。".into());
    }
    Ok(history(&current.saved, &workspace, now, notes))
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn cookie_slot_keeps_only_sign_in_and_requires_host_session() {
        assert_eq!(
            keep_cookie("auth=a; tracking=private; __Host-console_session=s; lang=zh"),
            Some("__Host-console_session=s; auth=a".into())
        );
        assert!(keep_cookie("auth=a").is_none());
        assert!(keep_cookie("__Host-console_session=\r\nx").is_none());
        assert!(keep_cookie("__Host-console_session=").is_none());
    }
    #[test]
    fn saved_holes_survive_tail_refresh_and_complete_empty_accounts_use_last_bound() {
        let now = 1791705600000;
        let saved = Saved {
            complete: false,
            pending: vec![Range {
                since: now - 2000000,
                until: now - 1000000,
            }],
            checked_through: Some(now - 60000),
            ..Default::default()
        };
        let todo = ranges(&saved, now);
        assert_eq!(todo.len(), 2);
        assert_eq!(todo[0].since, now - 960000);
        assert_eq!(todo[1], saved.pending[0]);
        let empty = Saved {
            complete: true,
            checked_through: Some(now - 1000),
            ..Default::default()
        };
        assert_eq!(ranges(&empty, now)[0].since, now - 901000);
    }
    #[test]
    fn only_go_counts_and_missing_cost_remains_unpriced_with_exact_tokens() {
        let now = chrono::Local::now();
        let make = |product: &str, id: &str, cost: Option<f64>| Item {
            id: id.into(),
            started_at: now.timestamp_millis() as f64,
            product: Some(product.into()),
            model: Some("m".into()),
            input_tokens: Some(9007199254740993),
            output_tokens: Some(20),
            cache_read_tokens: Some(30),
            cache_write_tokens: Some(5),
            cost,
        };
        let saved = Saved {
            items: vec![make("go", "a", None), make("zen", "b", Some(999.))],
            complete: true,
            ..Default::default()
        };
        let h = history(
            &saved,
            &Workspace {
                id: "org".into(),
                name: None,
            },
            now,
            vec![],
        );
        assert_eq!(h.days.len(), 32);
        let d = h.days.last().unwrap();
        assert_eq!(d.tokens, "9007199254741048");
        assert_eq!(d.cost, None);
        assert_eq!(d.unpriced_tokens, d.tokens);
        assert!(!h.partial);
    }
}
