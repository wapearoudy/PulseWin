//! A sub2api deployment's own accounting, read with a group API key.
//!
//! sub2api is an open-source gateway somebody runs themselves: it fronts their
//! Claude, Codex, Gemini and Grok subscriptions and re-sells them as an
//! OpenAI-shaped API. PulseWin carries it because it is the one shape this class
//! of service actually shares — the field names below are the ones a reader's
//! gateway answers with whether or not its operator has ever said what it runs.
//!
//! One route, `GET {address}/v1/usage`, with the group key as a bearer token.
//! Nothing is ever sent through it: this reads accounting and does not make
//! model requests.
//!
//! **The address is the reader's, and that is the whole difference from every
//! other provider here.** The others know where to go; this one is told, which
//! means PulseWin can be pointed at any host on the internet. So the address is
//! checked before a key is ever attached to a request — see `super::gateway_url`
//! — and the rule is HTTPS, except on a private network where there is nothing
//! to intercept.
//!
//! **A key and an address are both required, and this port has no settings
//! window to type them into.** They arrive from `PULSEWIN_SUB2API_KEY` and
//! `PULSEWIN_SUB2API_BASE_URL`, or from one
//! `%APPDATA%\PulseWin\sub2api.json` holding `apiKey` and `baseUrl`.
//! `is_configured` asks for both — a key with nowhere to send it is not a
//! credential — and asks only from disk, never over the network.
//!
//! The reply carries whichever of four shapes the key's group is configured
//! for, and a key usually has exactly one:
//!
//! ```json
//! { "isValid": true, "planName": "…", "unit": "USD",
//!   "balance": 16.34, "remaining": 16.34,            // wallet
//!   "quota": { "limit": 50, "used": 12, "remaining": 38 },
//!   "subscription": { "daily_usage_usd": 3, "daily_limit_usd": 10, … },
//!   "rate_limits": [ { "window": "5h", "limit": 100, "used": 9,
//!                      "remaining": 91, "reset_at": "…" } ] }
//! ```
//!
//! **A wallet is money and not an allowance**, so it draws no percentage at
//! all — the same rule DeepSeek's balance-only mode follows, and for the same
//! reason: there is no denominator, and this app does not invent one. The rail
//! shows the money instead — here, on the detail line of a window with no ring.
//! The other three shapes state their own denominators and are ordinary windows.
//!
//! **The deployment's own "spent" word is carried on the detail line.** It has
//! a `remaining` per limit, and a limit can be spent while the fraction beside
//! it still reads under 100 — the original keeps that as a flag beside the
//! window, and this port's windows have no such field, so the word rides where
//! the reader can still see it.

use std::sync::Arc;

use serde_json::Value;

use super::{by_window_length, percent_from_fraction, Ctx, FetchFuture, Provider};
use crate::model::{parse_reset, ProviderUsage, UsageWindow};

const USAGE_PATH: &str = "/v1/usage";

/// Every suffix a reader may already have typed. People paste the deployment's
/// root, and people paste the route they tested with curl.
const TRIMMING: [&str; 2] = ["/v1", "/v1/usage"];

pub struct Sub2Api;

impl Provider for Sub2Api {
    fn id(&self) -> &'static str {
        "sub2api"
    }

    fn name(&self) -> &'static str {
        "sub2api"
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
    const ID: &str = "sub2api";
    const NAME: &str = "sub2api";

    let Some(key) = super::provider_key(ID) else {
        return ProviderUsage::failed(ID, NAME, super::missing_key(ID, ""));
    };
    let Some(address) = super::provider_base_url(ID) else {
        return ProviderUsage::failed(ID, NAME, super::missing_address(ID));
    };
    let Some(url) = super::gateway_url(&address, USAGE_PATH, &TRIMMING) else {
        return ProviderUsage::failed(ID, NAME, super::refused_address());
    };

    // The gateway client, so a redirect cannot move the key to another host
    // after the address has been checked.
    let response = ctx
        .gateway_client
        .get(url)
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
                format!("request failed: {}", super::describe_reqwest_error(&e)),
            )
        }
    };

    let status = response.status();
    let body = match response.text().await {
        Ok(t) => t,
        Err(e) => return ProviderUsage::failed(ID, NAME, format!("cannot read body: {e}")),
    };

    // Only 200 is a reading here, which is the reference's own switch: this
    // route answers 200 with a body that says the key is no good, so anything
    // else is the deployment failing rather than the key.
    if status != reqwest::StatusCode::OK {
        return match status.as_u16() {
            401 | 403 => ProviderUsage::failed(ID, NAME, "the deployment refused the key"),
            429 => ProviderUsage::failed(ID, NAME, "rate limited, try again shortly"),
            300..=399 => ProviderUsage::failed(
                ID,
                NAME,
                "the deployment redirected the request — the key was not accepted",
            ),
            _ => ProviderUsage::failed(
                ID,
                NAME,
                format!("the deployment returned {}", status.as_u16()),
            ),
        };
    }

    let json: Value = match serde_json::from_str(&body) {
        Ok(v) => v,
        Err(e) => return ProviderUsage::failed(ID, NAME, format!("bad JSON: {e}")),
    };

    super::claude_code::debug_dump(ID, &json);

    reading(&json)
}

