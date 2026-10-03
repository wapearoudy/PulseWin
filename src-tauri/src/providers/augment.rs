//! Augment Code's credits for the billing cycle: how many were used against
//! how many the cycle makes available, both as Augment reports them.
//!
//! Read from the two endpoints Augment's own account page calls on
//! `app.augmentcode.com`: `GET /api/credits` for the figures and
//! `GET /api/subscription` for the plan's name and when the cycle ends. The
//! second is a nicety — a reading stands without it.
//!
//! **The credential is a pasted cookie.** The original imports the session from
//! the browser (`Auth/BrowserCookies.swift`), which on macOS reads a Chromium
//! cookie store through the login keychain; PulseWin cannot, so the `Cookie`
//! header copied out of a signed-in request is the credential here instead.
//!
//! **What the reference implementation does and this does not.** When
//! `usageUnitsAvailable` is missing or zero, CodexBar adds the remaining
//! credits to the consumed ones and calls that the limit; a limit Augment did
//! not state is not one this draws against, so that reading is left off.

use std::sync::Arc;

use serde_json::Value;

use super::{describe_reqwest_error, percent_from_fraction, Ctx, FetchFuture, Provider};
use crate::model::{parse_reset, ProviderUsage, UsageWindow};

const ID: &str = "augment";
const NAME: &str = "Augment Code";

const CREDITS_URL: &str = "https://app.augmentcode.com/api/credits";
const SUBSCRIPTION_URL: &str = "https://app.augmentcode.com/api/subscription";

pub struct Augment;

impl Provider for Augment {
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
    let Some(cookie) = super::pasted::cookie(ID) else {
        return ProviderUsage::failed(ID, NAME, super::pasted::needed(ID, "Augment session cookie"));
    };

    let credits = match get(&ctx, CREDITS_URL, &cookie).await {
        Ok(body) => body,
        Err(error) => return ProviderUsage::failed(ID, NAME, error),
    };

    // Optional, as the reference has it: without it there is no plan name and
    // no reset, and the figures are still the figures.
    let subscription = get(&ctx, SUBSCRIPTION_URL, &cookie).await.ok();

    reading(&credits, subscription.as_deref())
}

/// One request with the pasted header, on the client that refuses redirects:
/// a `Cookie` set by hand rides a redirect to whatever host it names, so a
/// signed-out session has to arrive as the 3xx itself.
async fn get(ctx: &Ctx, url: &str, cookie: &str) -> Result<String, String> {
    let response = match ctx
        .gateway_client
        .get(url)
        .header("Cookie", cookie)
        .header("Accept", "application/json")
        .send()
        .await
    {
        Ok(response) => response,
        Err(e) => return Err(format!("request failed: {}", describe_reqwest_error(&e))),
    };

    let status = response.status();
    let body = match response.text().await {
        Ok(body) => body,
        Err(e) => return Err(format!("cannot read body: {e}")),
    };

    if status.is_redirection() {
        return Err("HTTP 3xx — the session has expired; copy a fresh cookie".to_string());
    }
    if !status.is_success() {
        let hints: &[(u16, &str)] = &[
            (401, " — the session has expired; copy a fresh cookie from app.augmentcode.com"),
            (403, " — the session has expired; copy a fresh cookie from app.augmentcode.com"),
            (429, " — rate limited, try again shortly"),
        ];
        return Err(super::http_failure(status, hints));
    }

    Ok(body)
}

