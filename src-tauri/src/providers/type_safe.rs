//! TypeSafe: the credit balance on the console's billing page, read with the
//! signed-in console session.
//!
//! **A pasted Cookie header, for now.** The console's session cookie has no
//! name anyone has written down — the reference implementation deliberately
//! keeps every cookie the console sets rather than assume one — so a "read from
//! browser" import has nothing it can name and the field takes the `Cookie`
//! header copied from a request to `console.typesafe.ai/settings/billing`
//! instead. That is this provider's route, not a workaround around one.
//!
//! The figures come from a Next.js server action, not an API:
//!
//! 1. The action's id is found in the billing page's own scripts — the page
//!    lists its chunks, and one of them names `getBillingOverviewResult` beside
//!    its id. Found once and kept for twelve hours; the page and its chunks are
//!    fetched again only when the id goes stale (the action answers 404).
//! 2. `POST /settings/billing` with `Next-Action: <id>` and a body of `[]` —
//!    the read the page itself makes. It changes nothing.
//! 3. The reply is React's line format: `id:{json}` per line, and the line
//!    carrying an `ok` key is the result.
//!
//! The balance is money and nothing else, so it is drawn as a balance: no
//! fraction is invented for a purse with no stated size. The cycle's spend has
//! no limit to measure it against, and each credit grant expires rather than
//! resets, so neither is a row — the balance already sums what is left of them.
//!
//! The shapes are second-hand — taken from the reference implementation and its
//! tests, not from captured replies.

use std::sync::{Arc, Mutex, OnceLock};

use chrono::{DateTime, Utc};
use serde_json::Value;

use super::{describe_reqwest_error, find_ignoring_case, Ctx, FetchFuture, Provider};
use crate::credentials;
use crate::model::ProviderUsage;

const ORIGIN: &str = "https://console.typesafe.ai";
const BILLING_PAGE: &str = "https://console.typesafe.ai/settings/billing";

/// How long a found action id is trusted before the page is read again.
const ACTION_LIFETIME_SECONDS: i64 = 12 * 3_600;
/// At most this many of the page's scripts are opened looking for it.
const MAXIMUM_CHUNKS: usize = 60;
/// How long the console gets for the page and the action, and for one script.
const PAGE_TIMEOUT_SECONDS: u64 = 15;
const CHUNK_TIMEOUT_SECONDS: u64 = 5;

pub struct TypeSafe;

impl Provider for TypeSafe {
    fn id(&self) -> &'static str {
        "typesafe"
    }

    fn name(&self) -> &'static str {
        "TypeSafe"
    }

    /// Whether a pasted cookie is on the machine. No network call.
    fn is_configured(&self) -> bool {
        pasted_cookie().is_some()
    }

    fn fetch(&self, ctx: Arc<Ctx>) -> FetchFuture {
        Box::pin(async move { fetch_inner(ctx).await })
    }
}

/// The Cookie header the fetch reads: an environment override first, then
/// `%APPDATA%\PulseWin\typesafe.json`.
///
/// Both names are read because the credential **is** a header: `cookie` is what
/// someone pasting a request will call it, and `apiKey` is what the settings
/// window writes for every provider, so a value pasted there works too rather
/// than being silently absent.
fn pasted_cookie() -> Option<String> {
    for name in ["cookie", "token", "key"] {
        if let Some(value) = credentials::env_override("typesafe", name) {
            return Some(value);
        }
    }

    let path = super::settings_path("typesafe")?;
    let json = credentials::read_json(&path)?;
    credentials::dig_first_str(&json, &["cookie", "apiKey", "api_key", "key", "token"])
}

/// The failure when neither place has a header to send.
fn missing_cookie() -> String {
    let file = super::settings_path("typesafe")
        .map(|path| path.display().to_string())
        .unwrap_or_else(|| "%APPDATA%\\PulseWin\\typesafe.json".to_string());
    format!(
        "no console cookie (set PULSEWIN_TYPESAFE_COOKIE, or create {file} with \
         {{\"apiKey\": \"<the Cookie header of a request to {BILLING_PAGE}>\"}})"
    )
}

