//! The GLM Coding Plan's limits, from Zhipu's quota endpoint.
//!
//! **Two providers, one service.** Z.ai and BigModel are the same company's
//! international and mainland storefronts, answering the same JSON at the same
//! path on different hosts — but they are separate accounts with separate keys,
//! and a key for one is refused by the other. The original gives each a ring,
//! because someone with only the mainland plan should not have to know that an
//! international one exists to configure their own.
//!
//! `GET {host}/api/monitor/usage/quota/limit`, with the key as a bearer token.
//! Undocumented, like most of the routes here, and it can change without
//! notice.
//!
//! **The reply wraps its payload in a status of its own** — `success` and
//! `code`, both of which have to say 200 even when HTTP did. A refused key
//! comes back as HTTP 200 with `success: false`, so reading only the status
//! line would report an empty plan rather than a bad key.
//!
//! VERIFY ON A REAL MACHINE: both hosts are live services PulseWin cannot test
//! from here. Set `PULSEWIN_DEBUG=1` to dump a payload that fails to parse.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde::Serialize;
use serde_json::Value;

use super::{describe_reqwest_error, percent_from_fraction, by_window_length, Ctx, FetchFuture, Provider};
use crate::console_reads::Failure;
use crate::credentials;
use crate::model::{parse_reset, ProviderUsage, UsageWindow};

const QUOTA_PATH: &str = "/api/monitor/usage/quota/limit";

/// The two hosts. `api.z.ai` is the international one; mainland accounts live
/// on BigModel and are not reachable there.
const INTERNATIONAL_HOST: &str = "https://api.z.ai";
const MAINLAND_HOST: &str = "https://open.bigmodel.cn";

/// Z.ai, the international storefront.
pub struct Zai;

/// Zhipu (the original's `.glmCoding`), the mainland storefront. Its id is
/// kebab-cased where the original's raw value is `glmCoding`; nothing on disk
/// keys off it here, and the account it names is a different one from Z.ai's.
pub struct Zhipu;

impl Provider for Zai {
    fn id(&self) -> &'static str {
        "zai"
    }

    fn name(&self) -> &'static str {
        "z.ai"
    }

    /// The key for the international storefront only. The mainland tool's file
    /// is **deliberately not consulted**: it is a different account, and
    /// counting it as a way in would draw a card for a plan this machine does
    /// not have — the same reason the fetch never sends that key here.
    fn is_configured(&self) -> bool {
        super::provider_key(self.id()).is_some()
    }

    fn fetch(&self, ctx: Arc<Ctx>) -> FetchFuture {
        Box::pin(async move {
            fetch_inner(ctx, "zai", "z.ai", INTERNATIONAL_HOST, false).await
        })
    }
}

impl Provider for Zhipu {
    fn id(&self) -> &'static str {
        "glm-coding"
    }

    fn name(&self) -> &'static str {
        "Zhipu"
    }

    /// Either key the fetch accepts: one entered for this provider, or the one
    /// a mainland tool has already written to `~/.coding-relay/glm-api-key`.
    fn is_configured(&self) -> bool {
        super::provider_key(self.id()).is_some() || stored_mainland_key().is_some()
    }

    fn fetch(&self, ctx: Arc<Ctx>) -> FetchFuture {
        Box::pin(async move {
            fetch_inner(ctx, "glm-coding", "Zhipu", MAINLAND_HOST, true).await
        })
    }
}

/// A key already sitting on this machine, for the mainland plan only.
///
/// The relay and console tools that set GLM up write the key to a one-line
/// file, so anyone already using it configures nothing. **Never consulted for
/// the international route**: they are separate accounts, and quietly sending a
/// BigModel key to `api.z.ai` would report a refused key for a plan the user
/// does not have.
///
/// Only the first readable line is taken, and it is taken **carefully**. The
/// original's note is worth keeping: `split(separator: "\n")` does not cut a
/// CRLF file at all in Swift, so a file written on Windows yielded the whole
/// thing as the "key" — and a header value containing a newline is silently
/// discarded, so the request went out with no `Authorization` at all, came back
/// 401, and was reported as a refused key about a key that was correct. Rust's
/// `lines()` splits CRLF properly, which is the behaviour the original wanted.
fn stored_mainland_key() -> Option<String> {
    let candidates: &[&[&str]] = &[
        &[".coding-relay", "glm-api-key"],
        &[".config", "bigmodel", "api_key"],
        &[".config", "zhipu", "api_key"],
    ];

    for parts in candidates {
        let path = credentials::home_relative(parts).into_iter().next()?;
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue;
        };
        let key = text.lines().next().unwrap_or("").trim().to_string();
        if !key.is_empty() {
            return Some(key);
        }
    }
    None
}

