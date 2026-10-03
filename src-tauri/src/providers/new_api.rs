//! A New API deployment's remaining credit, read with an `sk-` key.
//!
//! [New API](https://github.com/QuantumNous/new-api) is the gateway most
//! self-run relays are built on — a fork line going back to one-api — and it
//! answers the **OpenAI-compatible billing routes**, which is what makes it
//! readable at all. The key is the ordinary one already in the reader's client
//! config; there is no separate credential to go and find.
//!
//! **A key and an address are both required, and this port has no settings
//! window to type them into.** They arrive from `PULSEWIN_NEW_API_KEY` and
//! `PULSEWIN_NEW_API_BASE_URL`, or from one `%APPDATA%\PulseWin\new-api.json`
//! holding `apiKey` and `baseUrl`; people paste a gateway's root, and people
//! paste the base URL out of their client's config — which for an
//! OpenAI-compatible endpoint ends in `/v1`. `is_configured` asks for both, and
//! asks only from disk, never over the network.
//!
//! Three requests, run together. The first two carry the key:
//!
//! ```text
//! GET {root}/v1/dashboard/billing/subscription   → hard_limit_usd, access_until
//! GET {root}/v1/dashboard/billing/usage          → total_usage
//! GET {root}/api/status                          → quota_display_type   (no key)
//! ```
//!
//! ```json
//! { "object": "billing_subscription", "has_payment_method": true,
//!   "soft_limit_usd": 25, "hard_limit_usd": 25,
//!   "system_hard_limit_usd": 25, "access_until": 0 }
//! { "object": "list", "total_usage": 1234.5 }
//! ```
//!
//! ## Two figures, and neither means what its name says
//!
//! **`hard_limit_usd` is not a limit.** New API computes it as
//! `remaining + used` — see `controller/billing.go` — so it is everything the
//! key has ever had rather than a ceiling. **`total_usage` is in hundredths**,
//! because OpenAI's own route reported cents and the compatibility is literal.
//! So what is actually read is:
//!
//! ```text
//! remaining = hard_limit_usd − total_usage / 100
//! ```
//!
//! Both sides are New API's own numbers, so the subtraction is arithmetic on
//! reported figures rather than an inference.
//!
//! ## And no percentage is drawn from them
//!
//! `used / (remaining + used)` looks like a ring and is not one. Which of two
//! unrelated things that denominator is depends on a **server** setting,
//! `DisplayTokenStatEnabled`, that appears nowhere in the reply:
//!
//! - token stats **on** — the figures are the key's own, and a key with a quota
//!   set on it really does have that allowance, so the fraction would be right.
//! - token stats **off** — the figures are the *account's*, and `used` is
//!   lifetime. Somebody who has spent $900 over a year and just topped up $100
//!   would be shown as 90% spent with a full wallet, and every top-up would
//!   grow the denominator.
//!
//! One reply, two meanings, no way to tell them apart. So this draws the money
//! and no fraction, exactly as a sub2api wallet and DeepSeek's balance-only mode
//! do — the money rides on the detail line of a window with no ring.
//!
//! ## The unit is not in the reply either
//!
//! The `_usd` in those field names is a lie on most deployments: New API
//! converts into whatever its operator set as the display type and keeps the
//! OpenAI field name. A CNY site reports yuan in `hard_limit_usd`; a TOKENS site
//! reports a token count. `GET /api/status` is public, needs no key, and carries
//! `quota_display_type` — so it is asked, and a site whose unit is not a
//! currency reports **no money at all** rather than tokens wearing a dollar
//! sign.

use std::sync::Arc;

use serde_json::Value;

use super::{balance_window, Ctx, FetchFuture, Provider};
use crate::model::ProviderUsage;

const SUBSCRIPTION_PATH: &str = "/v1/dashboard/billing/subscription";
const USAGE_PATH: &str = "/v1/dashboard/billing/usage";
const STATUS_PATH: &str = "/api/status";

/// What New API substitutes for the amount when a key has `UnlimitedQuota` set
/// — a literal in `controller/billing.go` rather than a flag in the reply,
/// which is why it has to be recognised by value.
const UNLIMITED_SENTINEL: f64 = 100_000_000.0;

/// People paste a gateway's root, and people paste the base URL out of their
/// client's config, which ends in `/v1`.
const TRIMMING: [&str; 1] = ["/v1"];