/// The mapping, kept apart from the requests so a fixture can drive it.
fn reading(credits: &str, subscription: Option<&str>) -> ProviderUsage {
    // Not a dictionary of figures — an HTML sign-in page served with a 200, say
    // — is not a reply this can read.
    let reply: Value = match serde_json::from_str(credits) {
        Ok(value @ Value::Object(_)) => value,
        Ok(_) => return ProviderUsage::failed(ID, NAME, "the reply is not a credits object"),
        Err(e) => return ProviderUsage::failed(ID, NAME, format!("bad JSON: {e}")),
    };

    let subscription = subscription
        .and_then(|body| serde_json::from_str::<Value>(body).ok())
        .filter(Value::is_object);

    let plan = subscription
        .as_ref()
        .and_then(|body| body.get("planName"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .map(str::to_string);

    let used = figure(&reply, "usageUnitsConsumedThisBillingCycle");
    let available = figure(&reply, "usageUnitsAvailable");

    let (Some(used), Some(available)) = (used, available) else {
        return ProviderUsage::failed(ID, NAME, "no limits reported in the reply");
    };
    if used < 0.0 || available <= 0.0 {
        return ProviderUsage::failed(ID, NAME, "no limits reported in the reply");
    }

    // Credits, with the reset Augment states and no length it claims: a billing
    // cycle's thirty days are a sort key only.
    let reset = subscription
        .as_ref()
        .and_then(|body| body.get("billingPeriodEnd"))
        .and_then(parse_reset);
    let window =
        UsageWindow::new("Credits", Some(percent_from_fraction(used / available))).with_reset(reset);

    ProviderUsage::ok(ID, NAME, vec![window]).with_plan(plan)
}

/// A reported figure that may arrive as a number or as a numeric string.
fn figure(value: &Value, key: &str) -> Option<f64> {
    super::dig_number(value.get(key)).filter(|figure| figure.is_finite())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn reads_the_cycle_the_plan_and_its_end() {
        let credits = json!({
            "usageUnitsRemaining": 125,
            "usageUnitsConsumedThisBillingCycle": 375,
            "usageUnitsAvailable": 500
        })
        .to_string();
        let subscription = json!({
            "planName": "Pro",
            "billingPeriodEnd": "2026-11-01T00:00:00Z"
        })
        .to_string();

        let usage = reading(&credits, Some(&subscription));
        assert_eq!(usage.windows[0].label, "Credits");
        assert_eq!(usage.windows[0].percent_used, Some(75.0));
        assert_eq!(
            usage.windows[0].resets_at.as_deref(),
            Some("2026-11-01T00:00:00Z")
        );
        assert_eq!(usage.plan.as_deref(), Some("Pro"));
    }

    #[test]
    fn the_figures_stand_without_the_subscription_reply() {
        let credits = json!({ "usageUnitsConsumedThisBillingCycle": 1, "usageUnitsAvailable": "4" })
            .to_string();
        let usage = reading(&credits, None);
        assert_eq!(usage.windows[0].percent_used, Some(25.0));
        assert_eq!(usage.windows[0].resets_at, None);
        assert_eq!(usage.plan, None);
    }

    #[test]
    fn a_missing_or_zero_allowance_is_left_off_rather_than_invented() {
        let no_size = json!({ "usageUnitsConsumedThisBillingCycle": 375, "usageUnitsRemaining": 125 })
            .to_string();
        assert!(reading(&no_size, None).error.is_some());

        let zero = json!({ "usageUnitsConsumedThisBillingCycle": 0, "usageUnitsAvailable": 0 })
            .to_string();
        assert!(reading(&zero, None).error.is_some());

        let negative = json!({ "usageUnitsConsumedThisBillingCycle": -1, "usageUnitsAvailable": 10 })
            .to_string();
        assert!(reading(&negative, None).error.is_some());
    }

    #[test]
    fn a_sign_in_page_served_as_json_is_unreadable() {
        assert!(reading("\"<!DOCTYPE html>\"", None).error.is_some());
        assert!(reading("[1, 2, 3]", None).error.is_some());
        assert!(reading("not json", None).error.is_some());
    }

    #[test]
    fn a_spend_past_the_allowance_clamps_to_a_full_ring() {
        let over = json!({ "usageUnitsConsumedThisBillingCycle": 700, "usageUnitsAvailable": 500 })
            .to_string();
        assert_eq!(reading(&over, None).windows[0].percent_used, Some(100.0));
    }
}
