//! StepFun's Step Plan, read the way its own console reads it.
//!
//! **The credential is a pasted cookie.** The Step API key buys inference; the
//! plan's allowance is only on the console, which asks
//! `POST /api/step.openapi.devcenter.Dashboard/QueryStepPlanRateLimit` with the
//! signed-in cookies. So that session is the credential, and the original takes
//! it out of the browser's own store — which on macOS means Full Disk Access or
//! a keychain prompt. PulseWin cannot do either, so the `Cookie` header copied
//! out of a signed-in request is the credential here. The documented
//! `GET /v1/accounts` answers a key, but with the account's prepaid balance,
//! which is a separate system from the plan.
//!
//! **Two sites, two accounts.** `platform.stepfun.com` and
//! `platform.stepfun.ai` are separate sign-ins, and a session for one is never
//! sent to the other: which one is read is a choice, not a fallback —
//! `PULSEWIN_STEP_FUN_SITE` (or the `site` field of the same settings file),
//! `international` for `platform.stepfun.ai`, the mainland site otherwise,
//! which is the original's own default.
//!
//! **An allow list**: the session has published names, so nothing else the host
//! set is forwarded. `Oasis-Webid` has to agree with the device the token was
//! issued to, and `INGRESSCOOKIE` pins the load balancer.
//!
//! **Two plans, two shapes.** Since 2026-06-18 StepFun sells a Token Plan: a
//! monthly pool of Credits plus 30-day top-up packs, each a bucket with its own
//! size, remainder and end date. The Coding Plan before it — still renewed for
//! anybody who kept auto-renew on — meters a five-hour and a weekly window as a
//! remaining fraction and a reset time. One reply carries whichever the account
//! has, the other's fields zeroed. The bucket end dates are not carried: this
//! port's window has nowhere to put an expiry, and a reset the service never
//! stated is worse than a line that is missing.

use std::sync::Arc;

use chrono::{DateTime, Utc};
use serde_json::Value;

use super::{describe_reqwest_error, percent_from_fraction, Ctx, FetchFuture, Provider};
use crate::credentials;
use crate::model::{ProviderUsage, UsageWindow};

const ID: &str = "step-fun";
const NAME: &str = "StepFun";

/// The session itself. The console answers nothing without it.
const REQUIRED_COOKIES: [&str; 1] = ["Oasis-Token"];
/// Sent when present.
const OPTIONAL_COOKIES: [&str; 2] = ["Oasis-Webid", "INGRESSCOOKIE"];
const MAXIMUM_HEADER: usize = 32_768;

/// What the console's own request carries: its app id and its platform.
const APP_ID: &str = "10300";
const PLATFORM: &str = "web";

pub struct StepFun;

impl Provider for StepFun {
    fn id(&self) -> &'static str {
        ID
    }

    fn name(&self) -> &'static str {
        NAME
    }

    /// Whether a pasted session carrying the token is on the machine. No
    /// network call.
    fn is_configured(&self) -> bool {
        super::pasted::cookie(ID).is_some_and(|pasted| normalize(&pasted).is_ok())
    }

    fn fetch(&self, ctx: Arc<Ctx>) -> FetchFuture {
        Box::pin(async move { fetch_inner(ctx).await })
    }
}

// ---------------------------------------------------------------------------
// The two sites
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Site {
    China,
    International,
}

impl Site {
    fn host(&self) -> &'static str {
        match self {
            Site::China => "platform.stepfun.com",
            Site::International => "platform.stepfun.ai",
        }
    }

    fn origin(&self) -> String {
        format!("https://{}", self.host())
    }

    fn endpoint(&self, method: &str) -> String {
        format!(
            "{}/api/step.openapi.devcenter.Dashboard/{method}",
            self.origin()
        )
    }

    /// The page that makes this request, so the `Referer` is true.
    fn usage_page(&self) -> String {
        format!("{}/plan-usage", self.origin())
    }

    fn named(typed: &str) -> Self {
        let typed = typed.trim().to_lowercase();
        if typed.contains("international") || typed.contains("stepfun.ai") || typed.contains("global")
        {
            Site::International
        } else {
            Site::China
        }
    }
}

/// Which site is read, from the two places this port has. The mainland site is
/// the original's own default.
fn site() -> Site {
    if let Some(typed) = credentials::env_override(ID, "site") {
        return Site::named(&typed);
    }
    if let Some(path) = super::settings_path(ID) {
        if let Some(json) = credentials::read_json(&path) {
            if let Some(typed) = credentials::dig_str(&json, "site") {
                return Site::named(&typed);
            }
        }
    }
    Site::China
}

