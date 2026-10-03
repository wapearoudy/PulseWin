//! xKiro (not AWS's Kiro): a plan's spend windows, the daily free-token
//! allowance, and the pay-as-you-go wallet.
//!
//! Read with a key the user enters, from xKiro's documented usage route:
//! `GET https://api.xkiro.com/v1/usage`, which xKiro says costs nothing and
//! counts against no limit. The reply is snake_case throughout, and money is
//! written as fixed-point strings (`"200.000000"`) — a bare number is read too.
//!
//! The reference implementation reads only the free tokens. The plan windows
//! and the wallet are in the same documented reply, each with the figures they
//! need — a window states its length, its cap, what is spent and when it resets
//! — so they are read here too.
//!
//! **The free-token reset is xKiro's documented rule**, midnight UTC, and not a
//! figure in the reply: `used_today` and `limit_per_day` name the day, and the
//! docs name its boundary.

use std::sync::Arc;

use chrono::{DateTime, SecondsFormat, Utc};
use serde_json::Value;

use super::{by_window_length, describe_reqwest_error, percent_from_fraction, Ctx, FetchFuture, Provider};
use crate::model::ProviderUsage;

const ENDPOINT: &str = "https://api.xkiro.com/v1/usage";

pub struct XKiro;

impl Provider for XKiro {
    fn id(&self) -> &'static str {
        "xkiro"
    }

    fn name(&self) -> &'static str {
        "xKiro"
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
    const ID: &str = "xkiro";
    const NAME: &str = "xKiro";

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

/// xKiro writes money as fixed-point strings: `"200.000000"`. A number is read
/// too, and anything else is no figure.
fn money(value: Option<&Value>) -> Option<f64> {
    match value? {
        Value::String(text) => text
            .trim()
            .parse::<f64>()
            .ok()
            .filter(|value| value.is_finite()),
        other => other.as_f64().filter(|value| value.is_finite()),
    }
}

fn number(value: Option<&Value>) -> Option<f64> {
    value.and_then(Value::as_f64).filter(|v| v.is_finite())
}

/// The reply is snake_case, and the original reads it with a decoder that
/// converts snake_case to camelCase — which means a key written without
/// underscores matches under either spelling. Both are read here for the same
/// reason: the service may write either, and the original accepts either.
fn field<'a>(object: &'a Value, names: &[&str]) -> Option<&'a Value> {
    names.iter().find_map(|name| object.get(*name))
}

/// Named by the length xKiro states: five hours and a week are the two it
/// documents; anything else by its number of hours or days.
fn label_for(seconds: i64) -> String {
    match seconds {
        18_000 => "5h".to_string(),
        86_400 => "Daily".to_string(),
        604_800 => "7d".to_string(),
        other => super::humanize_window_seconds(other),
    }
}

/// The next midnight UTC, which is when xKiro's free tokens turn over.
fn next_midnight_utc(now: DateTime<Utc>) -> Option<String> {
    let tomorrow = now.date_naive().succ_opt()?;
    let midnight = tomorrow.and_hms_opt(0, 0, 0)?;
    Some(midnight.and_utc().to_rfc3339_opts(SecondsFormat::Secs, true))
}

/// The mapping, kept apart from the request so a fixture can drive it.
fn reading(json: &Value) -> ProviderUsage {
    reading_at(json, Utc::now())
}