/// Every limit the reply states a denominator for, shortest first, plus the
/// wallet when the group sells one.
///
/// **Nothing here is inferred.** A shape whose limit is missing, zero or not
/// finite produces no window rather than a fraction of a number nobody gave,
/// and the wallet produces none at all because money is not an allowance.
fn reading(json: &Value) -> ProviderUsage {
    const ID: &str = "sub2api";
    const NAME: &str = "sub2api";

    if !json.is_object() {
        return ProviderUsage::failed(ID, NAME, "unreadable reply");
    }

    // The deployment's own word for a key it will not serve. It answers 200 and
    // says so in the body, so this is not an HTTP status away.
    if json.get("isValid").and_then(Value::as_bool) == Some(false) {
        return ProviderUsage::failed(ID, NAME, "the deployment refused the key");
    }

    let mut rows = Vec::new();

    // Rolling windows first: they are the only ones that report a reset, so
    // they are the only ones whose length may be drawn as a clock.
    for rate in json
        .get("rate_limits")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or_default()
    {
        let Some(label) = rate
            .get("window")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|label| !label.is_empty())
        else {
            continue;
        };
        let (Some(seconds), Some(fraction)) = (
            seconds_of(label),
            fraction(number(rate.get("used")), number(rate.get("limit"))),
        ) else {
            continue;
        };

        rows.push((
            seconds,
            UsageWindow::new(
                label_for_seconds(seconds),
                Some(percent_from_fraction(fraction)),
            )
            // The label *is* the length — "5h" is a statement, not a sort key —
            // so this one may be divided by, and its reset is a real one.
            .with_reset(rate.get("reset_at").and_then(parse_reset))
            .with_detail(spent_detail(rate.get("remaining"))),
        ));
    }

    if let Some(quota) = json.get("quota") {
        if let Some(fraction) = fraction(number(quota.get("used")), number(quota.get("limit"))) {
            rows.push((
                // A total allowance with no period at all. Thirty days is a
                // sort key so it lands after the rolling windows, and it says
                // so by claiming no length in its heading.
                30 * 86_400,
                UsageWindow::new("Spend", Some(percent_from_fraction(fraction)))
                    .with_detail(spent_detail(quota.get("remaining"))),
            ));
        }
    }

    if let Some(subscription) = json.get("subscription") {
        for (heading, seconds, used, limit) in [
            ("24h", 86_400, "daily_usage_usd", "daily_limit_usd"),
            ("7d", 7 * 86_400, "weekly_usage_usd", "weekly_limit_usd"),
            (
                "Monthly",
                30 * 86_400,
                "monthly_usage_usd",
                "monthly_limit_usd",
            ),
        ] {
            let Some(fraction) = fraction(
                number(subscription.get(used)),
                number(subscription.get(limit)),
            ) else {
                continue;
            };
            // The reply names the period and never says when it turns over, so
            // there is no clock to draw and no length to divide by — only a
            // name and a sort order.
            rows.push((
                seconds,
                UsageWindow::new(heading, Some(percent_from_fraction(fraction))),
            ));
        }
    }

    let mut windows = by_window_length(rows);

    let prepaid = wallet(json);
    if let Some((amount, currency)) = &prepaid {
        windows.push(super::balance_window("Balance", format!("{amount:.2} {currency}")));
    }

    if windows.is_empty() {
        return ProviderUsage::failed(ID, NAME, "no limits reported");
    }

    let plan = json
        .get("planName")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|plan| !plan.is_empty())
        .map(str::to_string);

    let mut usage = ProviderUsage::ok(ID, NAME, windows).with_plan(plan);
    if let Some((amount, currency)) = prepaid { usage = usage.with_credit_remaining(amount, currency); }
    usage
}

