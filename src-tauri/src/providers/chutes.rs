//! Chutes: a subscription's rolling window and its monthly allowance, each an
//! amount used out of a limit the service states.
//!
//! Read with a key the user enters, from Chutes' management API:
//! `GET https://api.chutes.ai/users/me/subscription_usage`.
//!
//! **Narrower than the reference implementation on purpose.** That one searches
//! the whole reply for anything with a `limit` and a `used`, guesses a lane
//! from the word "rolling" or "month", fills in four hours or thirty days where
//! no length is given, and reads a percentage under 1 as a fraction. None of
//! that is done here: the rolling window is read only where its length is
//! stated — in minutes, hours or seconds, or in its name — and the monthly one
//! only where it is named so. The per-chute pay-as-you-go quotas, which state
//! no period at all, are left off.

use std::sync::Arc;

use chrono::{DateTime, SecondsFormat};
use serde_json::Value;

use super::{by_window_length, describe_reqwest_error, percent_from_fraction, Ctx, FetchFuture, Provider};
use crate::model::{ProviderUsage, UsageWindow};

const ENDPOINT: &str = "https://api.chutes.ai/users/me/subscription_usage";

pub struct Chutes;

impl Provider for Chutes {
    fn id(&self) -> &'static str {
        "chutes"
    }

    fn name(&self) -> &'static str {
        "Chutes"
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
    const ID: &str = "chutes";
    const NAME: &str = "Chutes";

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

/// Keys compared the way the original compares them: lower-cased, with
/// everything but letters and digits dropped, so `rolling_window`,
/// `rollingWindow` and `RollingWindow` are one name.
fn normalized(key: &str) -> String {
    key.to_lowercase()
        .chars()
        .filter(|c| c.is_alphanumeric())
        .collect()
}

/// The first of `keys` this object carries under any spelling, skipping nulls.
fn value_in<'a>(object: &'a Value, keys: &[&str]) -> Option<&'a Value> {
    let map = object.as_object()?;
    for key in keys {
        let found = map
            .iter()
            .find(|(name, _)| normalized(name) == *key)
            .map(|(_, value)| value);
        if let Some(value) = found {
            if !value.is_null() {
                return Some(value);
            }
        }
    }
    None
}

const LIMIT_KEYS: &[&str] = &[
    "limit",
    "cap",
    "max",
    "quota",
    "monthlylimit",
    "requestlimit",
    "tokenlimit",
];
const USED_KEYS: &[&str] = &[
    "used",
    "usage",
    "consumed",
    "requests",
    "requestcount",
    "tokens",
    "tokenusage",
];
const REMAINING_KEYS: &[&str] = &["remaining", "available", "left"];
const PERCENT_USED_KEYS: &[&str] = &["percentused", "usagepercent", "usedpercent"];
const RESET_KEYS: &[&str] = &[
    "resetat",
    "resetsat",
    "nextresetat",
    "renewsat",
    "periodend",
    "currentperiodend",
    "windowend",
];
const MONTHLY_KEYS: &[&str] = &["monthly", "monthlyusage", "billingperiod"];

/// A number, or one written as a string. A boolean is neither.
fn number(value: Option<&Value>) -> Option<f64> {
    match value? {
        Value::Number(n) => n.as_f64(),
        Value::String(text) => text.trim().parse::<f64>().ok(),
        _ => None,
    }
    .filter(|value| value.is_finite())
}

/// A length the payload states in so many minutes, hours or seconds.
fn stated_seconds(payload: &Value) -> Option<i64> {
    const UNITS: [(&[&str], f64); 3] = [
        (
            &["windowminutes", "periodminutes", "durationminutes"],
            60.0,
        ),
        (&["windowhours", "periodhours", "durationhours"], 3_600.0),
        (
            &["windowseconds", "periodseconds", "durationseconds"],
            1.0,
        ),
    ];

    for (keys, multiplier) in UNITS {
        if let Some(found) = number(value_in(payload, keys)).filter(|number| *number > 0.0) {
            let seconds = (found * multiplier).round();
            if seconds >= 3_600.0 && seconds < i32::MAX as f64 {
                return Some(seconds as i64);
            }
            return None;
        }
    }
    None
}

/// The rolling lane whose length is stated — by the payload itself or by its
/// name — or `None`. A lane that states none is passed over, so a bare
/// `rolling_window` never hides a `four_hour` beside it.
fn rolling(body: &Value) -> Option<(&Value, i64)> {
    let map = body.as_object()?;
    let mut names: Vec<&String> = map.keys().collect();
    names.sort();

    for name in names {
        let value = &map[name];
        if !value.is_object() {
            continue;
        }
        let named = match normalized(name).as_str() {
            "rolling" | "rollingwindow" => None,
            "rolling4h" | "fourhour" | "fourhourusage" | "window4h" => Some(4 * 3_600),
            _ => continue,
        };
        let Some(seconds) = stated_seconds(value).or(named) else {
            continue;
        };
        return Some((value, seconds));
    }
    None
}

