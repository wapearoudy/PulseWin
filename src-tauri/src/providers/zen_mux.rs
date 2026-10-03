//! ZenMux, a model gateway sold by subscription: a rolling five-hour and a
//! rolling seven-day quota, each reported as a share used by the service
//! itself, and a pay-as-you-go balance beside them.
//!
//! Read with a **Management** API key the user enters — ZenMux's ordinary
//! inference keys are refused by these endpoints — from
//! `GET https://zenmux.ai/api/v1/management/subscription/detail`, and the
//! balance from `…/payg/balance`.
//!
//! The balance is best-effort: the quotas are the reading, and a balance that
//! cannot be had leaves them standing rather than failing the refresh. A
//! monthly allowance the reply sizes but does not say how much of is used is
//! left off.

use std::sync::Arc;

use serde_json::Value;

use super::{by_window_length, describe_reqwest_error, percent_from_fraction, Ctx, FetchFuture, Provider};
use crate::model::{parse_reset, ProviderUsage, UsageWindow};

const SUBSCRIPTION_ENDPOINT: &str = "https://zenmux.ai/api/v1/management/subscription/detail";
const BALANCE_ENDPOINT: &str = "https://zenmux.ai/api/v1/management/payg/balance";

pub struct ZenMux;

impl Provider for ZenMux {
    fn id(&self) -> &'static str {
        "zenmux"
    }

    fn name(&self) -> &'static str {
        "ZenMux"
    }

    /// The management key the fetch reads, and nothing else.
    fn is_configured(&self) -> bool {
        super::provider_key(self.id()).is_some()
    }

    fn fetch(&self, ctx: Arc<Ctx>) -> FetchFuture {
        Box::pin(async move { fetch_inner(ctx).await })
    }
}

async fn fetch_inner(ctx: Arc<Ctx>) -> ProviderUsage {
    const ID: &str = "zenmux";
    const NAME: &str = "ZenMux";

    let key = match super::provider_key(ID) {
        Some(key) => key,
        None => return ProviderUsage::failed(ID, NAME, super::missing_key(ID, "")),
    };

    let subscription = match ask(&ctx, SUBSCRIPTION_ENDPOINT, &key).await {
        Ok(json) => json,
        Err(message) => return ProviderUsage::failed(ID, NAME, message),
    };

    // Asked only once the quotas are in hand; whatever becomes of it, the
    // quotas are what is shown.
    let balance = ask(&ctx, BALANCE_ENDPOINT, &key).await.ok();

    reading(&subscription, balance.as_ref())
}

async fn ask(ctx: &Ctx, url: &str, key: &str) -> Result<Value, String> {
    let response = ctx
        .client
        .get(url)
        .header("Authorization", format!("Bearer {key}"))
        .header("Accept", "application/json")
        .send()
        .await
        .map_err(|e| format!("request failed: {}", describe_reqwest_error(&e)))?;

    let status = response.status();
    let body = response
        .text()
        .await
        .map_err(|e| format!("cannot read body: {e}"))?;

    if !status.is_success() {
        let hints: &[(u16, &str)] = &[
            (401, " — the API key was refused"),
            (403, " — the API key was refused"),
            (429, " — rate limited, try again shortly"),
        ];
        return Err(super::http_failure(status, hints));
    }

    serde_json::from_str(&body).map_err(|e| format!("bad JSON: {e}"))
}

/// The replies are snake_case, and the original reads them with a decoder that
/// converts snake_case to camelCase — which means a key written without
/// underscores matches under either spelling. Both are read here for the same
/// reason: the service may write either, and the original accepts either.
fn field<'a>(object: &'a Value, names: &[&str]) -> Option<&'a Value> {
    names.iter().find_map(|name| object.get(*name))
}

