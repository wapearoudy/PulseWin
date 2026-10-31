//! Cursor quota.
//!
//! Cursor keeps its session in a SQLite `state.vscdb` rather than a JSON file,
//! and its usage API is private and cookie-authenticated. PulseWin therefore
//! cannot silently reuse a login the way it can for the CLI tools: the user
//! supplies the session cookie once via
//! `PULSEWIN_CURSOR_COOKIE` or `%APPDATA%\PulseWin\cursor.json`.
//!
//! `cursor.json` shape: `{ "cookie": "WorkosCursorSessionToken=user_xxx%3A%3Ajwt" }`
//!
//! Cursor bills a month and the plan includes two pools: one for Cursor's own
//! models (Composer, Cursor Grok) and one for everything else. Newer plans
//! include usage by the **week** instead; their usage summary reports both
//! monthly pools at 0% while the editor draws the week from
//! `get-plan-info` and `get-current-period-usage`. Enterprise accounts may
//! report neither pool but one shared money allowance.
//!
//! `https://cursor.com/api/usage-summary`,
//! `https://cursor.com/api/dashboard/get-plan-info` and
//! `https://cursor.com/api/dashboard/get-current-period-usage` are undocumented
//! and can change without notice.

use std::sync::Arc;

use serde::Deserialize;
use serde_json::Value;

use super::{Ctx, FetchFuture, Provider};
use crate::credentials::{self, cursor_state_db_paths};
use crate::model::{parse_reset, ProviderUsage, UsageWindow, WindowKind};

const USAGE_SUMMARY_URL: &str = "https://cursor.com/api/usage-summary";
const PLAN_INFO_URL: &str = "https://cursor.com/api/dashboard/get-plan-info";
const PERIOD_USAGE_URL: &str = "https://cursor.com/api/dashboard/get-current-period-usage";
/// Older Cursor deployments answered this per-model endpoint before
/// `usage-summary` existed. It is kept as a fallback only.
const LEGACY_USAGE_URL_TEMPLATE: &str = "https://www.cursor.com/api/usage?user={user}";

pub struct Cursor;

impl Provider for Cursor {
    fn id(&self) -> &'static str {
        "cursor"
    }

    fn name(&self) -> &'static str {
        "Cursor"
    }

    /// The session cookie the fetch reads: `PULSEWIN_CURSOR_COOKIE`, or the
    /// `cookie` field of `%APPDATA%\PulseWin\cursor.json`.
    ///
    /// The SQLite store Cursor keeps its own login in is **not** evidence here:
    /// this port cannot read it, so a machine whose only Cursor login is that
    /// file has nothing PulseWin can fetch with.
    fn is_configured(&self) -> bool {
        if credentials::env_override(self.id(), "cookie").is_some() {
            return true;
        }
        config_path()
            .and_then(|path| credentials::read_json(&path))
            .and_then(|json| credentials::dig_str(&json, "cookie"))
            .is_some()
    }

    fn fetch(&self, ctx: Arc<Ctx>) -> FetchFuture {
        Box::pin(async move { fetch_inner(ctx).await })
    }
}

struct CursorAuth {
    cookie: String,
    user_id: Option<String>,
}

/// One request's failure. `status` is `None` for transport, body-read and JSON
/// errors; an HTTP status is kept so authentication failures are not retried
/// against the legacy endpoint.
struct RequestFailure {
    message: String,
    status: Option<u16>,
}

impl RequestFailure {
    fn other(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            status: None,
        }
    }
}

#[derive(Clone, Copy)]
enum Method {
    Get,
    Post,
}

