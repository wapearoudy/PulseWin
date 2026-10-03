//! Raycast AI: the account's AI credits — how many are left out of the
//! period's total, and when the next ones arrive.
//!
//! Read from the call Raycast's own settings page makes:
//! `GET https://www.raycast.com/frontend_api/current_user/ai_credits`.
//!
//! **The credential is a pasted cookie.** The original imports the session out
//! of the browser the reader signed in with (`Auth/BrowserCookies.swift`),
//! which on macOS reads a Chromium cookie store through the login keychain;
//! PulseWin cannot, so the `Cookie` header copied out of a signed-in request is
//! the credential here instead. Only `__raycast_session` and `csrf_token` are
//! kept — a browser store holds the site's analytics and preference cookies
//! beside its session, and what is not kept never leaves the process.
//!
//! **Credits, not a month.** `next_credits_at` is when the allowance renews;
//! nothing states how long the period is, so none is claimed, and the window is
//! named for what it is rather than for a length nobody reported.
//!
//! The shape is second-hand — taken from the original and its fixture, not from
//! a captured reply.

use std::sync::Arc;

use serde_json::Value;

use super::{describe_reqwest_error, percent_from_fraction, Ctx, FetchFuture, Provider};
use crate::model::{parse_reset, ProviderUsage, UsageWindow};

const ID: &str = "raycast-ai";
const NAME: &str = "Raycast AI";

const ENDPOINT: &str = "https://www.raycast.com/frontend_api/current_user/ai_credits";

/// The session, and the CSRF token the site keeps beside it. The session is the
/// one that has to be there; a header without it is no sign-in.
const COOKIES: [&str; 2] = ["__raycast_session", "csrf_token"];

pub struct RaycastAi;

impl Provider for RaycastAi {
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
        return ProviderUsage::failed(ID, NAME, super::pasted::needed(ID, "Raycast session cookie"));
    };

    // Made on the client that refuses redirects: a `Cookie` header set by hand
    // rides a redirect to whatever host it names, so a signed-out session has
    // to arrive as the 3xx to the sign-in page rather than be followed with the
    // session attached.
    let response = ctx
        .gateway_client
        .get(ENDPOINT)
        .header("Cookie", cookie)
        .header("Accept", "application/json")
        .header("Origin", "https://www.raycast.com")
        .header("Referer", "https://www.raycast.com/settings")
        .send()
        .await;

    let response = match response {
        Ok(response) => response,
        Err(e) => {
            return ProviderUsage::failed(ID, NAME, format!("request failed: {}", describe_reqwest_error(&e)))
        }
    };

    let status = response.status();
    let body = match response.text().await {
        Ok(body) => body,
        Err(e) => return ProviderUsage::failed(ID, NAME, format!("cannot read body: {e}")),
    };

    if status.is_redirection() {
        return ProviderUsage::failed(ID, NAME, expired());
    }
    if !status.is_success() {
        let hints: &[(u16, &str)] = &[
            (401, " — the session has expired; copy a fresh cookie from raycast.com"),
            (403, " — the session has expired; copy a fresh cookie from raycast.com"),
            (429, " — rate limited, try again shortly"),
        ];
        return ProviderUsage::failed(ID, NAME, super::http_failure(status, hints));
    }

    reading(&body)
}

fn expired() -> String {
    "HTTP 3xx — the session has expired; copy a fresh cookie from raycast.com".to_string()
}

/// The mapping, kept apart from the request so a fixture can drive it.
fn reading(body: &str) -> ProviderUsage {
    let reply: Value = match serde_json::from_str(body) {
        Ok(value @ Value::Object(_)) => value,
        Ok(_) => return ProviderUsage::failed(ID, NAME, "the reply is not an object"),
        Err(e) => return ProviderUsage::failed(ID, NAME, format!("bad JSON: {e}")),
    };

    // Both figures from Raycast, or no ring: a total of zero is no allowance,
    // and a remainder alone has nothing to be a fraction of. A negative
    // remainder is a figure that is not one, not an overdraft.
    let window = match (
        amount(&reply, "remaining_balance_credits"),
        amount(&reply, "total_balance_credits"),
    ) {
        (Some(remaining), Some(total)) if remaining >= 0.0 && total > 0.0 => {
            UsageWindow::new(
                "Credits",
                Some(percent_from_fraction((total - remaining).max(0.0) / total)),
            )
            .with_reset(reply.get("next_credits_at").and_then(parse_reset))
        }
        _ => return ProviderUsage::failed(ID, NAME, "no limits reported in the reply"),
    };

    let tier = reply
        .get("funding_subscription")
        .and_then(|funding| funding.get("tier"))
        .and_then(Value::as_str);

    ProviderUsage::ok(ID, NAME, vec![window]).with_plan(plan(tier))
}

