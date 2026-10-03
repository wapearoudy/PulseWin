//! Atlas Cloud: the account's available balance, in the currency the service
//! states. A balance, not an allowance — so no ring.
//!
//! Read with a key the user enters, from Atlas Cloud's billing API:
//! `GET https://api.atlascloud.ai/public/v1/balance`. Reading it needs a key
//! with account-balance permission: the account owner's, or a team's Account
//! Admin or Finance key.
//!
//! Coding Plan quotas are a separate meter behind another route and are not
//! read. A negative balance is kept as reported: it is money owed.

use std::sync::Arc;

use serde_json::Value;

use super::{describe_reqwest_error, Ctx, FetchFuture, Provider};
use crate::model::ProviderUsage;

const ENDPOINT: &str = "https://api.atlascloud.ai/public/v1/balance";

pub struct AtlasCloud;

impl Provider for AtlasCloud {
    fn id(&self) -> &'static str {
        "atlas-cloud"
    }

    fn name(&self) -> &'static str {
        "Atlas Cloud"
    }

    /// The key the fetch reads, and nothing else: an env var or
    /// `%APPDATA%\PulseWin\atlas-cloud.json`.
    fn is_configured(&self) -> bool {
        super::provider_key(self.id()).is_some()
    }

    fn fetch(&self, ctx: Arc<Ctx>) -> FetchFuture {
        Box::pin(async move { fetch_inner(ctx).await })
    }
}

async fn fetch_inner(ctx: Arc<Ctx>) -> ProviderUsage {
    const ID: &str = "atlas-cloud";
    const NAME: &str = "Atlas Cloud";

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
            (403, " — the key cannot read this account's balance"),
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
///
/// The two envelope fields are checked rather than assumed: this route answers
/// several shapes, and a balance that belongs to something else is not this
/// account's.
fn reading(json: &Value) -> ProviderUsage {
    const ID: &str = "atlas-cloud";
    const NAME: &str = "Atlas Cloud";

    let object = json.get("object").and_then(|v| v.as_str());
    let scope = json.get("scope").and_then(|v| v.as_str());
    if object != Some("balance") || scope != Some("account") {
        return ProviderUsage::failed(ID, NAME, "bad reply: not an account balance");
    }

    let amount = json
        .get("available")
        .and_then(|available| available.get("value"))
        .and_then(|v| v.as_str())
        .map(str::trim)
        .and_then(|text| text.parse::<f64>().ok())
        .filter(|amount| amount.is_finite());

    let currency = json
        .get("available")
        .and_then(|available| available.get("currency"))
        .and_then(|v| v.as_str())
        .map(|text| text.trim().to_uppercase())
        .filter(|currency| currency.len() == 3 && currency.chars().all(|c| c.is_ascii_alphabetic()));

    let (Some(amount), Some(currency)) = (amount, currency) else {
        return ProviderUsage::failed(ID, NAME, "no balance in response");
    };

    ProviderUsage::ok(
        ID,
        NAME,
        vec![super::balance_window("Balance", format!("{amount:.2} {currency}"))],
    )
    .with_credit_remaining(amount, currency)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// The reply's shape, as the service documents it.
    fn fixture() -> Value {
        json!({
            "object": "balance",
            "scope": "account",
            "available": { "value": "42.75", "currency": "usd" }
        })
    }

    #[test]
    fn reads_the_available_balance_in_the_currency_it_names() {
        let usage = reading(&fixture());
        assert_eq!(usage.credit_remaining.as_ref().unwrap().amount, 42.75);
        assert_eq!(usage.credit_remaining.as_ref().unwrap().currency, "USD");
        assert_eq!(usage.windows[0].label, "Balance");
        assert_eq!(usage.windows[0].detail.as_deref(), Some("42.75 USD"));
        assert_eq!(usage.windows[0].percent_used, None);
    }

    #[test]
    fn money_owed_is_kept_as_reported() {
        let mut reply = fixture();
        reply["available"]["value"] = json!("-12.50");
        assert_eq!(reading(&reply).windows[0].detail.as_deref(), Some("-12.50 USD"));
    }

    #[test]
    fn a_balance_that_is_not_an_accounts_is_not_this_accounts() {
        let mut reply = fixture();
        reply["scope"] = json!("team");
        assert!(reading(&reply).error.is_some());

        let mut reply = fixture();
        reply["object"] = json!("wallet");
        assert!(reading(&reply).error.is_some());
    }

    #[test]
    fn a_currency_that_is_not_a_currency_is_refused() {
        let mut reply = fixture();
        reply["available"]["currency"] = json!("dollars");
        assert!(reading(&reply).error.is_some());

        let mut reply = fixture();
        reply["available"]["currency"] = json!("U$D");
        assert!(reading(&reply).error.is_some());
    }

    #[test]
    fn a_value_that_is_not_a_figure_is_unreadable() {
        let mut reply = fixture();
        reply["available"]["value"] = json!("a lot");
        assert!(reading(&reply).error.is_some());
        assert!(reading(&json!({})).error.is_some());
    }
}
