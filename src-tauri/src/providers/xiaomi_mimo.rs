//! Xiaomi's MiMo open platform, read through the console's own endpoints.
//!
//! **A browser session, not a key.** The platform issues API keys for
//! inference, and none of them answer the console's account routes — the plan
//! and the balance are behind the same `api-platform_serviceToken` cookie the
//! web console uses. The original reads that cookie out of the browser
//! (`Auth/BrowserCookies.swift`: the login keychain for a Chromium store, Full
//! Disk Access for Safari); PulseWin reads neither, so the `Cookie` header
//! copied out of a signed-in request is the credential here instead.
//!
//! **The ring is the Coding Plan, not the balance.** The account carries two
//! separate things: a monthly token allowance bought as a plan, and a prepaid
//! cash balance for anything past it. Only the first has a denominator, so only
//! the first is a ring — the balance rides along as money on the card. An
//! account with no plan is not a fault; it is an account that buys tokens by
//! the yuan, and it says so.

use std::sync::Arc;

use serde_json::Value;

use super::{describe_reqwest_error, percent_from_fraction, Ctx, FetchFuture, Provider};
use crate::model::{ProviderUsage, UsageWindow};

const ID: &str = "xiaomi-mimo";
const NAME: &str = "Xiaomi MiMo";

const BASE: &str = "https://platform.xiaomimimo.com/api/v1";
const CONSOLE: &str = "https://platform.xiaomimimo.com/#/console/balance";

/// The two the platform will not answer without.
const REQUIRED: [&str; 2] = ["api-platform_serviceToken", "userId"];
/// Sent when present. The console includes them and the endpoints work without
/// them, so they are carried rather than required.
const OPTIONAL: [&str; 2] = ["api-platform_ph", "api-platform_slh"];

pub struct XiaomiMiMo;

impl Provider for XiaomiMiMo {
    fn id(&self) -> &'static str {
        ID
    }

    fn name(&self) -> &'static str {
        NAME
    }

    /// Whether a pasted cookie is on the machine. No network call.
    fn is_configured(&self) -> bool {
        super::pasted::cookie(ID).is_some()
    }

    fn fetch(&self, ctx: Arc<Ctx>) -> FetchFuture {
        Box::pin(async move { fetch_inner(ctx).await })
    }
}

/// Why a read ended without figures. Kept as a type because the *worst* of
/// several routes is what gets reported, and one blanket sentence would send
/// the reader to the wrong remedy.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Reason {
    /// Nothing to send at all.
    Missing,
    /// Something to send, and not a header the platform would take.
    Invalid,
    /// The session is there and the platform refused it — expired, or signed
    /// out elsewhere. Separate from `Missing`, because one is "set this up" and
    /// the other is "you already did, do it again".
    SessionExpired,
    NoPlan,
    Unreadable,
    RateLimited,
    ServerError,
    /// Nothing came back at all — no network, DNS, TLS, a timeout.
    Unreachable(String),
}

impl Reason {
    fn message(self) -> String {
        match self {
            Reason::Missing => super::pasted::needed(ID, "Xiaomi MiMo session cookie"),
            Reason::Invalid => "the pasted cookie is not one the console takes — it needs \
                                `api-platform_serviceToken` and `userId`, with no quotes or \
                                spaces in either value"
                .to_string(),
            Reason::SessionExpired => {
                "HTTP 401 — the session has expired; copy a fresh cookie from \
                 platform.xiaomimimo.com"
                    .to_string()
            }
            Reason::NoPlan => {
                "the account has no Coding Plan — the console reports no monthly allowance, \
                 which is an answer and not a fault"
                    .to_string()
            }
            Reason::Unreadable => "the reply could not be read".to_string(),
            Reason::RateLimited => "HTTP 429 — rate limited, try again shortly".to_string(),
            Reason::ServerError => "the service returned an error".to_string(),
            Reason::Unreachable(why) => format!("request failed: {why}"),
        }
    }

    /// The most actionable of several, for the case where nothing answered.
    ///
    /// A session that has to be signed in again outranks a timeout: if one
    /// route says the login is refused and another merely timed out, the login
    /// is the thing to tell the reader about.
    fn rank(&self) -> u8 {
        match self {
            Reason::SessionExpired | Reason::Missing | Reason::Invalid => 3,
            Reason::RateLimited | Reason::ServerError => 2,
            Reason::Unreachable(_) => 1,
            Reason::NoPlan | Reason::Unreadable => 0,
        }
    }
}