/// The storefront a provider id reads: the international host, or the mainland
/// one. The two are separate accounts, so the host is part of what an answer is
/// *about* — never a default.
fn host_for(id: &str) -> Option<(&'static str, bool)> {
    match id {
        "zai" => Some((INTERNATIONAL_HOST, false)),
        "glm-coding" => Some((MAINLAND_HOST, true)),
        _ => None,
    }
}

/// The key this route uses: what the user entered wins, and only the mainland
/// route falls back to a key a mainland tool already wrote to this machine.
fn resolve_key(id: &str, mainland: bool) -> Option<String> {
    super::provider_key(id).or_else(|| mainland.then(stored_mainland_key).flatten())
}

async fn fetch_inner(
    ctx: Arc<Ctx>,
    id: &'static str,
    name: &'static str,
    host: &str,
    mainland: bool,
) -> ProviderUsage {
    // What the user entered wins, so a stale file cannot quietly override a
    // deliberate choice — the same order OpenCode Go's two sources take.
    let Some(key) = resolve_key(id, mainland) else {
        let extra = if mainland {
            ", or sign in with a mainland tool that writes ~/.coding-relay/glm-api-key"
        } else {
            ""
        };
        return ProviderUsage::failed(id, name, super::missing_key(id, extra));
    };

    let url = format!("{host}{QUOTA_PATH}");
    let response = ctx
        .client
        .get(&url)
        .header("Authorization", format!("Bearer {key}"))
        .header("Accept", "application/json")
        .send()
        .await;

    let response = match response {
        Ok(r) => r,
        Err(e) => {
            return ProviderUsage::failed(
                id,
                name,
                format!("request failed: {}", describe_reqwest_error(&e)),
            )
        }
    };

    let status = response.status();
    let body = match response.text().await {
        Ok(t) => t,
        Err(e) => return ProviderUsage::failed(id, name, format!("cannot read body: {e}")),
    };

    if !status.is_success() {
        let hints: &[(u16, &str)] = &[
            (401, " — the API key was refused"),
            (403, " — the API key was refused"),
            (429, " — rate limited, try again shortly"),
        ];
        return ProviderUsage::failed(id, name, super::http_failure(status, hints));
    }

    let json: Value = match serde_json::from_str(&body) {
        Ok(v) => v,
        Err(e) => return ProviderUsage::failed(id, name, format!("bad JSON: {e}")),
    };

    super::claude_code::debug_dump(id, &json);

    // The envelope's own verdict. A key the service refuses arrives here as a
    // perfectly good HTTP 200, so this is the only place it can be seen.
    let success = json.get("success").and_then(|v| v.as_bool());
    let code = json.get("code").and_then(|v| v.as_i64());
    if success != Some(true) || code != Some(200) {
        return ProviderUsage::failed(id, name, problem(&json).message());
    }

    let limits = json
        .get("data")
        .and_then(|d| d.get("limits"))
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();

    let windows = windows_from(&limits);
    if windows.is_empty() {
        return ProviderUsage::failed(id, name, "no quota windows in response");
    }

    ProviderUsage::ok(id, name, windows).with_plan(plan_label(&json))
}

/// What the envelope's refusal actually was.
///
/// **The keywords used to be English only, and the mainland host answers in
/// Chinese**, so nothing ever matched and everything fell through to the code —
/// which only knew HTTP's numbers. Measured against the live endpoint: a key of
/// the wrong shape gets `401 令牌已过期或验证不正确`, a well-formed key that the
/// host does not recognise gets `1000 身份验证失败。`, and a missing header gets
/// `1001`. Only the first was mapped, so the common case — a key from the
/// *other* region, which is the right shape and the wrong account — was
/// reported as "the service returned an error" and sent people looking for an
/// outage.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Problem {
    /// The key works and the account has no running coding plan.
    NoCodingPlan,
    KeyRefused,
    RateLimited,
    ServerError,
}

impl Problem {
    fn message(self) -> String {
        match self {
            // The original has a case of its own for this, because the remedy
            // is to buy a plan rather than to check a key.
            Problem::NoCodingPlan => {
                "this account has no coding plan — the key works, the subscription does not".to_string()
            }
            Problem::KeyRefused => "the API key was refused".to_string(),
            Problem::RateLimited => "rate limited, try again shortly".to_string(),
            Problem::ServerError => "the service returned an error".to_string(),
        }
    }
}