// ---------------------------------------------------------------------------
// The cookies
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CookieProblem {
    /// Nothing was pasted, or nothing was left once the `Cookie:` name was
    /// taken off.
    Missing,
    /// A control character, a value that cannot be forwarded, or no token.
    Invalid,
}

/// A `Cookie:` header reduced to the names above.
///
/// Tolerates a pasted `Cookie:` prefix and refuses control characters: a header
/// built from an arbitrary string is a header injection if a value carries a
/// newline.
fn normalize(input: &str) -> Result<String, CookieProblem> {
    if input.chars().any(|c| !c.is_ascii() || c.is_ascii_control()) {
        return Err(CookieProblem::Invalid);
    }

    let mut header = input.trim();
    if header.len() >= "cookie:".len() && header[.."cookie:".len()].eq_ignore_ascii_case("cookie:") {
        header = header["cookie:".len()..].trim();
    }
    if header.is_empty() {
        return Err(CookieProblem::Missing);
    }
    if header.len() > MAXIMUM_HEADER {
        return Err(CookieProblem::Invalid);
    }

    let mut kept: Vec<String> = Vec::new();
    let mut seen: Vec<&str> = Vec::new();

    for pair in header.split(';') {
        let Some((name, value)) = pair.split_once('=') else {
            continue;
        };
        let name = name.trim();
        let value = value.trim();
        if !REQUIRED_COOKIES.contains(&name) && !OPTIONAL_COOKIES.contains(&name) {
            continue;
        }
        // A value this cannot pass on unaltered is refused rather than dropped:
        // the token is not a cookie to lose quietly.
        if value.is_empty() || value.contains('"') || value.contains('\\') || value.contains(' ') {
            return Err(CookieProblem::Invalid);
        }
        // The first of a host-only and a domain row wins, as in any cookie
        // header.
        if seen.contains(&name) {
            continue;
        }
        seen.push(name);
        kept.push(format!("{name}={value}"));
    }

    if !REQUIRED_COOKIES.iter().all(|name| seen.contains(name)) {
        return Err(CookieProblem::Invalid);
    }
    Ok(kept.join("; "))
}

fn cookie_problem(problem: CookieProblem) -> String {
    match problem {
        CookieProblem::Missing => super::pasted::needed(ID, "StepFun session cookie"),
        CookieProblem::Invalid => format!(
            "the pasted cookie for {ID} carries no StepFun session — paste one `Cookie` header \
             with `Oasis-Token` from a signed-in {} request, on one line",
            site().host()
        ),
    }
}

/// The device id the console sends as `oasis-webid`, which must match the one
/// the token was issued to.
///
/// The `Oasis-Webid` cookie when the session carries it. Otherwise the token's
/// own `device_id` claim: the token is a JWT, or an `access...refresh` pair
/// whose refresh half carries the claim. Read, not verified — it only has to be
/// repeated back to the server that signed it.
fn web_id(header: &str) -> Option<String> {
    let mut values: Vec<(&str, &str)> = Vec::new();
    for pair in header.split(';') {
        if let Some((name, value)) = pair.split_once('=') {
            values.push((name.trim(), value.trim()));
        }
    }

    let value = |wanted: &str| values.iter().find(|(name, _)| *name == wanted).map(|(_, v)| *v);
    if let Some(webid) = value("Oasis-Webid").filter(|webid| !webid.is_empty()) {
        return Some(webid.to_string());
    }

    let token = value("Oasis-Token")?;
    // The refresh half is the one that carries the claim, so the halves are
    // tried from the end.
    let halves: Vec<&str> = token.split("...").collect();
    for half in halves.into_iter().rev() {
        if let Some(id) = device_id(half) {
            return Some(id);
        }
    }
    None
}

/// The `device_id` claim out of one half of a token, when it is a JWT that has
/// one.
fn device_id(jwt: &str) -> Option<String> {
    let parts: Vec<&str> = jwt.split('.').collect();
    if parts.len() < 2 {
        return None;
    }

    let claims = base64(parts[1])?;
    let claims: Value = serde_json::from_slice(&claims).ok()?;
    let id = claims.get("device_id")?.as_str()?;
    if id.is_empty() {
        None
    } else {
        Some(id.to_string())
    }
}