async fn fetch_inner(ctx: Arc<Ctx>) -> ProviderUsage {
    const ID: &str = "cursor";
    const NAME: &str = "Cursor";

    let auth = match resolve_auth() {
        Ok(a) => a,
        Err(e) => return ProviderUsage::failed(ID, NAME, e),
    };

    let summary = match request_json(&ctx, &auth.cookie, USAGE_SUMMARY_URL, Method::Get).await {
        Ok(value) => value,
        Err(error) => {
            // A spent cookie or a rate limit is not evidence that the old
            // endpoint can answer; return the real reason instead of asking
            // twice. Any other failure keeps pre-1.12 Cursor deployments
            // working through the old per-model endpoint.
            if matches!(error.status, Some(401 | 403 | 429)) {
                return ProviderUsage::failed(ID, NAME, error.message);
            }
            return match fetch_legacy(&ctx, &auth).await {
                Ok(usage) => usage,
                Err(legacy) => ProviderUsage::failed(
                    ID,
                    NAME,
                    format!("{}; legacy endpoint: {legacy}", error.message),
                ),
            };
        }
    };

    super::claude_code::debug_dump(ID, &summary);

    let reply: Reply = match serde_json::from_value(summary) {
        Ok(reply) => reply,
        Err(error) => return ProviderUsage::failed(ID, NAME, format!("bad reply: {error}")),
    };

    let mut windows = windows_from_reply(&reply);

    // A plan whose included usage is weekly: the two monthly pools are not
    // what it spends, so they give way to the week. A plan-info call that
    // fails leaves the summary's pools as they were; once the plan has said
    // weekly, a week that cannot be read is a failed read.
    if let Ok(value) = request_json(&ctx, &auth.cookie, PLAN_INFO_URL, Method::Post).await {
        if let Ok(plan_info) = serde_json::from_value::<PlanInfoReply>(value) {
            if let Some(plan) = plan_info.plan_info.filter(|plan| plan.is_weekly()) {
                let usage_value =
                    match request_json(&ctx, &auth.cookie, PERIOD_USAGE_URL, Method::Post).await {
                        Ok(value) => value,
                        Err(error) => return ProviderUsage::failed(ID, NAME, error.message),
                    };
                let usage: PeriodUsageReply = match serde_json::from_value(usage_value) {
                    Ok(usage) => usage,
                    Err(error) => {
                        return ProviderUsage::failed(ID, NAME, format!("bad reply: {error}"))
                    }
                };
                windows = weekly_windows(&plan, &usage, windows);
            }
        }
    }

    if windows.is_empty() {
        return ProviderUsage::failed(ID, NAME, "no quota windows in response");
    }

    let plan = plan_name(reply.membership_type.as_deref());
    let credit = remaining(&reply);
    let mut usage = ProviderUsage::ok(ID, NAME, windows)
        .with_plan(plan)
        .with_account(auth.user_id);
    if let Some(amount) = credit {
        usage = usage.with_credit_remaining(amount, "USD");
    }
    usage
}

async fn request_json(
    ctx: &Ctx,
    cookie: &str,
    url: &str,
    method: Method,
) -> Result<Value, RequestFailure> {
    let mut request = match method {
        Method::Get => ctx.gateway_client.get(url),
        Method::Post => ctx.gateway_client.post(url),
    };
    request = request
        .header("Cookie", cookie)
        .header("Accept", "application/json");
    if matches!(method, Method::Post) {
        request = request
            .header("Content-Type", "application/json")
            .header("Origin", "https://cursor.com")
            .body("{}");
    }

    let response = request.send().await.map_err(|error| {
        RequestFailure::other(format!(
            "request failed: {}",
            super::describe_reqwest_error(&error)
        ))
    })?;

    let status = response.status();
    let body = response
        .text()
        .await
        .map_err(|error| RequestFailure::other(format!("cannot read body: {error}")))?;

    if !status.is_success() {
        let hint = match status.as_u16() {
            401 | 403 => " — the session cookie expired; copy a fresh one from cursor.com",
            429 => " — Cursor is rate limiting requests; try again later",
            _ => "",
        };
        return Err(RequestFailure {
            message: format!("HTTP {status}{hint}"),
            status: Some(status.as_u16()),
        });
    }

    serde_json::from_str(&body).map_err(|error| RequestFailure::other(format!("bad JSON: {error}")))
}

/// The pre-1.12 fallback: per-model request counts rather than a plan summary.
async fn fetch_legacy(ctx: &Ctx, auth: &CursorAuth) -> Result<ProviderUsage, String> {
    let url = match &auth.user_id {
        Some(user) => LEGACY_USAGE_URL_TEMPLATE.replace("{user}", user),
        // Cursor accepts the session cookie alone on some deployments.
        None => "https://www.cursor.com/api/usage".to_string(),
    };

    let json = request_json(ctx, &auth.cookie, &url, Method::Get)
        .await
        .map_err(|error| error.message)?;
    super::claude_code::debug_dump("cursor", &json);

    let windows = extract_windows(&json);
    if windows.is_empty() {
        return Err("no quota windows in response".to_string());
    }

    let plan = credentials::dig_first_str(&json, &["plan", "membershipType", "subscriptionType"]);
    Ok(ProviderUsage::ok("cursor", "Cursor", windows)
        .with_plan(plan)
        .with_account(auth.user_id.clone()))
}

// ---------------------------------------------------------------------------
// Reply shapes
// ---------------------------------------------------------------------------

/// `GET /api/usage-summary`.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Reply {
    /// Cursor's billing cycle end, an RFC3339 timestamp.
    billing_cycle_end: Option<String>,
    membership_type: Option<String>,
    individual_usage: Option<IndividualUsage>,
    team_usage: Option<TeamUsage>,
}

