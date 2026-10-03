//! Qwen Cloud's individual Token Plan: a five-hour, a weekly and a monthly
//! allowance, each reported by the service as the share of it already used.
//!
//! Read the way the console's own subscription page reads it: the page's
//! security token first (from the page, a cookie, or the account endpoint),
//! then `POST https://cs-data.qwencloud.com/data/api.json` for the plan's usage
//! and, for its tier's name only, its subscription.
//!
//! **The credential is a pasted cookie.** The original imports the session out
//! of the Chromium browser the reader signed in with
//! (`Auth/BrowserCookies.swift`), which on macOS reads a cookie store through
//! the login keychain; PulseWin cannot, so the `Cookie` header copied out of a
//! signed-in request is the credential here. A sign-in ticket has to be one of
//! the three the console uses; the account markers, the CSRF token the console
//! checks, the browser id it echoes back and its security token when kept as a
//! cookie come along, and nothing else leaves the paste.
//!
//! **What is left out.** The plan's credit totals per window (the
//! `quota-config` route) are not asked for: this port has nowhere to show them,
//! and the share used is already the service's own figure. A reply in the older
//! subscription-summary shape is not read for figures; one that counts no
//! subscription at all is said as "no plan".
//!
//! The shape is second-hand — taken from the original and its fixtures, not
//! from a captured reply.

use std::sync::Arc;

use chrono::{DateTime, Utc};
use serde_json::{json, Map, Value};

use super::alibaba_coding_plan::console;
use super::{describe_reqwest_error, Ctx, FetchFuture, Provider};
use crate::model::ProviderUsage;

const ID: &str = "qwen-cloud";
const NAME: &str = "Qwen Cloud";

const ORIGIN: &str = "https://home.qwencloud.com";
const PAGE: &str = "https://home.qwencloud.com/billing/subscription/token-plan-individual";
const USER_INFO: &str = "https://home.qwencloud.com/tool/user/info.json";
const GATEWAY: &str = "https://cs-data.qwencloud.com/data/api.json";

const USAGE_API: &str = "zeldaHttp.apikeyMgr./tokenplan/personal/api/v2/usage";
const SUBSCRIPTION_API: &str = "zeldaHttp.apikeyMgr./tokenplan/personal/api/v2/subscription";
/// The individual plan, international: the only one Qwen Cloud sells.
const COMMODITY_CODE: &str = "sfm_tokenplansolo_public_intl";

/// A sign-in ticket first, any one of the three, which has to be there; then
/// the account markers, the CSRF token the console checks, the browser id it
/// echoes back, and its security token when kept as a cookie. Nothing else
/// leaves the paste.
const COOKIES: [&str; 7] = [
    "login_qwencloud_ticket|login_aliyunid_ticket|qwen_sso_ticket",
    "login_current_pk",
    "login_aliyunid_pk",
    "login_aliyunid_csrf",
    "csrf",
    "cna",
    "sec_token",
];

pub struct QwenCloud;

