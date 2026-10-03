//! Nous Portal, Nous Research's subscription for the Hermes inference API: a
//! monthly credit grant, and money bought on top of it.
//!
//! **Read with the login Hermes Agent saved, and never renewed.** The portal's
//! account endpoint takes only the OAuth access token Hermes mints — an API key
//! buys inference and is refused there. Hermes keeps that token in
//! `~/.hermes/auth.json` (and a shared copy in `shared/nous_auth.json`), which
//! is where this port reads it too. It lives about an hour, and its refresh
//! token is single-use: the portal rotates it on every refresh and revokes the
//! whole session when an old one is replayed. A second client refreshing behind
//! Hermes's back would sign Hermes out, so this only reads the current token
//! and, once it has lapsed, says so **without sending it**. Any `hermes`
//! command renews it.
//!
//! `is_configured` is the same question the fetch asks first — is there a token
//! here that has not lapsed — and it is answered from the file and the clock
//! alone, with no network call. An aged-out token is not a way in: the request
//! would only be refused, and that is already known.
//!
//! `GET {portal}/api/oauth/account` with the token. The portal is the one
//! Hermes stored only when it is `nousresearch.com` or beneath it, over https;
//! anything else is ignored and the default portal asked instead, so an edited
//! file cannot send the token somewhere else.
//!
//! The shape is second-hand — taken from CodexBar's Nous provider and its
//! tests, not from a captured reply — and the fixture below says so.
//!
//! Spendable credit keeps its numeric amount and currency beside the windows.
//! The display-only balance row has no utilization fraction.

use std::path::PathBuf;
use std::sync::Arc;

use chrono::{DateTime, TimeDelta, Utc};
use serde_json::Value;

use super::{percent_from_fraction, Ctx, FetchFuture, Provider};
use crate::credentials;
use crate::model::{ProviderUsage, UsageWindow};

const DEFAULT_PORTAL: &str = "https://portal.nousresearch.com";
const ACCOUNT_PATH: &str = "/api/oauth/account";

/// A token this close to its expiry is treated as expired, so a request never
/// races the portal's clock.
const EXPIRY_SKEW: TimeDelta = TimeDelta::seconds(60);

pub struct NousPortal;

impl Provider for NousPortal {
    fn id(&self) -> &'static str {
        "nous-portal"
    }

    fn name(&self) -> &'static str {
        "Nous Portal"
    }

    /// A login Hermes saved that has not lapsed. See the module docs: an
    /// aged-out token is not a credential this provider may use.
    fn is_configured(&self) -> bool {
        stored_login(Utc::now()).is_ok()
    }

    fn fetch(&self, ctx: Arc<Ctx>) -> FetchFuture {
        Box::pin(async move { fetch_inner(ctx).await })
    }
}

/// The two files Hermes writes, the per-profile one first.
fn login_files() -> Vec<PathBuf> {
    let mut paths = credentials::home_relative(&[".hermes", "auth.json"]);
    paths.extend(credentials::home_relative(&[".hermes", "shared", "nous_auth.json"]));
    paths
}

async fn fetch_inner(ctx: Arc<Ctx>) -> ProviderUsage {
    const ID: &str = "nous-portal";
    const NAME: &str = "Nous Portal";

    let login = match stored_login(Utc::now()) {
        Ok(login) => login,
        Err(reason) => return ProviderUsage::failed(ID, NAME, reason),
    };

    let response = ctx
        .client
        .get(format!("{}{ACCOUNT_PATH}", login.portal))
        .header("Authorization", format!("Bearer {}", login.token))
        .header("Accept", "application/json")
        .send()
        .await;

    let response = match response {
        Ok(r) => r,
        Err(e) => {
            return ProviderUsage::failed(
                ID,
                NAME,
                format!("request failed: {}", super::describe_reqwest_error(&e)),
            )
        }
    };

    let status = response.status();
    let body = match response.text().await {
        Ok(t) => t,
        Err(e) => return ProviderUsage::failed(ID, NAME, format!("cannot read body: {e}")),
    };

    if !status.is_success() {
        let refused = "the saved Nous login has expired — run any `hermes` command to renew it";
        let hints: &[(u16, &str)] = &[
            (401, refused),
            (403, refused),
            (429, " — rate limited, try again shortly"),
        ];
        return ProviderUsage::failed(ID, NAME, super::http_failure(status, hints));
    }

    let json: Value = match serde_json::from_str(&body) {
        Ok(v) => v,
        Err(e) => return ProviderUsage::failed(ID, NAME, format!("bad JSON: {e}")),
    };

    super::claude_code::debug_dump(ID, &json);

    reading(&json)
}

