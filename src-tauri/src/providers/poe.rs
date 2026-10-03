//! Poe: the account's point balance. Poe reports no allowance and no period
//! beside it, so there is no ring — only the balance.
//!
//! Read with a key the user enters, from Poe's usage API:
//! `GET https://api.poe.com/usage/current_balance`.
//!
//! The reference also pages through `/usage/points_history` for points spent
//! per day and per bot. That is spend with no limit behind it, which this app
//! has nowhere to show, so it is not asked for. Points are Poe's own unit, not
//! money, and are never compared against a currency.

use std::sync::Arc;

use serde_json::Value;

use super::{describe_reqwest_error, Ctx, FetchFuture, Provider};
use crate::model::ProviderUsage;

const ENDPOINT: &str = "https://api.poe.com/usage/current_balance";

pub struct Poe;

impl Provider for Poe {
    fn id(&self) -> &'static str {
        "poe"
    }

    fn name(&self) -> &'static str {
        "Poe"
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
    const ID: &str = "poe";
    const NAME: &str = "Poe";

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
        Ok(response) => response,
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
    const ID: &str = "poe";
    const NAME: &str = "Poe";

    // Poe writes the balance as a number, or as a numeric string.
    let points = super::dig_number(json.get("current_point_balance"));

    let Some(points) = points.filter(|points| points.is_finite()) else {
        return ProviderUsage::failed(ID, NAME, "no balance in response");
    };

    ProviderUsage::ok(ID, NAME, vec![super::balance_window("Balance", text(points))])
}

/// "1,234 points". The original formats with the user's locale for the
/// thousands separators; this port has no locale, so the figure is rounded to
/// whole points, which is all Poe's own unit is ever reported in.
fn text(points: f64) -> String {
    format!("{} points", points.round() as i64)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn reads_a_numeric_balance() {
        let usage = reading(&json!({ "current_point_balance": 1234.0 }));
        assert_eq!(usage.windows[0].label, "Balance");
        assert_eq!(usage.windows[0].detail.as_deref(), Some("1234 points"));
        assert_eq!(usage.windows[0].percent_used, None);
    }

    #[test]
    fn reads_a_balance_sent_as_a_string() {
        let usage = reading(&json!({ "current_point_balance": "1234" }));
        assert_eq!(usage.windows[0].detail.as_deref(), Some("1234 points"));
    }

    #[test]
    fn a_fractional_balance_is_rounded_to_whole_points() {
        assert_eq!(text(12.6), "13 points");
        assert_eq!(text(0.0), "0 points");
    }

    #[test]
    fn a_reply_with_no_usable_figure_is_unreadable() {
        assert!(reading(&json!({ "current_point_balance": "many" })).error.is_some());
        assert!(reading(&json!({})).error.is_some());
    }
}