impl Provider for QwenCloud {
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

async fn fetch_inner(ctx: Arc<Ctx>) -> ProviderUsage {
    let Some(cookie) = super::pasted::cookie(ID).and_then(|header| super::pasted::keep(&header, &COOKIES))
    else {
        return ProviderUsage::failed(ID, NAME, super::pasted::needed(ID, "Qwen Cloud session cookie"));
    };

    let token = match security_token(&ctx, &cookie).await {
        Ok(token) => token,
        Err(reason) => return ProviderUsage::failed(ID, NAME, reason),
    };

    let usage = match ask(&ctx, USAGE_API, &[], &token, &cookie).await {
        Ok(body) => body,
        Err(reason) => return ProviderUsage::failed(ID, NAME, reason),
    };

    let found = reading(&usage, Utc::now());
    if found.error.is_some() {
        return found;
    }

    // The tier's name only; a failure here costs the label, not the figures.
    let plan = match ask(
        &ctx,
        SUBSCRIPTION_API,
        &[("commodityCode", COMMODITY_CODE)],
        &token,
        &cookie,
    )
    .await
    {
        Ok(body) => plan_name(&body),
        Err(_) => None,
    };

    found.with_plan(plan)
}

// ---------------------------------------------------------------------------
// The security token
// ---------------------------------------------------------------------------

/// The console's `sec_token`, which every gateway call carries: from the
/// subscription page, then from the session's own cookie, then from the account
/// endpoint. None of the three is a sign-in the console accepts.
async fn security_token(ctx: &Ctx, cookie: &str) -> Result<String, String> {
    let mut page_failure: Option<String> = None;

    match reply(ctx, PAGE, "text/html,application/xhtml+xml", cookie).await {
        Err(reason) => page_failure = Some(reason),
        Ok((status, body)) if status == 200 => {
            if !is_sign_in_page(&body) {
                if let Some(token) = security_token_in_page(&body) {
                    return Ok(token);
                }
            }
        }
        Ok((status, _)) => {
            if (500..=599).contains(&status) {
                page_failure = Some(server_error());
            }
        }
    }

    if let Some(token) = cookie_value("sec_token", cookie) {
        return Ok(token);
    }

    if let Ok((200, body)) = reply(ctx, USER_INFO, "application/json, text/plain, */*", cookie).await {
        if let Ok(object) = serde_json::from_str::<Value>(&body) {
            let tree = console::expanded(&object);
            if let Some(root) = tree.as_object() {
                if let Some(token) =
                    console::first_string_in(&["secToken", "sec_token", "csrfToken", "token"], root)
                {
                    return Ok(token);
                }
            }
        }
    }

    // A page that could not be reached says more than a token not found.
    Err(page_failure.unwrap_or_else(expired))
}

/// The five forms the console writes its security token in, in the order the
/// original tries them.
fn security_token_in_page(html: &str) -> Option<String> {
    quoted_pair(html, "secToken")
        .or_else(|| quoted_pair(html, "sec_token"))
        .or_else(|| assigned_value(html, "secToken"))
        .or_else(|| assigned_value(html, "sec_token"))
        .or_else(|| assigned_value(html, "csrfToken"))
}

/// `"NAME"\s*:\s*"([^"]+)"` — the name in double quotes, a colon, and a
/// double-quoted value.
fn quoted_pair(html: &str, name: &str) -> Option<String> {
    let mut from = 0;
    while let Some(at) = super::find_ignoring_case(&html[from..], name) {
        let start = from + at;
        let after = start + name.len();
        from = after;

        let quoted = html.get(..start).is_some_and(|head| head.ends_with('"'))
            && html.get(after..).is_some_and(|rest| rest.starts_with('"'));
        if !quoted {
            continue;
        }

        let Some(rest) = html.get(after + 1..) else {
            continue;
        };
        let Some(rest) = rest.trim_start().strip_prefix(':') else {
            continue;
        };
        let Some(rest) = rest.trim_start().strip_prefix('"') else {
            continue;
        };
        if let Some(end) = rest.find('"') {
            let token = rest[..end].trim();
            if !token.is_empty() {
                return Some(token.to_string());
            }
        }
    }
    None
}

/// `NAME['"]?\s*[:=]\s*['"]([^'"]+)['"]` — the name, the quote it may have been
/// written with, a colon or an equals, and the value in either mark.
fn assigned_value(html: &str, name: &str) -> Option<String> {
    let mut from = 0;
    while let Some(at) = super::find_ignoring_case(&html[from..], name) {
        let after = from + at + name.len();
        from = after;

        let rest = &html[after..];
        let rest = match rest.chars().next() {
            Some(quote @ ('"' | '\'')) => &rest[quote.len_utf8()..],
            _ => rest,
        };
        let rest = rest.trim_start();
        let rest = match rest.chars().next() {
            Some(':') | Some('=') => &rest[1..],
            _ => continue,
        };
        let rest = rest.trim_start();
        let quote = match rest.chars().next() {
            Some(quote @ ('"' | '\'')) => quote,
            _ => continue,
        };
        let rest = &rest[quote.len_utf8()..];

        let Some(end) = rest.find(|c| c == '"' || c == '\'') else {
            continue;
        };
        let token = rest[..end].trim();
        if !token.is_empty() {
            return Some(token.to_string());
        }
    }
    None
}

/// Whether a page is one of the console's sign-in pages rather than a billing
/// one.
fn is_sign_in_page(html: &str) -> bool {
    let page = html.to_lowercase();
    const SIGN_IN: [&str; 4] = [
        "passport.alibabacloud.com",
        "signin.aliyun.com",
        "account.alibabacloud.com/login",
        "login.qwencloud.com",
    ];
    SIGN_IN.iter().any(|host| page.contains(host))
        || (page.contains("login") && page.contains("password") && page.contains("sign in"))
}

/// A cookie's value out of a `name=value; …` header, and not a name that merely
/// ends in this one.
fn cookie_value(name: &str, header: &str) -> Option<String> {
    let prefix = format!("{name}=");
    header.split(';').find_map(|part| {
        let value = part.trim().strip_prefix(&prefix)?;
        if value.is_empty() {
            None
        } else {
            Some(value.to_string())
        }
    })
}

// ---------------------------------------------------------------------------
// The gateway
// ---------------------------------------------------------------------------

async fn ask(
    ctx: &Ctx,
    api: &str,
    data: &[(&str, &str)],
    token: &str,
    cookie: &str,
) -> Result<String, String> {
    let url = format!(
        "{GATEWAY}?action=IntlBroadScopeAspnGateway&product=sfm_bailian&api={api}&_v=undefined"
    );
    let params = params(api, data, cookie);

    let body = form(&[
        ("product", "sfm_bailian".to_string()),
        ("action", "IntlBroadScopeAspnGateway".to_string()),
        ("sec_token", token.to_string()),
        ("region", "ap-southeast-1".to_string()),
        ("language", "en-US".to_string()),
        ("params", params),
    ]);

    // The client that refuses redirects, so a signed-out session shows here as
    // the redirect to the sign-in page itself.
    let mut request = ctx
        .gateway_client
        .post(&url)
        .header("Content-Type", "application/x-www-form-urlencoded")
        .header("Accept", "application/json, text/plain, */*")
        .header("Cookie", cookie)
        .header("Origin", ORIGIN)
        .header("Referer", PAGE)
        .header("X-Requested-With", "XMLHttpRequest")
        .body(body);

    if let Some(csrf) = cookie_value("login_aliyunid_csrf", cookie).or_else(|| cookie_value("csrf", cookie)) {
        request = request
            .header("x-xsrf-token", &csrf)
            .header("x-csrf-token", &csrf);
    }

    let (status, body) = send(request).await?;

    if (300..400).contains(&status) {
        return Err(expired());
    }
    if !(200..300).contains(&status) {
        return Err(super::http_failure(
            reqwest::StatusCode::from_u16(status).unwrap_or(reqwest::StatusCode::BAD_REQUEST),
            &[
                (401, " — the session has expired; copy a fresh cookie from qwencloud.com"),
                (403, " — the session has expired; copy a fresh cookie from qwencloud.com"),
                (429, " — rate limited, try again shortly"),
            ],
        ));
    }

    Ok(body)
}

/// The envelope the console page wraps every call in: the gateway's own
/// parameters, the account's markers, and the browser id it echoes back.
fn params(api: &str, data: &[(&str, &str)], cookie: &str) -> String {
    let mut cornerstone: Map<String, Value> = Map::new();
    cornerstone.insert("feTraceId".to_string(), json!(trace_id()));
    cornerstone.insert("feURL".to_string(), json!(PAGE));
    cornerstone.insert("protocol".to_string(), json!("V2"));
    cornerstone.insert("console".to_string(), json!("ONE_CONSOLE"));
    cornerstone.insert("productCode".to_string(), json!("p_efm"));
    cornerstone.insert("domain".to_string(), json!("home.qwencloud.com"));
    cornerstone.insert("consoleSite".to_string(), json!("QWENCLOUD"));
    cornerstone.insert("userNickName".to_string(), json!(""));
    cornerstone.insert("userPrincipalName".to_string(), json!(""));
    cornerstone.insert("xsp_lang".to_string(), json!("en-US"));
    if let Some(browser) = cookie_value("cna", cookie) {
        cornerstone.insert("X-Anonymous-Id".to_string(), json!(browser));
    }

    let mut payload: Map<String, Value> = Map::new();
    for (key, value) in data {
        payload.insert((*key).to_string(), json!(value));
    }
    payload.insert("cornerstoneParam".to_string(), Value::Object(cornerstone));

    // Sorted keys, as the console's own page writes them: this port's JSON map
    // is a sorted one, so the two agree without asking.
    json!({ "Api": api, "V": "1.0", "Data": Value::Object(payload) }).to_string()
}

/// A fresh trace id in the shape the console's own page writes — a lowercase
/// v4 UUID. This port has no UUID dependency and nothing reads the value back,
/// so it is built from the clock and a counter rather than asked for.
fn trace_id() -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);

    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_nanos() as u64)
        .unwrap_or(0);
    let mut bits = nanos ^ COUNTER.fetch_add(1, Ordering::Relaxed).wrapping_mul(0x9E37_79B9_7F4A_7C15);
    bits ^= bits << 13;
    bits ^= bits >> 7;
    bits ^= bits << 17;

    let hex = format!("{bits:016x}{nanos:016x}");
    format!(
        "{}-{}-4{}-a{}-{}",
        &hex[0..8],
        &hex[8..12],
        &hex[13..16],
        &hex[17..20],
        &hex[20..32]
    )
}

