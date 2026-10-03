//! Factory, the company behind the Droid agent: either a five-hour, a weekly
//! and a monthly limit, each a percentage Factory states, or — on the older
//! billing — a Standard and a Premium token allowance for the billing period.
//!
//! Read with a key the reader supplies (`fk-…`, from app.factory.ai's API keys
//! page), sent as a bearer token to `api.factory.ai` and nowhere else. Three
//! requests, in the order Factory's own web app makes them:
//!
//! 1. `GET /api/app/auth/me` — the plan's name and the user's id. This is the
//!    one that says whether the key is any good.
//! 2. `GET /api/billing/limits` — the token-rate-limits billing. Only an
//!    account that says `usesTokenRateLimitsBilling` is read from here; any
//!    other answer, including a failure, means the older billing is asked.
//! 3. `GET /api/organization/subscription/usage` — the Standard and Premium
//!    allowances, for everyone else.
//!
//! The shapes are second-hand — taken from CodexBar's Factory provider and its
//! tests, not from a captured reply — and the fixtures below say so.
//!
//! **What CodexBar does and this does not.** It draws a window whose stated end
//! has passed as 0%, because Factory's own page does; here a figure for a
//! window that is over is left off, and comes back when the service states a
//! new one. It treats an allowance over a trillion tokens as "unlimited" and
//! draws usage against a hundred million it chose; here that limit is left off.
//! And it reads a `usedRatio` above 1 as a percentage when the allowance is
//! missing; here a ratio whose scale is a guess is not read at all.
//!
//! **The money bought on top of the limits has no field of its own here.** The
//! original carries it as a separate credit balance beside the windows; this
//! port's result has only windows, so it rides as a window with no fraction and
//! the amount on its detail line — the same shape the balance-only providers
//! use. It is still shown for the same two reasons: the account can spend it,
//! or it has some.

use std::sync::Arc;

use chrono::{DateTime, Utc};
use serde_json::Value;

use super::{percent_from_fraction, Ctx, FetchFuture, Provider};
use crate::model::ProviderUsage;

const BASE: &str = "https://api.factory.ai";
const ME_PATH: &str = "/api/app/auth/me";
const LIMITS_PATH: &str = "/api/billing/limits";
const USAGE_PATH: &str = "/api/organization/subscription/usage";

/// Above this an allowance is Factory's way of writing "unlimited", and a
/// fraction of it is not a figure anybody reported.
const UNLIMITED: f64 = 1e12;

pub struct Factory;

impl Provider for Factory {
    fn id(&self) -> &'static str {
        "factory"
    }

    fn name(&self) -> &'static str {
        "Factory"
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
    const ID: &str = "factory";
    const NAME: &str = "Factory";

    let Some(key) = super::provider_key(ID) else {
        return ProviderUsage::failed(ID, NAME, super::missing_key(ID, ""));
    };

    let me = match get(&ctx, &format!("{BASE}{ME_PATH}"), &key).await {
        Ok(body) => body,
        Err(problem) => return ProviderUsage::failed(ID, NAME, problem),
    };

    let identity = account(me.as_deref());

    // The newer billing, when the account is on it. Anything else — a failure
    // included — is Factory saying to ask the older route, which is what its
    // own web app does.
    if let Ok(body) = get(&ctx, &format!("{BASE}{LIMITS_PATH}"), &key).await {
        let json = body.and_then(|text| serde_json::from_str::<Value>(&text).ok());
        if let Some(json) = json {
            if let Some(reading) = limits_reading(&json, identity.plan.clone(), Utc::now()) {
                return reading;
            }
        }
    }

    let url = match identity.user_id.as_deref() {
        Some(user) => format!("{BASE}{USAGE_PATH}?useCache=true&userId={user}"),
        None => format!("{BASE}{USAGE_PATH}?useCache=true"),
    };

    let body = match get(&ctx, &url, &key).await {
        Ok(body) => body,
        Err(problem) => return ProviderUsage::failed(ID, NAME, problem),
    };

    let Some(text) = body else {
        return ProviderUsage::failed(ID, NAME, "cannot read body");
    };
    let json: Value = match serde_json::from_str(&text) {
        Ok(v) => v,
        Err(e) => return ProviderUsage::failed(ID, NAME, format!("bad JSON: {e}")),
    };

    super::claude_code::debug_dump(ID, &json);

    allowance_reading(&json, identity.plan)
}