// ---------------------------------------------------------------------------
// Hermes's saved login
// ---------------------------------------------------------------------------

struct StoredLogin {
    token: String,
    portal: String,
}

/// The first usable login, or why there is none.
///
/// A token that has lapsed is not sent: the answer would only be a refusal, and
/// it is already known. A file whose login has lapsed is remembered as such, so
/// "signed in and gone stale" reads differently from "never signed in" — the
/// two need different things from the reader.
fn stored_login(now: DateTime<Utc>) -> Result<StoredLogin, String> {
    let mut saw_expired = false;

    for path in login_files() {
        let Some(json) = credentials::read_json(&path) else {
            continue;
        };
        let Some(login) = login_from(&json) else {
            continue;
        };
        if let Some(expires_at) = login.expires_at {
            if expires_at - now <= EXPIRY_SKEW {
                saw_expired = true;
                continue;
            }
        }
        return Ok(StoredLogin {
            token: login.token,
            portal: login.portal,
        });
    }

    Err(if saw_expired {
        "the saved Nous login has expired — run any `hermes` command to renew it".to_string()
    } else {
        "no Nous login found — sign in with Hermes Agent so ~/.hermes/auth.json exists".to_string()
    })
}

struct Entry {
    token: String,
    portal: String,
    expires_at: Option<DateTime<Utc>>,
}

/// Hermes writes one of three shapes: `providers.nous`, a `credential_pool.nous`
/// list, or the state object on its own.
fn login_from(json: &Value) -> Option<Entry> {
    if let Some(nous) = json.get("providers").and_then(|p| p.get("nous")) {
        if let Some(entry) = entry(nous) {
            return Some(entry);
        }
    }

    // The pool's newest login, as Hermes itself picks it: the one whose token
    // lasts longest.
    if let Some(entries) = json
        .get("credential_pool")
        .and_then(|pool| pool.get("nous"))
        .and_then(Value::as_array)
    {
        if let Some(newest) = entries
            .iter()
            .filter_map(entry)
            .max_by_key(|entry| entry.expires_at)
        {
            return Some(newest);
        }
    }

    entry(json)
}

fn entry(state: &Value) -> Option<Entry> {
    let token = credentials::dig_str(state, "access_token")?;

    Some(Entry {
        portal: trusted_portal(state.get("portal_base_url").and_then(Value::as_str))
            .unwrap_or_else(|| DEFAULT_PORTAL.to_string()),
        expires_at: credentials::dig_str(state, "expires_at")
            .and_then(|text| parse_iso(&text))
            .or_else(|| jwt_expiry(&token)),
        token,
    })
}

/// The portal Hermes stored, only if it is Nous's own and over https.
fn trusted_portal(raw: Option<&str>) -> Option<String> {
    let mut text = raw?.trim().to_string();
    while text.ends_with('/') {
        text.pop();
    }
    if text.is_empty() {
        return None;
    }

    let url = reqwest::Url::parse(&text).ok()?;
    if url.scheme().to_lowercase() != "https" {
        return None;
    }
    let host = url.host_str()?.to_lowercase();
    if host != "nousresearch.com" && !host.ends_with(".nousresearch.com") {
        return None;
    }
    if !url.username().is_empty() || url.password().is_some() || url.port().is_some() {
        return None;
    }
    // A URL's path is "/" when none was typed where Swift's is empty: both are
    // "the portal itself", and anything else is a route this must not build on.
    if url.path() != "/" && !url.path().is_empty() {
        return None;
    }
    if url.query().is_some() || url.fragment().is_some() {
        return None;
    }

    Some(text)
}

