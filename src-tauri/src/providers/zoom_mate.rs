//! ZoomMate, Zoom's AI assistant: one credit allowance against a budget cap,
//! for a billing cycle whose start and end the service states.
//!
//! Read the way ZoomMate's own web client does: the session is exchanged for a
//! short-lived bearer token at `GET /ai-computer/api/v1/login/`, and that token
//! reads `GET /ai-computer/api/v1/credits/status`. Both are on `ai.zoom.us`,
//! with `zoommate.zoom.us` — which serves the same API — tried when the first
//! does not answer. The token is held for the one refresh and never stored;
//! nothing is written anywhere.
//!
//! **The credential is a pasted cookie.** The original imports Zoom's session
//! out of a Chromium browser's cookie store through the macOS login keychain,
//! which PulseWin cannot, so the `Cookie` header copied out of a signed-in
//! request is the credential here — kept to Zoom's session cookie and
//! Cloudflare's clearance beside it, which is all the API is given.
//!
//! **Redirects are not followed**: a `Cookie` header set by hand is carried
//! across a redirect to any host, so a signed-out session arrives as the
//! redirect to Zoom's sign-in page rather than being followed there.
//!
//! Left out: the credit history and the pace built on it. That is spend over
//! thirty days, which this port has no place for, and pace is the panel's own
//! work from the cycle it already draws.
//!
//! The shapes are second-hand — taken from the original and its fixtures, not
//! from a captured reply — and so are the cookie names.

use std::sync::Arc;

use serde_json::Value;

use super::{describe_reqwest_error, percent_from_fraction, Ctx, FetchFuture, Provider};
use crate::model::{ProviderUsage, UsageWindow};

const ID: &str = "zoom-mate";
const NAME: &str = "ZoomMate";

/// The two hosts that serve ZoomMate's API, in the order its web client uses
/// them. Nothing is sent anywhere else.
const HOSTS: [&str; 2] = ["ai.zoom.us", "zoommate.zoom.us"];
const ORIGIN: &str = "https://zoommate.zoom.us";

/// Zoom's session cookie, and Cloudflare's clearance beside it. Both are set on
/// the parent `zoom.us`, which is why that is the host read. The session is the
/// one that has to be there.
const COOKIES: [&str; 2] = ["_zm_ssid", "cf_clearance"];

pub struct ZoomMate;

impl Provider for ZoomMate {
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

/// Why a host did not answer, in the terms the retry is decided in.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Refusal {
    /// The host did not answer at all, or answered with something that is not
    /// this API's: the other host may do better.
    Unreachable(String),
    ServerError,
    /// The session was turned away, or the reply could not be read. Both hosts
    /// would say the same, so neither is asked twice.
    Settled(String),
}

impl Refusal {
    fn message(&self) -> String {
        match self {
            Refusal::Unreachable(why) => format!("request failed: {why}"),
            Refusal::ServerError => "the service reported an error".to_string(),
            Refusal::Settled(reason) => reason.clone(),
        }
    }

    fn worth_another_host(&self) -> bool {
        !matches!(self, Refusal::Settled(_))
    }
}

fn session_expired() -> Refusal {
    Refusal::Settled(
        "the session has expired; copy a fresh cookie from zoom.us".to_string(),
    )
}

fn unreadable() -> Refusal {
    Refusal::Settled("the reply could not be read".to_string())
}

async fn fetch_inner(ctx: Arc<Ctx>) -> ProviderUsage {
    let Some(cookie) = super::pasted::cookie(ID).and_then(|header| super::pasted::keep(&header, &COOKIES))
    else {
        return ProviderUsage::failed(ID, NAME, super::pasted::needed(ID, "Zoom session cookie"));
    };

    let mut last = Refusal::Unreachable("no host was asked".to_string());
    for host in HOSTS {
        match status(&ctx, host, &cookie).await {
            Ok(body) => return reading(&body),
            // Only a host that did not answer is worth asking the other one
            // about. A refused session or an unreadable reply would be the same
            // on both.
            Err(refusal) if refusal.worth_another_host() => last = refusal,
            Err(refusal) => return ProviderUsage::failed(ID, NAME, refusal.message()),
        }
    }

    ProviderUsage::failed(ID, NAME, last.message())
}

/// The session exchanged for a token, then the token for the status — both on
/// the same host.
async fn status(ctx: &Ctx, host: &str, cookie: &str) -> Result<String, Refusal> {
    // Written out rather than built by a query encoder: `:` and `/` are legal
    // in a query, and this is the address the web client itself asks.
    let login = format!("https://{host}/ai-computer/api/v1/login/?continue={ORIGIN}/");
    let login_reply = get(ctx, &login, cookie, None).await?;
    let token = token(&login_reply)?;

    get(
        ctx,
        &format!("https://{host}/ai-computer/api/v1/credits/status"),
        cookie,
        Some(&token),
    )
    .await
}