/// The header as pasted, with a leading `Cookie:` taken off if it came along.
fn cookie_header(pasted: &str) -> String {
    let trimmed = pasted.trim();
    match trimmed.get(.."cookie:".len()) {
        Some(head) if head.eq_ignore_ascii_case("cookie:") => {
            trimmed["cookie:".len()..].trim().to_string()
        }
        _ => trimmed.to_string(),
    }
}

// ---------------------------------------------------------------------------
// The action id, kept for the launch
// ---------------------------------------------------------------------------

/// One console, one id — so the cache is one slot rather than a map.
fn cache() -> &'static Mutex<Option<(String, DateTime<Utc>)>> {
    static CACHE: OnceLock<Mutex<Option<(String, DateTime<Utc>)>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(None))
}

/// Whether a found id is still trusted. `now` before `found_at` — a clock that
/// moved back — is still inside the window rather than outside it for ever.
fn still_fresh(found_at: DateTime<Utc>, now: DateTime<Utc>) -> bool {
    (now - found_at).num_seconds() < ACTION_LIFETIME_SECONDS
}

fn cached_action(now: DateTime<Utc>) -> Option<String> {
    let guard = cache().lock().ok()?;
    let (id, found_at) = guard.as_ref()?;
    if still_fresh(*found_at, now) {
        Some(id.clone())
    } else {
        None
    }
}

fn remember(id: Option<&str>, now: DateTime<Utc>) {
    if let Ok(mut guard) = cache().lock() {
        *guard = id.map(|id| (id.to_string(), now));
    }
}

// ---------------------------------------------------------------------------
// The fetch
// ---------------------------------------------------------------------------

/// One reply, as the console sent it: the status decides whether the body is
/// worth reading, so the two travel together.
struct Answer {
    status: reqwest::StatusCode,
    body: String,
}

/// The refusal a status is, or `None` when the body is worth reading.
///
/// `what` names the request, because the console answers a stale sign-in the
/// same way on both routes and the reader's next move is the same either way.
fn refusal(status: reqwest::StatusCode, what: &str) -> Option<String> {
    if status.is_success() {
        return None;
    }
    let hints: &[(u16, &str)] = &[
        (401, " — the console session has expired; paste a fresh Cookie header"),
        (403, " — the console session has expired; paste a fresh Cookie header"),
        (404, " — the console has been redeployed since that header was copied"),
        (429, " — rate limited, try again shortly"),
    ];
    let hint = hints
        .iter()
        .find(|(code, _)| *code == status.as_u16())
        .map(|(_, hint)| *hint)
        .unwrap_or("");
    Some(format!("HTTP {status}{hint} ({what})"))
}

/// The refusal of a status already known to be one, where there is no message
/// to derive: a provider must not panic on the way to a card.
fn refused(status: reqwest::StatusCode) -> String {
    refusal(status, "the console").unwrap_or_else(|| format!("HTTP {status}"))
}

async fn fetch_inner(ctx: Arc<Ctx>) -> ProviderUsage {
    const ID: &str = "typesafe";
    const NAME: &str = "TypeSafe";

    let cookie = match pasted_cookie().map(|pasted| cookie_header(&pasted)) {
        Some(cookie) if !cookie.is_empty() => cookie,
        _ => return ProviderUsage::failed(ID, NAME, missing_cookie()),
    };

    let now = Utc::now();
    let mut discovered = false;
    let mut action = match cached_action(now) {
        Some(cached) => cached,
        None => {
            discovered = true;
            match discover(&ctx, &cookie).await {
                Ok(found) => {
                    remember(Some(&found), now);
                    found
                }
                Err(reason) => return ProviderUsage::failed(ID, NAME, reason),
            }
        }
    };

    let mut reply = post(&ctx, &action, &cookie).await;

    // A 404 from the action is Next.js saying the id is stale: the console was
    // redeployed. Find it again, once.
    if let Ok(answer) = &reply {
        if answer.status == reqwest::StatusCode::NOT_FOUND && !discovered {
            remember(None, now);
            match discover(&ctx, &cookie).await {
                Ok(found) => {
                    action = found;
                    remember(Some(&action), now);
                }
                Err(reason) => return ProviderUsage::failed(ID, NAME, reason),
            }
            reply = post(&ctx, &action, &cookie).await;
        }
    }

    let answer = match reply {
        Ok(answer) => answer,
        Err(reason) => return ProviderUsage::failed(ID, NAME, reason),
    };
    if let Some(reason) = refusal(answer.status, "the billing action") {
        return ProviderUsage::failed(ID, NAME, reason);
    }

    reading(&answer.body)
}

