//! v0, Vercel's app builder: the account's billing allowance and its request
//! rate limit, each as a size and a remainder the service states.
//!
//! Read with a v0 Platform API key the user enters, from
//! `GET https://api.v0.dev/v1/user/billing` and `GET …/v1/rate-limits`.
//!
//! Billing comes in two shapes, named by `billingType`: `token`, a balance with
//! a total and a remainder and a cycle end, and `legacy`, a limit and a
//! remainder. Either way the units are v0's own; nothing here calls them
//! dollars. The on-demand balance beside the token allowance is in the same
//! unnamed unit and is left out rather than labelled with a guess. A remainder
//! that is not reported leaves that window off — the size alone is not a
//! percentage.
//!
//! The rate limit is best-effort: the billing allowance is the reading, and a
//! rate limit that cannot be had leaves it standing.

use std::sync::Arc;

use chrono::{DateTime, SecondsFormat};
use serde_json::Value;

use super::{by_window_length, describe_reqwest_error, percent_from_fraction, Ctx, FetchFuture, Provider};
use crate::model::ProviderUsage;

const BILLING_ENDPOINT: &str = "https://api.v0.dev/v1/user/billing";
const RATE_LIMIT_ENDPOINT: &str = "https://api.v0.dev/v1/rate-limits";

pub struct V0;

impl Provider for V0 {
    fn id(&self) -> &'static str {
        "v0"
    }

    fn name(&self) -> &'static str {
        "v0"
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
    const ID: &str = "v0";
    const NAME: &str = "v0";

    let key = match super::provider_key(ID) {
        Some(key) => key,
        None => return ProviderUsage::failed(ID, NAME, super::missing_key(ID, "")),
    };

    let billing = match ask(&ctx, BILLING_ENDPOINT, &key).await {
        Ok(json) => json,
        Err(message) => return ProviderUsage::failed(ID, NAME, message),
    };

    // Asked only once the allowance is in hand; whatever becomes of it, the
    // allowance is what is shown.
    let rate_limit = ask(&ctx, RATE_LIMIT_ENDPOINT, &key).await.ok();

    reading(&billing, rate_limit.as_ref())
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

/// Unix time, in seconds or in milliseconds; the reference implementation
/// accepts either, and so does this. Zero or less is no reset.
fn date(stamp: Option<f64>) -> Option<String> {
    let stamp = stamp.filter(|stamp| stamp.is_finite() && *stamp > 0.0)?;
    let seconds = if stamp >= 1_000_000_000_000.0 {
        stamp / 1_000.0
    } else {
        stamp
    };
    let at = DateTime::from_timestamp(seconds.floor() as i64, 0)?;
    Some(at.to_rfc3339_opts(SecondsFormat::Secs, true))
}

fn number(value: Option<&Value>) -> Option<f64> {
    value.and_then(Value::as_f64)
}

/// One window, from a size and what is left of it. Used is the size less what
/// is left, both as reported: a remainder above the size reads as nothing used,
/// not as a negative share.
fn window(
    label: &str,
    seconds: i64,
    limit: Option<f64>,
    remaining: Option<f64>,
    reset: Option<f64>,
) -> Option<(i64, crate::model::UsageWindow)> {
    let limit = limit.filter(|limit| limit.is_finite() && *limit > 0.0)?;
    let remaining = remaining.filter(|remaining| remaining.is_finite())?;
    let fraction = (limit - remaining).max(0.0) / limit;
    Some((
        seconds,
        crate::model::UsageWindow::new(label, Some(percent_from_fraction(fraction)))
            .with_reset(date(reset)),
    ))
}