async fn get(ctx: &Ctx, url: &str, cookie: &str, token: Option<&str>) -> Result<String, Refusal> {
    let mut request = ctx
        .gateway_client
        .get(url)
        .header("Accept", "application/json, text/plain, */*")
        .header("Origin", ORIGIN)
        .header("Referer", ORIGIN)
        .header("Cookie", cookie);

    if let Some(token) = token {
        request = request.header("Authorization", format!("Bearer {token}"));
    }

    let response = request
        .send()
        .await
        .map_err(|e| Refusal::Unreachable(describe_reqwest_error(&e)))?;

    let status = response.status();
    let body = response
        .text()
        .await
        .map_err(|e| Refusal::Unreachable(format!("cannot read body: {e}")))?;

    // Redirects are refused, so a signed-out session arrives as the redirect to
    // Zoom's sign-in page.
    if status.is_redirection() {
        return Err(session_expired());
    }
    if !status.is_success() {
        return match status.as_u16() {
            401 | 403 => Err(session_expired()),
            429 => Err(Refusal::Settled(super::http_failure(
                status,
                &[(429, " — rate limited, try again shortly")],
            ))),
            // Anything else the host says is the host's own trouble, and the
            // other one is worth asking.
            _ => Err(Refusal::ServerError),
        };
    }

    Ok(body)
}

// ---------------------------------------------------------------------------
// Reading the replies
// ---------------------------------------------------------------------------