fn reading_at(json: &Value, now: DateTime<Utc>) -> ProviderUsage {
    const ID: &str = "xkiro";
    const NAME: &str = "xKiro";

    if json.get("object").and_then(|v| v.as_str()) != Some("usage") {
        return ProviderUsage::failed(ID, NAME, "bad reply: not a usage object");
    }

    let mut rows: Vec<(i64, crate::model::UsageWindow)> = Vec::new();

    if let Some(windows) = json.get("windows").and_then(|v| v.as_array()) {
        for window in windows {
            // A whole number of hours, or it can't be named without rounding.
            let Some(seconds) = field(window, &["window_sec", "windowSec"])
                .and_then(Value::as_i64)
                .filter(|seconds| *seconds >= 3_600 && seconds % 3_600 == 0)
            else {
                continue;
            };
            let Some(spent) =
                money(field(window, &["spent_usd", "spentUsd"])).filter(|spent| *spent >= 0.0)
            else {
                continue;
            };
            let Some(cap) =
                money(field(window, &["cap_usd", "capUsd"])).filter(|cap| *cap > 0.0)
            else {
                continue;
            };

            let resets_at = number(field(window, &["resets_in_sec", "resetsInSec"]))
                .filter(|seconds| *seconds >= 0.0)
                .and_then(|seconds| timestamp(now.timestamp() as f64 + seconds));

            rows.push((
                seconds,
                crate::model::UsageWindow::new(
                    label_for(seconds),
                    Some(percent_from_fraction(spent / cap)),
                )
                .with_reset(resets_at),
            ));
        }
    }

    // A null daily limit is "unlimited": a statement, not a denominator.
    if let Some(free) = field(json, &["free_tokens", "freeTokens"]) {
        let used = number(field(free, &["used_today", "usedToday"])).filter(|used| *used >= 0.0);
        let limit =
            number(field(free, &["limit_per_day", "limitPerDay"])).filter(|limit| *limit > 0.0);
        if let (Some(used), Some(limit)) = (used, limit) {
            rows.push((
                86_400,
                crate::model::UsageWindow::new(
                    "Daily",
                    Some(percent_from_fraction(used / limit)),
                )
                .with_reset(next_midnight_utc(now)),
            ));
        }
    }

    let balance = json
        .get("wallet")
        .and_then(|wallet| money(field(wallet, &["balance_usd", "balanceUsd"])));
    if rows.is_empty() && balance.is_none() {
        return ProviderUsage::failed(ID, NAME, "no limits reported");
    }

    let mut windows = by_window_length(rows);
    if let Some(balance) = balance {
        windows.push(super::balance_window("Balance", format!("{balance:.2} USD")));
    }

    let plan = json
        .get("plan")
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|plan| !plan.is_empty())
        .map(title);

    let mut usage = ProviderUsage::ok(ID, NAME, windows).with_plan(plan);
    if let Some(balance) = balance { usage = usage.with_credit_remaining(balance, "USD"); }
    usage
}

fn timestamp(seconds: f64) -> Option<String> {
    DateTime::from_timestamp(seconds.floor() as i64, 0)
        .map(|at| at.to_rfc3339_opts(SecondsFormat::Secs, true))
}