/// One pot of money. `used`, `limit` and `remaining` are **cents**; the lane
/// percentages are percentages.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Allowance {
    enabled: Option<bool>,
    used: Option<f64>,
    limit: Option<f64>,
    remaining: Option<f64>,
    /// Cursor's own models — what the account page calls "Cursor Models".
    auto_percent_used: Option<f64>,
    /// Everything else, which the page calls "Other Models".
    api_percent_used: Option<f64>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct IndividualUsage {
    plan: Option<Allowance>,
    on_demand: Option<Allowance>,
    /// Enterprise plans report one shared allowance here rather than the two
    /// model pools. Its money fields are cents.
    overall: Option<Allowance>,
}

/// The same two pots for an account billed as a team.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct TeamUsage {
    pooled: Option<Allowance>,
    on_demand: Option<Allowance>,
}

/// `POST /api/dashboard/get-plan-info`.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PlanInfoReply {
    plan_info: Option<PlanInfo>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PlanInfo {
    included_usage_period: Option<Period>,
    included_usage_resets_at: Option<Stamp>,
}

impl PlanInfo {
    fn is_weekly(&self) -> bool {
        self.included_usage_period
            .as_ref()
            .is_some_and(Period::is_weekly)
    }
}

/// `INCLUDED_USAGE_PERIOD_WEEKLY`, or `2`, its number in the editor's schema.
/// Anything else — monthly, unspecified, a name not seen yet — is not weekly.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Period {
    Weekly,
    Other,
}

impl Period {
    fn is_weekly(&self) -> bool {
        matches!(self, Period::Weekly)
    }
}

impl<'de> Deserialize<'de> for Period {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Raw {
            Number(i64),
            Name(String),
        }

        Ok(match Raw::deserialize(deserializer)? {
            Raw::Number(2) => Period::Weekly,
            Raw::Number(_) => Period::Other,
            Raw::Name(name) => {
                let name = name.to_uppercase();
                if name == "INCLUDED_USAGE_PERIOD_WEEKLY" || name == "WEEKLY" {
                    Period::Weekly
                } else {
                    Period::Other
                }
            }
        })
    }
}

/// A moment in **milliseconds**, written as a number or as a string. Absent
/// or unreadable is `None`, never the epoch.
#[derive(Debug, Clone)]
struct Stamp {
    date: Option<String>,
}

impl<'de> Deserialize<'de> for Stamp {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Raw {
            Number(f64),
            Text(String),
        }

        let milliseconds = match Raw::deserialize(deserializer)? {
            Raw::Number(number) => Some(number),
            Raw::Text(text) => text.trim().parse::<f64>().ok(),
        };
        let date = milliseconds.and_then(|milliseconds| {
            if !milliseconds.is_finite() || milliseconds <= 0.0 {
                return None;
            }
            chrono::DateTime::<chrono::Utc>::from_timestamp_millis(milliseconds as i64)
                .map(|date| date.to_rfc3339_opts(chrono::SecondsFormat::Millis, true))
        });
        Ok(Self { date })
    }
}

/// `POST /api/dashboard/get-current-period-usage`. Only the one figure the
/// editor draws as the week is read.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PeriodUsageReply {
    plan_usage: Option<PlanUsage>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PlanUsage {
    /// A percentage, like the pools' — 25 means 25%.
    total_percent_used: Option<f64>,
}

// ---------------------------------------------------------------------------
// Mapping
// ---------------------------------------------------------------------------

/// The week, in place of the two monthly pools; extra spend, when the account
/// has it, stays as it was.
///
/// The figure is `totalPercentUsed`, because that is what the editor draws
/// under "Weekly usage limit" on such a plan, with `includedUsageResetsAt`
/// as its reset. The period is the plan's own word, so the seven days are a
/// stated length. **No figure is no window** — not a week at 0%.
fn weekly_windows(
    plan: &PlanInfo,
    usage: &PeriodUsageReply,
    summary: Vec<UsageWindow>,
) -> Vec<UsageWindow> {
    let mut found = Vec::new();
    if let Some(percent) = usage
        .plan_usage
        .as_ref()
        .and_then(|usage| usage.total_percent_used)
    {
        if percent.is_finite() {
            found.push(
                UsageWindow::new("7d", Some(percent.clamp(0.0, 100.0)))
                    .with_id(Some("weekly".to_string()))
                    .with_kind(WindowKind::Limit)
                    .with_duration(Some(7.0 * 86_400.0))
                    .with_reset(
                        plan.included_usage_resets_at
                            .as_ref()
                            .and_then(|stamp| stamp.date.clone()),
                    )
                    .with_exhausted(percent >= 100.0),
            );
        }
    }
    found.extend(
        summary
            .into_iter()
            .filter(|window| window.id.as_deref() == Some("onDemand")),
    );
    found
}