pub struct NewApi;

impl Provider for NewApi {
    fn id(&self) -> &'static str {
        "new-api"
    }

    fn name(&self) -> &'static str {
        "New API"
    }

    /// The key **and** the address the fetch needs. See the module docs.
    fn is_configured(&self) -> bool {
        super::gateway_configured(self.id())
    }

    fn fetch(&self, ctx: Arc<Ctx>) -> FetchFuture {
        Box::pin(async move { fetch_inner(ctx).await })
    }
}

async fn fetch_inner(ctx: Arc<Ctx>) -> ProviderUsage {
    const ID: &str = "new-api";
    const NAME: &str = "New API";

    let Some(key) = super::provider_key(ID) else {
        return ProviderUsage::failed(ID, NAME, super::missing_key(ID, ""));
    };
    let Some(address) = super::provider_base_url(ID) else {
        return ProviderUsage::failed(ID, NAME, super::missing_address(ID));
    };

    let (Some(subscription_url), Some(usage_url), Some(status_url)) = (
        super::gateway_url(&address, SUBSCRIPTION_PATH, &TRIMMING),
        super::gateway_url(&address, USAGE_PATH, &TRIMMING),
        super::gateway_url(&address, STATUS_PATH, &TRIMMING),
    ) else {
        return ProviderUsage::failed(ID, NAME, super::refused_address());
    };

    // All three at once, as the reference asks them: two of them are the same
    // site answering, and the third is a public route.
    let (subscription, usage, status) = futures::future::join3(
        get(&ctx, &subscription_url, Some(&key)),
        get(&ctx, &usage_url, Some(&key)),
        // **No key on this one.** It is the site's public configuration and
        // nothing about it is scoped to an account, so there is no reason to
        // hand it a credential.
        get(&ctx, &status_url, None),
    )
    .await;

    let subscription = match subscription {
        Ok(reply) => reply,
        Err(problem) => return ProviderUsage::failed(ID, NAME, problem),
    };

    // A key with no ceiling at all. A complete answer, not a fault — and
    // `remaining` computed against the sentinel would be a hundred million of
    // something.
    if is_unlimited(&subscription) {
        return ProviderUsage::failed(ID, NAME, "no limits reported — this key has no ceiling");
    }

    let usage = match usage {
        Ok(reply) => reply,
        Err(problem) => return ProviderUsage::failed(ID, NAME, problem),
    };

    let status = status.ok();

    // A status route that did not answer leaves the unit unknown. That is not a
    // reason to fail the reading, it is a reason not to put a currency symbol
    // on a number — so this reports nothing rather than guessing dollars.
    let (Some(remaining), Some(currency)) = (remaining(&subscription, &usage), currency(status.as_ref()))
    else {
        return ProviderUsage::failed(
            ID,
            NAME,
            "no limits reported — the deployment does not say what its figures are denominated in",
        );
    };

    let plan = status
        .as_ref()
        .and_then(|status| status.get("data"))
        .and_then(|data| data.get("system_name"))
        .and_then(Value::as_str)
        .filter(|name| !name.is_empty())
        .map(str::to_string);

    balance_usage(remaining, currency, plan)
}

fn balance_usage(remaining: f64, currency: String, plan: Option<String>) -> ProviderUsage {
    ProviderUsage::ok(
        "new-api",
        "New API",
        vec![balance_window(
            "Balance",
            format!("{remaining:.2} {currency}"),
        )],
    )
    .with_plan(plan)
    .with_credit_remaining(remaining, currency)
}

/// One route's reply, or the line that says why there is none.
///
/// Only 200 is a reading, which is the reference's own switch: the two billing
/// routes are OpenAI-compatible and answer a refusal with a status, so anything
/// else is the deployment failing rather than a shape to read.
async fn get(ctx: &Ctx, url: &str, key: Option<&str>) -> Result<Value, String> {
    let mut request = ctx
        .gateway_client
        .get(url)
        .header("Accept", "application/json");

    if let Some(key) = key {
        request = request.header("Authorization", format!("Bearer {key}"));
    }

    let response = request
        .send()
        .await
        .map_err(|e| format!("request failed: {}", super::describe_reqwest_error(&e)))?;

    let status = response.status();
    let body = response
        .text()
        .await
        .map_err(|e| format!("cannot read body: {e}"))?;

    if status != reqwest::StatusCode::OK {
        return Err(match status.as_u16() {
            401 | 403 => "the deployment refused the key".to_string(),
            429 => "rate limited, try again shortly".to_string(),
            300..=399 => {
                "the deployment redirected the request — the key was not accepted".to_string()
            }
            _ => format!("the deployment returned {}", status.as_u16()),
        });
    }

    serde_json::from_str(&body).map_err(|e| format!("bad JSON: {e}"))
}