/// The `exp` claim of a JWT, for a login saved without an expiry.
fn jwt_expiry(token: &str) -> Option<DateTime<Utc>> {
    let mut parts = token.split('.');
    let _header = parts.next()?;
    let payload = parts.next()?;
    parts.next()?;
    if parts.next().is_some() {
        return None;
    }

    let claims: Value = serde_json::from_slice(&base64url(payload)?).ok()?;
    let exp = super::dig_number(claims.get("exp"))?;
    if !exp.is_finite() {
        return None;
    }
    DateTime::from_timestamp(exp as i64, 0)
}

/// The bytes a base64url payload names.
///
/// Both alphabets are accepted, because the point is to read a claim out of a
/// token rather than to police what minted it.
fn base64url(text: &str) -> Option<Vec<u8>> {
    fn digit(byte: u8) -> Option<u32> {
        match byte {
            b'A'..=b'Z' => Some(u32::from(byte - b'A')),
            b'a'..=b'z' => Some(u32::from(byte - b'a') + 26),
            b'0'..=b'9' => Some(u32::from(byte - b'0') + 52),
            b'-' | b'+' => Some(62),
            b'_' | b'/' => Some(63),
            _ => None,
        }
    }

    let mut out = Vec::new();
    let mut accumulator: u32 = 0;
    let mut bits: u32 = 0;

    for byte in text.bytes() {
        if byte == b'=' {
            break;
        }
        accumulator = (accumulator << 6) | digit(byte)?;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((accumulator >> bits) as u8);
        }
    }

    Some(out)
}

/// The instant an ISO 8601 timestamp names, with or without fractional seconds.
fn parse_iso(text: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(text.trim())
        .ok()
        .map(|at| at.with_timezone(&Utc))
}

// ---------------------------------------------------------------------------
// Reading the reply
// ---------------------------------------------------------------------------

/// A money field: a finite number, or a decimal written as a string. Missing is
/// nothing; anything else is a reply that cannot be trusted.
enum Amount {
    Missing,
    Value(f64),
    Malformed,
}

fn amount(value: Option<&Value>) -> Amount {
    match value {
        None | Some(Value::Null) => Amount::Missing,
        Some(Value::Number(number)) => match number.as_f64() {
            Some(value) if value.is_finite() => Amount::Value(value),
            _ => Amount::Malformed,
        },
        Some(Value::String(text)) => {
            let text = text.trim();
            if !is_decimal(text) {
                return Amount::Malformed;
            }
            match text.parse::<f64>() {
                Ok(value) if value.is_finite() => Amount::Value(value),
                _ => Amount::Malformed,
            }
        }
        _ => Amount::Malformed,
    }
}

/// `12`, `12.`, `.5`, `+12.5`, `-3` — a decimal and nothing else.
///
/// Hand-rolled rather than a regular expression, and deliberately narrow: this
/// is the check that decides whether a figure may be trusted at all, so a
/// string with anything else in it is a reply to be refused rather than
/// salvaged.
fn is_decimal(text: &str) -> bool {
    let rest = text.strip_prefix(['+', '-']).unwrap_or(text);
    if rest.is_empty() {
        return false;
    }

    let (whole, fraction) = match rest.split_once('.') {
        Some((whole, fraction)) => (whole, Some(fraction)),
        None => (rest, None),
    };

    let digits = |text: &str| text.chars().all(|c| c.is_ascii_digit());

    match fraction {
        None => !whole.is_empty() && digits(whole),
        Some(fraction) => {
            // A second point is not a decimal, and neither is a lone ".".
            !fraction.contains('.')
                && digits(whole)
                && digits(fraction)
                && !(whole.is_empty() && fraction.is_empty())
        }
    }
}

