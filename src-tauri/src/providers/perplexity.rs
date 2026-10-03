//! Perplexity: the API credit on the account, and the monthly credit that comes
//! with a subscription when it is the only credit there is.
//!
//! Read from the route the account's usage page calls:
//! `GET https://www.perplexity.ai/rest/billing/credits?version=2.18&source=default`.
//!
//! **The credential is a pasted cookie.** The original imports the session out
//! of the browser the reader signed in with (`Auth/BrowserCookies.swift`),
//! which on macOS reads a Chromium cookie store through the login keychain;
//! PulseWin cannot, so the `Cookie` header copied out of a signed-in request is
//! the credential here instead, and only `__Secure-next-auth.session-token` is
//! kept out of it.
//!
//! **Money, in cents.** Every figure is a count of US cents: the balance, each
//! grant, the total used. The balance is shown as a balance, which draws no
//! ring.
//!
//! **A ring only where the split is stated.** Perplexity reports how much each
//! grant was and how much was used in total — not which grant it was taken
//! from. The original's neighbour spends the total down the subscription's
//! grant first, then purchased, then bonus, and draws a ring for each; that
//! order is a guess. The subscription's ring is drawn here only when that grant
//! is the only one on the account, where the total can have come from nowhere
//! else, and none at all when there is no such grant — never a full ring
//! standing in for one that is missing.
//!
//! **Redirects are not followed**, so the session is never carried to another
//! host; a redirect is an expired session.
//!
//! The shape is second-hand — taken from the original and its fixture, not from
//! a captured reply.

use std::sync::Arc;

use chrono::{DateTime, Utc};
use serde_json::Value;

use super::{describe_reqwest_error, percent_from_fraction, Ctx, FetchFuture, Provider};
use crate::model::{ProviderUsage, UsageWindow};

const ID: &str = "perplexity";
const NAME: &str = "Perplexity";

const ENDPOINT: &str = "https://www.perplexity.ai/rest/billing/credits?version=2.18&source=default";

/// The one cookie the site signs in with, and the one that has to be there.
const COOKIES: [&str; 1] = ["__Secure-next-auth.session-token"];

/// What the usage page's own request carries. The site sits behind a bot screen
/// that turns away a request looking like no browser, and the port's client
/// names itself, so this one request says otherwise.
const BROWSER_USER_AGENT: &str = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 \
                                  (KHTML, like Gecko) Chrome/143.0.0.0 Safari/537.36";

pub struct Perplexity;

impl Provider for Perplexity {
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
        return ProviderUsage::failed(
            ID,
            NAME,
            super::pasted::needed(ID, "Perplexity session cookie"),
        );
    };

    // The client that refuses redirects, so a signed-out session arrives as the
    // 3xx to the sign-in page rather than being carried there.
    let response = ctx
        .gateway_client
        .get(ENDPOINT)
        .header("Cookie", cookie)
        .header("Accept", "application/json")
        .header("Origin", "https://www.perplexity.ai")
        .header("Referer", "https://www.perplexity.ai/account/usage")
        .header("User-Agent", BROWSER_USER_AGENT)
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
        return ProviderUsage::failed(
            ID,
            NAME,
            "HTTP 3xx — the session has expired; copy a fresh cookie from perplexity.ai",
        );
    }
    if !status.is_success() {
        let hints: &[(u16, &str)] = &[
            (401, " — the session has expired; copy a fresh cookie from perplexity.ai"),
            (403, " — the session has expired; copy a fresh cookie from perplexity.ai"),
            (429, " — rate limited, try again shortly"),
        ];
        return ProviderUsage::failed(ID, NAME, super::http_failure(status, hints));
    }

    reading(&body, Utc::now())
}