/// The read the billing page itself makes: no state changes on the console.
async fn post(ctx: &Ctx, action: &str, cookie: &str) -> Result<Answer, String> {
    let request = ctx
        .client
        .post(BILLING_PAGE)
        .header("Cookie", cookie)
        .header("Origin", ORIGIN)
        .header("Next-Action", action)
        .header("Accept", "text/x-component")
        .header("Content-Type", "application/json")
        .timeout(std::time::Duration::from_secs(PAGE_TIMEOUT_SECONDS))
        .body("[]");
    send(request).await
}

async fn send(request: reqwest::RequestBuilder) -> Result<Answer, String> {
    let response = request
        .send()
        .await
        .map_err(|e| format!("request failed: {}", describe_reqwest_error(&e)))?;
    let status = response.status();
    let body = response
        .text()
        .await
        .map_err(|e| format!("cannot read body: {e}"))?;
    Ok(Answer { status, body })
}

/// Find the billing action's id in the page's own scripts.
async fn discover(ctx: &Ctx, cookie: &str) -> Result<String, String> {
    let page = match send(
        ctx.client
            .get(BILLING_PAGE)
            .header("Cookie", cookie)
            .header("Accept", "text/html")
            .timeout(std::time::Duration::from_secs(PAGE_TIMEOUT_SECONDS)),
    )
    .await
    {
        Ok(answer) => {
            if let Some(reason) = refusal(answer.status, "the billing page") {
                return Err(reason);
            }
            answer.body
        }
        Err(reason) => return Err(reason),
    };

    // A lapsed session lands on the sign-in page, redirected or not.
    if is_login_landing(&page) {
        return Err(refused(reqwest::StatusCode::UNAUTHORIZED));
    }

    // The scripts are the site's public code: asked for without the cookie, so
    // a chunk is one request that cannot leak the session anywhere.
    for chunk in chunk_urls(&page) {
        let answer = match send(
            ctx.client
                .get(&chunk)
                .timeout(std::time::Duration::from_secs(CHUNK_TIMEOUT_SECONDS)),
        )
        .await
        {
            Ok(answer) => answer,
            // A chunk that will not load is not the answer, but the next one
            // may be.
            Err(_) => continue,
        };

        if answer.status == reqwest::StatusCode::TOO_MANY_REQUESTS {
            return Err(refused(reqwest::StatusCode::TOO_MANY_REQUESTS));
        }
        if answer.status.is_server_error() {
            return Err(refused(answer.status));
        }
        if !answer.status.is_success() {
            continue;
        }
        if let Some(id) = action_id(&answer.body) {
            return Ok(id);
        }
    }

    Err("bad reply: the billing page names no action this build can find".to_string())
}

// ---------------------------------------------------------------------------
// Reading the page
// ---------------------------------------------------------------------------