/// The mapping, kept apart from the request so a fixture can drive it.
fn reading(billing: &Value, rate_limit: Option<&Value>) -> ProviderUsage {
    const ID: &str = "v0";
    const NAME: &str = "v0";

    let Some(data) = billing.get("data") else {
        return ProviderUsage::failed(ID, NAME, "bad reply: no billing data");
    };

    // v0's own credits over a billing cycle whose length is not stated.
    let allowance = match billing.get("billingType").and_then(|v| v.as_str()) {
        Some("token") => window(
            "Credits",
            30 * 86_400,
            number(data.get("balance").and_then(|b| b.get("total"))),
            number(data.get("balance").and_then(|b| b.get("remaining"))),
            number(data.get("billingCycle").and_then(|c| c.get("end"))),
        ),
        Some("legacy") => window(
            "Credits",
            30 * 86_400,
            number(data.get("limit")),
            number(data.get("remaining")),
            number(data.get("reset")),
        ),
        _ => return ProviderUsage::failed(ID, NAME, "bad reply: unknown billing type"),
    };

    let mut rows: Vec<(i64, crate::model::UsageWindow)> = Vec::new();
    if let Some(row) = allowance {
        rows.push(row);
    }

    // Counted in requests, with a reset and no stated length. A day is only
    // where it sorts.
    if let Some(quota) = rate_limit {
        if let Some(row) = window(
            "Requests",
            86_400,
            number(quota.get("limit")),
            number(quota.get("remaining")),
            number(quota.get("reset")),
        ) {
            rows.push(row);
        }
    }

    let windows = by_window_length(rows);
    if windows.is_empty() {
        return ProviderUsage::failed(ID, NAME, "no limits reported");
    }

    ProviderUsage::ok(ID, NAME, windows)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn token_billing() -> Value {
        json!({
            "billingType": "token",
            "data": { "balance": { "total": 200.0, "remaining": 50.0 },
                      "billingCycle": { "end": 1_790_000_000.0 } }
        })
    }

    fn rate_limit() -> Value {
        json!({ "limit": 1000.0, "remaining": 250.0, "reset": 1_790_000_000.0 })
    }

    #[test]
    fn reads_the_token_allowance_and_the_request_lane_shortest_first() {
        let usage = reading(&token_billing(), Some(&rate_limit()));
        let labels: Vec<&str> = usage.windows.iter().map(|w| w.label.as_str()).collect();
        assert_eq!(labels, vec!["Requests", "Credits"]);
        assert_eq!(usage.windows[0].percent_used, Some(75.0));
        assert_eq!(usage.windows[1].percent_used, Some(75.0));
    }

    #[test]
    fn reads_the_legacy_allowance() {
        let legacy = json!({
            "billingType": "legacy",
            "data": { "limit": 40.0, "remaining": 30.0, "reset": 1_790_000_000.0 }
        });
        let usage = reading(&legacy, None);
        assert_eq!(usage.windows.len(), 1);
        assert_eq!(usage.windows[0].label, "Credits");
        assert_eq!(usage.windows[0].percent_used, Some(25.0));
        assert_eq!(
            usage.windows[0].resets_at.as_deref(),
            Some("2026-09-21T14:13:20Z")
        );
    }

    /// A remainder above the size is nothing used, not a negative share.
    #[test]
    fn a_remainder_above_the_size_is_nothing_used() {
        let reply = json!({
            "billingType": "legacy",
            "data": { "limit": 10.0, "remaining": 25.0 }
        });
        assert_eq!(reading(&reply, None).windows[0].percent_used, Some(0.0));
    }

    /// A size alone is not a percentage, so a window without a remainder is
    /// left off rather than drawn.
    #[test]
    fn a_size_without_a_remainder_is_left_off() {
        let reply = json!({ "billingType": "legacy", "data": { "limit": 10.0 } });
        assert!(reading(&reply, None).error.is_some());
    }

    /// The billing allowance is the reading: a rate limit that cannot be had
    /// leaves it standing.
    #[test]
    fn a_missing_rate_limit_leaves_the_allowance_standing() {
        let usage = reading(&token_billing(), None);
        assert_eq!(usage.windows.len(), 1);
        assert_eq!(usage.windows[0].label, "Credits");
    }

    #[test]
    fn stamps_are_read_in_seconds_and_in_milliseconds() {
        let seconds = json!({ "billingType": "legacy",
                              "data": { "limit": 1.0, "remaining": 1.0, "reset": 1_790_000_000.0 } });
        let millis = json!({ "billingType": "legacy",
                             "data": { "limit": 1.0, "remaining": 1.0, "reset": 1_790_000_000_000.0 } });
        assert_eq!(
            reading(&seconds, None).windows[0].resets_at,
            reading(&millis, None).windows[0].resets_at
        );
        assert_eq!(date(Some(0.0)), None);
        assert_eq!(date(Some(-5.0)), None);
        assert_eq!(date(None), None);
    }

    #[test]
    fn a_reply_this_build_cannot_read_is_unreadable() {
        assert!(reading(&json!({}), None).error.is_some());
        assert!(reading(&json!({ "billingType": "subscription", "data": {} }), None)
            .error
            .is_some());
        assert!(reading(&json!({ "billingType": "legacy" }), None).error.is_some());
    }
}