/// A form body with everything but the unreserved characters escaped, so a
/// `+`, `&` or `=` inside the token or the JSON arrives as itself.
fn form(fields: &[(&str, String)]) -> String {
    fields
        .iter()
        .map(|(name, value)| format!("{}={}", escape(name), escape(value)))
        .collect::<Vec<String>>()
        .join("&")
}

fn escape(text: &str) -> String {
    let mut escaped = String::new();
    for byte in text.bytes() {
        let c = byte as char;
        if c.is_ascii_alphanumeric() || matches!(c, '-' | '.' | '_' | '~') {
            escaped.push(c);
        } else {
            escaped.push_str(&format!("%{byte:02X}"));
        }
    }
    escaped
}

// ---------------------------------------------------------------------------
// Reading the replies
// ---------------------------------------------------------------------------

/// The mapping, kept apart from the requests so a fixture can drive it.
fn reading(body: &str, now: DateTime<Utc>) -> ProviderUsage {
    let Ok(raw) = serde_json::from_str::<Value>(body) else {
        return unreadable_or_sign_in(body);
    };
    let tree = console::expanded(&raw);
    let Some(root) = tree.as_object() else {
        return unreadable_or_sign_in(body);
    };

    match console::failure(root) {
        Some(console::Failure::SignedOut) => return ProviderUsage::failed(ID, NAME, expired()),
        Some(console::Failure::Failed) => {
            return ProviderUsage::failed(ID, NAME, server_error())
        }
        None => {}
    }

    let windows = super::alibaba_token_plan::rolling_windows(root, None, now);
    if !windows.is_empty() {
        return ProviderUsage::ok(ID, NAME, windows);
    }

    // The older summary shape, counting the account's subscriptions.
    if console::first_int_in(&["TotalCount", "totalCount"], root) == Some(0) {
        return ProviderUsage::failed(ID, NAME, "this account has no plan with usage limits");
    }

    ProviderUsage::failed(ID, NAME, "no limits reported in the reply")
}

