//! Synthetic: a rolling five-hour allowance, a weekly token allowance and an
//! hourly search allowance, each stated by the service as a percentage or as an
//! amount used out of a limit.
//!
//! Read with a key the user enters, from Synthetic's documented quota route:
//! `GET https://api.synthetic.new/v2/quotas`.
//!
//! Only the three named slots are read. The reference implementation falls back
//! to any object anywhere in the reply that carries a number called `limit` or
//! `used`; that guesses at what a lane is and how long it lasts, so it is not
//! done here.
//!
//! **No reset for the rolling lanes.** The five-hour and weekly allowances
//! regenerate a slice at a time: `nextTickAt` / `nextRegenAt` is the next slice,
//! not a turnover. Drawn as a reset it would move forward every few minutes and
//! every one of those would read as the window starting again. Only the search
//! lane's `renewsAt` is a real reset.

use std::sync::Arc;

use serde_json::Value;

use super::{by_window_length, describe_reqwest_error, percent_from_fraction, Ctx, FetchFuture, Provider};
use crate::credentials;
use crate::model::{parse_reset, ProviderUsage, UsageWindow};

const ENDPOINT: &str = "https://api.synthetic.new/v2/quotas";

pub struct Synthetic;

impl Provider for Synthetic {
    fn id(&self) -> &'static str {
        "synthetic"
    }

    fn name(&self) -> &'static str {
        "Synthetic"
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
    const ID: &str = "synthetic";
    const NAME: &str = "Synthetic";

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
    const ID: &str = "synthetic";
    const NAME: &str = "Synthetic";

    // The slots sit at the root or under `data`; the reference accepts both.
    let slots = slots(json);
    let mut rows: Vec<(i64, UsageWindow)> = Vec::new();

    if let Some(slot) = slots.get("rollingFiveHourLimit") {
        if let Some(used) = fraction(None, number(slot.get("remaining")), number(slot.get("max"))) {
            rows.push((
                5 * 3_600,
                UsageWindow::new("5h", Some(percent_from_fraction(used)))
                    .with_detail(limited_detail(slot)),
            ));
        }
    }

    if let Some(slot) = slots.get("weeklyTokenLimit") {
        if let Some(used) = weekly_fraction(slot) {
            rows.push((
                7 * 86_400,
                UsageWindow::new("7d", Some(percent_from_fraction(used)))
                    .with_detail(limited_detail(slot)),
            ));
        }
    }

    // "Search" is Synthetic's search API, a product of its own, so it is a
    // scope and stays untranslated.
    if let Some(slot) = slots.get("search").and_then(|search| search.get("hourly")) {
        if let Some(used) = fraction(
            number(slot.get("requests")),
            number(slot.get("remaining")),
            number(slot.get("limit")),
        ) {
            rows.push((
                3_600,
                UsageWindow::new("1h Search", Some(percent_from_fraction(used)))
                    .with_reset(slot.get("renewsAt").and_then(parse_reset))
                    .with_detail(limited_detail(slot)),
            ));
        }
    }

    let windows = by_window_length(rows);
    if windows.is_empty() {
        return ProviderUsage::failed(ID, NAME, "no quota windows in response");
    }

    ProviderUsage::ok(ID, NAME, windows).with_plan(plan(&slots))
}

/// The reply's slots, wherever this reply happened to put them.
fn slots(json: &Value) -> Value {
    let root_has_slots = json.get("rollingFiveHourLimit").is_some()
        || json.get("weeklyTokenLimit").is_some()
        || json.get("search").and_then(|s| s.get("hourly")).is_some();

    if root_has_slots {
        json.clone()
    } else {
        json.get("data").cloned().unwrap_or_else(|| json.clone())
    }
}

/// The provider's own word that the lane is out, which this port's window model
/// has no flag for — so it is written into the detail rather than dropped.
fn limited_detail(slot: &Value) -> Option<String> {
    match slot.get("limited").and_then(|v| v.as_bool()) {
        Some(true) => Some("limited".to_string()),
        _ => None,
    }
}

fn plan(slots: &Value) -> Option<String> {
    credentials::dig_str(slots, "plan")
        .map(|plan| plan.trim().to_string())
        .filter(|plan| !plan.is_empty())
}

/// Used out of a limit, from whichever two of used / remaining / limit the slot
/// states. A limit of zero or less, or a negative figure, is not an allowance
/// and gives nothing.
fn fraction(used: Option<f64>, remaining: Option<f64>, limit: Option<f64>) -> Option<f64> {
    let limit = limit.filter(|limit| limit.is_finite() && *limit > 0.0)?;
    if let Some(used) = used.filter(|used| used.is_finite() && *used >= 0.0) {
        return Some(used / limit);
    }
    if let Some(remaining) = remaining.filter(|remaining| remaining.is_finite() && *remaining >= 0.0) {
        return Some((limit - remaining).max(0.0) / limit);
    }
    None
}