/// The first letter of each word, as Swift's `.capitalized` writes it.
fn title(plan: &str) -> String {
    plan.split_whitespace()
        .map(|word| {
            let mut chars = word.chars();
            match chars.next() {
                Some(first) => first.to_uppercase().collect::<String>() + &chars.as_str().to_lowercase(),
                None => String::new(),
            }
        })
        .collect::<Vec<String>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn at() -> DateTime<Utc> {
        DateTime::parse_from_rfc3339("2026-10-02T02:00:00Z")
            .unwrap()
            .with_timezone(&Utc)
    }

    fn fixture() -> Value {
        json!({
            "object": "usage",
            "plan": "pro",
            "windows": [
                { "kind": "weekly", "window_sec": 604_800, "spent_usd": "20.000000",
                  "cap_usd": "200.000000", "resets_in_sec": 86_400.0 },
                { "kind": "5h", "window_sec": 18_000, "spent_usd": "2.000000",
                  "cap_usd": "20.000000" }
            ],
            "free_tokens": { "used_today": 4_000.0, "limit_per_day": 10_000.0 },
            "wallet": { "balance_usd": "12.500000" }
        })
    }

    #[test]
    fn reads_the_windows_the_free_tokens_and_the_wallet() {
        let usage = reading_at(&fixture(), at());
        assert_eq!(usage.credit_remaining.as_ref().unwrap().amount, 12.5);
        assert_eq!(usage.credit_remaining.as_ref().unwrap().currency, "USD");
        let labels: Vec<&str> = usage.windows.iter().map(|w| w.label.as_str()).collect();
        assert_eq!(labels, vec!["5h", "Daily", "7d", "Balance"]);
        assert_eq!(usage.windows[0].percent_used, Some(10.0));
        assert_eq!(usage.windows[1].percent_used, Some(40.0));
        assert_eq!(usage.windows[2].percent_used, Some(10.0));
        assert_eq!(usage.plan.as_deref(), Some("Pro"));
    }

    #[test]
    fn a_window_resets_so_many_seconds_from_now() {
        let usage = reading_at(&fixture(), at());
        let weekly = usage.windows.iter().find(|w| w.label == "7d").unwrap();
        assert_eq!(weekly.resets_at.as_deref(), Some("2026-10-03T02:00:00Z"));
        // The five-hour window states no countdown, so it carries no reset.
        let five_hour = usage.windows.iter().find(|w| w.label == "5h").unwrap();
        assert!(five_hour.resets_at.is_none());
    }

    /// The free tokens turn over at midnight UTC — xKiro's documented rule,
    /// not a figure in the reply.
    #[test]
    fn the_free_tokens_reset_at_the_next_midnight_utc() {
        let usage = reading_at(&fixture(), at());
        let daily = usage.windows.iter().find(|w| w.label == "Daily").unwrap();
        assert_eq!(daily.resets_at.as_deref(), Some("2026-10-03T00:00:00Z"));
    }

    #[test]
    fn the_wallet_is_a_balance_and_not_a_ring() {
        let usage = reading_at(&fixture(), at());
        let wallet = usage.windows.last().unwrap();
        assert_eq!(wallet.label, "Balance");
        assert_eq!(wallet.detail.as_deref(), Some("12.50 USD"));
        assert_eq!(wallet.percent_used, None);
    }

    /// A null daily limit is unlimited — a statement, not a denominator — and a
    /// window whose length is not a whole number of hours cannot be named
    /// without rounding, so it is left off.
    #[test]
    fn unlimited_free_tokens_and_unnameable_lengths_are_left_off() {
        let reply = json!({
            "object": "usage",
            "windows": [ { "window_sec": 7_000, "spent_usd": "1", "cap_usd": "10" } ],
            "free_tokens": { "used_today": 4_000.0, "limit_per_day": null },
            "wallet": { "balance_usd": "1.000000" }
        });
        let usage = reading_at(&reply, at());
        assert_eq!(usage.windows.len(), 1);
        assert_eq!(usage.windows[0].label, "Balance");
    }

    /// A length that is a whole number of hours but none of the familiar ones
    /// is still drawn, named by that length.
    #[test]
    fn an_unfamiliar_whole_hour_length_is_named_by_its_length() {
        let reply = json!({
            "object": "usage",
            "windows": [ { "window_sec": 7_200, "spent_usd": "1", "cap_usd": "10" } ]
        });
        let usage = reading_at(&reply, at());
        assert_eq!(usage.windows[0].label, "2h");
        assert_eq!(usage.windows[0].percent_used, Some(10.0));
    }

    #[test]
    fn a_reply_this_build_cannot_read_is_unreadable() {
        assert!(reading_at(&json!({}), at()).error.is_some());
        assert!(reading_at(&json!({ "object": "usage" }), at()).error.is_some());
        assert!(reading_at(&json!({ "object": "usage", "windows": [] }), at())
            .error
            .is_some());
        // Not a usage object.
        assert!(reading_at(&json!({ "object": "list", "wallet": { "balance_usd": "1" } }), at())
            .error
            .is_some());
        // A cap of zero is not a denominator.
        assert!(reading_at(
            &json!({ "object": "usage",
                     "windows": [ { "window_sec": 18_000, "spent_usd": "1", "cap_usd": "0" } ] }),
            at()
        )
        .error
        .is_some());
        // A figure that is a word is not a figure.
        assert!(reading_at(
            &json!({ "object": "usage",
                     "windows": [ { "window_sec": 18_000, "spent_usd": "a lot", "cap_usd": "10" } ] }),
            at()
        )
        .error
        .is_some());
    }

    #[test]
    fn a_spend_past_the_cap_clamps_to_a_full_ring() {
        let reply = json!({
            "object": "usage",
            "windows": [ { "window_sec": 18_000, "spent_usd": "25", "cap_usd": "20" } ]
        });
        assert_eq!(reading_at(&reply, at()).windows[0].percent_used, Some(100.0));
    }
}