/// Base64 as a JWT writes it — URL-safe, and usually unpadded — or the padded
/// form. This port has no base64 dependency, and a decode is a dozen lines.
fn base64(text: &str) -> Option<Vec<u8>> {
    let mut out: Vec<u8> = Vec::new();
    let mut buffer: u32 = 0;
    let mut bits: u32 = 0;

    for byte in text.bytes() {
        let value = match byte {
            b'A'..=b'Z' => byte - b'A',
            b'a'..=b'z' => byte - b'a' + 26,
            b'0'..=b'9' => byte - b'0' + 52,
            b'+' | b'-' => 62,
            b'/' | b'_' => 63,
            // The padding, and anything that is not the alphabet.
            _ => continue,
        } as u32;
        buffer = (buffer << 6) | value;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((buffer >> bits) as u8);
        }
    }

    if out.is_empty() {
        None
    } else {
        Some(out)
    }
}

// ---------------------------------------------------------------------------
// The console's route
// ---------------------------------------------------------------------------

async fn fetch_inner(ctx: Arc<Ctx>) -> ProviderUsage {
    let Some(pasted) = super::pasted::cookie(ID) else {
        return ProviderUsage::failed(ID, NAME, super::pasted::needed(ID, "StepFun session cookie"));
    };
    let header = match normalize(&pasted) {
        Ok(header) => header,
        Err(problem) => return ProviderUsage::failed(ID, NAME, cookie_problem(problem)),
    };

    let site = site();
    let body = match post(&ctx, "QueryStepPlanRateLimit", &header, site).await {
        Ok(body) => body,
        Err(reason) => return ProviderUsage::failed(ID, NAME, reason),
    };

    let mut snapshot = match parse(&body) {
        Ok(snapshot) => snapshot,
        Err(reason) => return ProviderUsage::failed(ID, NAME, reason),
    };

    // The plan's name is a second request and a nicety. Its failure costs the
    // name and nothing else.
    if let Ok(status) = post(&ctx, "GetStepPlanStatus", &header, site).await {
        snapshot.plan_name = plan_name(&status);
    }

    let windows = windows(&snapshot, Utc::now());
    if windows.is_empty() {
        return ProviderUsage::failed(ID, NAME, "No Step Plan on this StepFun account.");
    }

    ProviderUsage::ok(ID, NAME, windows).with_plan(snapshot.plan_name)
}

async fn post(ctx: &Ctx, method: &str, header: &str, site: Site) -> Result<String, String> {
    let mut request = ctx
        .gateway_client
        .post(site.endpoint(method))
        .header("Cookie", header)
        .header("Content-Type", "application/json")
        .header("Accept", "application/json")
        .header("Origin", site.origin())
        .header("Referer", site.usage_page())
        // A token presented from another device id is refused as stolen.
        .header("oasis-appid", APP_ID)
        .header("oasis-platform", PLATFORM)
        .header(
            "User-Agent",
            "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 \
             (KHTML, like Gecko) Chrome/143.0.0.0 Safari/537.36",
        )
        .body("{}");

    if let Some(webid) = web_id(header) {
        request = request.header("oasis-webid", webid);
    }

    let response = request
        .send()
        .await
        .map_err(|e| format!("request failed: {}", describe_reqwest_error(&e)))?;

    let status = response.status();
    let body = response
        .text()
        .await
        .map_err(|e| format!("cannot read body: {e}"))?;

    if status.is_success() {
        return Ok(body);
    }
    Err(match status.as_u16() {
        // Signed out: `{"code":"unauthenticated","message":"auth failed: …"}`
        // with a 401, measured.
        401 | 403 => "StepFun's saved session expired. Sign in again in your browser.".to_string(),
        429 => super::http_failure(status, &[(429, " — rate limited, try again shortly")]),
        500..=599 => "the service reported an error".to_string(),
        _ => "the reply could not be read".to_string(),
    })
}

// ---------------------------------------------------------------------------
// The reply
// ---------------------------------------------------------------------------

/// One of the Token Plan's credit buckets: the month's pool, or a top-up pack.
/// Sizes in Credits (1M Credit = ¥1).
#[derive(Debug)]
struct Bucket {
    total: f64,
    remaining: f64,
    /// When it is refilled — a quarterly or yearly plan's next monthly issue.
    next_reset_at: Option<DateTime<Utc>>,
}

