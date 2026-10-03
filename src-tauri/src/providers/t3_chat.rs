//! T3 Chat: the four-hour window and the monthly allowance, each reported as
//! a percentage by the service itself.
//!
//! Read from the call T3 Chat's own settings page makes:
//! `GET https://t3.chat/api/trpc/getCustomerData`. The reply is tRPC's streamed
//! JSON, one value per line, with the customer's record somewhere inside one of
//! them.
//!
//! **The credential is a pasted cookie.** The original imports the session from
//! the browser (`Auth/BrowserCookies.swift`), which on macOS reads a Chromium
//! cookie store through the login keychain; PulseWin cannot, so the `Cookie`
//! header copied out of a signed-in request is the credential here instead.
//! Only `wos-session` and `_vcrcs` go with it.
//!
//! A percentage T3 Chat leaves out is left off, never drawn as zero.
//! `usagePeriodPercentage` is not read in place of the monthly figure: nothing
//! says which period it is.

use std::sync::Arc;

use serde_json::Value;

use super::{describe_reqwest_error, percent_from_fraction, Ctx, FetchFuture, Provider};
use crate::model::{ProviderUsage, UsageWindow};

const ID: &str = "t3-chat";
const NAME: &str = "T3 Chat";

const ENDPOINT: &str = "https://t3.chat/api/trpc/getCustomerData";
/// The batch input the settings page sends: one call, no session id.
const INPUT: &str = r#"{"0":{"json":{"sessionId":null},"meta":{"values":{"sessionId":["undefined"]}}}}"#;

/// The names that carry the session, and nothing else the host set.
const COOKIES: [&str; 2] = ["wos-session", "_vcrcs"];

pub struct T3Chat;

impl Provider for T3Chat {
    fn id(&self) -> &'static str {
        ID
    }

    fn name(&self) -> &'static str {
        NAME
    }

    /// Whether a pasted cookie is on the machine. No network call.
    fn is_configured(&self) -> bool {
        super::pasted::cookie(ID).is_some()
    }

    fn fetch(&self, ctx: Arc<Ctx>) -> FetchFuture {
        Box::pin(async move { fetch_inner(ctx).await })
    }
}

async fn fetch_inner(ctx: Arc<Ctx>) -> ProviderUsage {
    let Some(cookie) = super::pasted::cookie(ID) else {
        return ProviderUsage::failed(ID, NAME, super::pasted::needed(ID, "T3 Chat session cookie"));
    };
    let Some(cookie) = super::pasted::keep(&cookie, &COOKIES) else {
        return ProviderUsage::failed(ID, NAME, super::pasted::needed(ID, "T3 Chat session cookie"));
    };

    let mut url = match reqwest::Url::parse(ENDPOINT) {
        Ok(url) => url,
        Err(e) => return ProviderUsage::failed(ID, NAME, format!("bad endpoint: {e}")),
    };
    url.query_pairs_mut()
        .append_pair("batch", "1")
        .append_pair("input", INPUT);

    let response = ctx
        .gateway_client
        .get(url)
        .header("Cookie", &cookie)
        .header("Accept", "*/*")
        .header("Origin", "https://t3.chat")
        .header("Referer", "https://t3.chat/settings/customization")
        .header("trpc-accept", "application/jsonl")
        .header("x-trpc-source", "web-client")
        .header("x-trpc-batch", "true")
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
        Ok(body) => body,
        Err(e) => return ProviderUsage::failed(ID, NAME, format!("cannot read body: {e}")),
    };

    if status.is_redirection() {
        return ProviderUsage::failed(
            ID,
            NAME,
            "HTTP 3xx — the session has expired; copy a fresh cookie",
        );
    }
    if !status.is_success() {
        let hints: &[(u16, &str)] = &[
            (401, " — the session has expired; copy a fresh cookie from t3.chat"),
            (403, " — the session has expired; copy a fresh cookie from t3.chat"),
            (429, " — rate limited, try again shortly"),
        ];
        return ProviderUsage::failed(ID, NAME, super::http_failure(status, hints));
    }

    reading(&body)
}

/// The mapping, kept apart from the request so a fixture can drive it.
fn reading(body: &str) -> ProviderUsage {
    let Some(customer) = body
        .lines()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .find_map(|value| customer_record(&value))
    else {
        return ProviderUsage::failed(ID, NAME, "no customer record in the reply");
    };

    let mut windows: Vec<UsageWindow> = Vec::new();

    // "Four hour" is in the field's name, so the length is stated.
    if let Some(percent) = percent(customer.get("usageFourHourPercentage")) {
        let reset = customer
            .get("usageFourHourNextResetAt")
            .or_else(|| customer.get("usageWindowNextResetAt"))
            .and_then(stamp);
        windows.push(
            UsageWindow::new("4h", Some(percent_from_fraction(percent / 100.0))).with_reset(reset),
        );
    }

    // The subscription's billing period: a name and a sort key, with the reset
    // the subscription states. `billingNextResetAt` is the usage window's, not
    // this one's, and is not used for it.
    if let Some(percent) = percent(customer.get("usageMonthPercentage")) {
        let reset = customer
            .get("subscription")
            .and_then(|subscription| subscription.get("currentPeriodEnd"))
            .and_then(stamp);
        windows.push(
            UsageWindow::new("Monthly", Some(percent_from_fraction(percent / 100.0)))
                .with_reset(reset),
        );
    }

    if windows.is_empty() {
        return ProviderUsage::failed(ID, NAME, "no limits reported in the reply");
    }

    ProviderUsage::ok(ID, NAME, windows).with_plan(plan(&customer))
}