/// A body that is not a reply: a sign-in page in its place, or something this
/// build cannot read.
fn unreadable_or_sign_in(body: &str) -> ProviderUsage {
    let text: String = body.chars().take(4_096).collect::<String>().to_lowercase();
    let reason = if text.contains("<html") && is_sign_in_page(&text) {
        expired()
    } else {
        "the reply could not be read".to_string()
    };
    ProviderUsage::failed(ID, NAME, reason)
}

/// The tier the subscription names, as the console spells it on the page.
fn plan_name(body: &str) -> Option<String> {
    let raw = serde_json::from_str::<Value>(body).ok()?;
    let tree = console::expanded(&raw);
    let root = tree.as_object()?;
    let code = console::first_string_in(&["specCode", "spec_code", "planName", "plan_name"], root)?;

    let lowered = code.to_lowercase();
    if !["lite", "standard", "pro", "max"].contains(&lowered.as_str()) {
        return Some(code);
    }
    let mut chars = code.chars();
    match chars.next() {
        Some(first) => Some(first.to_uppercase().collect::<String>() + &chars.as_str().to_lowercase()),
        None => None,
    }
}

fn expired() -> String {
    "the session has expired; copy a fresh cookie from qwencloud.com".to_string()
}

fn server_error() -> String {
    "the service reported an error".to_string()
}

// ---------------------------------------------------------------------------
// The one request that has no status to classify
// ---------------------------------------------------------------------------

/// One request whose **status is read rather than classified**: the security
/// token is looked for on a page that may answer anything.
async fn reply(ctx: &Ctx, url: &str, accept: &str, cookie: &str) -> Result<(u16, String), String> {
    send(
        ctx.gateway_client
            .get(url)
            .header("Cookie", cookie)
            .header("Accept", accept),
    )
    .await
}