/// The wallet, where the group sells one.
///
/// `balance` is the deployment's own word for it. Root `remaining` is the same
/// figure in a wallet group and the remainder of something else in the others,
/// so it is read as money only when there is no quota and no subscription for
/// it to be the remainder *of*.
fn wallet(json: &Value) -> Option<(f64, String)> {
    let wallet_only = json.get("quota").is_none() && json.get("subscription").is_none();
    let amount = number(json.get("balance"))
        .or_else(|| wallet_only.then(|| number(json.get("remaining"))).flatten())?;
    let currency = currency(json)?;

    Some((amount, currency))
}

/// What the wallet is denominated in, or `None` if it cannot be said.
///
/// **None rather than a default.** A deployment selling credits reports a
/// `unit` that is not a currency, and calling those dollars is a figure with the
/// wrong name on it. A missing unit is the ordinary case and is USD, which is
/// what sub2api's own dashboard assumes.
fn currency(json: &Value) -> Option<String> {
    let unit = json
        .get("unit")
        .or_else(|| json.get("quota").and_then(|quota| quota.get("unit")))
        .and_then(Value::as_str)
        .map(|unit| unit.trim().to_uppercase());

    let Some(unit) = unit else {
        return Some("USD".to_string());
    };
    if unit.is_empty() {
        return Some("USD".to_string());
    }
    // An operator can also define a three-letter point/token unit. Only known
    // monetary currencies can become spendable credit or a low-money alert.
    if matches!(unit.as_str(), "USD"|"CNY"|"EUR"|"GBP"|"JPY"|"CHF"|"CAD"|"AUD"|"NZD"|"HKD"|"SGD"|"TWD"|"KRW"|"INR"|"BRL"|"MXN"|"AED"|"SAR"|"TRY"|"RUB"|"ZAR"|"IDR"|"VND"|"THB"|"PLN"|"SEK"|"NOK"|"DKK") {
        return Some(unit);
    }
    None
}

/// `5h` / `1d` / `7d` as seconds. A count and one of two units, so a deployment
/// that reports `3h` is read rather than dropped.
///
/// **Not `m`.** In a scheme of hours and days it could as well be a month as a
/// minute, and read as minutes a `1m` window drew a sixty-second clock. A
/// length this app cannot be sure of is not stated.
fn seconds_of(window: &str) -> Option<i64> {
    let text = window.to_lowercase();
    let mut chars = text.chars();
    let unit = chars.next_back()?;
    let count: i64 = chars.as_str().parse().ok()?;
    if count <= 0 {
        return None;
    }

    match unit {
        'h' => Some(count * 3_600),
        'd' => Some(count * 86_400),
        _ => None,
    }
}

/// The three lengths this port has names for, and a plain length for everything
/// else.
fn label_for_seconds(seconds: i64) -> String {
    match seconds {
        18_000 => "5h".to_string(),
        86_400 => "24h".to_string(),
        604_800 => "7d".to_string(),
        other => super::humanize_window_seconds(other),
    }
}

/// How much of a stated allowance is gone, or `None` where one was not stated.
///
/// **A limit of zero is not a limit.** Dividing by it produces infinity, which
/// the display clamps to a full ring — an account reported as spent on the
/// strength of a field the deployment left blank.
fn fraction(used: Option<f64>, limit: Option<f64>) -> Option<f64> {
    let limit = limit.filter(|limit| limit.is_finite() && *limit > 0.0)?;
    let used = used.filter(|used| used.is_finite())?;
    Some((used / limit).clamp(0.0, 1.0))
}

/// Whether the deployment says this limit has nothing left.
///
/// Its own `remaining`, not the arithmetic above: a field that is absent has
/// said nothing, and reading that as zero marks a limit spent on the strength
/// of silence.
fn is_spent(remaining: Option<&Value>) -> bool {
    number(remaining).is_some_and(|remaining| remaining <= 0.0)
}

/// The deployment's word, kept on the line where a flag would have gone.
fn spent_detail(remaining: Option<&Value>) -> Option<String> {
    is_spent(remaining).then(|| "spent".to_string())
}