/// The first object anywhere in `value` that is the customer's record.
fn customer_record(value: &Value) -> Option<Value> {
    match value {
        Value::Object(map) => {
            let named = map.contains_key("usageFourHourPercentage")
                || map.contains_key("usageMonthPercentage")
                || (map.contains_key("subscription") && map.contains_key("usageBand"));
            if named {
                return Some(value.clone());
            }
            map.values().find_map(customer_record)
        }
        Value::Array(items) => items.iter().find_map(customer_record),
        _ => None,
    }
}

/// A percentage the service reported: a finite number, never negative, and
/// never a boolean.
fn percent(value: Option<&Value>) -> Option<f64> {
    super::dig_number(value).filter(|percent| percent.is_finite() && *percent >= 0.0)
}

/// Epoch milliseconds, or seconds when the figure is too small to be
/// milliseconds — T3 Chat uses both.
fn stamp(value: &Value) -> Option<String> {
    let raw = super::dig_number(Some(value)).filter(|raw| raw.is_finite() && *raw > 0.0)?;
    let seconds = if raw > 10_000_000_000.0 { raw / 1_000.0 } else { raw };
    super::stamp_from_epoch_seconds(seconds)
}

/// "pro" → "Pro", "pro-max" → "Pro Max". T3 Chat's own plan name.
fn plan(customer: &Value) -> Option<String> {
    let raw = customer
        .get("subscription")
        .and_then(|subscription| subscription.get("productName"))
        .or_else(|| customer.get("subTier"))
        .and_then(Value::as_str)?
        .trim();
    if raw.is_empty() {
        return None;
    }

    Some(
        raw.split('-')
            .map(|word| {
                let mut chars = word.chars();
                match chars.next() {
                    Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
                    None => String::new(),
                }
            })
            .collect::<Vec<String>>()
            .join(" "),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn stream(customer: Value) -> String {
        // tRPC's line format: a result line, then the value with the record in.
        format!(
            "{}\n{}\n",
            json!([{ "result": { "data": { "json": null } } }]),
            json!([{ "result": { "data": { "json": customer } } }])
        )
    }

    #[test]
    fn reads_the_four_hour_and_the_monthly_figures() {
        let body = stream(json!({
            "usageFourHourPercentage": 40,
            "usageFourHourNextResetAt": 1_790_000_000_000i64,
            "usageMonthPercentage": 12.5,
            "subscription": { "productName": "pro-max", "currentPeriodEnd": 1_795_000_000_000i64 }
        }));

        let usage = reading(&body);
        let labels: Vec<&str> = usage.windows.iter().map(|w| w.label.as_str()).collect();
        assert_eq!(labels, vec!["4h", "Monthly"]);
        assert_eq!(usage.windows[0].percent_used, Some(40.0));
        assert_eq!(usage.windows[1].percent_used, Some(12.5));
        assert_eq!(
            usage.windows[0].resets_at.as_deref(),
            Some("2026-09-21T14:13:20Z")
        );
        assert_eq!(usage.plan.as_deref(), Some("Pro Max"));
    }

    #[test]
    fn a_percentage_the_service_left_out_is_left_off() {
        let body = stream(json!({ "usageFourHourPercentage": 10, "subTier": "pro" }));
        let usage = reading(&body);
        assert_eq!(usage.windows.len(), 1);
        assert_eq!(usage.windows[0].label, "4h");
        assert_eq!(usage.plan.as_deref(), Some("Pro"));
    }

    #[test]
    fn the_usage_windows_reset_stands_in_when_the_four_hours_has_none() {
        let body = stream(json!({
            "usageFourHourPercentage": 1,
            "usageWindowNextResetAt": 1_790_000_000
        }));
        assert_eq!(
            reading(&body).windows[0].resets_at.as_deref(),
            Some("2026-09-21T14:13:20Z")
        );
    }

    #[test]
    fn a_record_with_no_percentage_at_all_is_unreadable() {
        assert!(reading(&stream(json!({ "usageBand": "high" }))).error.is_some());
        assert!(reading("not json at all").error.is_some());
    }

    #[test]
    fn a_negative_percentage_is_not_a_reading() {
        let body = stream(json!({ "usageFourHourPercentage": -5, "usageMonthPercentage": 1 }));
        let usage = reading(&body);
        assert_eq!(usage.windows.len(), 1);
        assert_eq!(usage.windows[0].label, "Monthly");
    }
}