/// What is left: New API's own total, less New API's own usage.
///
/// **Hundredths, not units.** `total_usage` is the amount times a hundred, and
/// reading it as units makes a wallet look a hundred times emptier than it is —
/// which is a notification announcing an account as spent.
fn remaining(subscription: &Value, usage: &Value) -> Option<f64> {
    let total = super::dig_number(subscription.get("hard_limit_usd"))
        .filter(|total| total.is_finite())?;
    let used = super::dig_number(usage.get("total_usage")).filter(|used| used.is_finite())?;

    Some(total - used / 100.0)
}

/// Whether this key has no ceiling.
///
/// New API substitutes a literal hundred million for the amount rather than
/// setting a flag, so this is recognised by value. All three fields are written
/// from the same variable, so agreement across them is what separates the
/// sentinel from a deployment that genuinely sold somebody exactly that much.
fn is_unlimited(subscription: &Value) -> bool {
    ["hard_limit_usd", "soft_limit_usd", "system_hard_limit_usd"]
        .iter()
        .all(|field| super::dig_number(subscription.get(*field)) == Some(UNLIMITED_SENTINEL))
}

/// What the figures are denominated in, or `None` when it cannot be said.
///
/// **None rather than a default.** The `_usd` in the field names is not
/// evidence: New API converts into the operator's chosen display type and keeps
/// OpenAI's field name, so a CNY site reports yuan in `hard_limit_usd`. A site
/// counting tokens or a custom unit is not money at all and reports none rather
/// than tokens wearing a dollar sign.
///
/// The older `display_in_currency` is read only when the newer field is absent
/// — it distinguishes money from tokens and nothing more, so it can only ever
/// answer USD, which is what the builds that carried it assumed.
fn currency(status: Option<&Value>) -> Option<String> {
    let payload = status?.get("data")?;

    let display_type = payload
        .get("quota_display_type")
        .and_then(Value::as_str)
        .map(|text| text.trim().to_uppercase())
        .filter(|text| !text.is_empty());

    if let Some(display_type) = display_type {
        return match display_type.as_str() {
            "USD" => Some("USD".to_string()),
            "CNY" => Some("CNY".to_string()),
            // Tokens and an operator's own unit are not currencies. Neither is
            // anything this does not recognise: a new display type added
            // upstream must not be quietly priced in dollars.
            _ => None,
        };
    }

    match payload.get("display_in_currency").and_then(Value::as_bool) {
        Some(true) => Some("USD".to_string()),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// The two billing replies and the status route, as the service documents
    /// them.
    fn fixture() -> (Value, Value) {
        (
            json!({
                "object": "billing_subscription",
                "has_payment_method": true,
                "soft_limit_usd": 25.0,
                "hard_limit_usd": 25.0,
                "system_hard_limit_usd": 25.0,
                "access_until": 0
            }),
            json!({ "object": "list", "total_usage": 1234.5 }),
        )
    }

    fn reading(subscription: &Value, usage: &Value, status: Option<&Value>) -> ProviderUsage {
        // The same mapping the fetch runs, kept here so a fixture can drive it
        // without a network.
        let Some(remaining) = remaining(subscription, usage) else {
            return ProviderUsage::failed("new-api", "New API", "no limits reported");
        };
        let Some(currency) = currency(status) else {
            return ProviderUsage::failed("new-api", "New API", "no limits reported");
        };
        balance_usage(remaining, currency, None)
    }

    #[test]
    fn what_is_left_is_the_total_less_the_usage_in_hundredths() {
        let (subscription, usage) = fixture();
        // 25 − 1234.5/100 = 12.655.
        let left = remaining(&subscription, &usage).unwrap();
        assert!((left - 12.655).abs() < 1e-9, "{left}");

        let status = json!({ "success": true, "data": { "quota_display_type": "USD" } });
        let usage = reading(&subscription, &usage, Some(&status));
        assert!((usage.credit_remaining.as_ref().unwrap().amount - 12.655).abs() < 1e-9);
        assert_eq!(usage.credit_remaining.as_ref().unwrap().currency, "USD");
        assert_eq!(usage.windows[0].label, "Balance");
        // 12.654999999999998, which is the arithmetic and not a tidier number:
        // the two-decimal rendering of it is what the original would print.
        assert_eq!(usage.windows[0].detail.as_deref(), Some("12.65 USD"));
        // No fraction, ever: the only one available means two different things.
        assert_eq!(usage.windows[0].percent_used, None);
    }

    #[test]
    fn reading_the_usage_as_units_would_empty_a_wallet_a_hundred_times_over() {
        let (subscription, usage) = fixture();
        let left = remaining(&subscription, &usage).unwrap();
        // The figure as written, not divided by a hundred, would be negative.
        assert!(left > 0.0);
        assert_eq!(usage["total_usage"], json!(1234.5));
    }

    #[test]
    fn a_ceiling_of_a_hundred_million_in_all_three_fields_is_no_ceiling() {
        let unlimited = json!({
            "hard_limit_usd": 100_000_000.0,
            "soft_limit_usd": 100_000_000.0,
            "system_hard_limit_usd": 100_000_000.0
        });
        assert!(is_unlimited(&unlimited));

        // Agreement across the fields is what separates the sentinel from a
        // deployment that really sold somebody that much.
        let nearly = json!({
            "hard_limit_usd": 100_000_000.0,
            "soft_limit_usd": 100_000_000.0,
            "system_hard_limit_usd": 99_999_999.0
        });
        assert!(!is_unlimited(&nearly));

        // A field that is absent has not agreed to anything.
        let missing = json!({
            "hard_limit_usd": 100_000_000.0,
            "soft_limit_usd": 100_000_000.0
        });
        assert!(!is_unlimited(&missing));
    }

    #[test]
    fn the_unit_comes_from_the_site_and_is_never_assumed() {
        let named = |data: Value| currency(Some(&json!({ "success": true, "data": data })));

        assert_eq!(named(json!({ "quota_display_type": "USD" })).as_deref(), Some("USD"));
        assert_eq!(named(json!({ "quota_display_type": "cny" })).as_deref(), Some("CNY"));
        // Tokens, a custom unit, and a type nobody here has seen: not money.
        assert_eq!(named(json!({ "quota_display_type": "TOKENS" })), None);
        assert_eq!(named(json!({ "quota_display_type": "CUSTOM" })), None);
        assert_eq!(named(json!({ "quota_display_type": "EUR" })), None);
        // The older field, only when the newer one says nothing.
        assert_eq!(
            named(json!({ "display_in_currency": true })).as_deref(),
            Some("USD")
        );
        assert_eq!(named(json!({ "display_in_currency": false })), None);
        assert_eq!(
            named(json!({ "quota_display_type": "TOKENS", "display_in_currency": true })),
            None
        );
        // Nothing at all: no unit, so no money is shown.
        assert_eq!(named(json!({})), None);
        assert_eq!(currency(None), None);
        assert_eq!(currency(Some(&json!({ "success": true }))), None);
    }

    #[test]
    fn a_status_route_that_did_not_answer_costs_the_currency_and_nothing_else() {
        let (subscription, usage) = fixture();
        // The figures are still arithmetic; there is simply no name for them.
        let left = remaining(&subscription, &usage).unwrap();
        assert!((left - 12.655).abs() < 1e-9, "{left}");
        assert_eq!(currency(None), None);
        assert!(reading(&subscription, &usage, None).error.is_some());
    }

    #[test]
    fn a_reply_without_the_two_figures_is_no_reading() {
        let usage = json!({ "total_usage": 1.0 });
        assert_eq!(remaining(&json!({}), &usage), None);
        assert_eq!(remaining(&json!({ "hard_limit_usd": 1.0 }), &json!({})), None);
    }

    #[test]
    fn the_plan_is_the_sites_own_name() {
        let status = json!({ "success": true, "data": {
            "quota_display_type": "USD", "system_name": "My Relay"
        } });
        // The fetch reads the plan from the same reply the currency comes from.
        assert_eq!(
            status
                .get("data")
                .and_then(|data| data.get("system_name"))
                .and_then(Value::as_str),
            Some("My Relay")
        );
    }
}