async fn fetch_inner(ctx: Arc<Ctx>) -> ProviderUsage {
    let Some(pasted) = super::pasted::cookie(ID) else {
        return ProviderUsage::failed(ID, NAME, Reason::Missing.message());
    };
    let header = match normalize(&pasted) {
        Ok(header) => header,
        Err(reason) => return ProviderUsage::failed(ID, NAME, reason.message()),
    };

    // The plan is what the ring is for, so its failure is the call's failure.
    // The balance is a line on the card, so a balance route that does not
    // answer costs that line and nothing else.
    let detail = fetch(&ctx, "tokenPlan/detail", &header).await;
    let usage = fetch(&ctx, "tokenPlan/usage", &header).await;
    let balance = fetch(&ctx, "balance", &header).await;

    // Every route is the same envelope, so one expired session shows up on all
    // three. Reported from whichever answered rather than from a fourth
    // request made only to ask.
    for body in [&detail, &usage, &balance] {
        if let Ok(body) = body {
            if let Some(reason) = refusal(body) {
                return ProviderUsage::failed(ID, NAME, reason.message());
            }
        }
    }

    // Nothing answered. Reported as the worst of the three rather than as one
    // blanket sentence, so a session problem outranks a timeout and the reader
    // is sent to the right remedy. `try?` here made every status the reader
    // bothers to classify unreachable — an HTTP 401, a 429 and a 500 all came
    // out as "the reply could not be read".
    if detail.is_err() && usage.is_err() && balance.is_err() {
        let worst = [&detail, &usage, &balance]
            .into_iter()
            .filter_map(|route| route.as_ref().err())
            .max_by_key(|reason| reason.rank())
            .cloned()
            .unwrap_or(Reason::Unreadable);
        return ProviderUsage::failed(ID, NAME, worst.message());
    }

    let detail = detail.as_deref().ok();
    let usage = usage.as_deref().ok();

    let plan = match plan(detail, usage) {
        Plan::Missing => return ProviderUsage::failed(ID, NAME, Reason::NoPlan.message()),
        Plan::Unreadable => return ProviderUsage::failed(ID, NAME, Reason::Unreadable.message()),
        Plan::Found {
            used,
            limit,
            period_end,
            code,
        } => (used, limit, period_end, code),
    };

    let mut windows = vec![
        UsageWindow::new(
            "Monthly",
            Some(percent_from_fraction(plan.0 as f64 / plan.1 as f64)),
        )
        .with_reset(plan.2),
    ];

    // The prepaid balance as a line on the card: money, never a ring, because
    // there is no allowance to compare it against.
    if let Some((amount, currency)) =
        balance.as_deref().ok().and_then(|body| money(body).ok().flatten())
    {
        windows.push(super::balance_window(
            "Balance",
            format!("{amount:.2} {currency}"),
        ));
    }

    ProviderUsage::ok(ID, NAME, windows).with_plan(plan.3)
}

/// One route's body, or why there is none.
async fn fetch(ctx: &Ctx, path: &str, cookie: &str) -> Result<String, Reason> {
    let response = ctx
        .gateway_client
        .get(format!("{BASE}/{path}"))
        .header("Cookie", cookie)
        .header("Accept", "application/json, text/plain, */*")
        .header("Origin", "https://platform.xiaomimimo.com")
        .header("Referer", CONSOLE)
        .send()
        .await
        .map_err(|e| Reason::Unreachable(describe_reqwest_error(&e)))?;

    // An expired session is answered by redirecting the API call at the login
    // flow, so a 3xx here is a sign-in problem rather than a moved endpoint.
    let status = response.status();
    let body = response
        .text()
        .await
        .map_err(|e| Reason::Unreachable(format!("cannot read body: {e}")))?;

    match status.as_u16() {
        200 => Ok(body),
        300..=399 | 401 | 403 => Err(Reason::SessionExpired),
        429 => Err(Reason::RateLimited),
        500..=599 => Err(Reason::ServerError),
        _ => Err(Reason::Unreadable),
    }
}

/// The envelope's own verdict. The platform answers a refused session with
/// **HTTP 200** and a code in the body.
fn refusal(body: &str) -> Option<Reason> {
    let code = serde_json::from_str::<Value>(body)
        .ok()?
        .get("code")?
        .as_i64()?;
    match code {
        0 => None,
        401 | 403 => Some(Reason::SessionExpired),
        _ => None,
    }
}

enum Plan {
    /// The account has no Coding Plan: a real state, not a failed read.
    Missing,
    Unreadable,
    Found {
        used: i64,
        limit: i64,
        period_end: Option<String>,
        code: Option<String>,
    },
}