/// A figure that may arrive as a number or as a numeric string — Raycast writes
/// its amounts both ways.
fn amount(value: &Value, key: &str) -> Option<f64> {
    super::dig_number(value.get(key)).filter(|figure| figure.is_finite())
}

/// Raycast's plan names, as its own pages write them. A tier this build does
/// not know is still shown rather than dropped.
fn plan(tier: Option<&str>) -> Option<String> {
    let tier = tier?.trim();
    if tier.is_empty() {
        return None;
    }
    Some(match tier {
        "pro" => "Pro".to_string(),
        "pro_plus" => "Pro+".to_string(),
        "max" => "Max".to_string(),
        other => other.to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Second-hand, from the original's fixture: the reply CodexBar's Raycast
    /// plugin describes, which is what the route is read against.
    fn fixture() -> String {
        r#"{"remaining_balance_credits":"750","total_balance_credits":1000,"next_credits_at":"2026-07-01T00:00:00Z","funding_subscription":{"tier":"pro_plus"}}"#
            .to_string()
    }

    #[test]
    fn reads_the_credits_as_used_out_of_the_total() {
        let usage = reading(&fixture());
        assert_eq!(usage.plan.as_deref(), Some("Pro+"));
        assert_eq!(usage.windows.len(), 1);
        assert_eq!(usage.windows[0].label, "Credits");
        // 250 of 1,000 spent is a quarter, whichever way Raycast wrote it.
        assert_eq!(usage.windows[0].percent_used, Some(25.0));
        assert_eq!(
            usage.windows[0].resets_at.as_deref(),
            Some("2026-07-01T00:00:00Z")
        );
    }

    #[test]
    fn a_remainder_with_no_total_or_a_total_of_zero_draws_nothing() {
        for reply in [
            r#"{"remaining_balance_credits":50}"#,
            r#"{"remaining_balance_credits":0,"total_balance_credits":0}"#,
            r#"{"remaining_balance_credits":-1,"total_balance_credits":100}"#,
        ] {
            assert!(reading(reply).error.is_some(), "read {reply}");
        }
    }

    #[test]
    fn a_spent_allowance_reads_full() {
        let spent = r#"{"remaining_balance_credits":0,"total_balance_credits":100}"#;
        assert_eq!(reading(spent).windows[0].percent_used, Some(100.0));
    }

    #[test]
    fn a_reply_that_is_not_an_object_cannot_be_read() {
        assert!(reading("not json").error.is_some());
        assert!(reading("[]").error.is_some());
        // An object with nothing in it is a reply with no figures in it.
        assert!(reading("{}").error.is_some());
    }

    /// Only the session and the CSRF token are kept, and the session is the one
    /// that has to be there.
    #[test]
    fn only_the_session_and_csrf_cookies_are_kept() {
        let kept = super::super::pasted::keep("x=1; csrf_token=t; __raycast_session=s", &COOKIES);
        assert_eq!(kept.as_deref(), Some("csrf_token=t; __raycast_session=s"));
        assert_eq!(super::super::pasted::keep("csrf_token=t", &COOKIES), None);
    }

    #[test]
    fn a_tier_the_build_does_not_know_is_still_shown() {
        assert_eq!(plan(Some("pro")).as_deref(), Some("Pro"));
        assert_eq!(plan(Some("max")).as_deref(), Some("Max"));
        assert_eq!(plan(Some(" team ")).as_deref(), Some("team"));
        assert_eq!(plan(Some("   ")), None);
        assert_eq!(plan(None), None);
    }
}
