//! Neuralwatt, inference priced by energy: a subscription's kilowatt-hour
//! allowance for the current period, the key's own spending allowance where one
//! is set, and the prepaid balance in US dollars.
//!
//! Read with an API key the user enters, from
//! `GET https://api.neuralwatt.com/v1/quota`. The reply is snake_case
//! throughout.
//!
//! The prepaid balance and the subscription are separate things: credits do not
//! reset and are spent as you go, the allowance is billed against kWh and turns
//! over with the period. Neither is folded into the other. A key that has been
//! blocked is not drawn as a full ring — the flag is not a figure — and the
//! month's spend, which has no limit beside it, is left out.

use std::sync::Arc;

use chrono::{DateTime, SecondsFormat};
use serde_json::Value;

use super::{by_window_length, describe_reqwest_error, percent_from_fraction, Ctx, FetchFuture, Provider};
use crate::model::{parse_reset, ProviderUsage, UsageWindow};

const ENDPOINT: &str = "https://api.neuralwatt.com/v1/quota";

const MONTH: i64 = 30 * 86_400;

pub struct Neuralwatt;

impl Provider for Neuralwatt {
    fn id(&self) -> &'static str {
        "neuralwatt"
    }

    fn name(&self) -> &'static str {
        "Neuralwatt"
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
    const ID: &str = "neuralwatt";
    const NAME: &str = "Neuralwatt";

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

fn number(value: Option<&Value>) -> Option<f64> {
    value.and_then(Value::as_f64)
}

/// The reply is snake_case, and the original reads it with a decoder that
/// converts snake_case to camelCase — which means a key written without
/// underscores matches under either spelling. Both are read here for the same
/// reason: the service may write either, and the original accepts either.
fn field<'a>(object: &'a Value, names: &[&str]) -> Option<&'a Value> {
    names.iter().find_map(|name| object.get(*name))
}

fn non_negative(value: Option<&Value>) -> Option<f64> {
    number(value).filter(|value| value.is_finite() && *value >= 0.0)
}

fn positive(value: Option<&Value>) -> Option<f64> {
    number(value).filter(|value| value.is_finite() && *value > 0.0)
}

fn instant(value: Option<&Value>) -> Option<DateTime<chrono::Utc>> {
    let text = parse_reset(value?)?;
    DateTime::parse_from_rfc3339(&text)
        .ok()
        .map(|at| at.with_timezone(&chrono::Utc))
}

/// The subscription's kWh for this period. The allowance is the one stated, or
/// failing that what was used plus what is left — both stated.
fn allowance(subscription: &Value) -> Option<(i64, UsageWindow)> {
    let used = non_negative(field(subscription, &["kwh_used", "kwhUsed"]))?;
    let included = positive(field(subscription, &["kwh_included", "kwhIncluded"])).or_else(|| {
        non_negative(field(subscription, &["kwh_remaining", "kwhRemaining"]))
            .and_then(|left| positive_f64(used + left))
    })?;

    // The period is stated by its two ends, so its length may be divided by.
    // Without both, thirty days is only where it sorts.
    let start = instant(field(
        subscription,
        &["current_period_start", "currentPeriodStart"],
    ));
    let end = instant(field(
        subscription,
        &["current_period_end", "currentPeriodEnd"],
    ));
    let stated = start
        .zip(end)
        .map(|(start, end)| (end - start).num_seconds())
        .filter(|seconds| *seconds > 0);

    let monthly = matches!(
        field(subscription, &["billing_interval", "billingInterval"])
            .and_then(|v| v.as_str())
            .map(str::to_lowercase)
            .as_deref(),
        Some("month") | Some("monthly")
    );

    let (label, seconds) = if monthly {
        ("Monthly".to_string(), stated.unwrap_or(MONTH))
    } else if let Some(stated) = stated {
        (super::humanize_window_seconds(stated), stated)
    } else {
        ("Credits".to_string(), MONTH)
    };

    let fraction = used / included;
    Some((
        seconds,
        UsageWindow::new(label, Some(percent_from_fraction(fraction)))
            .with_reset(end.map(|end| end.to_rfc3339_opts(SecondsFormat::Secs, true))),
    ))
}

