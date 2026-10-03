//! Charm's Hyper: the account's Hypercredit balance, and nothing else — the
//! service reports no allowance and no period, so there is no ring.
//!
//! Read with a key the user enters, from Hyper's credits route:
//! `GET https://hyper.charm.land/v1/credits`.
//!
//! The reference also tries a browser session for hyper.charm.land before the
//! key. This port has none: reading a Chromium cookie store needs its
//! decryption key out of the login keychain, which is macOS-only — and the key
//! reaches the same figure. Hypercredits are Charm's own unit rather than
//! money, so the balance is shown and never compared against a currency.

use std::sync::Arc;

use serde_json::Value;

use super::{describe_reqwest_error, Ctx, FetchFuture, Provider};
use crate::model::ProviderUsage;

const ENDPOINT: &str = "https://hyper.charm.land/v1/credits";

pub struct Hyper;

impl Provider for Hyper {
    fn id(&self) -> &'static str {
        "hyper"
    }

    fn name(&self) -> &'static str {
        "Hyper"
    }

    /// The key the fetch reads. The browser session the reference also tries is
    /// not a route this port has, so it is not one this check counts.
    fn is_configured(&self) -> bool {
        super::provider_key(self.id()).is_some()
    }

    fn fetch(&self, ctx: Arc<Ctx>) -> FetchFuture {
        Box::pin(async move { fetch_inner(ctx).await })
    }
}

async fn fetch_inner(ctx: Arc<Ctx>) -> ProviderUsage {
    const ID: &str = "hyper";
    const NAME: &str = "Hyper";

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
    const ID: &str = "hyper";
    const NAME: &str = "Hyper";

    let Some(balance) = json
        .get("balance")
        .and_then(|v| v.as_f64())
        .filter(|balance| balance.is_finite() && *balance >= 0.0)
    else {
        return ProviderUsage::failed(ID, NAME, "no balance in response");
    };

    ProviderUsage::ok(ID, NAME, vec![super::balance_window("Balance", credits_text(balance))])
}

/// "1,234.5 HC". HC is Charm's own abbreviation for Hypercredits, the unit's
/// name, and reads the same in every language.
///
/// The original formats with the user's locale, which also supplies the
/// thousands separators; this port has no locale of its own, so the number is
/// written plainly and the trailing zeroes are trimmed rather than padded.
fn credits_text(balance: f64) -> String {
    let mut text = format!("{balance:.2}");
    if text.contains('.') {
        text = text.trim_end_matches('0').trim_end_matches('.').to_string();
    }
    format!("{text} HC")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn reads_the_hypercredit_balance() {
        let usage = reading(&json!({ "balance": 1234.5 }));
        assert_eq!(usage.windows[0].label, "Balance");
        assert_eq!(usage.windows[0].detail.as_deref(), Some("1234.5 HC"));
        assert_eq!(usage.windows[0].percent_used, None);
    }

    #[test]
    fn a_whole_balance_carries_no_decimal_point() {
        assert_eq!(credits_text(1200.0), "1200 HC");
        assert_eq!(credits_text(0.0), "0 HC");
    }

    #[test]
    fn a_negative_balance_is_not_a_reading() {
        assert!(reading(&json!({ "balance": -1.0 })).error.is_some());
        assert!(reading(&json!({ "balance": "12" })).error.is_some());
        assert!(reading(&json!({})).error.is_some());
    }
}