/// A Coding Plan window: what is left, as StepFun states it, and when it
/// resets.
#[derive(Debug)]
struct Window {
    remaining_fraction: f64,
    resets_at: DateTime<Utc>,
}

#[derive(Debug)]
enum Plan {
    /// The Token Plan. Buckets when the reply lists them; otherwise only the
    /// remaining fractions it states, which cannot be added together because
    /// their sizes are not given.
    Credits {
        buckets: Vec<Bucket>,
        subscription_left: Option<f64>,
        top_up_left: Option<f64>,
    },
    /// The Coding Plan's two windows. Either may be absent.
    Windows {
        five_hour: Option<Window>,
        weekly: Option<Window>,
    },
}

/// What one read of the console returns.
#[derive(Debug)]
struct Snapshot {
    plan: Plan,
    /// "Plus", "Mini" — the subscription's own name. None when the second
    /// request failed; the figures do not depend on it.
    plan_name: Option<String>,
}

fn parse(body: &str) -> Result<Snapshot, String> {
    let reply: Value = serde_json::from_str(body).map_err(|_| unreadable())?;
    if !reply.is_object() {
        return Err(unreadable());
    }

    if number(reply.get("status")) != Some(1.0) {
        // A refusal inside a 200. The console words an auth failure as such;
        // anything else is a reply this cannot use.
        let said = ["desc", "message", "code"]
            .iter()
            .filter_map(|key| reply.get(*key).and_then(Value::as_str))
            .collect::<Vec<&str>>()
            .join(" ")
            .to_lowercase();
        return Err(if said.contains("auth") || said.contains("token") {
            "StepFun's saved session expired. Sign in again in your browser.".to_string()
        } else {
            unreadable()
        });
    }

    let five_hour = window(
        reply.get("five_hour_usage_left_rate"),
        reply.get("five_hour_usage_reset_time"),
    );
    let weekly = window(
        reply.get("weekly_usage_left_rate"),
        reply.get("weekly_usage_reset_time"),
    );

    // **Classified by what the reply carries, not by `plan_family`.** A live
    // window — a reset stated — is the Coding Plan; a Token Plan sends its
    // windows as zero with a reset of "0", which is "no window", not "spent".
    if five_hour.is_some() || weekly.is_some() {
        return Ok(Snapshot {
            plan: Plan::Windows { five_hour, weekly },
            plan_name: None,
        });
    }

    let credit = reply.get("plan_credit_rate_limit").cloned().unwrap_or(Value::Null);
    let buckets: Vec<Bucket> = credit
        .get("credit_buckets")
        .and_then(Value::as_array)
        .map(|entries| entries.iter().filter_map(bucket).collect())
        .unwrap_or_default();

    let subscription_left = fraction(credit.get("subscription_credit_left_rate"));
    let top_up_left = fraction(credit.get("topup_credit_left_rate"));

    if buckets.is_empty() && subscription_left.is_none() {
        return Err("No Step Plan on this StepFun account.".to_string());
    }

    Ok(Snapshot {
        plan: Plan::Credits {
            buckets,
            subscription_left,
            top_up_left,
        },
        plan_name: None,
    })
}

/// One credit bucket, when the reply states a size and a remainder for it.
fn bucket(entry: &Value) -> Option<Bucket> {
    let total = number(entry.get("credit_total")).filter(|total| *total > 0.0)?;
    let residual = number(entry.get("credit_residual")).filter(|residual| *residual >= 0.0)?;
    Some(Bucket {
        total,
        remaining: residual.min(total),
        next_reset_at: stamp(entry.get("next_reset_at")),
    })
}

/// A window only when it states a reset: StepFun zeroes both fields for a
/// window the plan does not have.
fn window(left: Option<&Value>, reset: Option<&Value>) -> Option<Window> {
    let resets_at = stamp(reset)?;
    let remaining = number(left).filter(|remaining| remaining.is_finite())?;
    Some(Window {
        remaining_fraction: remaining.clamp(0.0, 1.0),
        resets_at,
    })
}

/// A stated fraction, 0…1. StepFun sends zero for "not on this plan" as well as
/// for "none left", so a zero on its own is not taken as either.
fn fraction(value: Option<&Value>) -> Option<f64> {
    number(value)
        .filter(|fraction| fraction.is_finite() && *fraction > 0.0)
        .map(|fraction| fraction.min(1.0))
}