/// The plan's month from the two routes the console draws it from.
///
/// **The envelope first.** The platform answers over HTTP 200 whatever
/// happened, so a body whose `code` is not zero is a failure wearing a
/// success's clothes — read without this, a `code` 500 carrying an empty
/// `items` came out as "no Coding Plan on this account", a fault reported as a
/// subscription.
fn plan(detail: Option<&str>, usage: Option<&str>) -> Plan {
    let Ok(usage) = envelope(usage) else {
        return Plan::Unreadable;
    };
    let detail = envelope(detail).ok();

    // `monthUsage.items` is a list because the console draws a row per bucket;
    // the plan's own allowance is the first. An empty list is an account with
    // no plan, which is why this is not a zero — a ring at 0% would say "you
    // have a full month left".
    let item = usage
        .pointer("/data/monthUsage/items/0")
        .filter(|item| item.get("limit").and_then(Value::as_i64).unwrap_or(0) > 0);
    let Some(item) = item else {
        return Plan::Missing;
    };
    let used = item.get("used").and_then(Value::as_i64).unwrap_or(0);
    let limit = item.get("limit").and_then(Value::as_i64).unwrap_or(0);

    let body = detail.as_ref().and_then(|detail| detail.get("data"));
    // An expired plan reports last month's numbers until it is renewed. Those
    // are not a current allowance, so they are not drawn.
    if body
        .and_then(|body| body.get("expired"))
        .and_then(Value::as_bool)
        == Some(true)
    {
        return Plan::Missing;
    }

    Plan::Found {
        used,
        limit,
        period_end: body
            .and_then(|body| body.get("currentPeriodEnd"))
            .and_then(Value::as_str)
            .and_then(console_date),
        code: body
            .and_then(|body| body.get("planCode"))
            .and_then(Value::as_str)
            .map(str::to_string),
    }
}

/// A route's body when its own envelope says it succeeded.
fn envelope(body: Option<&str>) -> Result<Value, Reason> {
    let body = body.ok_or(Reason::Unreadable)?;
    let json: Value = serde_json::from_str(body).map_err(|_| Reason::Unreadable)?;
    if json.get("code").and_then(Value::as_i64) != Some(0) {
        return Err(Reason::Unreadable);
    }
    Ok(json)
}

/// The prepaid balance, and in what currency. `Ok(None)` is an account whose
/// reply carries no balance; `Err` is one that carries something unreadable.
fn money(body: &str) -> Result<Option<(f64, String)>, Reason> {
    let json: Value = serde_json::from_str(body).map_err(|_| Reason::Unreadable)?;
    if json.get("code").and_then(Value::as_i64) != Some(0) {
        return Ok(None);
    }
    let Some(data) = json.get("data") else {
        return Ok(None);
    };
    let Some(raw) = data.get("balance") else {
        return Ok(None);
    };
    let amount = raw
        .as_str()
        .and_then(|text| text.trim().parse::<f64>().ok())
        .filter(|amount| amount.is_finite())
        .ok_or(Reason::Unreadable)?;
    let currency = data
        .get("currency")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|currency| !currency.is_empty())
        .ok_or(Reason::Unreadable)?;
    Ok(Some((amount, currency.to_string())))
}

/// The console's own format, in UTC. Not ISO-8601, so an RFC3339 parser
/// returns nothing on it and the card silently loses its reset.
fn console_date(text: &str) -> Option<String> {
    let naive = chrono::NaiveDateTime::parse_from_str(text.trim(), "%Y-%m-%d %H:%M:%S").ok()?;
    chrono::DateTime::from_timestamp(naive.and_utc().timestamp(), 0)
        .map(|at| at.to_rfc3339_opts(chrono::SecondsFormat::Secs, true))
}