/// The mapping, kept apart from the request so a fixture can drive it.
fn reading(body: &str, now: DateTime<Utc>) -> ProviderUsage {
    let root: Value = match serde_json::from_str(body) {
        Ok(value @ Value::Object(_)) => value,
        Ok(_) => return ProviderUsage::failed(ID, NAME, "the reply could not be read"),
        Err(_) => return ProviderUsage::failed(ID, NAME, "the reply could not be read"),
    };

    // The grants are what the reply is; a body without them is some other
    // route's answer, not a credits reading with nothing in it.
    let Some(grants) = field(&root, "credit_grants", "creditGrants").and_then(Value::as_array) else {
        return ProviderUsage::failed(ID, NAME, "the reply could not be read");
    };

    // A grant that has lapsed is no longer part of anything.
    let live: Vec<&Value> = grants
        .iter()
        .filter(|grant| match figure(grant, "expires_at_ts", "expiresAtTs") {
            Some(expiry) => DateTime::from_timestamp(expiry as i64, 0).is_some_and(|at| at > now),
            None => true,
        })
        .collect();

    let total = |kind: &str| -> f64 {
        live.iter()
            .filter(|grant| grant.get("type").and_then(Value::as_str) == Some(kind))
            .filter_map(|grant| figure(grant, "amount_cents", "amountCents"))
            .sum()
    };
    let recurring = total("recurring");
    let others: f64 = live
        .iter()
        .filter(|grant| grant.get("type").and_then(Value::as_str) != Some("recurring"))
        .filter_map(|grant| figure(grant, "amount_cents", "amountCents"))
        .sum();
    let purchased = figure(&root, "current_period_purchased_cents", "currentPeriodPurchasedCents")
        .unwrap_or(0.0);
    let used = figure(&root, "total_usage_cents", "totalUsageCents");

    let mut windows: Vec<UsageWindow> = Vec::new();

    // Only where the total can have come from nowhere else: one grant, nothing
    // bought, and a used figure the service states.
    if recurring > 0.0 && others <= 0.0 && purchased <= 0.0 {
        if let Some(used) = used.filter(|used| *used >= 0.0) {
            let reset = figure(&root, "renewal_date_ts", "renewalDateTs")
                .filter(|seconds| *seconds > 0.0)
                .and_then(|seconds| super::stamp_from_epoch_seconds(seconds));
            windows.push(
                UsageWindow::new("Credits", Some(percent_from_fraction(used / recurring)))
                    .with_reset(reset),
            );
        }
    }

    // Cents, as dollars. A negative balance is a figure that is not one.
    let balance = figure(&root, "balance_cents", "balanceCents")
        .filter(|cents| *cents >= 0.0)
        .map(|cents| cents / 100.0);

    if let Some(balance) = balance {
        windows.push(super::balance_window("Balance", format!("{balance:.2} USD")));
    }

    if windows.is_empty() {
        return ProviderUsage::failed(ID, NAME, "no limits reported in the reply");
    }

    let mut usage = ProviderUsage::ok(ID, NAME, windows);
    if let Some(balance) = balance { usage = usage.with_credit_remaining(balance, "USD"); }
    usage
}

/// Perplexity has answered with both spellings of every field.
fn field<'a>(object: &'a Value, snake: &str, camel: &str) -> Option<&'a Value> {
    object.get(snake).or_else(|| object.get(camel))
}