/// The month's grant, and what can still be spent.
fn reading(json: &Value) -> ProviderUsage {
    const ID: &str = "nous-portal";
    const NAME: &str = "Nous Portal";

    if !json.is_object() {
        return ProviderUsage::failed(ID, NAME, "unreadable reply");
    }
    if json.get("error").is_some_and(|error| !error.is_null()) {
        return ProviderUsage::failed(ID, NAME, "the portal returned an error");
    }

    let none = Value::Object(serde_json::Map::new());
    let subscription = json.get("subscription").unwrap_or(&none);
    let access = json.get("paid_service_access").unwrap_or(&none);

    let mut malformed = false;
    let mut read = |value: Option<&Value>| -> Option<f64> {
        match amount(value) {
            Amount::Missing => None,
            Amount::Value(number) => Some(number),
            Amount::Malformed => {
                malformed = true;
                None
            }
        }
    };

    let monthly = read(subscription.get("monthly_credits"));
    let remaining = read(subscription.get("credits_remaining"))
        .or_else(|| read(access.get("subscription_credits_remaining")));
    let purchased = read(json.get("purchased_credits_remaining"))
        .or_else(|| read(access.get("purchased_credits_remaining")));
    let total = read(access.get("total_usable_credits"));
    let rollover = read(subscription.get("rollover_credits"));

    if malformed || [monthly, remaining, purchased, total, rollover]
        .iter()
        .all(Option::is_none)
    {
        return ProviderUsage::failed(ID, NAME, "unreadable reply");
    }

    // The month's grant, as the portal states both its size and what is left of
    // it. A plan with no grant — the free tier — draws no ring rather than a
    // zero.
    let mut windows = Vec::new();
    if let (Some(monthly), Some(remaining)) = (monthly, remaining) {
        if monthly > 0.0 {
            let used = 0.0_f64.max(monthly - 0.0_f64.max(remaining));
            windows.push(
                UsageWindow::new("Monthly", Some(percent_from_fraction(used / monthly)))
                    // A billing cycle, not a stated length: a sort key only,
                    // and no reset is claimed beyond the end the portal states.
                    .with_reset(
                        subscription
                            .get("current_period_end")
                            .and_then(Value::as_str)
                            .and_then(parse_iso)
                            .map(|at| at.to_rfc3339_opts(chrono::SecondsFormat::Secs, true)),
                    ),
            );
        }
    }

    // What can still be spent, the grant and top-ups together; the top-ups
    // alone when that is all the portal says.
    let balance = total.or(purchased);
    if windows.is_empty() && balance.is_none() {
        return ProviderUsage::failed(ID, NAME, "no limits reported");
    }
    if let Some(balance) = balance {
        windows.push(super::balance_window(
            "Balance",
            format!("{balance:.2} USD"),
        ));
    }

    let plan = subscription
        .get("plan")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|plan| !plan.is_empty())
        .map(str::to_string);

    let mut usage = ProviderUsage::ok(ID, NAME, windows).with_plan(plan);
    if let Some(balance) = balance { usage = usage.with_credit_remaining(balance, "USD"); }
    usage
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn at(text: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(text).unwrap().with_timezone(&Utc)
    }

    /// The account reply, as CodexBar's tests describe it.
    fn fixture() -> Value {
        json!({
            "subscription": {
                "plan": "Pro",
                "monthly_credits": 20.0,
                "credits_remaining": 5.0,
                "rollover_credits": 1.0,
                "current_period_end": "2026-11-01T00:00:00Z"
            },
            "paid_service_access": { "total_usable_credits": 8.5 }
        })
    }

    #[test]
    fn reads_the_months_grant_and_what_is_left_to_spend() {
        let usage = reading(&fixture());
        assert_eq!(usage.credit_remaining.as_ref().unwrap().amount, 8.5);
        assert_eq!(usage.credit_remaining.as_ref().unwrap().currency, "USD");
        let labels: Vec<&str> = usage.windows.iter().map(|w| w.label.as_str()).collect();

        assert_eq!(labels, vec!["Monthly", "Balance"]);
        // 20 granted, 5 left: three quarters gone.
        assert_eq!(usage.windows[0].percent_used, Some(75.0));
        assert_eq!(
            usage.windows[0].resets_at.as_deref(),
            Some("2026-11-01T00:00:00Z")
        );
        assert_eq!(usage.windows[1].detail.as_deref(), Some("8.50 USD"));
        assert_eq!(usage.windows[1].percent_used, None);
        assert_eq!(usage.plan.as_deref(), Some("Pro"));
    }

    #[test]
    fn a_plan_with_no_grant_draws_no_ring_rather_than_a_zero() {
        let free = json!({
            "subscription": { "monthly_credits": 0.0, "credits_remaining": 0.0 },
            "paid_service_access": { "total_usable_credits": 3.0 }
        });
        let usage = reading(&free);
        assert_eq!(usage.windows.len(), 1);
        assert_eq!(usage.windows[0].label, "Balance");

        // Nothing at all to draw: no limits, not a ring at zero.
        let empty = json!({ "subscription": { "monthly_credits": 0.0 } });
        assert!(reading(&empty).error.is_some());
    }

    #[test]
    fn a_balance_is_the_total_where_it_is_stated_and_the_top_ups_otherwise() {
        let purchased = json!({
            "subscription": { "monthly_credits": 10.0, "credits_remaining": 10.0 },
            "purchased_credits_remaining": 4.25
        });
        assert_eq!(
            reading(&purchased).windows[1].detail.as_deref(),
            Some("4.25 USD")
        );

        // The nested spelling is the fallback for both.
        let nested = json!({
            "paid_service_access": {
                "subscription_credits_remaining": 6.0,
                "purchased_credits_remaining": 2.0,
                "total_usable_credits": 8.0
            }
        });
        assert_eq!(reading(&nested).windows[0].detail.as_deref(), Some("8.00 USD"));
    }

    #[test]
    fn a_figure_that_is_not_a_decimal_is_a_reply_to_refuse() {
        let bad = json!({ "subscription": { "monthly_credits": "twenty" } });
        assert!(reading(&bad).error.is_some());

        let boolean = json!({ "subscription": { "monthly_credits": true } });
        assert!(reading(&boolean).error.is_some());

        // A decimal written as a string is a figure, signed or not.
        let signed = json!({
            "subscription": { "monthly_credits": "+20", "credits_remaining": "5." }
        });
        assert_eq!(reading(&signed).windows[0].percent_used, Some(75.0));

        assert!(is_decimal("12"));
        assert!(is_decimal(".5"));
        assert!(is_decimal("-3."));
        assert!(!is_decimal(""));
        assert!(!is_decimal("."));
        assert!(!is_decimal("1.2.3"));
        assert!(!is_decimal("1e9"));
        assert!(!is_decimal("$12"));
    }

    #[test]
    fn a_stated_error_is_the_portals_not_a_reading() {
        let failed = json!({ "error": "invalid_token", "subscription": { "monthly_credits": 1.0 } });
        assert!(reading(&failed).error.is_some());
        // Explicit null is not an error.
        let ok = json!({ "error": null, "subscription": { "monthly_credits": 0.0 }, "purchased_credits_remaining": 1.0 });
        assert!(reading(&ok).error.is_none());
        assert!(reading(&json!("nope")).error.is_some());
    }

    #[test]
    fn the_portal_is_only_trusted_when_it_is_nouss_own_over_https() {
        let trusted = |raw: &str| trusted_portal(Some(raw));

        assert_eq!(
            trusted("https://portal.nousresearch.com/").as_deref(),
            Some("https://portal.nousresearch.com")
        );
        assert_eq!(
            trusted("https://portal.nousresearch.com").as_deref(),
            Some("https://portal.nousresearch.com")
        );
        // Anything that is not Nous's own host, or not https, is ignored — and
        // the default portal is asked instead.
        assert!(trusted("https://portal.nousresearch.com.evil.test").is_none());
        assert!(trusted("http://portal.nousresearch.com").is_none());
        assert!(trusted("https://evil.test").is_none());
        assert!(trusted("https://user:pw@portal.nousresearch.com").is_none());
        assert!(trusted("https://portal.nousresearch.com:8443").is_none());
        assert!(trusted("https://portal.nousresearch.com/api").is_none());
        assert!(trusted("https://portal.nousresearch.com?x=1").is_none());
        assert!(trusted("").is_none());
        assert!(trusted_portal(None).is_none());
    }

    #[test]
    fn a_login_is_read_from_any_of_the_three_shapes_hermes_writes() {
        let providers = json!({
            "providers": { "nous": { "access_token": "from-providers" } }
        });
        assert_eq!(login_from(&providers).unwrap().token, "from-providers");

        let pool = json!({
            "credential_pool": { "nous": [
                { "access_token": "older", "expires_at": "2026-10-02T01:00:00Z" },
                { "access_token": "newer", "expires_at": "2026-10-02T03:00:00Z" }
            ] }
        });
        assert_eq!(login_from(&pool).unwrap().token, "newer");

        let state = json!({ "access_token": "on-its-own" });
        assert_eq!(login_from(&state).unwrap().token, "on-its-own");

        assert!(login_from(&json!({ "access_token": "   " })).is_none());
        assert!(login_from(&json!({})).is_none());
    }

    #[test]
    fn the_portal_falls_back_to_nouss_own_when_the_file_names_another() {
        let edited = json!({
            "access_token": "t",
            "portal_base_url": "https://evil.test"
        });
        assert_eq!(login_from(&edited).unwrap().portal, DEFAULT_PORTAL);
    }

    #[test]
    fn a_login_that_has_lapsed_is_said_to_have_lapsed() {
        // An hour-long token, minted a minute ago, is still good.
        let expiry = at("2026-10-02T02:00:00Z");
        let good = json!({ "access_token": "t", "expires_at": "2026-10-02T02:00:00Z" });
        assert!(login_from(&good).unwrap().expires_at == Some(expiry));

        // A token already inside the skew is treated as expired, so no request
        // ever races the portal's clock.
        let soon = json!({ "access_token": "t", "expires_at": "2026-10-02T01:00:30Z" });
        let login = login_from(&soon).unwrap();
        assert!(login.expires_at.unwrap() - at("2026-10-02T01:00:00Z") <= EXPIRY_SKEW);
    }

    #[test]
    fn a_token_with_no_stated_expiry_is_dated_from_its_own_claim() {
        // {"exp":1760000000} — a JWT payload, base64url, with no padding.
        let payload = base64url_encode(br#"{"exp":1760000000}"#);
        let token = format!("header.{payload}.signature");

        let expiry = jwt_expiry(&token).unwrap();
        assert_eq!(expiry.timestamp(), 1_760_000_000);

        assert!(jwt_expiry("not-a-jwt").is_none());
        assert!(jwt_expiry("a.b").is_none());
        // A payload that is not JSON, and one with no `exp`.
        assert!(jwt_expiry(&format!("h.{}.s", base64url_encode(b"nope"))).is_none());
        assert!(jwt_expiry(&format!("h.{}.s", base64url_encode(b"{\"sub\":\"x\"}"))).is_none());
    }

    /// The inverse of `base64url`, for building a token in a test.
    fn base64url_encode(bytes: &[u8]) -> String {
        const ALPHABET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
        let mut out = String::new();
        for chunk in bytes.chunks(3) {
            let mut block = [0u8; 3];
            block[..chunk.len()].copy_from_slice(chunk);
            let value = u32::from(block[0]) << 16 | u32::from(block[1]) << 8 | u32::from(block[2]);
            for index in 0..(chunk.len() + 1) {
                let shift = 18 - index * 6;
                out.push(ALPHABET[((value >> shift) & 0x3f) as usize] as char);
            }
        }
        out
    }
}
