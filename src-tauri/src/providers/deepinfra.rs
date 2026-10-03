//! DeepInfra, pay-as-you-go inference: the prepaid balance in US dollars, and
//! — only where the account has set one — the billing cycle's spend against its
//! own spending limit.
//!
//! Read with an API key the user enters, from
//! `GET https://api.deepinfra.com/payment/checklist?compute_owed=true`.
//!
//! DeepInfra keeps its ledger inverted: prepaid money is a **negative**
//! `stripe_balance`, and spend not yet billed is `recent`. What is left is the
//! one taken from the other, both as reported; a positive result is money owed
//! and is shown as a negative balance, not as nothing.
//!
//! Left out on purpose: the month's spend (a figure with no limit beside it),
//! and a suspended account shown as a full ring — the checklist's flag is not a
//! share of anything, and drawing 100% from it would be a figure nobody
//! reported.

use std::sync::Arc;

use serde_json::Value;

use super::{describe_reqwest_error, percent_from_fraction, Ctx, FetchFuture, Provider};
use crate::model::{ProviderUsage, UsageWindow};

const ENDPOINT: &str = "https://api.deepinfra.com/payment/checklist?compute_owed=true";

pub struct DeepInfra;

impl Provider for DeepInfra {
    fn id(&self) -> &'static str {
        "deepinfra"
    }

    fn name(&self) -> &'static str {
        "DeepInfra"
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
    const ID: &str = "deepinfra";
    const NAME: &str = "DeepInfra";

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

/// All in US dollars.
fn reading(json: &Value) -> ProviderUsage {
    const ID: &str = "deepinfra";
    const NAME: &str = "DeepInfra";

    let ledger = json.get("stripe_balance").and_then(|v| v.as_f64());
    let recent = json.get("recent").and_then(|v| v.as_f64());

    let (Some(ledger), Some(recent)) = (ledger, recent) else {
        return ProviderUsage::failed(ID, NAME, "no balance in response");
    };
    if !ledger.is_finite() || !recent.is_finite() {
        return ProviderUsage::failed(ID, NAME, "no balance in response");
    }

    // A negative `recent` is not spend; it is read as none. Subtracted from
    // zero rather than negated, so an empty account is $0.00 and not -$0.00.
    let unbilled = recent.max(0.0);
    let available = 0.0 - (ledger + unbilled);

    let mut windows: Vec<UsageWindow> = Vec::new();

    // The account's own limit, where it has set one. No limit, or a limit of
    // zero or less, is no ring — never a ring at zero.
    if let Some(limit) = json
        .get("limit")
        .and_then(|v| v.as_f64())
        .filter(|limit| limit.is_finite() && *limit > 0.0)
    {
        let fraction = unbilled / limit;
        windows.push(
            UsageWindow::new("Spend", Some(percent_from_fraction(fraction)))
                .with_detail(Some(format!("{unbilled:.2} / {limit:.2} USD this cycle"))),
        );
    }

    windows.push(super::balance_window("Balance", format!("{available:.2} USD")));

    ProviderUsage::ok(ID, NAME, windows).with_credit_remaining(available, "USD")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn inverts_the_ledger_and_adds_the_unbilled_spend() {
        let reply = json!({ "stripe_balance": -50.0, "recent": 5.0 });
        let usage = reading(&reply);
        // $50 prepaid, $5 of it spent and not yet billed, so $45 is left.
        assert_eq!(usage.credit_remaining.as_ref().unwrap().amount, 45.0);
        assert_eq!(usage.credit_remaining.as_ref().unwrap().currency, "USD");
        assert_eq!(usage.windows[0].label, "Balance");
        assert_eq!(usage.windows[0].detail.as_deref(), Some("45.00 USD"));
        assert_eq!(usage.windows[0].percent_used, None);
    }

    #[test]
    fn a_spending_limit_draws_the_cycles_spend() {
        let reply = json!({ "stripe_balance": -50.0, "recent": 25.0, "limit": 100.0 });
        let usage = reading(&reply);
        assert_eq!(usage.windows.len(), 2);
        assert_eq!(usage.windows[0].label, "Spend");
        assert_eq!(usage.windows[0].percent_used, Some(25.0));
    }

    #[test]
    fn a_limit_of_zero_is_no_ring_rather_than_a_ring_at_zero() {
        let reply = json!({ "stripe_balance": -50.0, "recent": 5.0, "limit": 0.0 });
        let usage = reading(&reply);
        assert_eq!(usage.windows.len(), 1);
        assert_eq!(usage.windows[0].label, "Balance");
    }

    #[test]
    fn a_negative_recent_is_not_spend() {
        let reply = json!({ "stripe_balance": -10.0, "recent": -3.0 });
        let usage = reading(&reply);
        assert_eq!(usage.windows[0].detail.as_deref(), Some("10.00 USD"));
    }

    #[test]
    fn money_owed_is_shown_as_a_negative_balance() {
        let reply = json!({ "stripe_balance": 0.0, "recent": 7.5 });
        let usage = reading(&reply);
        assert_eq!(usage.windows[0].detail.as_deref(), Some("-7.50 USD"));
    }

    #[test]
    fn a_reply_without_the_two_ledger_figures_is_unreadable() {
        assert!(reading(&json!({ "stripe_balance": -1.0 })).error.is_some());
        assert!(reading(&json!({})).error.is_some());
    }
}