async fn send(request: reqwest::RequestBuilder) -> Result<(u16, String), String> {
    let response = request
        .send()
        .await
        .map_err(|e| format!("request failed: {}", describe_reqwest_error(&e)))?;
    let status = response.status().as_u16();
    let body = response
        .text()
        .await
        .map_err(|e| format!("cannot read body: {e}"))?;
    Ok((status, body))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn now() -> DateTime<Utc> {
        DateTime::from_timestamp(1_700_000_000, 0).unwrap()
    }

    /// Second-hand, from the original's fixtures: the replies CodexBar's Qwen
    /// Cloud and Alibaba Token Plan providers describe.
    fn usage_fixture() -> String {
        r#"{"code":"200","data":{"DataV2":{"data":"{\"success\":true,\"code\":0,\"data\":{\"per5HourPercentage\":0.03,\"per5HourResetTime\":1700003600000,\"per1WeekPercentage\":0.01,\"per1WeekResetTime\":1700086400000}}","success":true,"httpStatus":200}},"httpStatusCode":200,"successResponse":true}"#
            .to_string()
    }

    fn subscription_fixture() -> String {
        r#"{"code":"200","data":{"DataV2":{"data":{"success":true,"data":{"instanceCode":"sfm_tokenplansolo_public_intl-redacted","specCode":"standard","status":"VALID","remainingDays":29}},"success":true,"httpStatus":200}},"successResponse":true}"#
            .to_string()
    }

    fn no_subscription_fixture() -> String {
        r#"{"requestId":"00000000-0000-4000-8000-000000000001","code":"200","message":null,"data":{"RequestId":"00000000-0000-4000-8000-000000000001","Message":"Successful!","Data":{"Uid":7,"TotalSurplusValue":"0","TotalCount":0,"TotalValue":"0","ProductCode":"sfm_tokenplansolo_public_intl"},"Code":"Success","Success":true},"httpStatusCode":"200","successResponse":true}"#
            .to_string()
    }

    #[test]
    fn the_windows_are_read_as_the_shares_the_console_reports_through_its_envelope() {
        let usage = reading(&usage_fixture(), now());

        let labels: Vec<&str> = usage.windows.iter().map(|w| w.label.as_str()).collect();
        assert_eq!(labels, vec!["5h", "7d"]);
        assert_eq!(usage.windows[0].percent_used, Some(3.0));
        assert_eq!(usage.windows[1].percent_used, Some(1.0));
        // The resets are epoch milliseconds, read as the seconds they name.
        assert_eq!(
            usage.windows[0].resets_at.as_deref(),
            Some("2023-11-14T23:13:20Z")
        );
        assert_eq!(
            usage.windows[1].resets_at.as_deref(),
            Some("2023-11-15T22:13:20Z")
        );
    }

    #[test]
    fn a_monthly_window_is_read_without_claiming_a_length() {
        let usage = reading(
            r#"{"data":{"per1MonthPercentage":0.4,"per1MonthResetTime":1701000000000}}"#,
            now(),
        );
        assert_eq!(usage.windows.len(), 1);
        assert_eq!(usage.windows[0].label, "Monthly");
        assert_eq!(usage.windows[0].percent_used, Some(40.0));
    }

    #[test]
    fn the_subscription_names_the_tier_as_the_page_spells_it() {
        assert_eq!(plan_name(&subscription_fixture()).as_deref(), Some("Standard"));
        assert_eq!(plan_name("{}"), None);
    }

    #[test]
    fn an_account_that_counts_no_subscription_has_no_plan() {
        let usage = reading(&no_subscription_fixture(), now());
        assert_eq!(
            usage.error.as_deref(),
            Some("this account has no plan with usage limits")
        );
    }

    #[test]
    fn a_console_that_wants_a_sign_in_is_an_expired_session() {
        for body in [
            r#"{"code":"ConsoleNeedLogin","message":"You need to log in.","successResponse":false}"#,
            r#"{"statusCode":403,"message":"Forbidden"}"#,
            r#"{"code":"200","data":{"success":false,"errorCode":"PostonlyOrTokenError","errorMsg":"refresh page"},"successResponse":true}"#,
        ] {
            let usage = reading(body, now());
            assert!(
                usage.error.as_deref().unwrap_or("").contains("expired"),
                "read {body}"
            );
        }
    }

    #[test]
    fn a_workspace_the_account_may_not_use_is_a_failure_not_a_session_to_renew() {
        let body = r#"{"code":"200","data":{"success":false,"errorCode":"BailianGateway.Workspace.NotAuthorised"},"successResponse":true}"#;
        let usage = reading(body, now());
        assert!(usage.error.as_deref().unwrap_or("").contains("service"));
    }

    #[test]
    fn a_reply_that_cannot_be_read_and_a_sign_in_page_in_its_place() {
        let unreadable = reading("not-json", now());
        assert_eq!(unreadable.error.as_deref(), Some("the reply could not be read"));

        let page = r#"<html><a href="https://passport.alibabacloud.com/login">Sign in</a></html>"#;
        assert!(reading(page, now())
            .error
            .as_deref()
            .unwrap_or("")
            .contains("expired"));
    }

    #[test]
    fn a_figure_that_is_not_one_is_left_off_and_nothing_left_is_no_limits() {
        let body = r#"{"data":{"per5HourPercentage":-0.2,"per1WeekPercentage":null}}"#;
        assert_eq!(
            reading(body, now()).error.as_deref(),
            Some("no limits reported in the reply")
        );
    }

    #[test]
    fn the_pages_security_token_is_found_in_the_forms_the_console_writes_it() {
        assert_eq!(
            security_token_in_page(r#"<script>sec_token = "abc+/=";</script>"#).as_deref(),
            Some("abc+/=")
        );
        assert_eq!(
            security_token_in_page(r#"{"secToken":"xyz"}"#).as_deref(),
            Some("xyz")
        );
        assert_eq!(
            security_token_in_page(r#"<script>window.csrfToken='c1';</script>"#).as_deref(),
            Some("c1")
        );
        assert_eq!(security_token_in_page("<html></html>"), None);
        // An empty value is no token, and the search carries on.
        assert_eq!(
            security_token_in_page(r#"{"secToken":"","sec_token":"later"}"#).as_deref(),
            Some("later")
        );
    }

    #[test]
    fn the_form_keeps_reserved_characters_in_the_token_and_the_json_as_themselves() {
        let body = form(&[
            ("sec_token", "a+b&c=d/東".to_string()),
            ("params", r#"{"Api":"x"}"#.to_string()),
        ]);
        assert_eq!(
            body,
            "sec_token=a%2Bb%26c%3Dd%2F%E6%9D%B1&params=%7B%22Api%22%3A%22x%22%7D"
        );
    }

    #[test]
    fn the_gateway_envelope_carries_the_session_markers_and_the_browser_id() {
        let cookie = "login_qwencloud_ticket=t; login_aliyunid_csrf=c1; cna=anon";
        let envelope = params(USAGE_API, &[], cookie);

        assert!(envelope.contains(r#""Api":"zeldaHttp.apikeyMgr./tokenplan/personal/api/v2/usage""#));
        assert!(envelope.contains(r#""V":"1.0""#));
        assert!(envelope.contains(r#""X-Anonymous-Id":"anon""#));
        assert!(envelope.contains(r#""consoleSite":"QWENCLOUD""#));

        // A session with no browser id simply carries none.
        assert!(!params(USAGE_API, &[], "login_qwencloud_ticket=t").contains("X-Anonymous-Id"));
        // The tier's own call adds the commodity it asks about.
        assert!(params(SUBSCRIPTION_API, &[("commodityCode", COMMODITY_CODE)], cookie)
            .contains(r#""commodityCode":"sfm_tokenplansolo_public_intl""#));
    }

    /// Only the named cookies leave the paste, and not without a sign-in
    /// ticket.
    #[test]
    fn only_the_named_cookies_are_kept() {
        let kept = super::super::pasted::keep(
            "login_qwencloud_ticket=t; tracking=x; cna=anon",
            &COOKIES,
        );
        assert_eq!(kept.as_deref(), Some("login_qwencloud_ticket=t; cna=anon"));
        assert_eq!(super::super::pasted::keep("cna=anon", &COOKIES), None);
        // Any one of the three tickets is enough.
        assert!(super::super::pasted::keep("qwen_sso_ticket=s", &COOKIES).is_some());
    }

    #[test]
    fn a_trace_id_is_one() {
        let id = trace_id();
        assert_eq!(id.len(), 36);
        assert_eq!(id.as_bytes()[14], b'4');
        assert!(id.chars().all(|c| c.is_ascii_hexdigit() || c == '-'));
        assert_ne!(id, trace_id());
    }
}