/// Used and limit, both as the service states them — or a percentage it states,
/// on a 0–100 scale. A limit of zero is no allowance.
fn figures(payload: &Value) -> Option<(f64, f64)> {
    if let Some(percent) = number(value_in(payload, PERCENT_USED_KEYS)).filter(|p| *p >= 0.0) {
        return Some((percent, 100.0));
    }
    let limit = number(value_in(payload, LIMIT_KEYS)).filter(|limit| *limit > 0.0)?;
    if let Some(used) = number(value_in(payload, USED_KEYS)).filter(|used| *used >= 0.0) {
        return Some((used, limit));
    }
    if let Some(remaining) = number(value_in(payload, REMAINING_KEYS)).filter(|left| *left >= 0.0) {
        return Some(((limit - remaining).max(0.0), limit));
    }
    None
}

/// ISO 8601, or epoch seconds or milliseconds.
fn date(value: Option<&Value>) -> Option<String> {
    if let Some(text) = value.and_then(Value::as_str) {
        if let Some(reset) = crate::model::parse_reset(&Value::String(text.to_string())) {
            return Some(reset);
        }
    }
    let stamp = number(value).filter(|stamp| *stamp > 0.0)?;
    let seconds = if stamp > 10_000_000_000.0 {
        stamp / 1_000.0
    } else {
        stamp
    };
    DateTime::from_timestamp(seconds.floor() as i64, 0)
        .map(|at| at.to_rfc3339_opts(SecondsFormat::Secs, true))
}

/// Named by the length the lane states: five hours and a day are the two that
/// get their own name, anything else by its number of hours or days.
fn label_for(seconds: i64) -> String {
    match seconds {
        18_000 => "5h".to_string(),
        86_400 => "Daily".to_string(),
        other => super::humanize_window_seconds(other),
    }
}

/// Whether the subscription says it is running, where it says anything.
fn is_active(subscription: &Value) -> Option<bool> {
    if let Some(flag) = value_in(subscription, &["active", "isactive"]).and_then(Value::as_bool) {
        return Some(flag);
    }
    let status = value_in(subscription, &["status", "state"])
        .and_then(Value::as_str)?
        .to_lowercase();
    if status == "active" {
        return Some(true);
    }
    if [
        "free", "inactive", "canceled", "cancelled", "expired", "none",
    ]
    .contains(&status.as_str())
    {
        return Some(false);
    }
    None
}

