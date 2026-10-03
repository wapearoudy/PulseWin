//! ClawRouter, OpenClaw's routing gateway: the monthly budget on the policy a
//! key belongs to, as an amount spent and a limit the service states.
//!
//! Read with a ClawRouter key the user enters, from the hosted service's
//! `GET https://clawrouter.openclaw.ai/v1/usage`.
//!
//! Money is in micro-dollars. A policy with no budget (`configured: false`)
//! reports spend and requests and nothing to measure them against, so it is
//! "no limits reported" rather than a ring at zero. The budget's month is named
//! (`…/2026-07`) but not when, or in which zone, it turns over, so no reset is
//! inferred from it. The per-provider breakdown is spend with no limit and is
//! left out.

use std::sync::Arc;

use serde_json::Value;

use super::{describe_reqwest_error, percent_from_fraction, Ctx, FetchFuture, Provider};
use crate::model::{ProviderUsage, UsageWindow};

const ENDPOINT: &str = "https://clawrouter.openclaw.ai/v1/usage";

pub struct ClawRouter;

impl Provider for ClawRouter {
    fn id(&self) -> &'static str {
        "clawrouter"
    }

    fn name(&self) -> &'static str {
        "ClawRouter"
    }

    /// The key the fetch reads, and nothing else.
    fn is_configured(&self) -> bool {
        super::provider_key(self.id()).is_some()
    }

    fn fetch(&self, ctx: Arc<Ctx>) -> FetchFuture {
        Box::pin(async move { fetch_inner(ctx).await })
    }
}

async fn fetch_inner(ctx: Arc<Ctx>) -> ProviderUsage {
    const ID: &str = "clawrouter";
    const NAME: &str = "ClawRouter";

    let key = match super::provider_key(ID) {
        Some(key) => key,
        None => return ProviderUsage::failed(ID, NAME, super::missing_key(ID, "")),
    };

    let response = ctx
        .client
        .get(ENDPOINT)
        .header("Authorization", format!("Bearer {key}"))
        .header("Accept", "application/json")
        .send()
        .await;

    let response = match response {
        Ok(r) => r,
        Err(e) => {
            return ProviderUsage::failed(
                ID,
                NAME,
                format!("request failed: {}", describe_reqwest_error(&e)),
            )
        }
    };

    let status = response.status();
    let body = match response.text().await {
        Ok(t) => t,
        Err(e) => return ProviderUsage::failed(ID, NAME, format!("cannot read body: {e}")),
    };

    if !status.is_success() {
        let hints: &[(u16, &str)] = &[
            (401, " — the API key was refused"),
            (403, " — the API key was refused"),
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

/// The mapping, kept apart from the request so a fixture can drive it.
fn reading(json: &Value) -> ProviderUsage {
    const ID: &str = "clawrouter";
    const NAME: &str = "ClawRouter";

    let Some(configured) = json
        .get("budget")
        .and_then(|budget| budget.get("configured"))
        .and_then(|v| v.as_bool())
    else {
        return ProviderUsage::failed(ID, NAME, "bad reply: no budget object");
    };

    if !configured {
        return ProviderUsage::failed(ID, NAME, "this policy has no budget configured");
    }

    let limit = json
        .get("budget")
        .and_then(|budget| budget.get("limitMicros"))
        .and_then(|v| v.as_i64());
    let spent = json
        .get("budget")
        .and_then(|budget| budget.get("spentMicros"))
        .and_then(|v| v.as_i64());

    let (Some(limit), Some(spent)) = (limit, spent) else {
        return ProviderUsage::failed(ID, NAME, "bad reply: budget has no figures");
    };

    // No limit, or a limit of zero or less, is no ring — never a ring at zero.
    if limit <= 0 || spent < 0 {
        return ProviderUsage::failed(ID, NAME, "this policy has no budget configured");
    }

    let fraction = spent as f64 / limit as f64;
    let window = UsageWindow::new("Monthly", Some(percent_from_fraction(fraction))).with_detail(
        Some(format!(
            "{:.2} / {:.2} USD this month",
            spent as f64 / 1_000_000.0,
            limit as f64 / 1_000_000.0
        )),
    );

    ProviderUsage::ok(ID, NAME, vec![window])
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// The reply's shape: micro-dollars, and a flag saying whether a budget was
    /// ever set.
    fn fixture() -> Value {
        json!({
            "budget": {
                "configured": true,
                "limitMicros": 10_000_000,
                "spentMicros": 2_500_000,
                "period": "2026-07"
            }
        })
    }

    #[test]
    fn spends_the_budget_as_a_fraction_of_the_limit() {
        let usage = reading(&fixture());
        assert_eq!(usage.windows[0].label, "Monthly");
        assert_eq!(usage.windows[0].percent_used, Some(25.0));
        assert_eq!(
            usage.windows[0].detail.as_deref(),
            Some("2.50 / 10.00 USD this month")
        );
        // The budget's month is named but not when it turns over.
        assert!(usage.windows[0].resets_at.is_none());
    }

    #[test]
    fn a_policy_with_no_budget_is_no_limits_rather_than_a_ring_at_zero() {
        let reply = json!({ "budget": { "configured": false, "spentMicros": 400 } });
        assert!(reading(&reply).error.is_some());
    }

    #[test]
    fn a_limit_of_zero_is_the_same_nothing() {
        let reply = json!({ "budget": { "configured": true, "limitMicros": 0, "spentMicros": 0 } });
        assert!(reading(&reply).error.is_some());
    }

    #[test]
    fn a_spend_past_the_limit_clamps_to_a_full_ring() {
        let reply =
            json!({ "budget": { "configured": true, "limitMicros": 100, "spentMicros": 250 } });
        assert_eq!(reading(&reply).windows[0].percent_used, Some(100.0));
    }

    #[test]
    fn a_reply_without_the_budget_object_is_unreadable() {
        assert!(reading(&json!({})).error.is_some());
        assert!(reading(&json!({ "budget": { "limitMicros": 1 } })).error.is_some());
    }
}