/// The pay-as-you-go balance, in the currency ZenMux names. Kept negative when
/// it is: an overdue account is not an empty one.
fn balance(reply: &Value) -> Option<(f64, String)> {
    if reply.get("success").and_then(|v| v.as_bool()) != Some(true) {
        return None;
    }
    let data = reply.get("data")?;
    let amount = field(data, &["total_credits", "totalCredits"])?.as_f64()?;
    if !amount.is_finite() {
        return None;
    }
    let currency = data
        .get("currency")
        .and_then(|v| v.as_str())?
        .trim()
        .to_uppercase();
    // A currency code, not a word: three letters and nothing else.
    if currency.chars().count() != 3 || !currency.chars().all(|c| c.is_ascii_alphabetic()) {
        return None;
    }
    Some((amount, currency))
}

/// The mapping, kept apart from the request so a fixture can drive it.
fn reading(subscription: &Value, balance_reply: Option<&Value>) -> ProviderUsage {
    const ID: &str = "zenmux";
    const NAME: &str = "ZenMux";

    if subscription.get("success").and_then(|v| v.as_bool()) != Some(true) {
        return ProviderUsage::failed(ID, NAME, "bad reply: the service did not report success");
    }
    let Some(detail) = subscription.get("data") else {
        return ProviderUsage::failed(ID, NAME, "bad reply: no subscription detail");
    };

    // Both quotas are named for their length, so the length is stated.
    let named: [(&[&str], &str, i64); 2] = [
        (&["quota_5_hour", "quota5Hour"], "5h", 5 * 3_600),
        (&["quota_7_day", "quota7Day"], "7d", 7 * 86_400),
    ];

    let mut rows: Vec<(i64, UsageWindow)> = Vec::new();
    for (keys, label, seconds) in named {
        let Some(quota) = field(detail, keys) else {
            continue;
        };
        let Some(fraction) = field(quota, &["usage_percentage", "usagePercentage"])
            .and_then(Value::as_f64)
            .filter(|fraction| fraction.is_finite() && *fraction >= 0.0)
        else {
            continue;
        };
        // `usage_percentage` is written as a fraction: 0.0715 for 7.15%.
        rows.push((
            seconds,
            UsageWindow::new(label, Some(percent_from_fraction(fraction))).with_reset(
                field(quota, &["resets_at", "resetsAt"]).and_then(parse_reset),
            ),
        ));
    }

    let windows = by_window_length(rows);
    let payg = balance_reply.and_then(balance);

    if windows.is_empty() && payg.is_none() {
        return ProviderUsage::failed(ID, NAME, "no limits reported");
    }

    let mut windows = windows;
    if let Some((amount, currency)) = &payg {
        windows.push(super::balance_window("Balance", format!("{amount:.2} {currency}")));
    }

    let plan = detail
        .get("plan")
        .and_then(|plan| plan.get("tier"))
        .and_then(|v| v.as_str())
        .map(|tier| title(tier.trim()))
        .filter(|tier| !tier.is_empty());

    let mut usage = ProviderUsage::ok(ID, NAME, windows).with_plan(plan);
    if let Some((amount, currency)) = payg { usage = usage.with_credit_remaining(amount, currency); }
    usage
}