/// The mapping, kept apart from the request so a fixture can drive it.
fn reading(json: &Value) -> ProviderUsage {
    const ID: &str = "chutes";
    const NAME: &str = "Chutes";

    // Some replies wrap everything in `data` or `result`.
    let root = json;
    let body = value_in(root, &["data", "result"])
        .filter(|value| value.is_object())
        .unwrap_or(root);
    let empty = Value::Object(serde_json::Map::new());
    let subscription = value_in(body, &["subscription", "currentsubscription"])
        .filter(|value| value.is_object())
        .unwrap_or(&empty);

    let mut rows: Vec<(i64, UsageWindow)> = Vec::new();

    if let Some((payload, seconds)) = rolling(body) {
        if let Some((used, limit)) = figures(payload) {
            rows.push((
                seconds,
                UsageWindow::new(label_for(seconds), Some(percent_from_fraction(used / limit)))
                    .with_reset(date(value_in(payload, RESET_KEYS))),
            ));
        }
    }

    if let Some(payload) = value_in(body, MONTHLY_KEYS).filter(|value| value.is_object()) {
        if let Some((used, limit)) = figures(payload) {
            rows.push((
                // A billing month: a sort key, not a stated length.
                30 * 86_400,
                UsageWindow::new("Monthly", Some(percent_from_fraction(used / limit)))
                    .with_reset(date(value_in(payload, RESET_KEYS))),
            ));
        }
    }

    let windows = by_window_length(rows);
    if windows.is_empty() {
        // An answer rather than an outage: the account has no plan.
        return ProviderUsage::failed(
            ID,
            NAME,
            if is_active(subscription) == Some(false) {
                "this account has no plan"
            } else {
                "no usage windows in the reply"
            },
        );
    }

    let plan = value_in(subscription, &["planname", "plan", "tier"])
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|plan| !plan.is_empty())
        .map(str::to_string);

    ProviderUsage::ok(ID, NAME, windows).with_plan(plan)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn fixture() -> Value {
        json!({
            "subscription": { "plan_name": "Pro", "status": "active" },
            "rolling_window": { "window_hours": 5, "used": 120, "limit": 600,
                                "resets_at": "2026-10-02T07:00:00Z" },
            "monthly": { "used": 40, "limit": 200, "reset_at": 1_790_000_000 }
        })
    }

    #[test]
    fn reads_the_rolling_and_monthly_lanes_shortest_first() {
        let usage = reading(&fixture());
        let labels: Vec<&str> = usage.windows.iter().map(|w| w.label.as_str()).collect();
        assert_eq!(labels, vec!["5h", "Monthly"]);
        assert_eq!(usage.windows[0].percent_used, Some(20.0));
        assert_eq!(usage.windows[1].percent_used, Some(20.0));
        assert_eq!(usage.plan.as_deref(), Some("Pro"));
    }

    #[test]
    fn the_resets_are_read_as_a_stamp_and_as_a_time() {
        let usage = reading(&fixture());
        assert_eq!(
            usage.windows[0].resets_at.as_deref(),
            Some("2026-10-02T07:00:00Z")
        );
        assert_eq!(
            usage.windows[1].resets_at.as_deref(),
            Some("2026-09-21T14:13:20Z")
        );
    }

    /// A lane that states no length is passed over, so a bare `rolling_window`
    /// never hides a `four_hour` beside it.
    #[test]
    fn a_lane_with_no_stated_length_is_passed_over() {
        let reply = json!({
            "rolling_window": { "used": 1, "limit": 10 },
            "four_hour": { "used": 2, "limit": 10 }
        });
        let usage = reading(&reply);
        assert_eq!(usage.windows.len(), 1);
        assert_eq!(usage.windows[0].label, "4h");
        assert_eq!(usage.windows[0].percent_used, Some(20.0));
    }

    /// One named for its length needs nothing else, and a day gets its own
    /// name.
    #[test]
    fn a_named_daily_lane_is_drawn_by_its_name() {
        let reply = json!({ "rolling_daily": { "used": 1, "limit": 4 } });
        // `rolling_daily` is not a name the table knows, so it is passed over
        // like any other unknown lane.
        assert!(reading(&reply).error.is_some());

        let day = json!({ "window4h": { "window_seconds": 86_400, "used": 1, "limit": 4 } });
        let usage = reading(&day);
        assert_eq!(usage.windows[0].label, "Daily");
        assert_eq!(usage.windows[0].percent_used, Some(25.0));
    }

    /// A stated percentage is read on its own scale, before any limit.
    #[test]
    fn a_stated_percentage_is_used_before_a_limit() {
        let reply = json!({
            "rolling_window": { "window_hours": 5, "percent_used": 62.5, "limit": 0 }
        });
        assert_eq!(reading(&reply).windows[0].percent_used, Some(62.5));
    }

    #[test]
    fn a_remainder_is_counted_down_from_the_limit_and_never_negative() {
        let reply = json!({
            "rolling_window": { "window_hours": 5, "remaining": 30, "limit": 100 }
        });
        assert_eq!(reading(&reply).windows[0].percent_used, Some(70.0));

        let over = json!({
            "rolling_window": { "window_hours": 5, "remaining": 300, "limit": 100 }
        });
        assert_eq!(reading(&over).windows[0].percent_used, Some(0.0));
    }

    /// The reply may wrap everything in `data` or `result`, and a lane the
    /// account has not got says so rather than being unreadable.
    #[test]
    fn a_wrapped_reply_is_read_and_an_inactive_plan_says_so() {
        let wrapped = json!({ "data": {
            "rolling_window": { "window_hours": 5, "used": 1, "limit": 10 }
        } });
        assert_eq!(reading(&wrapped).windows[0].percent_used, Some(10.0));

        let inactive = json!({ "subscription": { "status": "free" } });
        let usage = reading(&inactive);
        assert_eq!(usage.error.as_deref(), Some("this account has no plan"));

        let unknown = json!({ "subscription": { "status": "mystery" } });
        assert_eq!(
            reading(&unknown).error.as_deref(),
            Some("no usage windows in the reply")
        );
    }

    #[test]
    fn a_limit_of_zero_is_not_an_allowance() {
        let reply = json!({ "rolling_window": { "window_hours": 5, "used": 1, "limit": 0 } });
        assert!(reading(&reply).error.is_some());
    }

    #[test]
    fn a_reply_with_nothing_recognisable_is_unreadable() {
        assert!(reading(&json!({})).error.is_some());
        assert!(reading(&json!({ "rolling_window": "not an object" }))
            .error
            .is_some());
        // A length under an hour is not a lane this build can name.
        let too_short = json!({ "rolling_window": { "window_minutes": 30, "used": 1, "limit": 10 } });
        assert!(reading(&too_short).error.is_some());
    }

    #[test]
    fn keys_are_compared_without_case_or_punctuation() {
        assert_eq!(normalized("rolling_window"), "rollingwindow");
        assert_eq!(normalized("RollingWindow"), "rollingwindow");
        let reply = json!({
            "Rolling_Window": { "WindowHours": 5, "Used": 3, "Limit": 12 }
        });
        assert_eq!(reading(&reply).windows[0].percent_used, Some(25.0));
    }
}