/// The console's sign-in route, as its own page names it in the React payload.
///
/// The payload is JSON inside a script, so its quotes arrive escaped or not
/// depending on which layer wrote them; dropping the backslashes is what the
/// two spellings have in common, and the route's own name is what is left.
fn is_login_landing(body: &str) -> bool {
    let unescaped: String = body.chars().filter(|c| *c != '\\').collect();
    unescaped.contains(r#""(auth)",{"children":["login""#)
}

/// The page's own script chunks: same origin, JavaScript, in order, each once,
/// and no more than [`MAXIMUM_CHUNKS`] of them.
fn chunk_urls(page: &str) -> Vec<String> {
    let mut urls: Vec<String> = Vec::new();
    let mut from = 0;

    while urls.len() < MAXIMUM_CHUNKS {
        let Some(at) = find_ignoring_case(&page[from..], "<script") else {
            break;
        };
        let start = from + at;
        let Some(end) = page[start..].find('>') else {
            break;
        };
        let tag = &page[start..start + end];
        from = start + end + 1;

        let Some(source) = attribute(tag, "src") else {
            continue;
        };
        // One leading slash is the site's own path; two are another host, which
        // is why they are not prefixed and then rejected by the origin test.
        let absolute = if source.starts_with('/') && !source.starts_with("//") {
            format!("{ORIGIN}{source}")
        } else {
            source.clone()
        };

        if !absolute.starts_with(&format!("{ORIGIN}/")) {
            continue;
        }
        if !ends_in_script(&absolute) {
            continue;
        }
        if urls.contains(&absolute) {
            continue;
        }
        urls.push(absolute);
    }

    urls
}

/// An attribute's value inside a tag, quoted with either mark.
fn attribute(tag: &str, name: &str) -> Option<String> {
    let mut from = 0;
    while let Some(at) = find_ignoring_case(&tag[from..], name) {
        let after = from + at + name.len();
        let rest = tag[after..].trim_start();
        if let Some(rest) = rest.strip_prefix('=') {
            let rest = rest.trim_start();
            if let Some(quote) = rest.chars().next().filter(|c| *c == '"' || *c == '\'') {
                let value = &rest[quote.len_utf8()..];
                if let Some(end) = value.find(quote) {
                    return Some(value[..end].to_string());
                }
            }
        }
        from = after;
    }
    None
}

/// `\.js($|\?)`, case-insensitively: a script URL, not a stylesheet whose path
/// happens to end in something else.
fn ends_in_script(url: &str) -> bool {
    let mut from = 0;
    while let Some(at) = find_ignoring_case(&url[from..], ".js") {
        let after = from + at + ".js".len();
        match url[after..].chars().next() {
            None | Some('?') => return true,
            _ => {}
        }
        from = after;
    }
    false
}

/// The id written just before `"getBillingOverviewResult"` in a chunk.
///
/// Forty hex characters or more, quoted, with nothing but an unparenthesised
/// gap of at most 150 bytes between it and the action's name — which is the
/// shape the console's own bundler writes.
fn action_id(chunk: &str) -> Option<String> {
    let mut from = 0;
    while let Some(at) = chunk[from..].find('"') {
        let open = from + at;
        let Some(close) = chunk[open + 1..].find('"') else {
            break;
        };
        let close = open + 1 + close;
        let candidate = &chunk[open + 1..close];

        if candidate.len() >= 40 && candidate.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            let rest = &chunk[close + 1..];
            if let Some(gap) = rest.find("\"getBillingOverviewResult\"") {
                if gap <= 150 && !rest[..gap].contains(')') {
                    return Some(candidate.to_string());
                }
            }
        }

        from = close + 1;
    }
    None
}

// ---------------------------------------------------------------------------
// Reading the reply
// ---------------------------------------------------------------------------

/// The mapping, kept apart from the request so a fixture can drive it.
fn reading(body: &str) -> ProviderUsage {
    const ID: &str = "typesafe";
    const NAME: &str = "TypeSafe";

    if is_login_landing(body) {
        return ProviderUsage::failed(
            ID,
            NAME,
            "the console session has expired; paste a fresh Cookie header",
        );
    }

    // Each line is `id:payload`; the result is the object with `ok`.
    let result = body
        .lines()
        .filter_map(|line| {
            let (id, payload) = line.split_once(':')?;
            if id.is_empty() {
                return None;
            }
            let value: Value = serde_json::from_str(payload).ok()?;
            value.get("ok")?;
            Some(value)
        })
        .next();

    let Some(result) = result else {
        return ProviderUsage::failed(ID, NAME, "bad reply: no result line");
    };
    if result.get("ok").and_then(Value::as_bool) != Some(true) {
        return ProviderUsage::failed(ID, NAME, "the service reported a failure");
    }

    let Some(billing) = result.get("data").and_then(|data| data.get("billing")) else {
        return ProviderUsage::failed(ID, NAME, "bad reply: no billing figures");
    };
    // A boolean is not a figure, and neither is a word: `as_f64` refuses both,
    // and a zero balance is a reading rather than an absent one.
    let Some(balance) = billing.get("balance").and_then(Value::as_f64) else {
        return ProviderUsage::failed(ID, NAME, "bad reply: no balance");
    };
    if !balance.is_finite() {
        return ProviderUsage::failed(ID, NAME, "bad reply: no balance");
    }

    let plan = billing
        .get("plan")
        .and_then(Value::as_str)
        .and_then(plan_name);

    // Money and nothing else: the console prices and shows credit in dollars,
    // and a purse with no stated size has no fraction to draw. A zero here
    // would also be a full red ring and a notification saying it was spent.
    ProviderUsage::ok(
        ID,
        NAME,
        vec![super::balance_window("Balance", format!("{balance:.2} USD"))],
    )
    .with_plan(plan)
    .with_credit_remaining(balance, "USD")
}