/// The weekly lane states a percentage left, on a 0–100 scale; failing that, a
/// dollar allowance and what is left of it.
fn weekly_fraction(slot: &Value) -> Option<f64> {
    if let Some(left) = number(slot.get("percentRemaining"))
        .filter(|left| left.is_finite() && (0.0..=100.0).contains(left))
    {
        return Some((100.0 - left) / 100.0);
    }
    fraction(
        None,
        dollars(slot.get("remainingCredits")),
        dollars(slot.get("maxCredits")),
    )
}

/// "$36.00" → 36. Anything else is not a figure.
fn dollars(value: Option<&Value>) -> Option<f64> {
    let text = value?.as_str()?;
    let cleaned: String = text
        .trim()
        .chars()
        .filter(|c| *c != '$' && *c != ',')
        .collect();
    cleaned.parse::<f64>().ok().filter(|v| v.is_finite())
}

fn number(value: Option<&Value>) -> Option<f64> {
    super::dig_number(value)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// The reply's shape: three lanes that each report different fields.
    fn fixture() -> Value {
        json!({
            "plan": "pro",
            "rollingFiveHourLimit": { "max": 100, "remaining": 40, "limited": false },
            "weeklyTokenLimit": { "percentRemaining": 25, "maxCredits": "$36.00",
                                  "remainingCredits": "$9.00" },
            "search": { "hourly": { "requests": 30, "limit": 100,
                                    "renewsAt": "2026-10-01T13:00:00Z" } }
        })
    }

    #[test]
    fn reads_the_three_named_lanes_shortest_first() {
        let usage = reading(&fixture());
        let labels: Vec<&str> = usage.windows.iter().map(|w| w.label.as_str()).collect();
        assert_eq!(labels, vec!["1h Search", "5h", "7d"]);
        assert_eq!(usage.plan.as_deref(), Some("pro"));
    }

    #[test]
    fn the_rolling_lane_counts_down_from_its_maximum() {
        let usage = reading(&fixture());
        let five_hour = usage.windows.iter().find(|w| w.label == "5h").unwrap();
        assert_eq!(five_hour.percent_used, Some(60.0));
        // A slice at a time is not a turnover, so there is no reset to draw.
        assert!(five_hour.resets_at.is_none());
    }

    #[test]
    fn the_weekly_lane_reports_a_percentage_left() {
        let usage = reading(&fixture());
        let weekly = usage.windows.iter().find(|w| w.label == "7d").unwrap();
        assert_eq!(weekly.percent_used, Some(75.0));
        assert!(weekly.resets_at.is_none());
    }

    #[test]
    fn the_weekly_lane_falls_back_to_the_dollar_allowance() {
        let payload = json!({
            "weeklyTokenLimit": { "maxCredits": "$36.00", "remainingCredits": "$9.00" }
        });
        let usage = reading(&payload);
        assert_eq!(usage.windows[0].percent_used, Some(75.0));
    }

    #[test]
    fn the_search_lane_reports_requests_and_keeps_its_reset() {
        let usage = reading(&fixture());
        let search = usage.windows.iter().find(|w| w.label == "1h Search").unwrap();
        assert_eq!(search.percent_used, Some(30.0));
        assert_eq!(search.resets_at.as_deref(), Some("2026-10-01T13:00:00Z"));
    }

    #[test]
    fn the_slots_may_arrive_under_data() {
        let payload = json!({
            "data": { "plan": "lite", "rollingFiveHourLimit": { "max": 10, "remaining": 5 } }
        });
        let usage = reading(&payload);
        assert_eq!(usage.windows[0].label, "5h");
        assert_eq!(usage.windows[0].percent_used, Some(50.0));
        assert_eq!(usage.plan.as_deref(), Some("lite"));
    }

    #[test]
    fn a_lane_the_provider_calls_limited_says_so() {
        let payload = json!({
            "rollingFiveHourLimit": { "max": 10, "remaining": 0, "limited": true }
        });
        let usage = reading(&payload);
        assert_eq!(usage.windows[0].percent_used, Some(100.0));
        assert_eq!(usage.windows[0].detail.as_deref(), Some("limited"));
    }

    #[test]
    fn a_limit_of_zero_is_not_an_allowance() {
        assert_eq!(fraction(Some(5.0), None, Some(0.0)), None);
        assert_eq!(fraction(None, Some(5.0), None), None);
    }

    #[test]
    fn dollars_are_read_out_of_a_formatted_string() {
        assert_eq!(dollars(Some(&json!("$36.00"))), Some(36.0));
        assert_eq!(dollars(Some(&json!("$1,200.50"))), Some(1200.50));
        assert_eq!(dollars(Some(&json!("free"))), None);
        assert_eq!(dollars(Some(&json!(36.0))), None);
    }

    #[test]
    fn a_reply_with_no_lane_left_is_unreadable() {
        assert!(reading(&json!({ "plan": "pro" })).error.is_some());
    }
}