/// A figure written either way, refusing a boolean the way every provider here
/// does.
fn figure(object: &Value, snake: &str, camel: &str) -> Option<f64> {
    super::dig_number(field(object, snake, camel)).filter(|figure| figure.is_finite())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Second-hand, from the original's fixture: the reply CodexBar's
    /// Perplexity plugin describes, which is what the route is read against.
    fn fixture() -> String {
        r#"{"version":"2.18","balance_cents":320,"renewal_date_ts":1790812800,"current_period_purchased_cents":0,"credit_grants":[{"type":"recurring","amount_cents":500,"expires_at_ts":1790812800}],"total_usage_cents":180}"#
            .to_string()
    }

    /// Before every expiry in the fixtures.
    fn now() -> DateTime<Utc> {
        DateTime::from_timestamp(1_788_000_000, 0).unwrap()
    }

    #[test]
    fn reads_the_subscriptions_credit_when_it_is_the_only_one_and_the_balance() {
        let usage = reading(&fixture(), now());
        assert_eq!(usage.credit_remaining.as_ref().unwrap().amount, 3.2);
        assert_eq!(usage.credit_remaining.as_ref().unwrap().currency, "USD");
        assert_eq!(usage.windows.len(), 2);
        assert_eq!(usage.windows[0].label, "Credits");
        // 180 of 500 cents spent.
        assert_eq!(usage.windows[0].percent_used, Some(36.0));
        assert_eq!(
            usage.windows[0].resets_at.as_deref(),
            Some("2026-10-01T00:00:00Z")
        );
        // Cents, as dollars, and no ring: a balance is not an allowance.
        assert_eq!(usage.windows[1].label, "Balance");
        assert_eq!(usage.windows[1].percent_used, None);
        assert_eq!(usage.windows[1].detail.as_deref(), Some("3.20 USD"));
    }

    #[test]
    fn beside_a_bonus_or_a_purchase_no_ring_is_drawn_and_the_balance_stands() {
        for reply in [
            r#"{"balance_cents":7250,"renewal_date_ts":1790812800,"current_period_purchased_cents":0,"credit_grants":[{"type":"recurring","amount_cents":10000},{"type":"promotional","amount_cents":20000,"expires_at_ts":1800000000}],"total_usage_cents":2750}"#,
            r#"{"balance_cents":0,"renewal_date_ts":1790812800,"current_period_purchased_cents":3000,"credit_grants":[{"type":"recurring","amount_cents":5000}],"total_usage_cents":8000}"#,
        ] {
            let usage = reading(reply, now());
            let labels: Vec<&str> = usage.windows.iter().map(|w| w.label.as_str()).collect();
            assert_eq!(labels, vec!["Balance"], "read {reply}");
            assert!(usage.windows[0].detail.is_some());
        }
    }

    #[test]
    fn a_bonus_that_has_lapsed_is_no_longer_beside_it() {
        let reply = r#"{"balance_cents":100,"credit_grants":[{"type":"recurring","amount_cents":1000},{"type":"promotional","amount_cents":500,"expires_at_ts":1700000000}],"total_usage_cents":250}"#;
        let usage = reading(reply, now());
        assert_eq!(usage.windows[0].label, "Credits");
        assert_eq!(usage.windows[0].percent_used, Some(25.0));
    }

    #[test]
    fn no_subscription_credit_draws_no_ring_never_a_full_one_standing_in() {
        let reply = r#"{"balance_cents":0,"renewal_date_ts":1790812800,"current_period_purchased_cents":0,"credit_grants":[],"total_usage_cents":0}"#;
        let usage = reading(reply, now());
        let labels: Vec<&str> = usage.windows.iter().map(|w| w.label.as_str()).collect();
        assert_eq!(labels, vec!["Balance"]);
        assert_eq!(usage.windows[0].detail.as_deref(), Some("0.00 USD"));
    }

    #[test]
    fn camel_case_spellings_are_read_too() {
        let reply = r#"{"balanceCents":500,"renewalDateTs":1790812800,"creditGrants":[{"type":"recurring","amountCents":500}],"totalUsageCents":100}"#;
        let usage = reading(reply, now());
        assert_eq!(usage.windows[0].percent_used, Some(20.0));
        assert_eq!(usage.windows[1].detail.as_deref(), Some("5.00 USD"));
    }

    #[test]
    fn a_reply_that_is_not_a_credits_reply_cannot_be_read() {
        for reply in ["not json", "{}", r#"{"balance_cents":5}"#, "[]"] {
            assert!(reading(reply, now()).error.is_some(), "read {reply}");
        }
    }

    #[test]
    fn figures_that_are_not_figures_are_left_off_and_nothing_left_is_no_limits() {
        let reply = r#"{"balance_cents":-4,"credit_grants":[{"type":"recurring","amount_cents":500}],"total_usage_cents":-1}"#;
        let usage = reading(reply, now());
        assert!(usage.error.is_some());
    }

    /// A spent subscription reads full, and the ring is never past full.
    #[test]
    fn a_spent_subscription_reads_full() {
        let reply = r#"{"balance_cents":0,"credit_grants":[{"type":"recurring","amount_cents":500}],"total_usage_cents":900}"#;
        assert_eq!(reading(reply, now()).windows[0].percent_used, Some(100.0));
    }

    /// Only the sign-in cookie is sent, and it is required.
    #[test]
    fn only_the_sign_in_cookie_is_kept() {
        let kept = super::super::pasted::keep(
            "__cf_bm=x; __Secure-next-auth.session-token=s; theme=dark",
            &COOKIES,
        );
        assert_eq!(kept.as_deref(), Some("__Secure-next-auth.session-token=s"));
        assert_eq!(
            super::super::pasted::keep("next-auth.csrf-token=abc", &COOKIES),
            None
        );
    }
}