fn windows_from_reply(reply: &Reply) -> Vec<UsageWindow> {
    let resets = reply
        .billing_cycle_end
        .as_deref()
        .and_then(|text| parse_reset(&Value::String(text.to_string())));
    // A team account reports the same shape under another name.
    let plan = reply
        .individual_usage
        .as_ref()
        .and_then(|usage| usage.plan.as_ref())
        .or_else(|| {
            reply
                .team_usage
                .as_ref()
                .and_then(|usage| usage.pooled.as_ref())
        });
    let overall = reply
        .individual_usage
        .as_ref()
        .and_then(|usage| usage.overall.as_ref());
    let mut found = Vec::new();

    // The two pools the plan includes, in the order and under the names the
    // account page gives them.
    if let Some(pool) = pool(
        plan.and_then(|allowance| allowance.auto_percent_used),
        "cursorModels",
        "Cursor Models",
        resets.clone(),
    ) {
        found.push(pool);
    }
    if let Some(pool) = pool(
        plan.and_then(|allowance| allowance.api_percent_used),
        "otherModels",
        "Other Models",
        resets.clone(),
    ) {
        found.push(pool);
    }

    // An account shape that reports no pools at all still has the money:
    // what the plan is worth and how much of it has gone. Only used as a
    // fallback, since on an account that does report pools this is a
    // different denominator and would read as a third, contradictory limit.
    if found.is_empty() {
        if let Some(window) = money(plan.or(overall), "plan", "Monthly", resets.clone()) {
            found.push(window);
        }
    }

    // Spending past the plan. Off unless the account has turned it on.
    if let Some(window) = money(
        reply
            .individual_usage
            .as_ref()
            .and_then(|usage| usage.on_demand.as_ref())
            .or_else(|| {
                reply
                    .team_usage
                    .as_ref()
                    .and_then(|usage| usage.on_demand.as_ref())
            }),
        "onDemand",
        "Spend",
        resets,
    ) {
        found.push(window);
    }

    found
}

/// One of the plan's two pools.
///
/// **The figure is a percentage, not a fraction** — 0.0267 means 0.0267%,
/// not 2.67%.
fn pool(
    percent: Option<f64>,
    id: &str,
    scope: &str,
    resets: Option<String>,
) -> Option<UsageWindow> {
    let percent = percent.filter(|percent| percent.is_finite())?;

    Some(
        UsageWindow::new("Monthly", Some(percent.clamp(0.0, 100.0)))
            .with_id(Some(id.to_string()))
            .with_scope(Some(scope.to_string()))
            .with_kind(WindowKind::Limit)
            .with_reset(resets)
            .with_exhausted(percent >= 100.0),
    )
}

/// A pot measured in money rather than as a share of a pool: how many of its
/// cents have gone. `detail` carries the USD amounts, because the local model
/// has no structured money field for a window.
fn money(
    allowance: Option<&Allowance>,
    id: &str,
    label: &str,
    resets: Option<String>,
) -> Option<UsageWindow> {
    let allowance = allowance?;
    if allowance.enabled == Some(false) {
        return None;
    }
    let used = allowance.used.filter(|value| value.is_finite())?;
    let limit = allowance.limit.filter(|value| value.is_finite())?;
    if limit <= 0.0 {
        return None;
    }

    let detail = format!("${:.2} / ${:.2}", used / 100.0, limit / 100.0);
    Some(
        UsageWindow::new(label, Some((used / limit * 100.0).clamp(0.0, 100.0)))
            .with_id(Some(id.to_string()))
            .with_kind(WindowKind::Limit)
            .with_reset(resets)
            .with_exhausted(used >= limit)
            .with_detail(Some(detail)),
    )
}

/// What is left of the plan's allowance, in dollars.
fn remaining(reply: &Reply) -> Option<f64> {
    let allowance = reply
        .individual_usage
        .as_ref()
        .and_then(|usage| usage.plan.as_ref())
        .or_else(|| {
            reply
                .team_usage
                .as_ref()
                .and_then(|usage| usage.pooled.as_ref())
        })
        .or_else(|| {
            reply
                .individual_usage
                .as_ref()
                .and_then(|usage| usage.overall.as_ref())
        })?;
    allowance
        .remaining
        .filter(|remaining| remaining.is_finite())
        .map(|remaining| remaining / 100.0)
}

/// "pro_plus" → "Pro+". An unfamiliar tier is tidied and passed through
/// rather than blanked.
fn plan_name(membership: Option<&str>) -> Option<String> {
    let membership = membership?;
    if membership.is_empty() {
        return None;
    }

    let mut name = String::new();
    for part in membership.split('_') {
        let piece = if part == "plus" {
            "+".to_string()
        } else {
            let mut chars = part.chars();
            match chars.next() {
                Some(first) => {
                    first.to_uppercase().collect::<String>() + &chars.as_str().to_lowercase()
                }
                None => String::new(),
            }
        };
        if name.is_empty() || piece == "+" {
            name.push_str(&piece);
        } else {
            name.push(' ');
            name.push_str(&piece);
        }
    }
    Some(name)
}