fn positive_f64(value: f64) -> Option<f64> {
    (value.is_finite() && value > 0.0).then_some(value)
}

/// The key's own spending allowance, where one is set. It never resets, so it
/// carries no clock whatever period the reply names.
fn key_allowance(allowance: &Value) -> Option<(i64, UsageWindow)> {
    let spent = non_negative(field(allowance, &["spent_usd", "spentUsd"]))?;
    let limit = positive(field(allowance, &["limit_usd", "limitUsd"]))?;

    let (label, seconds) = match allowance
        .get("period")
        .and_then(|v| v.as_str())
        .map(str::to_lowercase)
        .as_deref()
    {
        Some("daily") | Some("day") => ("Daily", 86_400),
        Some("weekly") | Some("week") => ("Weekly", 7 * 86_400),
        _ => ("Monthly", MONTH),
    };

    Some((
        seconds,
        UsageWindow::new(label, Some(percent_from_fraction(spent / limit))),
    ))
}

/// Prepaid credit left, as stated, or the total less what was used when only
/// those two are.
fn remaining(balance: &Value) -> Option<f64> {
    if let Some(remaining) = non_negative(field(balance, &["credits_remaining_usd", "creditsRemainingUsd"]))
    {
        return Some(remaining);
    }
    let total = non_negative(field(balance, &["total_credits_usd", "totalCreditsUsd"]))?;
    let used = non_negative(field(balance, &["credits_used_usd", "creditsUsedUsd"]))?;
    Some((total - used).max(0.0))
}

fn plan(raw: Option<&Value>) -> Option<String> {
    let plan = raw?.as_str()?.trim();
    if plan.is_empty() {
        return None;
    }
    let words: Vec<String> = plan
        .split('_')
        .map(|word| {
            let mut chars = word.chars();
            match chars.next() {
                Some(first) => first.to_uppercase().collect::<String>() + &chars.as_str().to_lowercase(),
                None => String::new(),
            }
        })
        .collect();
    Some(words.join(" "))
}