/// `pro` → `Pro`, `business-pro` → `Business-pro`: the first letter alone, as
/// the original writes it.
fn title(tier: &str) -> String {
    let mut chars = tier.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().collect::<String>() + &chars.as_str().to_lowercase(),
        None => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn fixture() -> Value {
        json!({
            "success": true,
            "data": {
                "plan": { "tier": "pro" },
                "quota_5_hour": { "usage_percentage": 0.0715, "resets_at": "2026-10-02T07:00:00Z" },
                "quota_7_day": { "usage_percentage": 0.42, "resets_at": "2026-10-09T00:00:00Z" }
            }
        })
    }

    fn balance_fixture() -> Value {
        json!({ "success": true, "data": { "currency": "usd", "total_credits": 12.5 } })
    }

    #[test]
    fn reads_the_two_quotas_shortest_first() {
        let usage = reading(&fixture(), Some(&balance_fixture()));
        assert_eq!(usage.credit_remaining.as_ref().unwrap().amount, 12.5);
        assert_eq!(usage.credit_remaining.as_ref().unwrap().currency, "USD");
        let labels: Vec<&str> = usage.windows.iter().map(|w| w.label.as_str()).collect();
        assert_eq!(labels, vec!["5h", "7d", "Balance"]);
        // 0.0715 is a fraction: 7.15%, not 0.0715%. The multiplication lands a
        // hair under the decimal, as it does for any such fraction.
        let five_hour = usage.windows[0].percent_used.unwrap();
        assert!((five_hour - 7.15).abs() < 1e-9, "{five_hour}");
        assert_eq!(usage.windows[1].percent_used, Some(42.0));
        assert_eq!(
            usage.windows[0].resets_at.as_deref(),
            Some("2026-10-02T07:00:00Z")
        );
        assert_eq!(usage.plan.as_deref(), Some("Pro"));
    }

    /// The service writes snake_case; a key with no underscores is the same
    /// name either way, and the original's decoder accepts both.
    #[test]
    fn the_camel_case_spelling_is_read_too() {
        let camel = json!({
            "success": true,
            "data": {
                "plan": { "tier": "pro" },
                "quota5Hour": { "usagePercentage": 0.5, "resetsAt": "2026-10-02T07:00:00Z" }
            }
        });
        let usage = reading(&camel, None);
        assert_eq!(usage.windows[0].percent_used, Some(50.0));
        assert_eq!(
            usage.windows[0].resets_at.as_deref(),
            Some("2026-10-02T07:00:00Z")
        );

        let camel_balance = json!({ "success": true, "data": { "currency": "USD", "totalCredits": 5.0 } });
        assert_eq!(
            reading(&fixture(), Some(&camel_balance)).windows[2]
                .detail
                .as_deref(),
            Some("5.00 USD")
        );
    }

    #[test]
    fn the_balance_is_drawn_as_a_balance_and_names_its_currency() {
        let usage = reading(&fixture(), Some(&balance_fixture()));
        assert_eq!(usage.windows[2].detail.as_deref(), Some("12.50 USD"));
        assert_eq!(usage.windows[2].percent_used, None);
    }

    /// An overdue account is not an empty one.
    #[test]
    fn a_negative_balance_is_kept() {
        let owing = json!({ "success": true, "data": { "currency": "USD", "totalCredits": -3.25 } });
        let usage = reading(&fixture(), Some(&owing));
        assert_eq!(usage.windows[2].detail.as_deref(), Some("-3.25 USD"));
    }

    /// The quotas are the reading: a balance that cannot be had leaves them
    /// standing, and a balance that is there stands alone when they cannot.
    #[test]
    fn either_half_stands_without_the_other() {
        let quotas_only = reading(&fixture(), None);
        assert_eq!(quotas_only.windows.len(), 2);

        let no_quotas = json!({ "success": true, "data": { "plan": { "tier": "free" } } });
        let balance_only = reading(&no_quotas, Some(&balance_fixture()));
        assert_eq!(balance_only.windows.len(), 1);
        assert_eq!(balance_only.windows[0].label, "Balance");
    }

    #[test]
    fn a_reply_this_build_cannot_read_is_unreadable() {
        assert!(reading(&json!({}), None).error.is_some());
        assert!(reading(&json!({ "success": false, "data": {} }), None).error.is_some());
        assert!(reading(&json!({ "success": true }), None).error.is_some());
        // A negative share is not a reading.
        let negative = json!({ "success": true,
            "data": { "quota_5_hour": { "usage_percentage": -0.5 } } });
        assert!(reading(&negative, None).error.is_some());
    }

    #[test]
    fn a_balance_without_a_currency_is_no_balance() {
        let unnamed = json!({ "success": true, "data": { "totalCredits": 5.0 } });
        let usage = reading(&fixture(), Some(&unnamed));
        assert_eq!(usage.windows.len(), 2);
    }
}