/// The bearer token is `data.nak`. A reply that says it failed is the session
/// being turned away; anything else is a reply this build cannot read.
fn token(body: &str) -> Result<String, Refusal> {
    let Ok(reply) = serde_json::from_str::<Value>(body) else {
        return Err(unreadable());
    };
    if !reply.is_object() {
        return Err(unreadable());
    }

    let nak = reply
        .get("data")
        .and_then(|data| data.get("nak"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|nak| !nak.is_empty());
    if let Some(nak) = nak {
        return Ok(nak.to_string());
    }

    match reply.get("success").and_then(Value::as_bool) {
        Some(false) => Err(session_expired()),
        _ => Err(unreadable()),
    }
}

/// The mapping, kept apart from the request so a fixture can drive it.
fn reading(body: &str) -> ProviderUsage {
    let Ok(reply) = serde_json::from_str::<Value>(body) else {
        return ProviderUsage::failed(ID, NAME, unreadable().message());
    };

    let Some(status) = reply.get("data").and_then(|data| data.get("credit_status")) else {
        return ProviderUsage::failed(ID, NAME, unreadable().message());
    };
    if !status.is_object() {
        return ProviderUsage::failed(ID, NAME, unreadable().message());
    }

    // Unlimited has no cap to measure against, and a cap of nothing is not one
    // either. Neither is drawn as 0%.
    let unlimited = status.get("is_unlimited").and_then(Value::as_bool) == Some(true);
    let cap = figure(status, "budget_cap").filter(|cap| *cap > 0.0);
    let used = match figure(status, "used_credit") {
        Some(used) => Some(used),
        None => match (cap, figure(status, "remaining_credit")) {
            (Some(cap), Some(remaining)) => Some(cap - remaining),
            _ => None,
        },
    }
    .filter(|used| *used >= 0.0);

    let (Some(cap), Some(used)) = (cap, used) else {
        return ProviderUsage::failed(ID, NAME, "no limits reported in the reply");
    };
    if unlimited {
        return ProviderUsage::failed(ID, NAME, "no limits reported in the reply");
    }

    // The cycle ends in epoch milliseconds, and that is where the allowance
    // comes back. Its length is the cycle's own — the original reads the start
    // only to state it — and this port's window carries no length, so only the
    // reset is read.
    let reset = milliseconds(status.get("cycle_end_date")).and_then(super::stamp_from_epoch);

    ProviderUsage::ok(
        ID,
        NAME,
        vec![UsageWindow::new(
            "Credits",
            Some(percent_from_fraction(used / cap)),
        )
        .with_reset(reset)],
    )
}

/// A figure the service reports, refusing a boolean the way every provider here
/// does.
fn figure(status: &Value, key: &str) -> Option<f64> {
    super::dig_number(status.get(key)).filter(|figure| figure.is_finite())
}

/// Epoch milliseconds, as the cycle's ends are stated.
fn milliseconds(value: Option<&Value>) -> Option<f64> {
    super::dig_number(value).filter(|milliseconds| milliseconds.is_finite() && *milliseconds > 0.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Second-hand, from the original's fixture: the status CodexBar's ZoomMate
    /// provider describes, which is what the route is read against.
    fn fixture() -> String {
        r#"{"data":{"credit_status":{"budget_cap":12345.0,"used_credit":678.0,"remaining_credit":11667.0,"overage_credit":0.0,"allow_overage":false,"cycle_start_date":1893456000000,"cycle_end_date":1896134399000,"is_quota_available":true,"is_unlimited":false}},"status_code":200,"error_message":null}"#
            .to_string()
    }

    /// One `credit_status` object, as the service nests it.
    fn status(fields: &str) -> String {
        format!(r#"{{"data":{{"credit_status":{{{fields}}}}}}}"#)
    }

    #[test]
    fn reads_the_credits_used_over_the_budget_cap_for_the_cycle_stated() {
        let usage = reading(&fixture());

        assert_eq!(usage.windows.len(), 1);
        assert_eq!(usage.windows[0].label, "Credits");
        assert_eq!(
            usage.windows[0].percent_used,
            Some(percent_from_fraction(678.0 / 12_345.0))
        );
        // Both ends of the cycle are stated, and the reset is where it ends.
        assert_eq!(
            usage.windows[0].resets_at.as_deref(),
            Some("2030-01-31T23:59:59Z")
        );
    }

    #[test]
    fn the_reset_is_where_the_cycle_ends() {
        let usage = reading(&status(
            r#""budget_cap":100,"used_credit":10,"cycle_end_date":1896134399000"#,
        ));
        assert_eq!(
            usage.windows[0].resets_at.as_deref(),
            Some("2030-01-31T23:59:59Z")
        );
        // A cycle with no stated end claims no reset.
        assert!(reading(&status(r#""budget_cap":100,"used_credit":10"#)).windows[0]
            .resets_at
            .is_none());
    }

    #[test]
    fn used_is_taken_from_what_is_left_of_the_cap_when_only_that_is_reported() {
        let usage = reading(&status(r#""budget_cap":200,"remaining_credit":150"#));
        assert_eq!(usage.windows[0].percent_used, Some(25.0));
    }

    #[test]
    fn past_the_cap_reads_full_rather_than_past_the_ring() {
        let over = reading(&status(
            r#""budget_cap":100,"used_credit":120,"allow_overage":false"#,
        ));
        assert_eq!(over.windows[0].percent_used, Some(100.0));
    }

    #[test]
    fn unlimited_a_cap_of_nothing_or_a_figure_that_is_not_one_draws_nothing() {
        for fields in [
            r#""budget_cap":100,"used_credit":10,"is_unlimited":true"#,
            r#""budget_cap":0,"used_credit":10"#,
            r#""used_credit":10"#,
            r#""budget_cap":100"#,
            r#""budget_cap":100,"used_credit":-5"#,
        ] {
            let usage = reading(&status(fields));
            assert!(usage.error.is_some(), "read {fields}");
        }
    }

    #[test]
    fn a_reply_without_a_credit_status_cannot_be_read() {
        for reply in ["not json", "{}", r#"{"data":{}}"#, r#"{"data":{"credit_status":[]}}"#] {
            assert!(reading(reply).error.is_some(), "read {reply}");
        }
    }

    /// The session is exchanged for the token in `data.nak`.
    #[test]
    fn the_session_is_exchanged_for_the_token() {
        let login = r#"{"success":true,"data":{"nak":"fake-minted-jwt","user_profile":{"email":"person@example.com"}}}"#;
        assert_eq!(token(login).as_deref(), Ok("fake-minted-jwt"));
    }

    #[test]
    fn an_exchange_that_says_it_failed_is_the_session_turned_away() {
        let refused = token(r#"{"success":false}"#).unwrap_err();
        assert_eq!(refused, session_expired());
        // Nothing usable in the reply, and not a refusal either.
        assert!(token(r#"{"success":true,"data":{"nak":""}}"#).is_err());
        assert!(token(r#"{"success":true,"data":{"nak":"   "}}"#).is_err());
        assert!(token("<html>").is_err());
    }

    /// Only Zoom's own two API hosts are ever asked, and only a host that did
    /// not answer sends the reader to the other one.
    #[test]
    fn only_zooms_own_two_hosts_are_asked() {
        assert_eq!(HOSTS, ["ai.zoom.us", "zoommate.zoom.us"]);
        assert!(Refusal::ServerError.worth_another_host());
        assert!(Refusal::Unreachable("no answer".to_string()).worth_another_host());
        assert!(!session_expired().worth_another_host());
        assert!(!unreadable().worth_another_host());
    }

    /// Only Zoom's session cookie and Cloudflare's clearance are kept, and the
    /// session is required.
    #[test]
    fn only_zooms_session_cookies_are_kept() {
        let kept = super::super::pasted::keep("_zm_ssid=abc; cf_clearance=c; _ga=x", &COOKIES);
        assert_eq!(kept.as_deref(), Some("_zm_ssid=abc; cf_clearance=c"));
        assert_eq!(super::super::pasted::keep("cf_clearance=1", &COOKIES), None);
    }
}