/// One authenticated GET, on the client that follows redirects — this is not a
/// gateway, the host is Factory's own and is not reader-supplied.
///
/// `Ok(None)` is never produced: a request either answers with a body or says
/// why it did not.
async fn get(ctx: &Ctx, url: &str, key: &str) -> Result<Option<String>, String> {
    let response = ctx
        .client
        .get(url)
        .header("Authorization", format!("Bearer {key}"))
        .header("Accept", "application/json")
        // The headers Factory's web app sends, which are the ones CodexBar has
        // seen it accept a key with.
        .header("x-factory-client", "web-app")
        .header("Origin", "https://app.factory.ai")
        .header("Referer", "https://app.factory.ai/")
        .send()
        .await
        .map_err(|e| format!("request failed: {}", super::describe_reqwest_error(&e)))?;

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

    Ok(Some(body))
}

// ---------------------------------------------------------------------------
// Who is asking
// ---------------------------------------------------------------------------

/// The plan's name and the user's id, when the reply carries them.
///
/// Neither is needed for a reading, so a reply without them is not a failure —
/// it is a key that works and an account that says nothing about itself.
struct Account {
    plan: Option<String>,
    user_id: Option<String>,
}

fn account(text: Option<&str>) -> Account {
    let json: Option<Value> = text.and_then(|text| serde_json::from_str(text).ok());

    let pick = |path: &str| -> Option<String> {
        json.as_ref()
            .and_then(|json| crate::credentials::dig_str(json, path))
            .map(|text| text.trim().to_string())
            .filter(|text| !text.is_empty())
    };

    let plan = pick("organization.subscription.orbSubscription.plan.name").or_else(|| {
        pick("organization.subscription.factoryTier").map(|tier| capitalized(&tier))
    });

    Account {
        plan,
        user_id: pick("userProfile.id"),
    }
}

