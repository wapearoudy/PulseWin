//! LongCat's API platform: the account's token allowance, and the fuel packs
//! (加油包) bought on top of it, each reported as tokens used or left out of a
//! stated total.
//!
//! Read from the routes the platform's usage page calls, with the pasted
//! `Cookie` header: `GET /api/v1/user-current` to prove the session,
//! `POST /api/pay/quota/metering/token-packs/summary` for an active token pack,
//! `GET /api/lc-platform/v1/tokenUsage` for the allowance when there is no such
//! pack, and `GET /api/lc-platform/v1/pending-fuel-packages` for the fuel
//! packs. The original imports the session out of a Chromium browser's cookie
//! store through the macOS login keychain, which PulseWin cannot; only the
//! Meituan passport token and the account id beside it are kept out of it.
//!
//! **Only figures the platform states.** The allowance needs a total and either
//! the tokens used or the tokens left; the fuel packs need their total and at
//! least one pack's tokens left. The original's neighbour fills a missing used
//! figure with zero and a missing remainder with the whole pack, which draws a
//! full ring nobody reported; here that row is left off.
//!
//! Neither states a length or a reset, so neither claims one: the allowance is
//! a credit allowance and the packs a top-up. The original hangs the packs'
//! soonest expiry on the row as an expiry and never as a reset, and this
//! port's window has nowhere to carry one — so the row is drawn without it
//! rather than with a reset the platform never stated.
//!
//! The shape is second-hand — taken from the original and its fixtures, not
//! from a captured reply.

use std::sync::Arc;

use serde_json::Value;

use super::{describe_reqwest_error, percent_from_fraction, Ctx, FetchFuture, Provider};
use crate::model::{ProviderUsage, UsageWindow};

const ID: &str = "long-cat";
const NAME: &str = "LongCat";

const ORIGIN: &str = "https://longcat.chat";
const USER_PATH: &str = "/api/v1/user-current";
const TOKEN_PACKS_PATH: &str = "/api/pay/quota/metering/token-packs/summary";
const TOKEN_USAGE_PATH: &str = "/api/lc-platform/v1/tokenUsage";
const FUEL_PATH: &str = "/api/lc-platform/v1/pending-fuel-packages";

/// The passport token first, which has to be there, and the account id beside
/// it. Nothing else leaves the browser.
const COOKIES: [&str; 2] = ["passport_token", "uid"];

pub struct LongCat;

impl Provider for LongCat {
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
        return ProviderUsage::failed(ID, NAME, super::pasted::needed(ID, "LongCat session cookie"));
    };

    // The one call that has to succeed before anything is believed: a session
    // the platform no longer takes answers here, not with zeros further on.
    if let Err(reason) = ask(&ctx, USER_PATH, false, &cookie).await {
        return ProviderUsage::failed(ID, NAME, reason);
    }

    // Best effort: some sessions are not let into this route at all.
    let packs = ask(&ctx, TOKEN_PACKS_PATH, true, &cookie).await.ok();

    let mut token_usage = None;
    if active_lot(packs.as_ref()).is_none() {
        match ask(&ctx, TOKEN_USAGE_PATH, false, &cookie).await {
            Ok(payload) => token_usage = Some(payload),
            Err(reason) => return ProviderUsage::failed(ID, NAME, reason),
        }
    }

    let fuel = ask(&ctx, FUEL_PATH, false, &cookie).await.ok();

    reading(packs.as_ref(), token_usage.as_ref(), fuel.as_ref())
}