/// Numbers arrive as JSON numbers or as decimal strings — the bucket sizes are
/// strings, the rates are not.
fn number(value: Option<&Value>) -> Option<f64> {
    super::dig_number(value).filter(|number| number.is_finite())
}

/// A Unix stamp in seconds or milliseconds. **Zero is no date**: StepFun writes
/// "0" for every time it does not have.
fn stamp(value: Option<&Value>) -> Option<DateTime<Utc>> {
    let raw = number(value).filter(|raw| *raw > 0.0)?;
    let seconds = if raw > 10_000_000_000.0 {
        raw / 1_000.0
    } else {
        raw
    };
    DateTime::from_timestamp(seconds as i64, 0)
}

/// `subscription.name` from `GetStepPlanStatus`, when the reply succeeded.
fn plan_name(body: &str) -> Option<String> {
    let reply: Value = serde_json::from_str(body).ok()?;
    if number(reply.get("status")) != Some(1.0) {
        return None;
    }
    let name = reply.get("subscription")?.get("name")?.as_str()?;
    let trimmed = name.trim();
    if trimmed.is_empty() {
        return None;
    }
    Some(trimmed.chars().take(40).collect())
}

fn unreadable() -> String {
    "the reply could not be read".to_string()
}

// ---------------------------------------------------------------------------
// The rings
// ---------------------------------------------------------------------------

fn windows(snapshot: &Snapshot, now: DateTime<Utc>) -> Vec<UsageWindow> {
    match &snapshot.plan {
        // The Coding Plan's two windows are the plan's stated lengths, so their
        // names are the lengths themselves.
        Plan::Windows { five_hour, weekly } => [
            five_hour
                .as_ref()
                .map(|window| coding_window(window, "5h")),
            weekly.as_ref().map(|window| coding_window(window, "7d")),
        ]
        .into_iter()
        .flatten()
        .collect(),

        Plan::Credits {
            buckets,
            subscription_left,
            top_up_left,
        } => {
            if !buckets.is_empty() {
                // **One ring for the month's pool and any packs**, as Qoder's
                // plan-plus-packs total is: they are spent from one balance,
                // soonest-lapsing first, so what is left is their sum.
                let total: f64 = buckets.iter().map(|bucket| bucket.total).sum();
                let remaining: f64 = buckets.iter().map(|bucket| bucket.remaining).sum();

                // A refill the reply states and that is still ahead. A monthly
                // plan states none: its pool simply ends, which is an expiry
                // and not a reset.
                let reset = buckets
                    .iter()
                    .filter_map(|bucket| bucket.next_reset_at)
                    .filter(|at| *at > now)
                    .min();

                return vec![UsageWindow::new(
                    "Credits",
                    Some(percent_from_fraction((total - remaining) / total)),
                )
                .with_reset(
                    reset.map(|at| at.to_rfc3339_opts(chrono::SecondsFormat::Secs, true)),
                )];
            }

            // No sizes, only fractions. The subscription's is the plan; a
            // pack's fraction of an unstated size cannot be added to it.
            let Some(left) = subscription_left.or(*top_up_left) else {
                return Vec::new();
            };
            vec![UsageWindow::new(
                "Credits",
                Some(percent_from_fraction(1.0 - left)),
            )]
        }
    }
}