fn problem(json: &Value) -> Problem {
    let said = json
        .get("msg")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_lowercase();

    // **Checked before anything else, because the code is useless here.** A
    // working key on an account with no running subscription answers `500` —
    // the vendor's generic number — with this sentence, and 500 alone would say
    // the service broke. The phrase is embedded in English on both hosts.
    if said.contains("coding plan") {
        return Problem::NoCodingPlan;
    }

    // Matched as text because the code list cannot be complete: this is one
    // vendor's private numbering, and it is not published in full. The Chinese
    // words are the same sentences the mainland host sends.
    const AUTH_WORDS: &[&str] = &[
        "token",
        "auth",
        "key",
        "unauthor",
        "forbidden",
        "credential",
        "身份验证",
        "鉴权",
        "认证",
        "令牌",
        "未授权",
        "无权限",
        "密钥",
    ];
    if AUTH_WORDS.iter().any(|word| said.contains(word)) {
        return Problem::KeyRefused;
    }

    match json.get("code").and_then(|v| v.as_i64()).unwrap_or(0) {
        // HTTP's numbers, which this envelope also uses.
        401 | 403 => Problem::KeyRefused,
        429 => Problem::RateLimited,
        // Zhipu's own 1000-series is authentication. 1000 and 1001 are
        // measured; the rest of the band is documented as the same family and
        // sending somebody to check their key is the better mistake.
        1000..=1099 => Problem::KeyRefused,
        _ => Problem::ServerError,
    }
}

/// The plan's name, under whichever of five keys this account's tier happens to
/// use. Passed through verbatim when unfamiliar — an unknown name still beats
/// no name.
fn plan_label(json: &Value) -> Option<String> {
    let data = json.get("data")?;
    credentials::dig_first_str(
        data,
        &["planName", "plan", "plan_type", "packageName", "level"],
    )
}

/// Shortest first, so a five-hour limit is read before a weekly one.
fn windows_from(limits: &[Value]) -> Vec<UsageWindow> {
    let rows = limits
        .iter()
        .enumerate()
        .filter_map(|(index, limit)| window_from(limit, index))
        .collect();
    by_window_length(rows)
}

fn window_from(limit: &Value, index: usize) -> Option<(i64, UsageWindow)> {
    // Only these three carry a quota. Anything else the service starts
    // reporting is left out rather than shown under a heading guessed at.
    let kind_code = credentials::dig_str(limit, "type")?;
    if !["TOKENS_LIMIT", "CREDIT_LIMIT", "TIME_LIMIT"].contains(&kind_code.as_str()) {
        return None;
    }

    let unit = credentials::dig_f64(limit, "unit")? as i64;
    let number = credentials::dig_f64(limit, "number")? as i64;

    let minutes = minutes_for(unit, number, &kind_code)?;
    let used = used_percent(limit)?;

    // The position is kept out of the label — the port has no window id — but
    // the original needs it for exactly the reason two limits can share a type
    // and a duration: without it, the rows are indistinguishable.
    let label = label_for(minutes, &kind_code);

    let seconds = minutes * 60;
    let window = UsageWindow::new(label, Some(used))
        .with_reset(credentials::dig_f64(limit, "nextResetTime").and_then(|ms| {
            parse_reset(&serde_json::json!(ms))
        }))
        .with_detail(Some(format!("limit {}", index + 1)));

    Some((seconds, window))
}

/// How long the window runs.
///
/// The reply states a `unit` code and a `number` of them. An unrecognised unit
/// means the length cannot be read, and a window with no length can be neither
/// named nor sorted — so it is dropped rather than given an invented one.
fn minutes_for(unit: i64, number: i64, kind_code: &str) -> Option<i64> {
    // A monthly MCP allowance is reported as "1 minute", which is a marker
    // rather than a duration — taken literally it would sort above a five-hour
    // limit and claim to reset every minute.
    if kind_code == "TIME_LIMIT" && unit == 5 && number == 1 {
        return Some(30 * 24 * 60);
    }

    if number <= 0 {
        return None;
    }

    let multiplier = match unit {
        1 => 1440,
        3 => 60,
        5 => 1,
        6 => 10080,
        _ => return None,
    };
    Some(number * multiplier)
}

/// The window's heading, plus the MCP scope where the limit is that lane: a
/// different allowance from the coding quota, and saying so is the only way two
/// rows of the same length tell apart.
fn label_for(minutes: i64, kind_code: &str) -> String {
    let base = match minutes {
        300 => "5h".to_string(),
        10080 => "7d".to_string(),
        43200 => "Monthly".to_string(),
        other => humanize_minutes(other),
    };
    if kind_code == "TIME_LIMIT" {
        format!("{base} MCP")
    } else {
        base
    }
}