/// `free_plan` → "Free"; `pro-monthly` → "Pro Monthly".
fn plan_name(raw: &str) -> Option<String> {
    let trimmed = raw.trim();
    if trimmed == "free_plan" {
        return Some("Free".to_string());
    }
    let words: Vec<String> = trimmed
        .split(|c| c == '_' || c == '-')
        .filter(|word| !word.is_empty())
        .map(|word| {
            let mut chars = word.chars();
            match chars.next() {
                Some(first) => {
                    first.to_uppercase().collect::<String>() + &chars.as_str().to_lowercase()
                }
                None => String::new(),
            }
        })
        .collect();
    if words.is_empty() {
        None
    } else {
        Some(words.join(" "))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(stamp: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(stamp)
            .unwrap()
            .with_timezone(&Utc)
    }

    /// Second-hand, from the original's fixture: the result line of the
    /// billing action, as React writes it.
    fn billing_result() -> String {
        r#"{"ok":true,"data":{"billing":{"plan":"free_plan","spent":0.01,"freeCreditsRemaining":4.98,"balance":4.98,"purchased":0,"resetsInDays":12,"cycleLabel":"September 2026","credits":[{"id":"00000000-0000-4000-8000-000000000001","amount":5,"remaining":4.98,"expiresAt":"2026-10-19T00:00:00Z"},{"id":"00000000-0000-4000-8000-000000000002","amount":2,"remaining":0,"expiresAt":"2026-10-19T00:00:00Z"}]}}}"#
            .to_string()
    }

    /// The reply as the console sends it: a chunk line, then the result.
    fn action_reply(result: &str) -> String {
        format!("0:{{\"a\":\"$@1\"}}\n1:{result}")
    }

    #[test]
    fn the_balance_is_read_from_the_result_line_and_draws_no_ring() {
        let usage = reading(&action_reply(&billing_result()));
        assert_eq!(usage.credit_remaining.as_ref().unwrap().amount, 4.98);
        assert_eq!(usage.credit_remaining.as_ref().unwrap().currency, "USD");
        assert_eq!(usage.plan.as_deref(), Some("Free"));
        assert_eq!(usage.windows.len(), 1);
        assert_eq!(usage.windows[0].label, "Balance");
        // Money and no fraction: a purse with no stated size has no ring.
        assert_eq!(usage.windows[0].percent_used, None);
        assert_eq!(usage.windows[0].detail.as_deref(), Some("4.98 USD"));
        // The grants expire rather than reset, so no reset is claimed.
        assert!(usage.windows[0].resets_at.is_none());
    }

    #[test]
    fn a_zero_balance_stays_zero() {
        let usage = reading(&action_reply(
            r#"{"ok":true,"data":{"billing":{"balance":0,"spent":3,"plan":"pro_monthly"}}}"#,
        ));
        assert_eq!(usage.windows[0].detail.as_deref(), Some("0.00 USD"));
        assert_eq!(usage.plan.as_deref(), Some("Pro Monthly"));
    }

    #[test]
    fn a_result_that_is_not_one_cannot_be_read() {
        for body in [
            r#"0:{"a":"$@1"}"#.to_string(),
            r#"1:{"ok":true,"data":{}}"#.to_string(),
            r#"1:{"ok":true,"data":{"billing":{"balance":null}}}"#.to_string(),
            r#"1:{"ok":true,"data":{"billing":{"balance":"bad"}}}"#.to_string(),
            // A boolean is not a figure either.
            r#"1:{"ok":true,"data":{"billing":{"balance":true}}}"#.to_string(),
            "not a reply".to_string(),
        ] {
            assert!(reading(&body).error.is_some(), "read {body}");
        }
    }

    #[test]
    fn a_result_that_says_it_failed_is_the_services() {
        let usage = reading(r#"1:{"ok":false}"#);
        assert!(usage.error.as_deref().unwrap().contains("service"));
    }

    /// The sign-in page, escaped or not, is a lapsed session.
    #[test]
    fn the_sign_in_page_is_a_lapsed_session() {
        let landing = r#"<script>self.__next_f.push([1,"0:{\"f\":[[[\"\",{\"children\":[\"(auth)\",{\"children\":[\"login\",{\"children\":[\"__PAGE__\",{}]}]}]}]]}"])</script>"#;
        assert!(is_login_landing(landing));
        assert!(is_login_landing(r#"["(auth)",{"children":["login""#));
        assert!(!is_login_landing(r#"["(dashboard)",{"children":["settings""#));
        assert!(reading(landing).error.is_some());
    }

    /// Only the console's own scripts are opened, once each.
    #[test]
    fn only_the_consoles_own_scripts_are_opened() {
        let page = r#"
        <script src="/_next/static/chunks/a.js"></script>
        <script async src='https://console.typesafe.ai/_next/static/chunks/b.js?v=2'></script>
        <script src="/_next/static/chunks/a.js"></script>
        <script src="https://cdn.example.com/evil.js"></script>
        <script src="//console.typesafe.ai.example.com/x.js"></script>
        <script src="/styles.css"></script>
        "#;
        assert_eq!(
            chunk_urls(page),
            vec![
                "https://console.typesafe.ai/_next/static/chunks/a.js".to_string(),
                "https://console.typesafe.ai/_next/static/chunks/b.js?v=2".to_string(),
            ]
        );
    }

    /// The action id is the one named beside the billing call.
    #[test]
    fn the_action_id_is_the_one_named_beside_the_billing_call() {
        let id = "f".repeat(40);
        let chunk = format!(
            "x(\"{id}\",c.callServer,void 0,c.findSourceMapURL,\"getBillingOverviewResult\")"
        );
        assert_eq!(action_id(&chunk).as_deref(), Some(id.as_str()));

        assert!(action_id(r#""abc123","getBillingOverviewResult""#).is_none());
        assert!(action_id("console.log('fixture')").is_none());

        // A name too far from any id, and a gap the pattern may not span.
        let far = format!("\"{id}\"{}\"getBillingOverviewResult\"", "x".repeat(200));
        assert!(action_id(&far).is_none());
        let closed = format!("\"{id}\"{}),\"getBillingOverviewResult\"", "x".repeat(10));
        assert!(action_id(&closed).is_none());
    }

    #[test]
    fn a_pasted_header_may_carry_its_name() {
        assert_eq!(cookie_header("Cookie: a=1; b=2"), "a=1; b=2");
        assert_eq!(cookie_header(" a=1 "), "a=1");
        assert_eq!(cookie_header("cookie:a=1"), "a=1");
        // A cookie whose own name begins with "cookie" is left alone.
        assert_eq!(cookie_header("cookie_jar=1"), "cookie_jar=1");
    }

    /// The id is trusted for twelve hours, and a clock that moved back does not
    /// strand it.
    #[test]
    fn a_found_id_is_trusted_for_twelve_hours() {
        let found = at("2026-10-02T00:00:00Z");
        assert!(still_fresh(found, at("2026-10-02T11:00:00Z")));
        assert!(!still_fresh(found, at("2026-10-02T13:00:00Z")));
        assert!(still_fresh(found, at("2026-10-01T23:00:00Z")));
    }

    /// The plan's own names are tidied, and one this build does not know is
    /// still shown rather than dropped.
    #[test]
    fn plan_names() {
        assert_eq!(plan_name("free_plan").as_deref(), Some("Free"));
        assert_eq!(plan_name("pro_monthly").as_deref(), Some("Pro Monthly"));
        assert_eq!(plan_name("pro-monthly").as_deref(), Some("Pro Monthly"));
        assert_eq!(plan_name("  TEAM  ").as_deref(), Some("Team"));
        assert_eq!(plan_name("__").as_deref(), None);
    }
}