/// A Coding Plan window: the fraction left, turned into the share used, with
/// the reset the service states. The five hours and the week are the plan's own
/// lengths, so the row is named for its length.
fn coding_window(window: &Window, label: &str) -> UsageWindow {
    UsageWindow::new(
        label,
        Some(percent_from_fraction(1.0 - window.remaining_fraction)),
    )
    .with_reset(Some(
        window
            .resets_at
            .to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Between the fixtures' activation and their expiry.
    fn now() -> DateTime<Utc> {
        DateTime::from_timestamp(1_790_236_800, 0).unwrap()
    }

    /// A real reply from a Token Plan (Plus), captured 2026-09-24.
    fn token_plan() -> String {
        r#"{"status":1, "desc":"", "five_hour_usage_left_rate":0, "five_hour_usage_reset_time":"0", "weekly_usage_left_rate":0, "weekly_usage_reset_time":"0", "plan_family":2, "plan_credit_rate_limit":{"subscription_credit_left_rate":0.9999462, "subscription_credit_reset_time":"0", "topup_credit_left_rate":0, "credit_buckets":[{"type":1, "credit_total":"1600000000", "credit_residual":"1599913834", "expire_at":"1791187256", "next_reset_at":"0"}]}}"#
            .to_string()
    }

    fn token_plan_top_up() -> String {
        r#"{"status":1, "desc":"", "five_hour_usage_left_rate":0, "five_hour_usage_reset_time":"0", "weekly_usage_left_rate":0, "weekly_usage_reset_time":"0", "plan_family":2, "plan_credit_rate_limit":{"subscription_credit_left_rate":0.1, "subscription_credit_reset_time":"0", "topup_credit_left_rate":1, "credit_buckets":[{"type":1, "credit_total":"400000000", "credit_residual":"40000000", "expire_at":"1791187256", "next_reset_at":"1790582456"}, {"type":2, "credit_total":"400000000", "credit_residual":"400000000", "expire_at":"1792396856", "next_reset_at":"0"}]}}"#
            .to_string()
    }

    /// The Coding Plan shape is second-hand.
    fn coding_plan() -> String {
        r#"{"status":1, "desc":"", "five_hour_usage_left_rate":0.75, "five_hour_usage_reset_time":"1790250000", "weekly_usage_left_rate":1, "weekly_usage_reset_time":1790640000, "plan_family":1}"#
            .to_string()
    }

    fn plan_status() -> String {
        r#"{"status":1, "desc":"", "subscription":{"plan_type":1, "name":"Plus", "status":1, "pay_channel":3, "activated_at":"1789891256", "expired_at":"1791187256", "auto_renew":false, "plan_id":"21", "source_channel_code":"", "plan_family":2}, "agreement":null, "plan_definition":{"type":1, "price":"9900", "duration_days":30, "plan_id":"21", "billing_cycle":1, "plan_family":2}, "can_resign":false}"#
            .to_string()
    }

    fn close(actual: Option<f64>, expected: f64) -> bool {
        actual.is_some_and(|actual| (actual - expected).abs() < 1e-9)
    }

    /// The session has published names, so nothing else the host set leaves.
    #[test]
    fn only_the_consoles_own_cookies_are_kept() {
        let kept = normalize(
            "Cookie: _ga=GA1.1; Oasis-Token=abc...def; Hm_lvt=1; Oasis-Webid=w1; INGRESSCOOKIE=i1; lang=zh",
        )
        .unwrap();
        assert_eq!(kept, "Oasis-Token=abc...def; Oasis-Webid=w1; INGRESSCOOKIE=i1");
    }

    #[test]
    fn no_token_is_no_session_and_a_control_character_is_refused() {
        assert_eq!(
            normalize("Oasis-Webid=w1; lang=zh"),
            Err(CookieProblem::Invalid)
        );
        assert_eq!(normalize("Cookie: "), Err(CookieProblem::Missing));
        assert_eq!(
            normalize("Oasis-Token=abc\r\nX-Injected: 1"),
            Err(CookieProblem::Invalid)
        );
    }

    /// A token sent from a device id other than its own is refused as stolen,
    /// so the id is the cookie's when there is one, and the token's claim when
    /// there is not.
    #[test]
    fn the_device_id_comes_from_the_cookie_else_from_the_token() {
        assert_eq!(
            web_id("Oasis-Token=a.b.c; Oasis-Webid=w1").as_deref(),
            Some("w1")
        );

        // The claim, base64url and unpadded as a JWT carries it.
        let claims = r#"{"device_id":"dev-42"}"#;
        let encoded: String = {
            const ALPHABET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
            let bytes = claims.as_bytes();
            let mut out = String::new();
            for chunk in bytes.chunks(3) {
                let mut buffer = 0u32;
                for (index, byte) in chunk.iter().enumerate() {
                    buffer |= (*byte as u32) << (16 - 8 * index);
                }
                for index in 0..=chunk.len() {
                    out.push(ALPHABET[((buffer >> (18 - 6 * index)) & 0x3F) as usize] as char);
                }
            }
            out
        };
        assert_eq!(
            web_id(&format!("Oasis-Token=x.e30.y...h.{encoded}.s")).as_deref(),
            Some("dev-42")
        );
        assert_eq!(web_id("Oasis-Token=opaque"), None);
    }

    /// A Token Plan's pool is one ring, with no reset claimed.
    #[test]
    fn a_token_plans_pool_is_one_ring() {
        let snapshot = parse(&token_plan()).unwrap();
        let windows = windows(&snapshot, now());

        assert_eq!(windows.len(), 1);
        assert_eq!(windows[0].label, "Credits");
        assert!((windows[0].percent_used.unwrap() - 86_166.0 / 1_600_000_000.0 * 100.0).abs() < 1e-12);
        // A monthly plan states no refill: its pool simply ends.
        assert_eq!(windows[0].resets_at, None);
    }

    /// A pool and a pack are spent from one balance, so the ring is their sum,
    /// and the pool's own refill is the reset.
    #[test]
    fn a_top_up_pack_adds_to_the_same_ring() {
        let windows = windows(&parse(&token_plan_top_up()).unwrap(), now());

        assert_eq!(windows.len(), 1);
        assert!(close(windows[0].percent_used, 45.0));
        assert_eq!(windows[0].resets_at.as_deref(), Some("2026-09-28T08:00:56Z"));
    }

    /// Everything zeroed: no window and no credit. The account has no plan,
    /// which is an answer, not a ring at 0% or 100%.
    #[test]
    fn a_reply_with_nothing_in_it_is_no_plan() {
        let empty = r#"{"status":1,"five_hour_usage_left_rate":0,"five_hour_usage_reset_time":"0","weekly_usage_left_rate":0,"weekly_usage_reset_time":"0","plan_credit_rate_limit":{"subscription_credit_left_rate":0,"topup_credit_left_rate":0,"credit_buckets":[]}}"#;
        let error = parse(empty).unwrap_err();
        assert_eq!(error, "No Step Plan on this StepFun account.");
    }

    /// A window counts only when it states a reset: that is how a Coding Plan
    /// is told from a Token Plan, whose windows come back as zero with "0".
    #[test]
    fn a_coding_plans_two_windows_from_the_fraction_left() {
        let windows = windows(&parse(&coding_plan()).unwrap(), now());

        let labels: Vec<&str> = windows.iter().map(|w| w.label.as_str()).collect();
        assert_eq!(labels, vec!["5h", "7d"]);
        assert!(close(windows[0].percent_used, 25.0));
        assert!(close(windows[1].percent_used, 0.0));
        assert_eq!(windows[0].resets_at.as_deref(), Some("2026-09-24T11:40:00Z"));
        assert_eq!(windows[1].resets_at.as_deref(), Some("2026-09-29T00:00:00Z"));
    }

    #[test]
    fn a_refusal_inside_a_200_is_an_expired_session_only_when_it_says_auth() {
        let expired = parse(r#"{"status":0,"desc":"auth failed: token is invalid"}"#).unwrap_err();
        assert!(expired.contains("expired"));
        assert_eq!(parse(r#"{"status":0,"desc":"busy"}"#).unwrap_err(), unreadable());
        assert_eq!(parse("<html>").unwrap_err(), unreadable());
    }

    #[test]
    fn the_plans_name_comes_from_the_status_reply() {
        assert_eq!(plan_name(&plan_status()).as_deref(), Some("Plus"));
        assert_eq!(plan_name(r#"{"status":0}"#), None);
        // A name longer than the card carries is cut, not refused.
        let long = format!(
            r#"{{"status":1,"subscription":{{"name":"{}"}}}}"#,
            "x".repeat(60)
        );
        assert_eq!(plan_name(&long).unwrap().chars().count(), 40);
    }

    /// A fraction of nothing is no credit, and a fraction past one is one.
    #[test]
    fn a_stated_fraction_is_held_between_nothing_and_one() {
        assert_eq!(fraction(Some(&serde_json::json!(0))), None);
        assert_eq!(fraction(Some(&serde_json::json!(0.4))), Some(0.4));
        assert_eq!(fraction(Some(&serde_json::json!(1.5))), Some(1.0));
    }

    /// Each site asks its own host, and only its own.
    #[test]
    fn each_site_asks_its_own_host() {
        assert_eq!(
            Site::China.endpoint("QueryStepPlanRateLimit"),
            "https://platform.stepfun.com/api/step.openapi.devcenter.Dashboard/QueryStepPlanRateLimit"
        );
        assert_eq!(Site::International.host(), "platform.stepfun.ai");
        // The mainland site is the original's own default.
        assert_eq!(Site::named(""), Site::China);
        assert_eq!(Site::named("international"), Site::International);
        assert_eq!(Site::named("stepfun.ai"), Site::International);
        assert_eq!(Site::named("china"), Site::China);
    }
}