/// One route, and the unwrapped `data` of its reply.
///
/// Made on the client that refuses redirects, so an expired session's bounce to
/// a sign-in page is seen as one and never followed with the cookie attached.
async fn ask(ctx: &Ctx, path: &str, post: bool, cookie: &str) -> Result<Value, String> {
    let url = format!("{ORIGIN}{path}");
    let mut request = ctx
        .gateway_client
        .request(if post { reqwest::Method::POST } else { reqwest::Method::GET }, &url)
        .header("Cookie", cookie)
        .header("Accept", "application/json, text/plain, */*")
        .header("Origin", ORIGIN)
        .header("Referer", format!("{ORIGIN}/platform/usage"));

    if post {
        request = request
            .header("Content-Type", "application/json")
            .body("{}");
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

    if status.is_redirection() {
        return Err(expired());
    }
    if !status.is_success() {
        let hints: &[(u16, &str)] = &[
            (401, " — the session has expired; copy a fresh cookie from longcat.chat"),
            (403, " — the session has expired; copy a fresh cookie from longcat.chat"),
            (429, " — rate limited, try again shortly"),
        ];
        return Err(super::http_failure(status, hints));
    }

    payload(&body)
}

/// The platform's envelope, `{ code, message, data }`: the `data` of a code
/// that is a success, or the reason there is none.
fn payload(body: &str) -> Result<Value, String> {
    let envelope: Value = serde_json::from_str(body).map_err(|_| unreadable())?;
    if !envelope.is_object() {
        return Err(unreadable());
    }

    if let Some(raw) = envelope.get("code") {
        let code = super::dig_number(Some(raw))
            .filter(|code| code.is_finite() && code.fract() == 0.0)
            .filter(|code| *code >= i64::MIN as f64 && *code <= i64::MAX as f64)
            .ok_or_else(unreadable)?;
        match code as i64 {
            0 | 200 => {}
            401 | 403 => return Err(expired()),
            _ => return Err("the service reported an error".to_string()),
        }
    }

    // A `data` that is there but is not an object is not fell back from: the
    // envelope said where its payload is, and it was not one.
    let data = envelope.get("data").unwrap_or(&envelope);
    if data.is_object() {
        Ok(data.clone())
    } else {
        Err(unreadable())
    }
}

fn expired() -> String {
    "the session has expired; copy a fresh cookie from longcat.chat".to_string()
}

fn unreadable() -> String {
    "the reply could not be read".to_string()
}

// ---------------------------------------------------------------------------
// Reading the replies
// ---------------------------------------------------------------------------

/// The windows, from the unwrapped `data` of each route. `None` for a route not
/// asked or not answered.
fn reading(packs: Option<&Value>, token_usage: Option<&Value>, fuel: Option<&Value>) -> ProviderUsage {
    let mut windows: Vec<UsageWindow> = Vec::new();

    if let Some(lot) = active_lot(packs) {
        if let (Some(total), Some(used)) = (
            number(lot, "totalToken"),
            number(lot, "consumedToken"),
        ) {
            if used >= 0.0 {
                windows.push(allowance(used, total));
            }
        }
    } else if let Some(token_usage) = token_usage {
        // The account-wide figure; `extData` beside it is per model.
        let usage = token_usage
            .get("usage")
            .filter(|value| value.is_object())
            .unwrap_or(token_usage);

        // The route has to state a total at all; one that does not is not a
        // reply this reads, and saying "no limits" would hide that.
        let Some(total) = number(usage, "totalToken") else {
            return ProviderUsage::failed(ID, NAME, unreadable());
        };
        if total > 0.0 {
            let used = number(usage, "usedToken")
                .or_else(|| number(usage, "availableToken").map(|available| total - available));
            if let Some(used) = used.filter(|used| *used >= 0.0) {
                windows.push(allowance(used, total));
            }
        }
    }

    if let Some(window) = fuel.and_then(fuel_window) {
        windows.push(window);
    }

    if windows.is_empty() {
        return ProviderUsage::failed(ID, NAME, "no limits reported in the reply");
    }

    ProviderUsage::ok(ID, NAME, windows)
}

/// A token pack counts only while the platform calls it active and gives it a
/// size; an expired or empty one leaves the allowance to the older route, which
/// is what the platform's page does.
fn active_lot(packs: Option<&Value>) -> Option<&Value> {
    let lot = packs?.get("currentLot")?;
    let active = lot
        .get("status")
        .and_then(Value::as_str)
        .is_some_and(|status| status.to_uppercase() == "ACTIVE");
    if !active {
        return None;
    }
    number(lot, "totalToken").filter(|total| *total > 0.0)?;
    Some(lot)
}

/// A share used out of a stated total. Neither route states a length or a
/// reset, so neither claims one.
fn allowance(used: f64, total: f64) -> UsageWindow {
    UsageWindow::new("Credits", Some(percent_from_fraction(used / total)))
}

/// The fuel packs as one top-up: their stated total, less what each pack says
/// it has left.
fn fuel_window(fuel: &Value) -> Option<UsageWindow> {
    let total = number(fuel, "totalQuota").filter(|total| *total > 0.0)?;

    let packs: Vec<&Value> = fuel
        .get("list")
        .and_then(Value::as_array)
        .map(|items| items.iter().collect())
        .unwrap_or_default();
    let left: Vec<f64> = packs
        .iter()
        .filter_map(|pack| number(pack, "availableToken"))
        .filter(|left| *left >= 0.0)
        .collect();
    if left.is_empty() {
        return None;
    }

    let remaining: f64 = left.iter().sum();
    let used = (total - remaining).max(0.0);
    Some(UsageWindow::new(
        "Top-up",
        Some(percent_from_fraction(used / total)),
    ))
}

/// A number, or a string that is one. Never a boolean.
fn number(value: &Value, key: &str) -> Option<f64> {
    super::dig_number(value.get(key)).filter(|number| number.is_finite())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Second-hand, from the original's fixtures: the replies CodexBar's
    /// LongCat provider describes, which is what these routes are read against.
    fn token_packs() -> Value {
        serde_json::from_str(
            r#"{"code":0,"message":"success","data":{"currentLot":{"totalToken":50000000,"consumedToken":1212576,"consumedRatio":0.02425152,"status":"ACTIVE"}}}"#,
        )
        .unwrap()
    }

    fn fuel_packages() -> Value {
        serde_json::from_str(
            r#"{"code":0,"message":"success","data":{"totalQuota":1000,"list":[{"availableToken":600,"expireTime":1750000000000},{"availableToken":150,"expireTime":"2025-10-09 12:00:00"}]}}"#,
        )
        .unwrap()
    }

    fn token_usage() -> Value {
        serde_json::from_str(
            r#"{"code":0,"message":"success","data":{"usage":{"totalToken":500000,"usedToken":120000,"availableToken":380000,"freeAvailableToken":380000},"extData":{"LongCat-Flash-Lite":{"totalToken":50000000,"usedToken":0}}}}"#,
        )
        .unwrap()
    }

    /// Every fixture, as the fetch unwraps it.
    fn data(body: Value) -> Value {
        payload(&body.to_string()).unwrap()
    }

    #[test]
    fn reads_an_active_token_pack_and_the_fuel_packs_over_their_stated_totals() {
        let usage = reading(
            Some(&data(token_packs())),
            None,
            Some(&data(fuel_packages())),
        );

        let labels: Vec<&str> = usage.windows.iter().map(|w| w.label.as_str()).collect();
        assert_eq!(labels, vec!["Credits", "Top-up"]);
        assert_eq!(
            usage.windows[0].percent_used,
            Some(percent_from_fraction(1_212_576.0 / 50_000_000.0))
        );
        // 1,000 in all, 600 + 150 left.
        assert_eq!(usage.windows[1].percent_used, Some(25.0));
        // Neither route states a reset, so neither claims one.
        assert!(usage.windows.iter().all(|w| w.resets_at.is_none()));
    }

    #[test]
    fn without_an_active_pack_the_older_routes_allowance_is_read() {
        let expired = data(serde_json::json!({
            "code": 0,
            "data": { "currentLot": { "totalToken": 50_000_000i64, "status": "EXPIRED" } }
        }));
        let usage = reading(Some(&expired), Some(&data(token_usage())), None);

        assert_eq!(usage.windows.len(), 1);
        assert_eq!(usage.windows[0].label, "Credits");
        assert_eq!(usage.windows[0].percent_used, Some(24.0));
    }

    #[test]
    fn tokens_left_stand_in_for_tokens_used_when_both_come_with_the_total() {
        let remainder = data(serde_json::json!({
            "code": 0,
            "data": { "usage": { "totalToken": 500_000, "availableToken": 380_000 } }
        }));
        let usage = reading(None, Some(&remainder), None);
        assert_eq!(usage.windows[0].percent_used, Some(24.0));
    }

    #[test]
    fn a_figure_that_is_not_reported_is_left_off_never_filled_with_zero() {
        // An active pack with a total but no consumed figure, and a fuel pack
        // with an expiry but no remainder.
        let no_used = data(serde_json::json!({
            "code": 0,
            "data": { "currentLot": { "totalToken": 1000, "status": "ACTIVE" } }
        }));
        let no_left = data(serde_json::json!({
            "code": 0,
            "data": { "totalQuota": 1000, "list": [{ "expireTime": 1_750_000_000_000i64 }] }
        }));
        assert!(reading(Some(&no_used), None, Some(&no_left)).error.is_some());
    }

    #[test]
    fn the_older_route_without_a_total_is_not_a_reply_this_reads() {
        let no_total = data(serde_json::json!({
            "code": 0,
            "data": { "usage": { "usedToken": 120_000 } }
        }));
        let usage = reading(None, Some(&no_total), None);
        assert_eq!(usage.error.as_deref(), Some("the reply could not be read"));
    }

    #[test]
    fn the_envelopes_code_decides() {
        // A session the platform no longer takes.
        for body in [r#"{"code":401,"message":"unauthorized"}"#, r#"{"code":403}"#] {
            assert!(payload(body).unwrap_err().contains("expired"), "read {body}");
        }
        assert!(payload(r#"{"code":500,"message":"busy"}"#)
            .unwrap_err()
            .contains("service"));
        // A code that is not a whole number is not one this reads.
        assert!(payload(r#"{"code":"x1"}"#).is_err());
        assert!(payload(r#"{"code":0,"data":[]}"#).is_err());
        assert!(payload("<html>").is_err());
        // A success code hands over its data.
        assert_eq!(
            payload(r#"{"code":0,"data":{"name":"Leo"}}"#).unwrap()["name"],
            serde_json::json!("Leo")
        );
    }

    /// Only the named cookies leave the browser, and not without the passport
    /// token.
    #[test]
    fn only_the_named_cookies_are_kept() {
        let kept = super::super::pasted::keep("passport_token=t; uid=42; _ga=x", &COOKIES);
        assert_eq!(kept.as_deref(), Some("passport_token=t; uid=42"));
        assert_eq!(super::super::pasted::keep("uid=42", &COOKIES), None);
    }

    #[test]
    fn a_pack_the_platform_does_not_call_active_is_not_the_allowance() {
        let ended = serde_json::json!({
            "currentLot": { "totalToken": 50_000_000i64, "consumedToken": 1, "status": "ended" }
        });
        assert!(active_lot(Some(&ended)).is_none());
        let empty = serde_json::json!({ "currentLot": { "totalToken": 0, "status": "ACTIVE" } });
        assert!(active_lot(Some(&empty)).is_none());
        assert!(active_lot(None).is_none());
    }
}