/// The mapping, kept apart from the request so a fixture can drive it.
fn reading(json: &Value) -> ProviderUsage {
    const ID: &str = "neuralwatt";
    const NAME: &str = "Neuralwatt";

    let Some(balance) = json.get("balance") else {
        return ProviderUsage::failed(ID, NAME, "bad reply: no balance object");
    };

    // The subscription first: it is what the plan is, and it drives the ring.
    // The key's own allowance, where set, beside it.
    let mut rows: Vec<(i64, UsageWindow)> = Vec::new();
    if let Some(subscription) = json.get("subscription") {
        if let Some(row) = allowance(subscription) {
            rows.push(row);
        }
    }
    if let Some(allowance) = json.get("key").and_then(|key| key.get("allowance")) {
        if let Some(row) = key_allowance(allowance) {
            rows.push(row);
        }
    }

    let prepaid = remaining(balance);
    if rows.is_empty() && prepaid.is_none() {
        return ProviderUsage::failed(ID, NAME, "no limits reported");
    }

    let mut windows = by_window_length(rows);
    if let Some(prepaid) = prepaid {
        windows.push(super::balance_window("Balance", format!("{prepaid:.2} USD")));
    }

    let mut usage = ProviderUsage::ok(ID, NAME, windows).with_plan(plan(json.get("subscription").and_then(|s| s.get("plan"))));
    if let Some(prepaid) = prepaid { usage = usage.with_credit_remaining(prepaid, "USD"); }
    usage
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn fixture() -> Value {
        json!({
            "balance": { "credits_remaining_usd": 12.5, "total_credits_usd": 20.0,
                         "credits_used_usd": 7.5 },
            "subscription": {
                "plan": "pro_plan",
                "billing_interval": "month",
                "current_period_start": "2026-09-15T00:00:00Z",
                "current_period_end": "2026-10-15T00:00:00Z",
                "kwh_included": 100.0,
                "kwh_used": 25.0,
                "kwh_remaining": 75.0
            },
            "key": { "allowance": { "limit_usd": 50.0, "spent_usd": 5.0, "period": "weekly" } }
        })
    }

    #[test]
    fn reads_the_subscription_and_the_key_allowance_shortest_first() {
        let usage = reading(&fixture());
        assert_eq!(usage.credit_remaining.as_ref().unwrap().amount, 12.5);
        assert_eq!(usage.credit_remaining.as_ref().unwrap().currency, "USD");
        let labels: Vec<&str> = usage.windows.iter().map(|w| w.label.as_str()).collect();
        assert_eq!(labels, vec!["Weekly", "Monthly", "Balance"]);
        assert_eq!(usage.windows[0].percent_used, Some(10.0));
        assert_eq!(usage.windows[1].percent_used, Some(25.0));
        assert_eq!(usage.plan.as_deref(), Some("Pro Plan"));
    }

    /// The period is stated by its two ends, so the window carries its reset.
    /// The key's allowance never turns over and carries none.
    #[test]
    fn only_the_subscription_has_a_reset() {
        let usage = reading(&fixture());
        assert_eq!(
            usage.windows[1].resets_at.as_deref(),
            Some("2026-10-15T00:00:00Z")
        );
        assert!(usage.windows[0].resets_at.is_none());
    }

    /// The prepaid balance and the subscription are separate things.
    #[test]
    fn the_prepaid_balance_stands_beside_the_allowances() {
        let usage = reading(&fixture());
        assert_eq!(usage.windows[2].label, "Balance");
        assert_eq!(usage.windows[2].detail.as_deref(), Some("12.50 USD"));
        assert_eq!(usage.windows[2].percent_used, None);

        // …and stands alone when there is nothing else.
        let credits_only = json!({ "balance": { "credits_remaining_usd": 4.25 } });
        let usage = reading(&credits_only);
        assert_eq!(usage.windows.len(), 1);
        assert_eq!(usage.windows[0].label, "Balance");
        assert_eq!(usage.windows[0].detail.as_deref(), Some("4.25 USD"));
    }

    /// The allowance is what was included, or failing that what was used plus
    /// what is left — both stated.
    #[test]
    fn the_allowance_falls_back_to_used_plus_remaining() {
        let reply = json!({
            "balance": { "credits_remaining_usd": 0.0 },
            "subscription": { "kwh_used": 30.0, "kwh_remaining": 70.0 }
        });
        let usage = reading(&reply);
        assert_eq!(usage.windows[0].percent_used, Some(30.0));
        // No ends stated, so the length is not claimed and it sorts as credits.
        assert_eq!(usage.windows[0].label, "Credits");
        assert!(usage.windows[0].resets_at.is_none());
    }

    #[test]
    fn a_reply_this_build_cannot_read_is_unreadable() {
        assert!(reading(&json!({})).error.is_some());
        assert!(reading(&json!({ "balance": {} })).error.is_some());
        // A used figure with no included and no remaining is not an allowance.
        assert!(reading(&json!({
            "balance": {}, "subscription": { "kwh_used": 5.0 }
        }))
        .error
        .is_some());
        // A limit of zero is not a denominator.
        assert!(reading(&json!({
            "balance": {}, "key": { "allowance": { "limit_usd": 0.0, "spent_usd": 1.0 } }
        }))
        .error
        .is_some());
        // Negative figures are not readings.
        assert!(reading(&json!({
            "balance": {}, "key": { "allowance": { "limit_usd": 10.0, "spent_usd": -1.0 } }
        }))
        .error
        .is_some());
    }

    #[test]
    fn a_spend_past_its_allowance_clamps_to_a_full_ring() {
        let reply = json!({
            "balance": {},
            "key": { "allowance": { "limit_usd": 10.0, "spent_usd": 40.0, "period": "daily" } }
        });
        let usage = reading(&reply);
        assert_eq!(usage.windows[0].label, "Daily");
        assert_eq!(usage.windows[0].percent_used, Some(100.0));
    }
}