fn config_path() -> Option<std::path::PathBuf> {
    credentials::config_relative(&["PulseWin", "cursor.json"])
        .into_iter()
        .next()
}

fn resolve_auth() -> Result<CursorAuth, String> {
    if let Some(cookie) = credentials::env_override("cursor", "cookie") {
        let user_id = user_from_cookie(&cookie);
        return Ok(CursorAuth { cookie, user_id });
    }

    if let Some(path) = config_path() {
        if let Some(json) = credentials::read_json(&path) {
            if let Some(cookie) = credentials::dig_str(&json, "cookie") {
                let user_id =
                    credentials::dig_str(&json, "userId").or_else(|| user_from_cookie(&cookie));
                return Ok(CursorAuth { cookie, user_id });
            }
        }
    }

    let db_hint = cursor_state_db_paths()
        .first()
        .map(|p| p.display().to_string())
        .unwrap_or_else(|| "%APPDATA%\\Cursor\\User\\globalStorage\\state.vscdb".to_string());

    let config_hint = config_path()
        .map(|p| p.display().to_string())
        .unwrap_or_else(|| "%APPDATA%\\PulseWin\\cursor.json".to_string());

    Err(format!(
        "Cursor needs a one-time session cookie. Set PULSEWIN_CURSOR_COOKIE, \
         or create {config_hint} with {{\"cookie\": \"...\"}}. \
         (Cursor's login lives in {db_hint}, which is a SQLite store PulseWin does not read.)"
    ))
}

/// Cursor's cookie is `WorkosCursorSessionToken=<user_id>::<jwt>`; the user id
/// is also what the legacy usage endpoint wants in its `user` query parameter.
fn user_from_cookie(cookie: &str) -> Option<String> {
    let value = cookie
        .split(';')
        .map(str::trim)
        .find_map(|part| part.strip_prefix("WorkosCursorSessionToken="))?;
    let decoded = value.replace("%3A%3A", "::").replace("%3a%3a", "::");
    let user = decoded.split("::").next()?.trim();
    if user.is_empty() {
        None
    } else {
        Some(user.to_string())
    }
}

