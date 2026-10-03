//! Vercel AI Gateway: the team's remaining credit, in US dollars. There is no
//! allowance and no period, so there is no ring to draw — only a balance.
//!
//! Read with an AI Gateway API key the user enters, from
//! `GET https://ai-gateway.vercel.sh/v1/credits`.
//!
//! The reply also carries lifetime spend. Spend with no limit beside it has
//! nowhere to go here, so it is left out.

use std::sync::Arc;

use serde_json::Value;

use super::{describe_reqwest_error, Ctx, FetchFuture, Provider};
use crate::model::ProviderUsage;

const ENDPOINT: &str = "https://ai-gateway.vercel.sh/v1/credits";

pub struct VercelAiGateway;

impl Provider for VercelAiGateway {
    fn id(&self) -> &'static str {
        "vercel-ai-gateway"
    }

    fn name(&self) -> &'static str {
        "Vercel AI Gateway"
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
    const ID: &str = "vercel-ai-gateway";
    const NAME: &str = "Vercel AI Gateway";

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

/// The balance is a decimal string, `"95.50"`, in US dollars.
fn reading(json: &Value) -> ProviderUsage {
    const ID: &str = "vercel-ai-gateway";
    const NAME: &str = "Vercel AI Gateway";

    let Some(amount) = json
        .get("balance")
        .and_then(|v| v.as_str())
        .map(str::trim)
        .and_then(|text| text.parse::<f64>().ok())
        .filter(|amount| amount.is_finite())
    else {
        return ProviderUsage::failed(ID, NAME, "no balance in response");
    };

    // Zero and below are kept: an empty or overdrawn team is a reading.
    ProviderUsage::ok(
        ID,
        NAME,
        vec![super::balance_window("Balance", format!("{amount:.2} USD"))],
    )
    .with_credit_remaining(amount, "USD")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn reads_the_decimal_string_the_service_sends() {
        let usage = reading(&json!({ "balance": "95.50" }));
        assert_eq!(usage.credit_remaining.as_ref().unwrap().amount, 95.5);
        assert_eq!(usage.credit_remaining.as_ref().unwrap().currency, "USD");
        assert_eq!(usage.windows.len(), 1);
        assert_eq!(usage.windows[0].label, "Balance");
        assert_eq!(usage.windows[0].detail.as_deref(), Some("95.50 USD"));
        // No allowance and no period, so no fraction to draw.
        assert_eq!(usage.windows[0].percent_used, None);
    }

    #[test]
    fn an_empty_team_reads_zero_rather_than_no_reading() {
        let usage = reading(&json!({ "balance": "0.00" }));
        assert_eq!(usage.windows[0].detail.as_deref(), Some("0.00 USD"));
    }

    #[test]
    fn an_overdrawn_team_is_kept_as_reported() {
        let usage = reading(&json!({ "balance": "-3.25" }));
        assert_eq!(usage.windows[0].detail.as_deref(), Some("-3.25 USD"));
    }

    #[test]
    fn a_reply_that_is_not_a_figure_is_unreadable() {
        assert!(reading(&json!({ "balance": "a lot" })).error.is_some());
        assert!(reading(&json!({ "balance": 95.5 })).error.is_some());
        assert!(reading(&json!({})).error.is_some());
    }
}