/// Swift's `capitalized`, for the tier Factory writes in lower case.
fn capitalized(text: &str) -> String {
    text.split_whitespace()
        .map(|word| {
            let mut chars = word.chars();
            match chars.next() {
                Some(first) => {
                    first.to_uppercase().collect::<String>() + &chars.as_str().to_lowercase()
                }
                None => String::new(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

// ---------------------------------------------------------------------------
// The token-rate-limits billing
// ---------------------------------------------------------------------------

/// The three windows, as `(json key, heading, seconds)`.
///
/// `monthly` is a billing month rather than a fixed thirty days, so its length
/// is only a sort key and is not claimed in the heading.
const SHAPES: [(&str, &str, i64); 3] = [
    ("fiveHour", "5h", 5 * 3_600),
    ("weekly", "7d", 7 * 86_400),
    ("monthly", "Monthly", 30 * 86_400),
];

/// The newer billing's reading, or `None` when this account is not on it and
/// the older route should be asked instead.
fn limits_reading(json: &Value, plan: Option<String>, now: DateTime<Utc>) -> Option<ProviderUsage> {
    const ID: &str = "factory";
    const NAME: &str = "Factory";

    if json.get("usesTokenRateLimitsBilling").and_then(Value::as_bool) != Some(true) {
        return None;
    }
    let pools = json.get("limits")?;

    let mut windows = pools
        .get("standard")
        .map(|pool| windows_of(pool, None, now))
        .unwrap_or_default();

    // Core is its own pool of models. CodexBar draws it only once it has
    // something in it, and so does this: an empty pool with no clock is a pool
    // the account is not using.
    if let Some(core) = pools.get("core") {
        let core_windows = windows_of(core, Some("Core"), now);
        if core_windows
            .iter()
            .any(|window| window.percent_used.unwrap_or(0.0) > 0.0 || window.resets_at.is_some())
        {
            windows.extend(core_windows);
        }
    }

    let balance = extra_usage(json);
    if windows.is_empty() && balance.is_none() {
        return Some(ProviderUsage::failed(ID, NAME, "no limits reported"));
    }
    if let Some(balance) = balance {
        windows.push(balance);
    }

    Some(ProviderUsage::ok(ID, NAME, windows).with_plan(plan))
}

fn windows_of(pool: &Value, scope: Option<&str>, now: DateTime<Utc>) -> Vec<crate::model::UsageWindow> {
    SHAPES
        .iter()
        .filter_map(|(key, heading, _)| {
            let window = pool.get(*key)?;

            let percent = super::dig_number(window.get("usedPercent"))
                .filter(|percent| percent.is_finite() && *percent >= 0.0)?;

            let stated_end = instant(window.get("windowEnd"));

            let reset = super::dig_number(window.get("secondsRemaining"))
                .filter(|seconds| seconds.is_finite() && *seconds > 0.0)
                .and_then(|seconds| {
                    chrono::Duration::try_seconds(seconds as i64).map(|span| now + span)
                })
                .or_else(|| stated_end.filter(|end| *end > now));

            // The window this figure belongs to is over and no new one has been
            // stated. CodexBar draws it as 0%; the figure is simply gone.
            if reset.is_none() && stated_end.is_some() {
                return None;
            }

            let label = match scope {
                Some(scope) => format!("{heading} {scope}"),
                None => heading.to_string(),
            };

            Some(
                crate::model::UsageWindow::new(label, Some(percent_from_fraction(percent / 100.0)))
                    .with_reset(reset.map(stamp)),
            )
        })
        .collect()
}

/// Money bought on top of the limits, in US cents. Shown when the account can
/// use it or has some; an account that has never been offered it is not told it
/// has none.
fn extra_usage(json: &Value) -> Option<crate::model::UsageWindow> {
    let cents = json.get("extraUsageBalanceCents").and_then(Value::as_i64)?;
    let allowed = json.get("extraUsageAllowed").and_then(Value::as_bool) == Some(true);
    if cents < 0 || (cents == 0 && !allowed) {
        return None;
    }

    Some(super::balance_window(
        "Extra usage",
        format!("{:.2} USD", cents as f64 / 100.0),
    ))
}

// ---------------------------------------------------------------------------
// The older billing
// ---------------------------------------------------------------------------

/// The Standard and Premium allowances for the billing period.
fn allowance_reading(json: &Value, plan: Option<String>) -> ProviderUsage {
    const ID: &str = "factory";
    const NAME: &str = "Factory";

    let Some(period) = json.get("usage") else {
        return ProviderUsage::failed(ID, NAME, "no usage in response");
    };

    let start = instant(period.get("startDate"));
    let end = instant(period.get("endDate"));

    // The period's length is stated when both ends are; otherwise thirty days
    // is a sort key and nothing more.
    let stated = start
        .zip(end)
        .map(|(start, end)| (end - start).num_seconds())
        .filter(|seconds| *seconds > 0);

    let windows: Vec<crate::model::UsageWindow> = [("standard", "Standard"), ("premium", "Premium")]
        .iter()
        .filter_map(|(key, heading)| {
            let tokens = period.get(*key)?;
            let fraction = fraction(tokens)?;

            Some(
                crate::model::UsageWindow::new(
                    format!("Monthly {heading}"),
                    Some(percent_from_fraction(fraction)),
                )
                .with_reset(end.map(stamp)),
            )
        })
        .collect();

    if windows.is_empty() {
        return ProviderUsage::failed(ID, NAME, "no limits reported");
    }

    // A period whose length was stated is what the rows are headed by; thirty
    // days is the fallback and claims nothing, so the heading is the month
    // either way.
    let _ = stated;

    ProviderUsage::ok(ID, NAME, windows).with_plan(plan)
}

/// Factory's own ratio where it gives a usable one, and otherwise the tokens
/// used against the allowance, both as reported.
///
/// The ratio is on a 0…1 scale. CodexBar clamps a hair over either end and so
/// does this; anything further out is a scale nobody stated. A ratio of zero
/// beside tokens used and a real allowance is Factory's cache lagging, and the
/// two counts are read instead — also CodexBar's rule. A zero with no allowance
/// at all is a pool this plan does not have.
fn fraction(tokens: &Value) -> Option<f64> {
    let used = super::dig_number(tokens.get("userTokens"))
        .filter(|used| used.is_finite() && *used >= 0.0);
    let allowance = super::dig_number(tokens.get("totalAllowance"))
        .filter(|allowance| allowance.is_finite() && *allowance > 0.0 && *allowance <= UNLIMITED);

    if let Some(ratio) = super::dig_number(tokens.get("usedRatio"))
        .filter(|ratio| ratio.is_finite() && *ratio >= -0.001 && *ratio <= 1.001)
    {
        let lagging = ratio <= 0.0 && used.unwrap_or(0.0) > 0.0 && allowance.is_some();
        let absent = ratio <= 0.0 && allowance.is_none();
        if !lagging && !absent {
            return Some(ratio.clamp(0.0, 1.0));
        }
    }

    Some(used? / allowance?)
}

// ---------------------------------------------------------------------------
// Dates
// ---------------------------------------------------------------------------

/// The instant a Factory date names.
///
/// A date arrives as seconds, as milliseconds, as either of those in a string,
/// or as ISO 8601 — CodexBar has seen all four. Anything past 10¹² is
/// milliseconds: as seconds it would be the year 33,000.
fn instant(value: Option<&Value>) -> Option<DateTime<Utc>> {
    let value = value?;

    match value {
        Value::Number(number) => epoch(number.as_f64()?),
        Value::String(text) => {
            let text = text.trim();
            if let Ok(number) = text.parse::<f64>() {
                return epoch(number);
            }
            let reset = crate::model::parse_reset(value)?;
            DateTime::parse_from_rfc3339(&reset)
                .ok()
                .map(|at| at.with_timezone(&Utc))
        }
        _ => None,
    }
}

fn epoch(number: f64) -> Option<DateTime<Utc>> {
    if !number.is_finite() || number <= 0.0 {
        return None;
    }
    let seconds = if number > 1e12 { number / 1_000.0 } else { number };
    DateTime::from_timestamp(seconds as i64, 0)
}

fn stamp(at: DateTime<Utc>) -> String {
    at.to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn at(text: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(text).unwrap().with_timezone(&Utc)
    }

    /// The newer billing's reply, as CodexBar's tests describe it.
    fn limits_fixture() -> Value {
        json!({
            "usesTokenRateLimitsBilling": true,
            "limits": {
                "standard": {
                    "fiveHour": { "usedPercent": 25.0, "windowEnd": "2026-10-02T05:00:00Z" },
                    "weekly": { "usedPercent": 50.0, "secondsRemaining": 3600.0 },
                    "monthly": { "usedPercent": 75.0, "windowEnd": "2026-11-01T00:00:00Z" }
                }
            }
        })
    }

    #[test]
    fn reads_the_three_limits_the_service_states() {
        let usage = limits_reading(&limits_fixture(), None, at("2026-10-02T01:00:00Z")).unwrap();
        let labels: Vec<&str> = usage.windows.iter().map(|w| w.label.as_str()).collect();

        assert_eq!(labels, vec!["5h", "7d", "Monthly"]);
        assert_eq!(usage.windows[0].percent_used, Some(25.0));
        assert_eq!(usage.windows[1].percent_used, Some(50.0));
        assert_eq!(usage.windows[2].percent_used, Some(75.0));
    }

    #[test]
    fn a_reset_is_the_countdown_when_there_is_one_and_the_end_when_there_is_not() {
        let usage = limits_reading(&limits_fixture(), None, at("2026-10-02T01:00:00Z")).unwrap();
        assert_eq!(
            usage.windows[0].resets_at.as_deref(),
            Some("2026-10-02T05:00:00Z")
        );
        // An hour remaining is an hour from now, not a timestamp in the reply.
        assert_eq!(
            usage.windows[1].resets_at.as_deref(),
            Some("2026-10-02T02:00:00Z")
        );
    }

    #[test]
    fn a_window_that_is_over_is_left_off_rather_than_drawn_as_zero() {
        let reply = json!({
            "usesTokenRateLimitsBilling": true,
            "limits": { "standard": {
                "fiveHour": { "usedPercent": 40.0, "windowEnd": "2026-10-01T00:00:00Z" },
                "weekly": { "usedPercent": 10.0, "windowEnd": "2026-10-09T00:00:00Z" }
            } }
        });
        let usage = limits_reading(&reply, None, at("2026-10-02T01:00:00Z")).unwrap();
        assert_eq!(usage.windows.len(), 1);
        assert_eq!(usage.windows[0].label, "7d");

        // Every window over, and no balance: nothing to draw at all.
        let reply = json!({
            "usesTokenRateLimitsBilling": true,
            "limits": { "standard": {
                "fiveHour": { "usedPercent": 40.0, "windowEnd": "2026-10-01T00:00:00Z" }
            } }
        });
        let usage = limits_reading(&reply, None, at("2026-10-02T01:00:00Z")).unwrap();
        assert!(usage.error.is_some());
    }

    #[test]
    fn the_core_pool_is_drawn_only_once_it_has_something_in_it() {
        let empty = json!({
            "usesTokenRateLimitsBilling": true,
            "limits": {
                "standard": { "fiveHour": { "usedPercent": 10.0, "secondsRemaining": 60.0 } },
                "core": { "fiveHour": {}, "weekly": {} }
            }
        });
        let usage = limits_reading(&empty, None, at("2026-10-02T01:00:00Z")).unwrap();
        assert_eq!(usage.windows.len(), 1);

        let used = json!({
            "usesTokenRateLimitsBilling": true,
            "limits": {
                "standard": { "fiveHour": { "usedPercent": 10.0, "secondsRemaining": 60.0 } },
                "core": { "fiveHour": { "usedPercent": 0.0, "secondsRemaining": 60.0 } }
            }
        });
        let usage = limits_reading(&used, None, at("2026-10-02T01:00:00Z")).unwrap();
        let labels: Vec<&str> = usage.windows.iter().map(|w| w.label.as_str()).collect();
        // A pool with a clock counts even at nothing spent: it is in use.
        assert_eq!(labels, vec!["5h", "5h Core"]);
    }

    #[test]
    fn an_account_on_the_older_billing_is_not_read_from_the_new_route() {
        assert!(limits_reading(&json!({ "limits": {} }), None, at("2026-10-02T01:00:00Z")).is_none());
        assert!(limits_reading(
            &json!({ "usesTokenRateLimitsBilling": false, "limits": {} }),
            None,
            at("2026-10-02T01:00:00Z")
        )
        .is_none());
    }

    #[test]
    fn money_on_top_of_the_limits_is_shown_only_when_it_is_usable_or_there() {
        let with = |cents: i64, allowed: bool| {
            let mut reply = limits_fixture();
            reply["extraUsageBalanceCents"] = json!(cents);
            reply["extraUsageAllowed"] = json!(allowed);
            limits_reading(&reply, None, at("2026-10-02T01:00:00Z")).unwrap()
        };

        let usage = with(1234, true);
        assert_eq!(usage.windows.len(), 4);
        assert_eq!(usage.windows[3].label, "Extra usage");
        assert_eq!(usage.windows[3].detail.as_deref(), Some("12.34 USD"));
        assert_eq!(usage.windows[3].percent_used, None);

        // Some money and no permission to spend it: still money in the account.
        assert_eq!(with(500, false).windows[3].detail.as_deref(), Some("5.00 USD"));
        // None, and never offered any: the account is not told it has none.
        assert_eq!(with(0, false).windows.len(), 3);
        assert_eq!(with(0, true).windows[3].detail.as_deref(), Some("0.00 USD"));
    }

    #[test]
    fn the_older_billing_reads_standard_and_premium() {
        let reply = json!({
            "usage": {
                "startDate": 1758931200,
                "endDate": "2026-10-31T00:00:00Z",
                "standard": { "userTokens": 250.0, "totalAllowance": 1000.0, "usedRatio": 0.25 },
                "premium": { "userTokens": 900.0, "totalAllowance": 1000.0, "usedRatio": 1.4 }
            }
        });
        let usage = allowance_reading(&reply, Some("Pro".to_string()));
        let labels: Vec<&str> = usage.windows.iter().map(|w| w.label.as_str()).collect();

        assert_eq!(labels, vec!["Monthly Standard", "Monthly Premium"]);
        assert_eq!(usage.windows[0].percent_used, Some(25.0));
        // A ratio past either end is a scale nobody stated: the counts are read
        // instead.
        assert_eq!(usage.windows[1].percent_used, Some(90.0));
        assert_eq!(
            usage.windows[0].resets_at.as_deref(),
            Some("2026-10-31T00:00:00Z")
        );
        assert_eq!(usage.plan.as_deref(), Some("Pro"));
    }

    #[test]
    fn a_lagging_ratio_beside_real_counts_is_not_believed() {
        let reply = json!({
            "usage": {
                "standard": { "userTokens": 250.0, "totalAllowance": 1000.0, "usedRatio": 0.0 },
                "premium": { "userTokens": 0.0, "totalAllowance": 1000.0, "usedRatio": 0.0 }
            }
        });
        let usage = allowance_reading(&reply, None);
        // Two counts and an allowance: read from the counts.
        assert_eq!(usage.windows[0].percent_used, Some(25.0));
        // No tokens used at all: the zero is the truth.
        assert_eq!(usage.windows[1].percent_used, Some(0.0));
    }

    #[test]
    fn an_allowance_factory_calls_unlimited_is_left_off() {
        let reply = json!({
            "usage": {
                "standard": { "userTokens": 5.0, "totalAllowance": 1e13, "usedRatio": 0.0 },
                "premium": { "userTokens": 5.0, "totalAllowance": 0.0 }
            }
        });
        // Both are unreadable: one denominator is "unlimited" and the other is
        // nothing, and neither is a limit anybody stated.
        assert!(allowance_reading(&reply, None).error.is_some());
    }

    #[test]
    fn a_date_is_read_in_any_of_the_four_shapes_factory_writes() {
        let epoch_of = |value: Value| instant(Some(&value)).map(stamp);

        assert_eq!(
            epoch_of(json!(1_759_276_800)).as_deref(),
            Some("2025-10-01T00:00:00Z")
        );
        assert_eq!(
            epoch_of(json!(1_759_276_800_000i64)).as_deref(),
            Some("2025-10-01T00:00:00Z")
        );
        assert_eq!(
            epoch_of(json!("1759276800")).as_deref(),
            Some("2025-10-01T00:00:00Z")
        );
        assert_eq!(
            epoch_of(json!("2026-10-31T00:00:00Z")).as_deref(),
            Some("2026-10-31T00:00:00Z")
        );
        assert_eq!(epoch_of(json!(0)), None);
        assert_eq!(epoch_of(json!("soon")), None);
        assert_eq!(epoch_of(json!(true)), None);
    }

    #[test]
    fn the_plan_comes_from_the_subscription_or_the_tier() {
        let named = json!({
            "organization": { "subscription": {
                "factoryTier": "pro",
                "orbSubscription": { "plan": { "name": "Pro Annual" } }
            } },
            "userProfile": { "id": " user-1 " }
        });
        let me = account(Some(&named.to_string()));
        assert_eq!(me.plan.as_deref(), Some("Pro Annual"));
        assert_eq!(me.user_id.as_deref(), Some("user-1"));

        let tier_only = json!({
            "organization": { "subscription": { "factoryTier": "team plan" } }
        });
        assert_eq!(
            account(Some(&tier_only.to_string())).plan.as_deref(),
            Some("Team Plan")
        );

        // Neither, and a reply that is not JSON at all, are both simply no
        // identity — never a failed reading.
        assert!(account(Some("{}")).plan.is_none());
        assert!(account(Some("not json")).plan.is_none());
        assert!(account(None).user_id.is_none());
    }
}