/// Cursor's older per-model reply, e.g.
/// `{ "gpt-4": { "numRequests": 12, "maxRequestUsage": 500 }, "startOfMonth": "..." }`
fn extract_windows(json: &Value) -> Vec<UsageWindow> {
    let mut out = Vec::new();

    let start_of_month = json.get("startOfMonth").and_then(parse_reset);
    // Cursor reports the *next* reset as the start of the next billing month.
    let resets_at = start_of_month
        .as_deref()
        .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
        .and_then(|dt| dt.checked_add_months(chrono::Months::new(1)))
        .map(|dt| dt.to_rfc3339_opts(chrono::SecondsFormat::Secs, true));

    let Some(map) = json.as_object() else {
        return out;
    };

    for (key, node) in map {
        if key == "startOfMonth" || !node.is_object() {
            continue;
        }

        let used = credentials::dig_first_f64(node, &["numRequests", "requests", "used"]);
        let max = credentials::dig_first_f64(
            node,
            &["maxRequestUsage", "maxRequests", "limit", "entitlement"],
        );

        if used.is_none() && max.is_none() {
            continue;
        }

        let percent = match (used, max) {
            (Some(u), Some(m)) if m > 0.0 => Some(((u / m) * 100.0).clamp(0.0, 100.0)),
            _ => None,
        };

        let detail = match (used, max) {
            (Some(u), Some(m)) => Some(format!("{} / {} requests", u.round(), m.round())),
            (Some(u), None) => Some(format!("{} requests", u.round())),
            _ => None,
        };

        out.push(
            UsageWindow::new(label_for(key), percent)
                .with_reset(resets_at.clone())
                .with_detail(detail),
        );
    }

    // Busiest model first, so the ring shows the thing that will run out.
    out.sort_by(|a, b| {
        b.percent_used
            .partial_cmp(&a.percent_used)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    out.truncate(4);
    out
}

fn label_for(key: &str) -> String {
    match key {
        "gpt-4" => "GPT-4".to_string(),
        "gpt-3.5-turbo" => "GPT-3.5".to_string(),
        other => other.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    // Fixtures reconstructed from Pulse 1.12.4's Cursor tests. The weekly
    // plan-info and period-usage replies are captured from a Premium account.
    const NORMAL: &str = r#"{
      "billingCycleEnd": "2026-10-01T00:00:00.000Z",
      "membershipType": "pro_plus",
      "individualUsage": {
        "plan": {
          "enabled": true, "used": 1200, "limit": 2000, "remaining": 800,
          "autoPercentUsed": 0.0267, "apiPercentUsed": 0.05
        },
        "onDemand": { "enabled": false, "used": 0, "limit": 0, "remaining": 0 }
      },
      "teamUsage": null
    }"#;

    const ONDEMAND: &str = r#"{
      "billingCycleEnd": "2026-10-01T00:00:00.000Z",
      "membershipType": "pro",
      "individualUsage": {
        "plan": {
          "enabled": true, "used": 500, "limit": 2000, "remaining": 1500,
          "autoPercentUsed": 10, "apiPercentUsed": 2
        },
        "onDemand": { "enabled": true, "used": 350, "limit": 1000, "remaining": 650 }
      }
    }"#;

    const FALLBACK_NO_POOLS: &str = r#"{
      "billingCycleEnd": "2026-10-01T00:00:00.000Z",
      "membershipType": "free",
      "individualUsage": {
        "plan": { "enabled": true, "used": 5, "limit": 20, "remaining": 15 }
      }
    }"#;

    const TEAM: &str = r#"{
      "billingCycleEnd": "2026-10-01T00:00:00.000Z",
      "membershipType": "business",
      "teamUsage": {
        "pooled": {
          "enabled": true, "used": 3000, "limit": 10000, "remaining": 7000,
          "autoPercentUsed": 15, "apiPercentUsed": 3
        },
        "onDemand": { "enabled": true, "used": 100, "limit": 500, "remaining": 400 }
      }
    }"#;

    const EXHAUSTED: &str = r#"{
      "billingCycleEnd": "2026-10-01T00:00:00.000Z",
      "membershipType": "pro",
      "individualUsage": {
        "plan": {
          "enabled": true, "used": 2000, "limit": 2000, "remaining": 0,
          "autoPercentUsed": 105, "apiPercentUsed": 0
        }
      }
    }"#;

    const MISSING_FIELDS: &str = r#"{ "membershipType": null }"#;

    const ENTERPRISE_OVERALL: &str = r#"{
      "billingCycleStart": "2026-10-01T00:00:00.000Z",
      "billingCycleEnd": "2026-11-01T00:00:00.000Z",
      "membershipType": "enterprise",
      "limitType": "team",
      "isUnlimited": false,
      "individualUsage": {
        "overall": { "enabled": true, "used": 80, "limit": 80000, "remaining": 79920 }
      },
      "teamUsage": {
        "onDemand": { "enabled": true, "used": 0, "limit": null, "remaining": null }
      }
    }"#;

    const PLAN_INFO_WEEKLY: &str = r#"{
      "planInfo": {
        "planName": "Premium",
        "price": "",
        "billingCycleEnd": "1794530890655",
        "includedUsagePeriod": "INCLUDED_USAGE_PERIOD_WEEKLY",
        "includedUsageResetsAt": "1792475010907"
      },
      "nextUpgrade": { "tier": "super", "name": "Super", "includedUsagePeriod": "INCLUDED_USAGE_PERIOD_WEEKLY" },
      "isMerged": true
    }"#;

    const PERIOD_USAGE_WEEKLY: &str = r#"{
      "billingCycleStart": "1791870210907",
      "billingCycleEnd": "1792475010907",
      "planUsage": {
        "remainingBonus": false,
        "autoPercentUsed": 35.51462621885157,
        "apiPercentUsed": 0,
        "totalPercentUsed": 35.51462621885157
      },
      "spendLimitUsage": { "limitType": "user" },
      "enabled": true,
      "displayMessage": "You've used 36% of your included usage"
    }"#;

    const PERIOD_USAGE_NO_FIGURE: &str = r#"{
      "billingCycleStart": "1791870210907",
      "billingCycleEnd": "1792475010907",
      "planUsage": { "remainingBonus": false },
      "enabled": true
    }"#;

    const PLAN_INFO_MONTHLY: &str = r#"{
      "planInfo": {
        "planName": "Pro+",
        "includedAmountCents": 7000,
        "price": "$60/mo",
        "billingCycleEnd": "1793491200000",
        "includedUsagePeriod": "INCLUDED_USAGE_PERIOD_MONTHLY"
      },
      "isMerged": false
    }"#;

    fn reply(json: &str) -> Reply {
        serde_json::from_str(json).expect("fixture should decode")
    }

    fn plan_info(json: &str) -> PlanInfo {
        serde_json::from_str::<PlanInfoReply>(json)
            .expect("plan fixture should decode")
            .plan_info
            .expect("planInfo present")
    }

    fn period_usage(json: &str) -> PeriodUsageReply {
        serde_json::from_str(json).expect("period fixture should decode")
    }

    // --- Existing tests ---------------------------------------------------

    #[test]
    fn extracts_user_id_from_cookie() {
        let cookie = "WorkosCursorSessionToken=user_01ABC%3A%3Ajwt.token.here; other=1";
        assert_eq!(user_from_cookie(cookie).as_deref(), Some("user_01ABC"));
    }

    #[test]
    fn cookie_without_token_yields_none() {
        assert!(user_from_cookie("other=1").is_none());
    }

    #[test]
    fn computes_percent_from_requests() {
        let payload = json!({
            "gpt-4": { "numRequests": 125, "maxRequestUsage": 500 },
            "startOfMonth": "2026-10-01T00:00:00.000Z"
        });
        let windows = extract_windows(&payload);
        assert_eq!(windows.len(), 1);
        assert_eq!(windows[0].label, "GPT-4");
        assert_eq!(windows[0].percent_used, Some(25.0));
        assert_eq!(windows[0].detail.as_deref(), Some("125 / 500 requests"));
        assert!(windows[0].resets_at.is_some());
    }

    #[test]
    fn sorts_busiest_first_and_truncates() {
        let payload = json!({
            "a": { "numRequests": 1, "maxRequestUsage": 100 },
            "b": { "numRequests": 90, "maxRequestUsage": 100 },
            "c": { "numRequests": 50, "maxRequestUsage": 100 },
            "d": { "numRequests": 10, "maxRequestUsage": 100 },
            "e": { "numRequests": 5, "maxRequestUsage": 100 }
        });
        let windows = extract_windows(&payload);
        assert_eq!(windows.len(), 4);
        assert_eq!(windows[0].label, "b");
    }

    #[test]
    fn skips_non_object_and_start_of_month() {
        let payload = json!({ "startOfMonth": "2026-10-01T00:00:00Z", "note": "hi" });
        assert!(extract_windows(&payload).is_empty());
    }

    // --- 1.12.4 parsing ---------------------------------------------------

    #[test]
    fn normal_reply_becomes_two_model_pools() {
        let reply = reply(NORMAL);
        let windows = windows_from_reply(&reply);

        assert_eq!(windows.len(), 2);
        assert_eq!(windows[0].id.as_deref(), Some("cursorModels"));
        assert_eq!(windows[0].scope.as_deref(), Some("Cursor Models"));
        assert_eq!(windows[0].label, "Monthly");
        assert_eq!(windows[1].id.as_deref(), Some("otherModels"));
        assert_eq!(windows[1].scope.as_deref(), Some("Other Models"));
        assert!((windows[0].percent_used.unwrap() - 0.0267).abs() < 1e-10);
        assert!((windows[1].percent_used.unwrap() - 0.05).abs() < 1e-10);
        assert!(windows
            .iter()
            .all(|w| w.resets_at.as_deref() == Some("2026-10-01T00:00:00Z")));
        assert!(windows.iter().all(|w| w.window_seconds.is_none()));
        assert!(windows.iter().all(|w| !w.is_exhausted));
        assert_eq!(
            plan_name(reply.membership_type.as_deref()).as_deref(),
            Some("Pro+")
        );
        assert_eq!(remaining(&reply), Some(8.0));
    }

    #[test]
    fn on_demand_is_a_third_window_only_when_enabled() {
        let windows = windows_from_reply(&reply(ONDEMAND));
        assert_eq!(
            windows.iter().map(|w| w.id.as_deref()).collect::<Vec<_>>(),
            vec![Some("cursorModels"), Some("otherModels"), Some("onDemand")]
        );
        assert_eq!(windows[2].label, "Spend");
        assert!((windows[2].percent_used.unwrap() - 35.0).abs() < 1e-9);
        assert_eq!(windows[2].detail.as_deref(), Some("$3.50 / $10.00"));
        assert!(!windows[2].is_exhausted);

        // `normal` has onDemand disabled: no row at all, not a 0% row.
        let normal = windows_from_reply(&reply(NORMAL));
        assert!(!normal.iter().any(|w| w.id.as_deref() == Some("onDemand")));
    }

    #[test]
    fn no_pools_fall_back_to_the_plan_money() {
        let windows = windows_from_reply(&reply(FALLBACK_NO_POOLS));
        assert_eq!(windows.len(), 1);
        assert_eq!(windows[0].id.as_deref(), Some("plan"));
        assert_eq!(windows[0].label, "Monthly");
        assert!((windows[0].percent_used.unwrap() - 25.0).abs() < 1e-9);
        assert_eq!(windows[0].detail.as_deref(), Some("$0.05 / $0.20"));
    }

    #[test]
    fn team_account_substitutes_the_team_pools() {
        let reply = reply(TEAM);
        let windows = windows_from_reply(&reply);
        assert_eq!(
            windows.iter().map(|w| w.id.as_deref()).collect::<Vec<_>>(),
            vec![Some("cursorModels"), Some("otherModels"), Some("onDemand")]
        );
        assert!((windows[0].percent_used.unwrap() - 15.0).abs() < 1e-9);
        assert!((windows[1].percent_used.unwrap() - 3.0).abs() < 1e-9);
        assert!((windows[2].percent_used.unwrap() - 20.0).abs() < 1e-9);
        assert_eq!(remaining(&reply), Some(70.0));
    }

    #[test]
    fn missing_fields_draw_nothing() {
        let reply = reply(MISSING_FIELDS);
        assert!(windows_from_reply(&reply).is_empty());
        assert_eq!(plan_name(reply.membership_type.as_deref()), None);
        assert_eq!(remaining(&reply), None);
    }

    #[test]
    fn a_pool_at_or_over_one_hundred_percent_is_exhausted() {
        let windows = windows_from_reply(&reply(EXHAUSTED));
        assert_eq!(windows[0].percent_used, Some(100.0));
        assert!(windows[0].is_exhausted);
        assert_eq!(windows[1].percent_used, Some(0.0));
        assert!(!windows[1].is_exhausted);
    }

    #[test]
    fn plan_names_are_tidied_not_blanked() {
        assert_eq!(plan_name(Some("pro_plus")).as_deref(), Some("Pro+"));
        assert_eq!(plan_name(Some("free")).as_deref(), Some("Free"));
        assert_eq!(
            plan_name(Some("mystery_tier")).as_deref(),
            Some("Mystery Tier")
        );
        assert_eq!(plan_name(Some("")), None);
        assert_eq!(plan_name(None), None);
    }

    // --- Weekly plans -----------------------------------------------------

    #[test]
    fn weekly_plan_replaces_the_monthly_pools() {
        let plan = plan_info(PLAN_INFO_WEEKLY);
        assert!(plan.is_weekly());

        let summary = windows_from_reply(&reply(ONDEMAND));
        let windows = weekly_windows(&plan, &period_usage(PERIOD_USAGE_WEEKLY), summary);

        assert_eq!(
            windows.iter().map(|w| w.id.as_deref()).collect::<Vec<_>>(),
            vec![Some("weekly"), Some("onDemand")]
        );
        let week = &windows[0];
        assert_eq!(week.label, "7d");
        assert!((week.percent_used.unwrap() - 35.51462621885157).abs() < 1e-9);
        assert_eq!(week.resets_at.as_deref(), Some("2026-10-20T05:43:30.907Z"));
        assert_eq!(week.window_seconds, Some(7.0 * 86_400.0));
        assert!(!week.is_exhausted);
    }

    #[test]
    fn monthly_plan_is_not_weekly() {
        assert!(!plan_info(PLAN_INFO_MONTHLY).is_weekly());
    }

    #[test]
    fn a_period_may_come_as_its_number() {
        let weekly: PlanInfoReply =
            serde_json::from_value(json!({"planInfo": {"includedUsagePeriod": 2}})).unwrap();
        assert!(weekly.plan_info.unwrap().is_weekly());

        let other: PlanInfoReply =
            serde_json::from_value(json!({"planInfo": {"includedUsagePeriod": 1}})).unwrap();
        assert!(!other.plan_info.unwrap().is_weekly());
    }

    #[test]
    fn a_week_without_a_figure_draws_no_window() {
        let windows = weekly_windows(
            &plan_info(PLAN_INFO_WEEKLY),
            &period_usage(PERIOD_USAGE_NO_FIGURE),
            windows_from_reply(&reply(NORMAL)),
        );
        assert!(windows.is_empty());
    }

    // --- Enterprise -------------------------------------------------------

    #[test]
    fn enterprise_overall_allowance_is_a_monthly_usd_window() {
        let reply = reply(ENTERPRISE_OVERALL);
        let windows = windows_from_reply(&reply);

        assert_eq!(windows.len(), 1);
        assert_eq!(windows[0].id.as_deref(), Some("plan"));
        assert_eq!(windows[0].label, "Monthly");
        assert_eq!(windows[0].kind, WindowKind::Limit);
        assert!((windows[0].percent_used.unwrap() - 0.1).abs() < 1e-9);
        assert_eq!(windows[0].detail.as_deref(), Some("$0.80 / $800.00"));
        assert_eq!(
            windows[0].resets_at.as_deref(),
            Some("2026-11-01T00:00:00Z")
        );
        assert_eq!(remaining(&reply), Some(799.2));
    }

    #[test]
    fn garbage_reply_fails_to_decode() {
        assert!(serde_json::from_str::<Reply>("not json at all").is_err());
    }
}