fn number(value: Option<&Value>) -> Option<f64> {
    super::dig_number(value)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn monetary_wallet_has_numeric_credit_but_tokens_and_points_do_not() {
        let usage = reading(&json!({"balance":12.345,"unit":"CNY"}));
        assert_eq!(usage.credit_remaining.as_ref().unwrap().amount,12.345);
        assert_eq!(usage.credit_remaining.as_ref().unwrap().currency,"CNY");
        for unit in ["tokens","points","PTS","TOK","credits"] { assert!(reading(&json!({"balance":123,"unit":unit})).credit_remaining.is_none()); }
        assert!(reading(&json!({"quota":{"used":1,"limit":10,"unit":"USD"},"remaining":9})).credit_remaining.is_none());
    }

    /// The four shapes, as the service documents them.
    fn rate_limits() -> Value {
        json!({
            "rate_limits": [
                { "window": "5h", "limit": 100, "used": 9, "remaining": 91,
                  "reset_at": "2026-10-02T05:00:00Z" },
                { "window": "7d", "limit": 500, "used": 500, "remaining": 0 }
            ]
        })
    }

    #[test]
    fn a_rolling_window_keeps_its_length_and_its_reset() {
        let usage = reading(&rate_limits());
        let labels: Vec<&str> = usage.windows.iter().map(|w| w.label.as_str()).collect();

        assert_eq!(labels, vec!["5h", "7d"]);
        assert_eq!(usage.windows[0].percent_used, Some(9.0));
        assert_eq!(
            usage.windows[0].resets_at.as_deref(),
            Some("2026-10-02T05:00:00Z")
        );
        assert_eq!(usage.windows[0].detail, None);
        // The deployment's own word for a limit with nothing left.
        assert_eq!(usage.windows[1].detail.as_deref(), Some("spent"));
        assert_eq!(usage.windows[1].percent_used, Some(100.0));
        assert!(usage.windows[1].resets_at.is_none());
    }

    #[test]
    fn a_window_length_that_cannot_be_read_is_not_stated() {
        let reply = json!({ "rate_limits": [
            { "window": "1m", "limit": 10, "used": 1 },
            { "window": "0h", "limit": 10, "used": 1 },
            { "window": "3h", "limit": 10, "used": 1 },
            { "window": "2d", "limit": 10, "used": 1 }
        ] });
        let usage = reading(&reply);
        // A minute in a scheme of hours and days could be a month: dropped. A
        // count of zero is not a window either.
        let labels: Vec<&str> = usage.windows.iter().map(|w| w.label.as_str()).collect();
        assert_eq!(labels, vec!["3h", "2d"]);
    }

    #[test]
    fn a_quota_draws_a_ring_and_a_wallet_does_not() {
        let quota = json!({ "quota": { "limit": 50, "used": 12, "remaining": 38, "unit": "USD" } });
        let usage = reading(&quota);
        assert_eq!(usage.windows.len(), 1);
        assert_eq!(usage.windows[0].label, "Spend");
        assert_eq!(usage.windows[0].percent_used, Some(24.0));
        assert!(usage.windows[0].resets_at.is_none());

        // A wallet is money, so it draws no percentage at all.
        let wallet = json!({ "balance": 16.34, "remaining": 16.34, "unit": "USD" });
        let usage = reading(&wallet);
        assert_eq!(usage.windows[0].label, "Balance");
        assert_eq!(usage.windows[0].percent_used, None);
        assert_eq!(usage.windows[0].detail.as_deref(), Some("16.34 USD"));
    }

    #[test]
    fn a_root_remainder_is_money_only_when_there_is_nothing_it_is_the_remainder_of() {
        // With a quota beside it, `remaining` is the quota's, not a wallet.
        let both = json!({
            "quota": { "limit": 50, "used": 12, "remaining": 38 },
            "balance": 16.34,
            "unit": "USD"
        });
        let labels: Vec<String> = reading(&both).windows.iter().map(|w| w.label.clone()).collect();
        // `balance` is still money — it is the deployment's own word for it.
        assert_eq!(labels, vec!["Spend", "Balance"]);

        let remainder_only = json!({ "quota": { "limit": 50, "used": 12 }, "remaining": 38 });
        let usage = reading(&remainder_only);
        assert_eq!(usage.windows.len(), 1);
        assert_eq!(usage.windows[0].label, "Spend");
    }

    #[test]
    fn the_wallet_is_named_in_the_unit_the_deployment_states() {
        // No window at all where the unit cannot be named: the money is not
        // shown under a currency the deployment did not state.
        let named = |unit: Value| {
            let mut reply = json!({ "balance": 1.0 });
            if !unit.is_null() {
                reply["unit"] = unit;
            }
            reading(&reply)
                .windows
                .first()
                .and_then(|window| window.detail.clone())
        };

        assert_eq!(named(json!("usd")).as_deref(), Some("1.00 USD"));
        assert_eq!(named(json!("cny")).as_deref(), Some("1.00 CNY"));
        // A missing unit is the ordinary case.
        assert_eq!(named(Value::Null).as_deref(), Some("1.00 USD"));
        assert_eq!(named(json!("   ")).as_deref(), Some("1.00 USD"));
        // Credits are not a currency, and calling them dollars puts the wrong
        // name on the figure.
        assert_eq!(named(json!("credits")), None);
        assert_eq!(named(json!("US$")), None);
    }

    #[test]
    fn a_subscription_states_its_periods_and_no_boundaries() {
        let reply = json!({ "subscription": {
            "daily_usage_usd": 3.0, "daily_limit_usd": 10.0,
            "weekly_usage_usd": 20.0, "weekly_limit_usd": 40.0,
            "monthly_usage_usd": 50.0, "monthly_limit_usd": 200.0
        } });
        let usage = reading(&reply);
        let labels: Vec<&str> = usage.windows.iter().map(|w| w.label.as_str()).collect();

        assert_eq!(labels, vec!["24h", "7d", "Monthly"]);
        assert_eq!(usage.windows[0].percent_used, Some(30.0));
        assert_eq!(usage.windows[2].percent_used, Some(25.0));
        assert!(usage.windows.iter().all(|w| w.resets_at.is_none()));
    }

    #[test]
    fn a_limit_of_zero_is_not_a_limit() {
        let reply = json!({
            "quota": { "limit": 0, "used": 5 },
            "rate_limits": [ { "window": "5h", "limit": 0, "used": 5 } ]
        });
        assert!(reading(&reply).error.is_some());
    }

    #[test]
    fn an_overdrawn_limit_is_a_full_ring_rather_than_more() {
        let reply = json!({ "quota": { "limit": 10, "used": 25 } });
        assert_eq!(reading(&reply).windows[0].percent_used, Some(100.0));
        // A `used` below zero is clamped too, not drawn as nothing.
        let reply = json!({ "quota": { "limit": 10, "used": -5 } });
        assert_eq!(reading(&reply).windows[0].percent_used, Some(0.0));
    }

    #[test]
    fn a_key_the_deployment_will_not_serve_arrives_as_a_good_http_status() {
        let reply = json!({ "isValid": false, "quota": { "limit": 10, "used": 1 } });
        let usage = reading(&reply);
        assert_eq!(usage.error.as_deref(), Some("the deployment refused the key"));

        // The plan comes across when the reply names one.
        let named = json!({ "planName": "Team", "quota": { "limit": 10, "used": 1 } });
        assert_eq!(reading(&named).plan.as_deref(), Some("Team"));
    }

    #[test]
    fn a_reply_with_nothing_to_draw_is_no_limits_rather_than_a_ring_at_zero() {
        assert!(reading(&json!({ "isValid": true })).error.is_some());
        assert!(reading(&json!({ "balance": 0.0, "unit": "USD" })).windows[0]
            .percent_used
            .is_none());
        assert!(reading(&json!("nope")).error.is_some());
    }

    #[test]
    fn the_windows_lead_with_the_shortest() {
        let reply = json!({
            "rate_limits": [ { "window": "7d", "limit": 10, "used": 1 } ],
            "quota": { "limit": 10, "used": 1 },
            "subscription": {
                "daily_usage_usd": 1.0, "daily_limit_usd": 10.0,
                "monthly_usage_usd": 1.0, "monthly_limit_usd": 10.0
            },
            "balance": 1.0
        });
        let usage = reading(&reply);
        let labels: Vec<&str> = usage.windows.iter().map(|w| w.label.as_str()).collect();
        // The quota and the month share a sort key, so the quota — which the
        // reply stated first — keeps its place ahead of the month.
        assert_eq!(labels, vec!["24h", "7d", "Spend", "Monthly", "Balance"]);
    }
}