/// A pasted `Cookie:` header reduced to the names the console sends, or the
/// reason there is none.
///
/// **An allow list**, because this session has published names: nothing else
/// the host set is forwarded. A header assembled from an arbitrary string is a
/// header injection if a value carries a newline, so the value is checked
/// rather than trusted — and a repeated name is kept once, as in any cookie
/// header, because a browser store routinely holds both a host-only and a
/// domain row for one session.
fn normalize(input: &str) -> Result<String, Reason> {
    if input.chars().any(|c| c.is_control() || !c.is_ascii()) {
        return Err(Reason::Invalid);
    }
    let trimmed = input.trim();
    let header = match trimmed.get(.."cookie:".len()) {
        Some(head) if head.eq_ignore_ascii_case("cookie:") => trimmed["cookie:".len()..].trim(),
        _ => trimmed,
    };
    if header.is_empty() {
        return Err(Reason::Missing);
    }
    if header.len() > 32_768 {
        return Err(Reason::Invalid);
    }

    let mut kept: Vec<String> = Vec::new();
    let mut seen: Vec<&str> = Vec::new();
    for pair in header.split(';') {
        let Some((name, value)) = pair.trim().split_once('=') else {
            continue;
        };
        if !REQUIRED.contains(&name) && !OPTIONAL.contains(&name) {
            continue;
        }
        if value.is_empty() || value.contains('"') || value.contains('\\') || value.contains(' ') {
            return Err(Reason::Invalid);
        }
        if seen.contains(&name) {
            continue;
        }
        seen.push(name);
        kept.push(format!("{name}={value}"));
    }

    if !REQUIRED.iter().all(|name| seen.contains(name)) {
        return Err(Reason::Invalid);
    }
    Ok(kept.join("; "))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn keeps_the_session_and_the_account_markers_and_nothing_else() {
        let header = "api-platform_serviceToken=tok; userId=42; _ga=GA1.2; theme=dark";
        assert_eq!(
            normalize(header).unwrap(),
            "api-platform_serviceToken=tok; userId=42"
        );
        // The optional two ride along when present.
        assert_eq!(
            normalize("userId=42; api-platform_serviceToken=tok; api-platform_ph=ph").unwrap(),
            "userId=42; api-platform_serviceToken=tok; api-platform_ph=ph"
        );
    }

    #[test]
    fn without_both_required_names_there_is_no_session() {
        assert_eq!(normalize("userId=42"), Err(Reason::Invalid));
        assert_eq!(normalize("   "), Err(Reason::Missing));
        assert_eq!(normalize("api-platform_serviceToken=a b; userId=1"), Err(Reason::Invalid));
        assert_eq!(normalize("api-platform_serviceToken=\"q\"; userId=1"), Err(Reason::Invalid));
    }

    #[test]
    fn a_repeated_name_is_kept_once() {
        assert_eq!(
            normalize("userId=42; userId=43; api-platform_serviceToken=tok").unwrap(),
            "userId=42; api-platform_serviceToken=tok"
        );
    }

    #[test]
    fn the_plan_is_the_first_month_bucket_with_a_size() {
        let usage = json!({
            "code": 0,
            "data": { "monthUsage": { "items": [ { "name": "Coding Plan", "used": 250, "limit": 1000 } ] } }
        })
        .to_string();
        let detail = json!({
            "code": 0,
            "data": { "planCode": "coding-pro", "currentPeriodEnd": "2026-11-01 00:00:00", "expired": false }
        })
        .to_string();

        match plan(Some(&detail), Some(&usage)) {
            Plan::Found { used, limit, period_end, code } => {
                assert_eq!((used, limit), (250, 1000));
                assert_eq!(period_end.as_deref(), Some("2026-11-01T00:00:00Z"));
                assert_eq!(code.as_deref(), Some("coding-pro"));
            }
            _ => panic!("expected a plan"),
        }
    }

    #[test]
    fn an_account_with_no_plan_says_so_rather_than_drawing_zero() {
        let empty = json!({ "code": 0, "data": { "monthUsage": { "items": [] } } }).to_string();
        assert!(matches!(plan(None, Some(&empty)), Plan::Missing));

        let zero = json!({
            "code": 0,
            "data": { "monthUsage": { "items": [ { "used": 0, "limit": 0 } ] } }
        })
        .to_string();
        assert!(matches!(plan(None, Some(&zero)), Plan::Missing));

        // A failure wearing a success's clothes is not a subscription.
        let failed = json!({ "code": 500, "data": { "monthUsage": { "items": [] } } }).to_string();
        assert!(matches!(plan(None, Some(&failed)), Plan::Unreadable));
    }

    #[test]
    fn an_expired_plans_numbers_are_not_a_current_allowance() {
        let usage = json!({
            "code": 0,
            "data": { "monthUsage": { "items": [ { "used": 10, "limit": 100 } ] } }
        })
        .to_string();
        let detail = json!({ "code": 0, "data": { "expired": true } }).to_string();
        assert!(matches!(plan(Some(&detail), Some(&usage)), Plan::Missing));
    }

    #[test]
    fn a_refused_session_in_a_good_http_status_is_read_from_the_body() {
        assert_eq!(
            refusal(&json!({ "code": 401 }).to_string()),
            Some(Reason::SessionExpired)
        );
        assert_eq!(refusal(&json!({ "code": 0 }).to_string()), None);
        assert_eq!(refusal("not json"), None);
    }

    #[test]
    fn the_balance_is_money_in_the_currency_the_console_names() {
        let body = json!({ "code": 0, "data": { "balance": "12.5", "currency": "CNY" } }).to_string();
        assert_eq!(money(&body).unwrap(), Some((12.5, "CNY".to_string())));

        let unreadable = json!({ "code": 0, "data": { "balance": "many", "currency": "CNY" } })
            .to_string();
        assert_eq!(money(&unreadable), Err(Reason::Unreadable));

        let none = json!({ "code": 0, "data": {} }).to_string();
        assert_eq!(money(&none).unwrap(), None);
    }
}