fn humanize_minutes(minutes: i64) -> String {
    if minutes % (24 * 60) == 0 {
        format!("{}d", minutes / (24 * 60))
    } else if minutes % 60 == 0 {
        format!("{}h", minutes / 60)
    } else {
        format!("{minutes}m")
    }
}

/// How much of the limit is gone, 0..100.
///
/// `percentage` is what the service intends to be read, but it is a whole
/// number — so a plan whose counts are also given is worked out from those
/// instead, which is finer. `remaining` is what is *left*, so the spend is the
/// difference; `currentValue` is the spend directly and wins when both are
/// present, since it is the one the service is counting up.
fn used_percent(limit: &Value) -> Option<f64> {
    let usage = credentials::dig_f64(limit, "usage");
    if let Some(usage) = usage.filter(|u| *u > 0.0) {
        let remaining = credentials::dig_f64(limit, "remaining");
        let current = credentials::dig_f64(limit, "currentValue");

        let used = match (remaining, current) {
            (Some(remaining), current) => (usage - remaining).max(current.unwrap_or(usage - remaining)),
            (None, Some(current)) => current,
            (None, None) => return None,
        };
        return Some(percent_from_fraction((used.min(usage)) / usage));
    }

    // **None rather than zero.** A limit that arrives with no figure at all is
    // not a limit at 0% — it is a limit whose reading is missing, and drawing a
    // full green ring for an account that may be out of quota is the one thing
    // this app is not allowed to do.
    let percentage = credentials::dig_f64(limit, "percentage")?;
    Some(percentage.clamp(0.0, 100.0))
}

// ---------------------------------------------------------------- history
//
// The pane's usage history comes from the same storefront as the quota above.
// Two things about it are load-bearing, both learned by the original:
//
// **The host has no default.** It once had one, and a z.ai bearer key then went
// to `open.bigmodel.cn`, whose refusal the pane showed as an account that had
// never been used — the wrong answer about the wrong site. The request names its
// site, and the site is part of the identity of the answer too: switching between
// the international and the mainland storefront must never be served the other
// one's series.
//
// **A changed key is a different account.** Replacing a key re-reads the series;
// it does not continue the previous account's chart. Here that is the cache key
// as well: it carries a fingerprint of the key, so a new key can never reuse the
// old one's figures.

/// How many days the pane asks for. Asking for 90 gets a 500.
pub const HISTORY_DAYS: i64 = 30;
const STATISTICS_PATH: &str = "/api/monitor/usage/model-usage";

/// One day of the account's token use, as the service reports it.
///
/// The route prices nothing — no input/output/cache split either — so no cost is
/// inferred here. Every caller gets the day's token count and the models that
/// spent them, which is what the original's `.providerStatistics` ledger holds.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HistoryDay {
    pub date: String,
    pub tokens: u64,
    pub models: BTreeMap<String, u64>,
}

/// What a cached series is a series *of*: the site, the account's key, and the
/// day the reading belongs to.
#[derive(Clone, Debug, PartialEq, Eq)]
struct HistoryKey {
    site: String,
    key: u64,
    day: String,
}

struct CachedHistory {
    key: HistoryKey,
    saved: Instant,
    days: Vec<HistoryDay>,
}

static HISTORY: tokio::sync::Mutex<Option<CachedHistory>> = tokio::sync::Mutex::const_new(None);

/// A fingerprint of the key, never the key: the cache key must not hold a
/// credential, and the same key must always read the same way within a run.
fn fingerprint(api_key: &str) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    api_key.hash(&mut hasher);
    hasher.finish()
}

fn history_key(host: &str, api_key: &str, day: &str) -> HistoryKey {
    HistoryKey {
        site: host.trim().trim_end_matches('/').to_ascii_lowercase(),
        key: fingerprint(api_key),
        day: day.to_string(),
    }
}

/// The series already held — only when it is this site, this key and this day.
fn cached_days(
    cached: Option<&CachedHistory>,
    key: &HistoryKey,
    now: Instant,
) -> Option<Vec<HistoryDay>> {
    let cached = cached?;
    if &cached.key != key {
        return None;
    }
    let age = now.checked_duration_since(cached.saved)?;
    (age < Duration::from_secs(60)).then(|| cached.days.clone())
}

