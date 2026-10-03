//! OpenAI's API platform: the prepaid credit left on the account, and nothing
//! else.
//!
//! Read with a key the user enters, from the billing route OpenAI's own
//! dashboard used for the balance:
//! `GET https://api.openai.com/v1/dashboard/billing/credit_grants`. It is not
//! in OpenAI's current public reference and it answers older user keys only.
//!
//! **No ring.** The API has no allowance to be a fraction of: what OpenAI
//! reports is money granted, spent and left. The balance is shown as a balance.
//! The organization's spend over the last days — what an Admin key reads — is a
//! total with no limit behind it, and this app has nowhere to show one.
//!
//! **A key the balance route turns away is not yet a bad key.** Admin and
//! project keys are refused there by design, so the refusal is checked against
//! the Admin API's cost route before anything is said: a key that reads costs
//! works, and the honest answer is that it reports no limits.

use std::sync::Arc;

use serde_json::Value;

use super::{describe_reqwest_error, Ctx, FetchFuture, Provider};
use crate::model::ProviderUsage;

const CREDIT_GRANTS: &str = "https://api.openai.com/v1/dashboard/billing/credit_grants";

/// One day of the organization's costs, asked for only to learn whether the key
/// is an Admin key. The figures are not read.
fn costs_probe() -> String {
    let start = chrono::Utc::now().timestamp() - 86_400;
    format!("https://api.openai.com/v1/organization/costs?start_time={start}&limit=1")
}

pub struct OpenAiPlatform;

impl Provider for OpenAiPlatform {
    fn id(&self) -> &'static str {
        "openai-api"
    }

    fn name(&self) -> &'static str {
        "OpenAI API"
    }

    /// The key the fetch reads. Whether the key is a *project* key the balance
    /// route refuses is a question only the service can answer, and the fetch
    /// asks it — this is presence, not permission.
    fn is_configured(&self) -> bool {
        super::provider_key(self.id()).is_some()
    }

    fn fetch(&self, ctx: Arc<Ctx>) -> FetchFuture {
        Box::pin(async move { fetch_inner(ctx).await })
    }
}

async fn fetch_inner(ctx: Arc<Ctx>) -> ProviderUsage {
    const ID: &str = "openai-api";
    const NAME: &str = "OpenAI API";

    let key = match super::provider_key(ID) {
        Some(key) => key,
        None => return ProviderUsage::failed(ID, NAME, super::missing_key(ID, "")),
    };

    let response = ctx
        .client
        .get(CREDIT_GRANTS)
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

    // `ProfileHTTP.classify`'s three answers, kept apart from the request so a
    // test can pin the mapping without a network.
    let code = status.as_u16();
    if !(status.is_success() || code == 429 || code >= 500) {
        // Refused, or a route this key's kind cannot see: ask the Admin API
        // whether the key itself is any good.
        return probe_admin(&ctx, ID, NAME, &key).await;
    }

    if !status.is_success() {
        return ProviderUsage::failed(
            ID,
            NAME,
            if code == 429 {
                "rate limited, try again shortly".to_string()
            } else {
                super::http_failure(status, &[])
            },
        );
    }

    let json: Value = match serde_json::from_str(&body) {
        Ok(v) => v,
        Err(e) => return ProviderUsage::failed(ID, NAME, format!("bad JSON: {e}")),
    };

    super::claude_code::debug_dump(ID, &json);

    reading(&json)
}

/// Does this key work at all? Only the status is read.
async fn probe_admin(ctx: &Ctx, id: &'static str, name: &'static str, key: &str) -> ProviderUsage {
    let response = ctx
        .client
        .get(costs_probe())
        .header("Authorization", format!("Bearer {key}"))
        .header("Accept", "application/json")
        .send()
        .await;

    match response {
        // The key is good; this route's own balance is simply not readable with
        // it, and saying so is the honest answer.
        Ok(r) if r.status().is_success() => ProviderUsage::failed(
            id,
            name,
            "this key reports no balance — an Admin or project key cannot read the credit route",
        ),
        Ok(r) => ProviderUsage::failed(
            id,
            name,
            super::http_failure(
                r.status(),
                &[
                    (401, " — the API key was refused"),
                    (403, " — the API key was refused"),
                    (429, " — rate limited, try again shortly"),
                ],
            ),
        ),
        Err(e) => ProviderUsage::failed(
            id,
            name,
            format!("request failed: {}", describe_reqwest_error(&e)),
        ),
    }
}

/// The route's figures are dollars. It names no currency because the platform
/// bills in one.
fn reading(json: &Value) -> ProviderUsage {
    const ID: &str = "openai-api";
    const NAME: &str = "OpenAI API";

    let granted = json.get("total_granted").and_then(|v| v.as_f64());
    let used = json.get("total_used").and_then(|v| v.as_f64());
    let available = json.get("total_available").and_then(|v| v.as_f64());

    if granted.is_none() && used.is_none() && available.is_none() {
        return ProviderUsage::failed(ID, NAME, "bad reply: no credit figures");
    }

    // A balance that isn't one is left off, and then there is nothing.
    let Some(available) = available.filter(|v| v.is_finite() && *v >= 0.0) else {
        return ProviderUsage::failed(ID, NAME, "no balance reported for this account");
    };

    ProviderUsage::ok(
        ID,
        NAME,
        vec![super::balance_window("Balance", format!("{available:.2} USD"))],
    )
    .with_credit_remaining(available, "USD")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn reads_the_credit_left() {
        let reply = json!({ "total_granted": 20.0, "total_used": 4.5, "total_available": 15.5 });
        let usage = reading(&reply);
        assert_eq!(usage.credit_remaining.as_ref().unwrap().amount, 15.5);
        assert_eq!(usage.credit_remaining.as_ref().unwrap().currency, "USD");
        assert_eq!(usage.windows.len(), 1);
        assert_eq!(usage.windows[0].percent_used, None);
        assert_eq!(usage.windows[0].detail.as_deref(), Some("15.50 USD"));
    }

    #[test]
    fn a_spent_down_account_reads_zero_not_unreadable() {
        let reply = json!({ "total_granted": 20.0, "total_used": 20.0, "total_available": 0.0 });
        let usage = reading(&reply);
        assert_eq!(usage.windows[0].detail.as_deref(), Some("0.00 USD"));
    }

    #[test]
    fn a_balance_that_is_not_one_leaves_nothing_to_show() {
        // A negative figure is not a balance this route reports, and there is
        // no ring to fall back on.
        let reply = json!({ "total_available": -1.0 });
        assert!(reading(&reply).error.is_some());
    }

    #[test]
    fn a_reply_with_no_credit_figures_at_all_is_unreadable() {
        assert!(reading(&json!({ "object": "credit_summary" })).error.is_some());
        assert!(reading(&json!({})).error.is_some());
    }

    #[test]
    fn the_costs_probe_asks_for_one_day_and_one_row() {
        let url = costs_probe();
        assert!(url.starts_with("https://api.openai.com/v1/organization/costs?start_time="));
        assert!(url.ends_with("&limit=1"));
    }
}
