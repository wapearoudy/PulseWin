//! DeepSeek web-console shapes from Pulse 1.9 DeepSeekConsole (Apache-2.0).
//! A separate DPAPI login slot; an API key cannot read these usage routes.
use chrono::TimeZone;
use serde::Serialize;
use serde_json::{json, Value};
use std::{
    collections::{BTreeMap, HashMap},
    sync::Mutex,
    time::{Duration, Instant},
};
use tauri::{Manager, State, WebviewUrl, WebviewWindow, WebviewWindowBuilder};
const ORIGIN: &str = "https://platform.deepseek.com";
const SLOT: &str = "deepseek-console";
const LABEL: &str = "deepseek-console-login";
static LOGIN: Mutex<u64> = Mutex::new(0);
static READ: crate::console_reads::ReadGate = crate::console_reads::ReadGate::new();
pub(crate) fn cancel_read() {
    READ.cancel()
}
fn path() -> Option<std::path::PathBuf> {
    crate::settings::directory().map(|p| p.join(format!("{SLOT}.json")))
}
pub fn token() -> Option<String> {
    valid_token(crate::credential_store::read_at(&path()?, SLOT).ok()??["token"].clone())
}
#[derive(Serialize)]
pub struct Connection {
    connected: bool,
}
#[tauri::command]
pub fn get_deepseek_console_connection() -> Connection {
    Connection {
        connected: token().is_some(),
    }
}
fn local(window: &WebviewWindow) -> Result<(), String> {
    if window.label() == "settings" {
        Ok(())
    } else {
        Err("请在设置窗口操作控制台登录。".into())
    }
}
#[tauri::command]
pub async fn begin_deepseek_console_login(
    app: tauri::AppHandle,
    window: WebviewWindow,
) -> Result<(), String> {
    local(&window)?;
    let mut generation = LOGIN.lock().unwrap();
    *generation += 1;
    if let Some(login) = app.get_webview_window(LABEL) {
        login.show().map_err(|_| "无法显示登录窗口。")?;
        login.set_focus().map_err(|_| "无法切换至登录窗口。")?;
        return Ok(());
    }
    WebviewWindowBuilder::new(&app, LABEL, WebviewUrl::External(ORIGIN.parse().unwrap()))
        .title("DeepSeek · 登录控制台")
        .inner_size(980., 720.)
        .incognito(true)
        .on_navigation(|url| {
            url.scheme() == "https"
                && url.host_str() == Some("platform.deepseek.com")
                && url.port().is_none()
        })
        .build()
        .map_err(|_| "无法打开 DeepSeek 登录窗口。")?;
    Ok(())
}
fn valid_token(value: Value) -> Option<String> {
    let text = value.as_str()?.trim();
    (!text.is_empty() && text.len() <= 16384 && !text.chars().any(char::is_control))
        .then(|| text.into())
}
#[tauri::command]
pub async fn finish_deepseek_console_login(
    app: tauri::AppHandle,
    window: WebviewWindow,
) -> Result<Connection, String> {
    local(&window)?;
    let generation = *LOGIN.lock().unwrap();
    let login = app
        .get_webview_window(LABEL)
        .ok_or("请先打开控制台并登录。")?;
    let url = login.url().map_err(|_| "无法读取登录页面。")?;
    if url.scheme() != "https" || url.host_str() != Some("platform.deepseek.com") {
        return Err("请返回 DeepSeek 官方控制台。".into());
    }
    let (send, receive) = tokio::sync::oneshot::channel();
    let send = Mutex::new(Some(send));
    login.eval_with_callback("(()=>{if(location.origin!=='https://platform.deepseek.com')return null;try{return JSON.parse(localStorage.getItem('userToken')||'null')?.value??null}catch{return null}})()",move|value|{if let Some(send)=send.lock().unwrap().take(){let _=send.send(value);}}).map_err(|_|"无法读取控制台登录状态。")?;
    let raw = tokio::time::timeout(Duration::from_secs(10), receive)
        .await
        .map_err(|_| "登录状态读取超时，请重试。")?
        .map_err(|_| "登录窗口已关闭。")?;
    let token = valid_token(serde_json::from_str(&raw).map_err(|_| "登录状态格式无法读取。")?)
        .ok_or("尚未完成控制台登录，请登录后再连接。")?;
    let mut current = LOGIN.lock().unwrap();
    if *current != generation {
        return Err("登录操作已变更，请重新连接。".into());
    }
    let path = path().ok_or("无法定位登录保存目录。")?;
    crate::credential_store::write_document(&path, SLOT, &json!({"token":token}))
        .map_err(|_| "无法加密保存控制台登录。")?;
    *current += 1;
    cancel_read();
    let _ = login.close();
    Ok(Connection { connected: true })
}
#[tauri::command]
pub fn remove_deepseek_console_login(window: WebviewWindow) -> Result<Connection, String> {
    local(&window)?;
    let mut generation = LOGIN.lock().unwrap();
    *generation += 1;
    cancel_read();
    if let Some(path) = path() {
        match std::fs::remove_file(path) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(_) => return Err("无法移除控制台登录。".into()),
        }
    }
    Ok(Connection { connected: false })
}
fn figure(v: &Value) -> Option<f64> {
    let n = v.as_f64().or_else(|| v.as_str()?.trim().parse().ok())?;
    (n.is_finite() && n.abs() < 1e15).then_some(n)
}
fn envelope(value: Value) -> Result<Value, String> {
    if let Some(code) = value["code"].as_i64().filter(|n| *n != 0) {
        return Err(if (40000..40100).contains(&code) {
            "控制台登录已失效，请重新登录。"
        } else {
            "控制台未能返回数据。"
        }
        .into());
    }
    if value["data"]["biz_code"].as_i64().unwrap_or(0) != 0
        || value["data"].get("biz_data").is_none()
    {
        return Err("控制台返回格式无法识别。".into());
    }
    Ok(value["data"]["biz_data"].clone())
}
async fn get(
    ctx: &crate::providers::Ctx,
    route: &str,
    query: &[(String, String)],
    token: &str,
) -> Result<Value, String> {
    let future = async {
        let mut response = ctx
            .gateway_client
            .get(format!("{ORIGIN}{route}"))
            .query(query)
            .bearer_auth(token)
            .header("Accept", "application/json")
            .send()
            .await
            .map_err(|_| "控制台请求失败，请检查网络或代理。")?;
        if response.status() == 401 {
            return Err("控制台登录已失效，请重新登录。".into());
        }
        if !response.status().is_success() {
            return Err(format!(
                "控制台请求失败（HTTP {}）。",
                response.status().as_u16()
            ));
        }
        if response
            .content_length()
            .is_some_and(|n| n > 8 * 1024 * 1024)
        {
            return Err("控制台数据超过读取上限。".into());
        }
        let mut bytes = vec![];
        while let Some(chunk) = response.chunk().await.map_err(|_| "控制台数据读取失败。")?
        {
            if bytes.len() + chunk.len() > 8 * 1024 * 1024 {
                return Err("控制台数据超过读取上限。".into());
            }
            bytes.extend_from_slice(&chunk)
        }
        envelope(serde_json::from_slice(&bytes).map_err(|_| "控制台数据格式无法读取。")?)
    };
    tokio::select! {result=future=>result,_=ctx.cancelled.cancelled()=>Err("控制台读取已取消。".into())}
}
pub async fn balance(ctx: &crate::providers::Ctx) -> Result<Value, String> {
    let token = token().ok_or("请在设置中连接 DeepSeek 控制台。")?;
    Ok(wallets(
        &get(ctx, "/api/v0/users/get_user_summary", &[], &token).await?,
    ))
}
fn wallets(summary: &Value) -> Value {
    let mut order = vec![];
    let mut sums: HashMap<String, [f64; 2]> = HashMap::new();
    for (kind, key) in ["normal_wallets", "bonus_wallets"].iter().enumerate() {
        for wallet in summary[*key].as_array().into_iter().flatten() {
            if let (Some(currency), Some(amount)) = (
                wallet["currency"].as_str().filter(|s| !s.is_empty()),
                figure(&wallet["balance"]),
            ) {
                if !sums.contains_key(currency) {
                    order.push(currency.to_owned());
                }
                sums.entry(currency.into()).or_default()[kind] += amount
            }
        }
    }
    json!({"balance_infos":order.into_iter().map(|currency|{let s=sums[&currency];json!({"currency":currency,"total_balance":s[0]+s[1],"topped_up_balance":s[0],"granted_balance":s[1]})}).collect::<Vec<_>>()})
}
fn range(now: chrono::DateTime<chrono::Local>) -> Option<Vec<(String, String)>> {
    let today = now.date_naive();
    let first = today - chrono::Duration::days(29);
    let end = today + chrono::Duration::days(1);
    let offset = now.offset().local_minus_utc();
    let whole = offset.div_euclid(3600) * 3600;
    let remainder = offset - whole;
    let stamp = |date: chrono::NaiveDate| {
        chrono::Local
            .from_local_datetime(&date.and_hms_opt(0, 0, 0)?)
            .earliest()
            .map(|d| d.timestamp() + i64::from(remainder))
    };
    Some(vec![
        ("start".into(), stamp(first)?.to_string()),
        ("end".into(), stamp(end)?.to_string()),
        ("tz".into(), whole.to_string()),
    ])
}
#[derive(Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Day {
    date: String,
    input: u64,
    output: u64,
    cache_read: u64,
    tokens: String,
    cost: Option<f64>,
    models: BTreeMap<String, u64>,
}
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct History {
    days: Vec<Day>,
    currency: Option<String>,
    checked_at: String,
    notes: Vec<String>,
}
fn ledger(
    amount: &Value,
    cost: &Value,
    preferred: Option<&str>,
    now: chrono::DateTime<chrono::Local>,
) -> History {
    let mut days: BTreeMap<String, Day> = (0..30)
        .map(|i| {
            let key = (now.date_naive() - chrono::Duration::days(i))
                .format("%Y-%m-%d")
                .to_string();
            (
                key.clone(),
                Day {
                    date: key,
                    tokens: "0".into(),
                    ..Default::default()
                },
            )
        })
        .collect();
    let mut notes = vec![];
    let half = figure(&amount["bucket"])
        .filter(|n| *n > 0.)
        .unwrap_or(86400.)
        / 2.;
    let date = |v: &Value| {
        figure(v)
            .and_then(|n| chrono::DateTime::from_timestamp((n + half) as i64, 0))
            .map(|d| {
                d.with_timezone(&chrono::Local)
                    .format("%Y-%m-%d")
                    .to_string()
            })
    };
    for series in amount["series"].as_array().into_iter().flatten() {
        for bucket in series["buckets"].as_array().into_iter().flatten() {
            let Some(day) = date(&bucket["time"]).and_then(|d| days.get_mut(&d)) else {
                continue;
            };
            let usage = &bucket["usage"];
            if !usage.is_object() {
                continue;
            }
            let mut counts = [0u64; 3];
            for (i, key) in [
                "PROMPT_CACHE_MISS_TOKEN",
                "RESPONSE_TOKEN",
                "PROMPT_CACHE_HIT_TOKEN",
            ]
            .iter()
            .enumerate()
            {
                if let Some(n) = figure(&usage[*key]).filter(|n| *n >= 0. && n.fract() == 0.) {
                    counts[i] = n as u64
                } else if usage.get(*key).is_some() {
                    notes.push("部分 Token 计数无法读取，结果可能不完整。".into())
                }
            }
            day.input = day.input.saturating_add(counts[0]);
            day.output = day.output.saturating_add(counts[1]);
            day.cache_read = day.cache_read.saturating_add(counts[2]);
            let total = counts.iter().fold(0u64, |s, n| s.saturating_add(*n));
            let model = series["model"]
                .as_str()
                .filter(|s| !s.is_empty())
                .unwrap_or("未命名模型");
            let count = day.models.entry(model.into()).or_default();
            *count = count.saturating_add(total)
        }
    }
    let purses = cost["data"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|p| p["currency"].as_str().is_some_and(|s| !s.is_empty()))
        .collect::<Vec<_>>();
    let has_money = |p: &&&Value| {
        p["series"].as_array().into_iter().flatten().any(|s| {
            s["buckets"]
                .as_array()
                .into_iter()
                .flatten()
                .any(|b| figure(&b["cost"]).is_some_and(|n| n > 0.))
        })
    };
    let chosen = preferred.and_then(|code| purses.iter().find(|p| p["currency"] == code));
    let purse = chosen
        .filter(has_money)
        .or_else(|| purses.iter().find(has_money))
        .or(chosen)
        .or_else(|| purses.first());
    let currency = purse.and_then(|p| p["currency"].as_str().map(str::to_owned));
    if let Some(purse) = purse {
        for day in days.values_mut() {
            day.cost = Some(0.)
        }
        for series in purse["series"].as_array().into_iter().flatten() {
            for bucket in series["buckets"].as_array().into_iter().flatten() {
                if let (Some(day), Some(cost)) = (
                    date(&bucket["time"]).and_then(|d| days.get_mut(&d)),
                    figure(&bucket["cost"]),
                ) {
                    day.cost = Some(day.cost.unwrap_or(0.) + cost)
                }
            }
        }
    }
    for day in days.values_mut() {
        day.tokens = day
            .input
            .saturating_add(day.output)
            .saturating_add(day.cache_read)
            .to_string()
    }
    notes.sort();
    notes.dedup();
    History {
        days: days.into_values().collect(),
        currency,
        checked_at: now.to_rfc3339(),
        notes,
    }
}
#[derive(Clone, PartialEq, Eq)]
struct CacheKey {
    login: u64,
    context: usize,
    day: String,
    currency: Option<String>,
}
struct CachedHistory {
    key: CacheKey,
    saved: Instant,
    value: History,
}
impl CachedHistory {
    fn fresh(&self, key: &CacheKey, now: Instant) -> bool {
        &self.key == key
            && now
                .checked_duration_since(self.saved)
                .is_some_and(|age| age < Duration::from_secs(60))
    }
}
static HISTORY: tokio::sync::Mutex<Option<CachedHistory>> = tokio::sync::Mutex::const_new(None);
fn complete_shapes(amount: &Value, cost: &Value) -> Result<(), String> {
    if !amount["series"].is_array() || !cost["data"].is_array() {
        return Err("控制台账单格式无法识别，未推断为零用量。".into());
    }
    Ok(())
}
#[tauri::command]
pub async fn get_deepseek_console_history(
    state: State<'_, crate::AppState>,
    force: Option<bool>,
) -> Result<History, String> {
    let cancel = READ.request();
    tokio::select! {r=read_deepseek_history(state,force)=>r,_=cancel.cancelled()=>Err("控制台读取已取消。".into())}
}
async fn read_deepseek_history(
    state: State<'_, crate::AppState>,
    force: Option<bool>,
) -> Result<History, String> {
    if !state
        .preferences
        .lock()
        .unwrap()
        .enabled_providers
        .iter()
        .any(|p| p == "deepseek")
    {
        return Err("请先启用 DeepSeek 账号。".into());
    }
    let token = token().ok_or("请先连接 DeepSeek 控制台。")?;
    let ctx = state.ctx.read().unwrap().clone();
    let now = chrono::Local::now();
    let key = CacheKey {
        login: *LOGIN.lock().unwrap(),
        context: std::sync::Arc::as_ptr(&ctx) as usize,
        day: now.date_naive().to_string(),
        currency: crate::credentials::env_override("deepseek", "currency"),
    };
    // Serialize concurrent consumers; only complete successful reads get freshness.
    let mut cache = HISTORY.lock().await;
    if self::token().as_deref() != Some(&token)
        || !state
            .preferences
            .lock()
            .unwrap()
            .enabled_providers
            .iter()
            .any(|p| p == "deepseek")
    {
        return Err("控制台读取已取消。".into());
    }
    if !force.unwrap_or(false) {
        if let Some(value) = cache.as_ref().filter(|c| c.fresh(&key, Instant::now())) {
            return Ok(value.value.clone());
        }
    }
    let query = range(now).ok_or("无法计算控制台日期范围。")?;
    let (amount, cost) = tokio::join!(
        get(&ctx, "/api/v0/usage/by_api_key/amount", &query, &token),
        get(&ctx, "/api/v0/usage/by_api_key/cost", &query, &token)
    );
    let amount = amount?;
    let cost = cost?;
    complete_shapes(&amount, &cost)?;
    let result = ledger(&amount, &cost, key.currency.as_deref(), now);
    if !state
        .preferences
        .lock()
        .unwrap()
        .enabled_providers
        .iter()
        .any(|p| p == "deepseek")
        || self::token().as_deref() != Some(&token)
    {
        return Err("控制台读取已取消。".into());
    }
    *cache = Some(CachedHistory {
        key,
        saved: Instant::now(),
        value: result.clone(),
    });
    Ok(result)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn complete_empty_bill_is_distinct_from_missing_bill_and_cache_never_crosses_accounts() {
        assert!(complete_shapes(&json!({"series":[]}), &json!({"data":[]})).is_ok());
        assert!(complete_shapes(&json!({}), &json!({"data":[]})).is_err());
        assert!(complete_shapes(&json!({"series":[]}), &json!({})).is_err());
        let saved = Instant::now();
        let key = CacheKey {
            login: 1,
            context: 10,
            day: "2026-10-11".into(),
            currency: Some("USD".into()),
        };
        let cached = CachedHistory {
            key: key.clone(),
            saved,
            value: ledger(
                &json!({"series":[]}),
                &json!({"data":[]}),
                None,
                chrono::Local::now(),
            ),
        };
        assert!(cached.fresh(&key, saved + Duration::from_secs(59)));
        assert!(!cached.fresh(&key, saved + Duration::from_secs(60)));
        for other in [
            CacheKey {
                login: 2,
                ..key.clone()
            },
            CacheKey {
                context: 11,
                ..key.clone()
            },
            CacheKey {
                day: "2026-10-12".into(),
                ..key.clone()
            },
            CacheKey {
                currency: Some("CNY".into()),
                ..key.clone()
            },
        ] {
            assert!(!cached.fresh(&other, saved));
        }
    }
    #[test]
    fn invalid_login_and_business_errors_are_not_balances() {
        assert!(valid_token(Value::Null).is_none());
        assert!(valid_token(json!("\n")).is_none());
        assert!(
            envelope(json!({"code":40003,"data":{"biz_code":0,"biz_data":{}}}))
                .unwrap_err()
                .contains("登录已失效")
        );
        assert!(envelope(json!({"code":0,"data":{"biz_code":1,"biz_data":{}}})).is_err());
    }
    #[test]
    fn wallet_currencies_stay_separate_and_missing_values_are_absent() {
        let v = wallets(
            &json!({"normal_wallets":[{"currency":"CNY","balance":"100"},{"currency":"USD","balance":5}],"bonus_wallets":[{"currency":"CNY","balance":"2"},{"currency":"EUR","balance":"nan"}]}),
        );
        assert_eq!(v["balance_infos"].as_array().unwrap().len(), 2);
        assert_eq!(v["balance_infos"][0]["total_balance"].as_f64(), Some(102.));
        assert_eq!(v["balance_infos"][1]["total_balance"].as_f64(), Some(5.));
        assert!(v.get("is_available").is_none())
    }
    #[test]
    fn billed_day_buckets_preserve_cache_and_select_currency_with_actual_charges() {
        let now = chrono::Local::now();
        let stamp = chrono::Local
            .from_local_datetime(&now.date_naive().and_hms_opt(0, 0, 0).unwrap())
            .earliest()
            .unwrap()
            .timestamp();
        let amount = json!({"series":[{"model":"m","buckets":[{"time":stamp,"usage":{"PROMPT_CACHE_MISS_TOKEN":"100","RESPONSE_TOKEN":20,"PROMPT_CACHE_HIT_TOKEN":30}}]}]});
        let cost = json!({"data":[{"currency":"CNY","series":[{"buckets":[{"time":stamp,"cost":0}]}]},{"currency":"USD","series":[{"buckets":[{"time":stamp,"cost":".02"}]}]}]});
        let h = ledger(&amount, &cost, Some("CNY"), now);
        assert_eq!(h.currency.as_deref(), Some("USD"));
        assert_eq!(h.days.len(), 30);
        let d = h.days.last().unwrap();
        assert_eq!(
            (&d.tokens, d.input, d.output, d.cache_read, d.cost),
            (&"150".to_owned(), 100, 20, 30, Some(0.02))
        );
        let no_money = ledger(&amount, &json!({}), None, now);
        assert!(no_money.days.last().unwrap().cost.is_none())
    }
}