/// The statistics address, and the window it asks for.
///
/// The labels the service returns are `yyyy-MM-dd HH:mm` for hourly buckets and
/// `yyyy-MM-dd` for daily ones; the timestamps sent are `yyyy-MM-dd HH:mm:ss`
/// with no zone offset and no `T`, which is what the original sends.
fn statistics_request(
    host: &str,
    now: chrono::DateTime<chrono::Local>,
    days: i64,
) -> (String, Vec<(&'static str, String)>) {
    let start = now.date_naive() - chrono::Duration::days(days);
    let end = now.naive_local();
    (
        format!("{host}{STATISTICS_PATH}"),
        vec![
            ("endTime", end.format("%Y-%m-%d %H:%M:%S").to_string()),
            ("startTime", start.format("%Y-%m-%d 00:00:00").to_string()),
        ],
    )
}

/// The day a bucket label belongs to.
fn day_of(label: &str) -> Option<chrono::NaiveDate> {
    let date: String = label.chars().take(10).collect();
    chrono::NaiveDate::parse_from_str(&date, "%Y-%m-%d").ok()
}

/// The days a statistics payload holds, or nothing when it holds no use.
///
/// Every calendar day between the first and the last is filled in, zeros
/// included: the chart draws equal-width bars with no date axis, so a missing
/// day would otherwise be drawn as a missing bar. An empty payload and an
/// all-zero one are both "nothing" — a chart of nothing is worse than no chart.
fn ledger(payload: &Value) -> Option<Vec<HistoryDay>> {
    let data = payload.get("data").unwrap_or(payload);
    let labels = data
        .get("xTime")
        .or_else(|| data.get("x_time"))
        .and_then(Value::as_array)?;
    if labels.is_empty() {
        return None;
    }
    let totals: Vec<f64> = data
        .get("tokensUsage")
        .and_then(Value::as_array)
        .map(|counts| counts.iter().map(|n| n.as_f64().unwrap_or(0.0)).collect())
        .unwrap_or_default();
    let series = data
        .get("modelDataList")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let mut days: BTreeMap<chrono::NaiveDate, (u64, BTreeMap<String, u64>)> = BTreeMap::new();
    for (index, label) in labels.iter().enumerate() {
        let Some(date) = label.as_str().and_then(day_of) else {
            continue;
        };
        let mut models: BTreeMap<String, u64> = BTreeMap::new();
        for one in &series {
            let name = one
                .get("modelName")
                .and_then(Value::as_str)
                .filter(|name| !name.is_empty())
                .unwrap_or("未命名模型");
            let count = one
                .get("tokensUsage")
                .and_then(Value::as_array)
                .and_then(|counts| counts.get(index))
                .and_then(Value::as_f64)
                .map(|count| count.round().max(0.0) as u64)
                .unwrap_or(0);
            *models.entry(name.to_string()).or_insert(0) += count;
        }
        let from_models: u64 = models.values().sum();
        // The totals array may be absent or short; a bucket whose total was never
        // counted still shows what its models counted.
        let total = totals
            .get(index)
            .map(|count| count.round().max(0.0) as u64)
            .unwrap_or(from_models);
        if total == 0 {
            continue;
        }
        let slot = days.entry(date).or_default();
        slot.0 += total;
        for (name, count) in models {
            *slot.1.entry(name).or_insert(0) += count;
        }
    }
    let first = *days.keys().next()?;
    let last = *days.keys().next_back()?;
    let mut out = vec![];
    let mut cursor = first;
    while cursor <= last {
        let (tokens, models) = days.remove(&cursor).unwrap_or_default();
        out.push(HistoryDay {
            date: cursor.format("%Y-%m-%d").to_string(),
            tokens,
            models,
        });
        cursor = cursor.succ_opt()?;
    }
    Some(out)
}

/// The account's usage history for a provider id, as its card reads it.
///
/// The cache is keyed on the site, the key and the day, so the two things the
/// original fixed here cannot come back: a site switch cannot be answered with
/// the other storefront's series, and a replaced key cannot be answered with the
/// previous account's.
pub async fn history(ctx: &Ctx, id: &str) -> Result<Vec<HistoryDay>, String> {
    let (host, mainland) = host_for(id).ok_or_else(|| format!("unknown provider: {id}"))?;
    let key_text = resolve_key(id, mainland).ok_or_else(|| super::missing_key(id, ""))?;
    let now = chrono::Local::now();
    let key = history_key(host, &key_text, &now.date_naive().to_string());
    let mut cache = HISTORY.lock().await;
    if let Some(days) = cached_days(cache.as_ref(), &key, Instant::now()) {
        return Ok(days);
    }
    let (url, query) = statistics_request(host, now, HISTORY_DAYS);
    let response = ctx
        .gateway_client
        .get(&url)
        .query(&query)
        .bearer_auth(&key_text)
        .header("Accept", "application/json")
        .timeout(Duration::from_secs(20))
        .send()
        .await
        .map_err(|_| Failure::Network.text())?;
    // A key set by hand must not ride a redirect to a host it was not checked
    // against, so a redirect is a stop — and so is any final address off-site.
    if response.status().is_redirection()
        || crate::console_reads::foreign_site(response.url().as_str(), host)
    {
        return Err(Failure::Redirected.text());
    }
    if !response.status().is_success() {
        return Err(Failure::of_status(response.status().as_u16()).text());
    }
    let body = response.text().await.map_err(|_| Failure::Network.text())?;
    let payload: Value = serde_json::from_str(&body).map_err(|_| Failure::UnreadableReply.text())?;
    super::claude_code::debug_dump(id, &payload);
    // The envelope's own verdict, exactly as the quota route above reads it.
    let success = payload.get("success").and_then(Value::as_bool);
    let code = payload.get("code").and_then(Value::as_i64);
    if success != Some(true) || code != Some(200) {
        return Err(Failure::UnreadableReply.text());
    }
    let days = ledger(&payload).unwrap_or_default();
    *cache = Some(CachedHistory {
        key,
        saved: Instant::now(),
        days: days.clone(),
    });
    Ok(days)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone as _;
    use serde_json::json;

    /// A captured statistics reply: hourly labels (the service switches to
    /// `yyyy-MM-dd` when it buckets by day), one model split, and one bucket
    /// whose label is not a date at all.
    fn statistics_fixture() -> Value {
        json!({
            "success": true,
            "code": 200,
            "msg": "ok",
            "data": {
                "xTime": ["2026-09-05 10:00", "2026-09-05 11:00", "2026-09-06 09:00", "nonsense"],
                "tokensUsage": [1600.0, 200.0, 5000.0, 1.0],
                "modelDataList": [
                    { "modelName": "glm-4.6", "tokensUsage": [1400.0, 200.0, 5000.0, 0.0] },
                    { "modelName": "glm-4-flash", "tokensUsage": [200.0, 0.0, 0.0, 0.0] }
                ]
            }
        })
    }

    /// The mainland storefront's own series, with a gap in the middle of it.
    fn mainland_fixture() -> Value {
        json!({
            "success": true,
            "code": 200,
            "data": {
                "xTime": ["2026-09-07", "2026-09-09"],
                "tokensUsage": [900.0, 200.0],
                "modelDataList": []
            }
        })
    }

    /// Switching storefronts must never be answered with the other site's
    /// series — the cache key is the site, not just the provider.
    #[test]
    fn a_site_switch_is_a_different_series() {
        let now = Instant::now();
        let international = ledger(&statistics_fixture()).expect("the international series");
        let mainland = ledger(&mainland_fixture()).expect("the mainland series");
        assert_ne!(international, mainland);
        let key = history_key(INTERNATIONAL_HOST, "zai-key", "2026-09-10");
        let cached = CachedHistory {
            key: key.clone(),
            saved: now,
            days: international.clone(),
        };
        assert_eq!(cached_days(Some(&cached), &key, now), Some(international));
        let other_site = history_key(MAINLAND_HOST, "zai-key", "2026-09-10");
        assert_ne!(key, other_site);
        assert_eq!(cached_days(Some(&cached), &other_site, now), None);
        // A moved base URL is a different site too, not the same answer.
        let moved = history_key("https://api.z.ai.example/mirror", "zai-key", "2026-09-10");
        assert_eq!(cached_days(Some(&cached), &moved, now), None);
        // And the next day's reading is read again rather than aged over.
        let tomorrow = history_key(INTERNATIONAL_HOST, "zai-key", "2026-09-11");
        assert_eq!(cached_days(Some(&cached), &tomorrow, now), None);
    }

    /// Replacing the key must re-read the history: same site, a different key,
    /// and the old account's series is not reused.
    #[test]
    fn a_changed_key_rereads_the_history() {
        let now = Instant::now();
        let day = "2026-09-10";
        let first = history_key(INTERNATIONAL_HOST, "first-key", day);
        let second = history_key(INTERNATIONAL_HOST, "second-key", day);
        assert_ne!(first, second);
        let series = ledger(&statistics_fixture()).expect("the first account's series");
        let cached = CachedHistory {
            key: first.clone(),
            saved: now,
            days: series.clone(),
        };
        assert_eq!(cached_days(Some(&cached), &first, now), Some(series));
        assert_eq!(cached_days(Some(&cached), &second, now), None);
        // The key itself never reaches the cache key, only its fingerprint.
        assert!(!format!("{second:?}").contains("second-key"));
        assert!(!format!("{second:?}").contains("first-key"));
        // The same key read twice is the same identity.
        assert_eq!(first, history_key(INTERNATIONAL_HOST, "first-key", day));
    }

    /// The statistics route names its own site and window, and the window is
    /// the original's: thirty days back to midnight, no zone offset, no `T`.
    #[test]
    fn the_statistics_request_names_its_site_and_window() {
        let now = chrono::Local
            .with_ymd_and_hms(2026, 9, 10, 12, 30, 0)
            .single()
            .expect("a real local time");
        let (url, query) = statistics_request(INTERNATIONAL_HOST, now, HISTORY_DAYS);
        assert_eq!(url, "https://api.z.ai/api/monitor/usage/model-usage");
        assert!(!url.contains("bigmodel"));
        let names: Vec<&str> = query.iter().map(|(name, _)| *name).collect();
        assert_eq!(names, vec!["endTime", "startTime"]);
        let value = |name: &str| {
            query
                .iter()
                .find(|(key, _)| *key == name)
                .map(|(_, value)| value.clone())
                .expect("the window bound")
        };
        assert_eq!(value("startTime"), "2026-08-11 00:00:00");
        assert_eq!(value("endTime"), "2026-09-10 12:30:00");
        let (mainland, _) = statistics_request(MAINLAND_HOST, now, HISTORY_DAYS);
        assert_eq!(mainland, "https://open.bigmodel.cn/api/monitor/usage/model-usage");
    }

    /// Hourly buckets are folded into their day, the labels that are not dates
    /// are skipped, and every day between the first and the last is drawn.
    #[test]
    fn the_ledger_folds_hours_into_days_and_fills_the_gaps() {
        let days = ledger(&statistics_fixture()).expect("two days of use");
        let dates: Vec<&str> = days.iter().map(|day| day.date.as_str()).collect();
        assert_eq!(dates, vec!["2026-09-05", "2026-09-06"]);
        assert_eq!(days[0].tokens, 1800);
        assert_eq!(days[1].tokens, 5000);
        assert_eq!(days[0].models.get("glm-4.6"), Some(&1600));
        assert_eq!(days[0].models.get("glm-4-flash"), Some(&200));
        let sparse = ledger(&mainland_fixture()).expect("three days across a gap");
        let tokens: Vec<u64> = sparse.iter().map(|day| day.tokens).collect();
        assert_eq!(tokens, vec![900, 0, 200]);
        assert_eq!(sparse[1].date, "2026-09-08");
    }

    /// Totals that were not counted fall back to the models, fractional counts
    /// are rounded rather than truncated, and a payload with no use at all is
    /// nothing rather than a chart of zeros.
    #[test]
    fn missing_totals_fall_back_to_the_models_and_an_empty_payload_is_nothing() {
        let payload = json!({
            "success": true,
            "code": 200,
            "data": {
                "xTime": ["2026-09-05", "2026-09-06"],
                "modelDataList": [{ "modelName": "glm-4.6", "tokensUsage": [12.5, 500.0] }]
            }
        });
        let days = ledger(&payload).expect("two days of use");
        assert_eq!(days.len(), 2);
        assert_eq!(days[0].tokens, 13);
        assert_eq!(days[0].models.get("glm-4.6"), Some(&13));
        assert_eq!(days[1].tokens, 500);
        assert!(ledger(&json!({
            "success": true,
            "code": 200,
            "data": { "xTime": [], "tokensUsage": [] }
        }))
        .is_none());
        assert!(ledger(&json!({
            "success": true,
            "code": 200,
            "data": { "xTime": ["2026-09-05"], "tokensUsage": [0.0] }
        }))
        .is_none());
    }

    /// A captured reply's shape: the envelope, and the three limit kinds the
    /// service actually sends. The unit codes are the vendor's — 3 is hours,
    /// 5 is minutes, 6 is weeks — so a five-hour limit is `unit: 3, number: 5`.
    fn fixture() -> Value {
        json!({
            "success": true,
            "code": 200,
            "msg": "ok",
            "data": {
                "planName": "GLM Coding Pro",
                "limits": [
                    { "type": "TOKENS_LIMIT", "unit": 3, "number": 5,
                      "percentage": 12.0, "nextResetTime": 1790000000000i64 },
                    { "type": "TOKENS_LIMIT", "unit": 6, "number": 1,
                      "percentage": 64.0 },
                    { "type": "TIME_LIMIT", "unit": 5, "number": 1,
                      "percentage": 3.0 }
                ]
            }
        })
    }

    fn limits_of(payload: &Value) -> Vec<Value> {
        payload["data"]["limits"].as_array().cloned().unwrap_or_default()
    }

    #[test]
    fn names_the_windows_the_service_states() {
        let windows = windows_from(&limits_of(&fixture()));
        let labels: Vec<&str> = windows.iter().map(|w| w.label.as_str()).collect();
        assert_eq!(labels, vec!["5h", "7d", "Monthly MCP"]);
        assert_eq!(windows[0].percent_used, Some(12.0));
        assert_eq!(windows[1].percent_used, Some(64.0));
        assert_eq!(windows[2].percent_used, Some(3.0));
    }

    #[test]
    fn the_reset_is_epoch_milliseconds() {
        let windows = windows_from(&limits_of(&fixture()));
        // 1790000000000 ms is 2026-09-21T14:13:20Z.
        assert_eq!(windows[0].resets_at.as_deref(), Some("2026-09-21T14:13:20Z"));
        assert!(windows[1].resets_at.is_none());
    }

    #[test]
    fn counts_are_finer_than_the_whole_percentage() {
        let limits = vec![json!({
            "type": "TOKENS_LIMIT", "unit": 3, "number": 5,
            "percentage": 12.0, "usage": 1000.0, "remaining": 875.0
        })];
        // 125 of 1000 gone is 12.5%, which the whole number could not say.
        assert_eq!(windows_from(&limits)[0].percent_used, Some(12.5));
    }

    #[test]
    fn a_measured_spend_wins_over_the_remainder() {
        let limits = vec![json!({
            "type": "TOKENS_LIMIT", "unit": 3, "number": 5,
            "usage": 100.0, "remaining": 90.0, "currentValue": 40.0
        })];
        assert_eq!(windows_from(&limits)[0].percent_used, Some(40.0));
    }

    #[test]
    fn a_limit_with_no_figure_is_left_unread() {
        let limits = vec![json!({ "type": "CREDIT_LIMIT", "unit": 1, "number": 1 })];
        assert!(windows_from(&limits).is_empty());
    }

    #[test]
    fn an_unknown_unit_is_dropped_rather_than_named() {
        let limits = vec![json!({
            "type": "TOKENS_LIMIT", "unit": 99, "number": 3, "percentage": 5.0
        })];
        assert!(windows_from(&limits).is_empty());
    }

    #[test]
    fn an_unknown_type_is_left_out() {
        let limits = vec![json!({
            "type": "SOMETHING_LIMIT", "unit": 3, "number": 300, "percentage": 5.0
        })];
        assert!(windows_from(&limits).is_empty());
    }

    #[test]
    fn the_plan_comes_from_whichever_key_the_tier_uses() {
        assert_eq!(plan_label(&fixture()).as_deref(), Some("GLM Coding Pro"));
        assert_eq!(
            plan_label(&json!({ "data": { "level": "pro" } })).as_deref(),
            Some("pro")
        );
        assert!(plan_label(&json!({ "data": {} })).is_none());
    }

    #[test]
    fn a_refused_key_arrives_as_a_good_http_status() {
        let reply = json!({ "success": false, "code": 1000, "msg": "身份验证失败。" });
        assert_eq!(problem(&reply), Problem::KeyRefused);
        let reply = json!({ "success": false, "code": 401, "msg": "令牌已过期或验证不正确" });
        assert_eq!(problem(&reply), Problem::KeyRefused);
        let reply = json!({ "success": false, "code": 1001, "msg": "" });
        assert_eq!(problem(&reply), Problem::KeyRefused);
    }

    #[test]
    fn no_coding_plan_is_not_a_bad_key() {
        // A working key on an account with no subscription answers 500 with
        // this sentence, and 500 alone would say the service broke.
        let reply = json!({
            "success": false, "code": 500,
            "msg": "There is no coding plan subscription for this account"
        });
        assert_eq!(problem(&reply), Problem::NoCodingPlan);
        assert!(Problem::NoCodingPlan.message().contains("no coding plan"));
    }

    #[test]
    fn an_unrecognised_failure_is_the_services_not_the_keys() {
        let reply = json!({ "success": false, "code": 500, "msg": "internal error" });
        assert_eq!(problem(&reply), Problem::ServerError);
        let reply = json!({ "success": false, "code": 1302, "msg": "too many requests" });
        assert_eq!(problem(&reply), Problem::ServerError);
    }
}
