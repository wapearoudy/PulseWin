//! Kilo Code: the account's credit balance, and the Kilo Pass allowance for
//! the current billing period when the account has a pass.
//!
//! Read from the tRPC routes Kilo's own dashboard calls, in one batched GET:
//! `https://app.kilo.ai/api/trpc/user.getCreditBlocks,kiloPass.getState`. The
//! key is one the reader supplies (`PULSEWIN_KILO_CODE_KEY`, or
//! `%APPDATA%\PulseWin\kilo-code.json`), or failing that the login the Kilo CLI
//! saved in `~/.local/share/kilo/auth.json` — the path the original reads.
//! `is_configured` is true when either is there, and makes no network call.
//! Each credential is refused in its own words, because the remedy differs: a
//! key that is turned away needs replacing, a stale CLI login needs signing in
//! again.
//!
//! The shape is second-hand — taken from CodexBar's Kilo provider and its
//! tests, not from a captured reply — and the fixture below says so.
//!
//! **What is not read.** CodexBar also draws a ring from the credit blocks —
//! the sum of what each block started with against what is left of them — and
//! hunts a dozen alternative key names for each figure. The blocks start and
//! expire at different times, so their sum is not an allowance anyone sells;
//! the balance Kilo reports is shown as a balance instead. Only the field names
//! seen in a Kilo reply are read. Organizations, which CodexBar can switch
//! between, are not: the personal account is what the key reads.
//!
//! The balance keeps its numeric USD amount beside the windows. Its display
//! row has no utilization fraction; low-balance rules consume the numeric field.

use std::path::PathBuf;
use std::sync::Arc;

use serde_json::Value;

use super::{percent_from_fraction, Ctx, FetchFuture, Provider};
use crate::credentials;
use crate::model::{parse_reset, ProviderUsage, UsageWindow};

/// The two procedures, batched, each with no input.
///
/// The `input` parameter is the JSON tRPC wants — `{"0":{"json":null},
/// "1":{"json":null}}` — already percent-encoded, so no URL parser can decide
/// for itself which of those characters are legal in a query.
const ENDPOINT: &str = "https://app.kilo.ai/api/trpc/user.getCreditBlocks,kiloPass.getState?batch=1&input=%7B%220%22%3A%7B%22json%22%3Anull%7D%2C%221%22%3A%7B%22json%22%3Anull%7D%7D";

/// The tiers Kilo sells, by the names its own pricing uses. Any other tier is
/// still a Kilo Pass, and is called only that.
const TIERS: [(&str, &str); 3] = [
    ("tier_19", "Starter"),
    ("tier_49", "Pro"),
    ("tier_199", "Expert"),
];

pub struct KiloCode;

impl Provider for KiloCode {
    fn id(&self) -> &'static str {
        "kilo-code"
    }

    fn name(&self) -> &'static str {
        "Kilo Code"
    }

    /// A key the reader supplied, or the login the Kilo CLI saved. The file
    /// only has to exist to be read here; whether the token in it still works
    /// is what the fetch finds out, and its refusal says which of the two it
    /// was.
    fn is_configured(&self) -> bool {
        super::provider_key(self.id()).is_some() || auth_path().is_some()
    }

    fn fetch(&self, ctx: Arc<Ctx>) -> FetchFuture {
        Box::pin(async move { fetch_inner(ctx).await })
    }
}

/// Where the Kilo CLI keeps its own login, as the original reads it.
fn auth_path() -> Option<PathBuf> {
    credentials::first_existing(&credentials::home_relative(&[".local", "share", "kilo", "auth.json"]))
}

async fn fetch_inner(ctx: Arc<Ctx>) -> ProviderUsage {
    const ID: &str = "kilo-code";
    const NAME: &str = "Kilo Code";

    // A pasted key first; the CLI's login only when there is none.
    let (token, refused) = match super::provider_key(ID) {
        Some(key) => (key, "the API key was refused"),
        None => match auth_path().and_then(|path| credentials::read_json(&path)) {
            Some(json) => match credentials::dig_str(&json, "kilo.access") {
                Some(token) => (token, "the saved Kilo login is stale — sign in with the Kilo CLI again"),
                None => {
                    return ProviderUsage::failed(
                        ID,
                        NAME,
                        "no Kilo login in ~/.local/share/kilo/auth.json",
                    )
                }
            },
            None => {
                return ProviderUsage::failed(
                    ID,
                    NAME,
                    super::missing_key(
                        ID,
                        ", or sign in with the Kilo CLI so ~/.local/share/kilo/auth.json exists",
                    ),
                )
            }
        },
    };

    let response = ctx
        .client
        .get(ENDPOINT)
        .header("Authorization", format!("Bearer {token}"))
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

    if !status.is_success() {
        let hints: &[(u16, &str)] = &[
            (401, refused),
            (403, refused),
            (429, " — rate limited, try again shortly"),
        ];
        return ProviderUsage::failed(ID, NAME, super::http_failure(status, hints));
    }

    let json: Value = match serde_json::from_str(&body) {
        Ok(v) => v,
        Err(e) => return ProviderUsage::failed(ID, NAME, format!("bad JSON: {e}")),
    };

    super::claude_code::debug_dump(ID, &json);

    reading(&json, refused)
}

/// The batched reply, or the reason it is not one.
fn reading(json: &Value, refused: &str) -> ProviderUsage {
    const ID: &str = "kilo-code";
    const NAME: &str = "Kilo Code";

    let Some(entries) = batch(json) else {
        return ProviderUsage::failed(ID, NAME, "unreadable reply");
    };

    // A batch with neither a result nor an error anywhere is not an answer.
    if !entries
        .iter()
        .flatten()
        .any(|entry| entry.get("result").is_some() || entry.get("error").is_some())
    {
        return ProviderUsage::failed(ID, NAME, "unreadable reply");
    }

    // A procedure answering with an error: refused if it says so, and
    // otherwise nothing this app can read. Each carries its own.
    for entry in entries.iter().flatten() {
        let Some(error) = entry.get("error") else {
            continue;
        };
        let text = error.to_string().to_lowercase();
        return if text.contains("unauthorized") || text.contains("forbidden") {
            ProviderUsage::failed(ID, NAME, refused)
        } else {
            ProviderUsage::failed(ID, NAME, "the service answered with an error this app cannot read")
        };
    }

    let blocks = entries.first().copied().flatten().and_then(payload);
    let pass = entries.get(1).copied().flatten().and_then(payload);

    let balance = blocks.and_then(balance);
    let subscription = pass.and_then(|pass| pass.get("subscription"));
    let window = subscription.and_then(window);

    let mut windows: Vec<UsageWindow> = window.into_iter().collect();
    if let Some(balance) = balance {
        windows.push(super::balance_window(
            "Balance",
            format!("{balance:.2} USD"),
        ));
    }

    if windows.is_empty() {
        return ProviderUsage::failed(ID, NAME, "no limits reported");
    }

    // A pass the account has is named by its tier; a tier nobody here knows is
    // still a Kilo Pass and is called only that.
    let plan = subscription.map(|subscription| {
        subscription
            .get("tier")
            .and_then(Value::as_str)
            .and_then(|tier| {
                TIERS
                    .iter()
                    .find(|(code, _)| *code == tier)
                    .map(|(_, name)| (*name).to_string())
            })
            .unwrap_or_else(|| "Kilo Pass".to_string())
    });

    let mut usage = ProviderUsage::ok(ID, NAME, windows).with_plan(plan);
    if let Some(balance) = balance { usage = usage.with_credit_remaining(balance, "USD"); }
    usage
}

/// The batch's replies in procedure order.
///
/// tRPC answers a batch as a JSON array, or as an object keyed by index; both
/// are read, and a procedure with no reply is nothing.
fn batch(json: &Value) -> Option<Vec<Option<&Value>>> {
    match json {
        Value::Array(items) => Some(items.iter().map(Some).collect()),
        Value::Object(map) => Some(
            (0..2)
                .map(|index| map.get(&index.to_string()))
                .collect(),
        ),
        _ => None,
    }
}

/// `result.data`, or `result.data.json` where the router wraps it.
fn payload(entry: &Value) -> Option<&Value> {
    let data = entry.get("result")?.get("data")?;
    match data.get("json") {
        Some(inner) => Some(inner),
        None => Some(data),
    }
}

/// In micro-dollars on the wire. The account's total where Kilo states it;
/// otherwise what is left in each block, added up.
fn balance(blocks: &Value) -> Option<f64> {
    let micro = match super::dig_number(blocks.get("totalBalance_mUsd")) {
        Some(total) => Some(total),
        None => {
            let left: Vec<f64> = blocks
                .get("creditBlocks")
                .and_then(Value::as_array)
                .map(|items| {
                    items
                        .iter()
                        .filter_map(|block| super::dig_number(block.get("balance_mUsd")))
                        .collect()
                })
                .unwrap_or_default();
            if left.is_empty() {
                None
            } else {
                Some(left.iter().sum())
            }
        }
    };

    let micro = micro.filter(|micro| micro.is_finite() && *micro >= 0.0)?;
    Some(micro / 1_000_000.0)
}

/// The pass's period: what has been used of the base credits and the bonus on
/// top, both as Kilo reports them.
///
/// A period with no stated size draws nothing. It resets when the pass bills
/// again; how long that is is not stated, so the thirty days are a sort key
/// only and the heading claims no length.
fn window(subscription: &Value) -> Option<UsageWindow> {
    let used = super::dig_number(subscription.get("currentPeriodUsageUsd"))
        .filter(|used| used.is_finite() && *used >= 0.0)?;
    let base = super::dig_number(subscription.get("currentPeriodBaseCreditsUsd"))
        .filter(|base| base.is_finite() && *base >= 0.0)?;
    let bonus = super::dig_number(subscription.get("currentPeriodBonusCreditsUsd"))
        .filter(|bonus| bonus.is_finite() && *bonus > 0.0)
        .unwrap_or(0.0);

    let size = base + bonus;
    if size <= 0.0 {
        return None;
    }

    Some(
        UsageWindow::new("Credits", Some(percent_from_fraction(used / size)))
            .with_reset(subscription.get("nextBillingAt").and_then(parse_reset)),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// The batched reply, as CodexBar's tests describe it.
    fn fixture() -> Value {
        json!([
            { "result": { "data": { "json": {
                "totalBalance_mUsd": 12_340_000,
                "creditBlocks": [ { "balance_mUsd": 1_000_000 }, { "balance_mUsd": 2_000_000 } ]
            } } } },
            { "result": { "data": { "json": {
                "subscription": {
                    "tier": "tier_49",
                    "currentPeriodUsageUsd": 3.0,
                    "currentPeriodBaseCreditsUsd": 10.0,
                    "currentPeriodBonusCreditsUsd": 5.0,
                    "nextBillingAt": "2026-11-01T00:00:00Z"
                }
            } } } }
        ])
    }

    #[test]
    fn reads_the_pass_and_the_balance() {
        let usage = reading(&fixture(), "the API key was refused");
        assert_eq!(usage.credit_remaining.as_ref().unwrap().amount, 12.34);
        assert_eq!(usage.credit_remaining.as_ref().unwrap().currency, "USD");
        let labels: Vec<&str> = usage.windows.iter().map(|w| w.label.as_str()).collect();

        assert_eq!(labels, vec!["Credits", "Balance"]);
        // 3 used of 10 base and 5 bonus.
        assert_eq!(usage.windows[0].percent_used, Some(20.0));
        assert_eq!(
            usage.windows[0].resets_at.as_deref(),
            Some("2026-11-01T00:00:00Z")
        );
        // Micro-dollars on the wire.
        assert_eq!(usage.windows[1].detail.as_deref(), Some("12.34 USD"));
        assert_eq!(usage.windows[1].percent_used, None);
        assert_eq!(usage.plan.as_deref(), Some("Pro"));
    }

    #[test]
    fn the_balance_falls_back_to_what_is_left_in_each_block() {
        let reply = json!([
            { "result": { "data": {
                "creditBlocks": [ { "balance_mUsd": 1_500_000 }, { "balance_mUsd": 500_000 } ]
            } } },
            {}
        ]);
        let usage = reading(&reply, "refused");
        assert_eq!(usage.windows[0].detail.as_deref(), Some("2.00 USD"));
    }

    #[test]
    fn a_balance_the_service_did_not_state_is_not_zero() {
        // No total and no blocks: nothing to show, not a balance of nothing.
        let reply = json!([
            { "result": { "data": { "json": {} } } },
            { "result": { "data": { "json": { "subscription": {
                "currentPeriodUsageUsd": 1.0, "currentPeriodBaseCreditsUsd": 10.0
            } } } } }
        ]);
        let usage = reading(&reply, "refused");
        assert_eq!(usage.windows.len(), 1);
        assert_eq!(usage.windows[0].label, "Credits");

        // A negative total is not a reading either.
        let negative = json!([
            { "result": { "data": { "json": { "totalBalance_mUsd": -1 } } } },
            {}
        ]);
        assert!(reading(&negative, "refused").error.is_some());
    }

    #[test]
    fn a_period_with_no_stated_size_draws_nothing() {
        let with = |subscription: Value| {
            let reply = json!([
                {},
                { "result": { "data": { "json": { "subscription": subscription } } } }
            ]);
            reading(&reply, "refused")
        };

        // No base at all, and a base of zero with no bonus, are both "no
        // period stated".
        assert!(with(json!({ "currentPeriodUsageUsd": 1.0 })).error.is_some());
        assert!(with(json!({
            "currentPeriodUsageUsd": 1.0, "currentPeriodBaseCreditsUsd": 0.0
        }))
        .error.is_some());

        // A bonus alone is a period: it is credited to the pass.
        let usage = with(json!({
            "currentPeriodUsageUsd": 1.0,
            "currentPeriodBaseCreditsUsd": 0.0,
            "currentPeriodBonusCreditsUsd": 4.0
        }));
        assert_eq!(usage.windows[0].percent_used, Some(25.0));
    }

    #[test]
    fn a_tier_nobody_here_knows_is_still_a_pass() {
        let reply = json!([
            {},
            { "result": { "data": { "json": { "subscription": {
                "tier": "tier_999",
                "currentPeriodUsageUsd": 1.0,
                "currentPeriodBaseCreditsUsd": 10.0
            } } } } }
        ]);
        assert_eq!(reading(&reply, "refused").plan.as_deref(), Some("Kilo Pass"));

        // No subscription at all is no plan, and not a failure.
        let no_pass = json!([
            { "result": { "data": { "json": { "totalBalance_mUsd": 0 } } } },
            {}
        ]);
        let usage = reading(&no_pass, "refused");
        assert!(usage.plan.is_none());
        // Zero is a balance the service stated.
        assert_eq!(usage.windows[0].detail.as_deref(), Some("0.00 USD"));
    }

    #[test]
    fn an_error_from_either_procedure_is_read_in_its_own_words() {
        let unauthorized = json!([
            {},
            { "error": { "json": { "message": "UNAUTHORIZED" } } }
        ]);
        let usage = reading(&unauthorized, "the saved Kilo login is stale");
        assert_eq!(usage.error.as_deref(), Some("the saved Kilo login is stale"));

        let other = json!([
            { "error": { "json": { "message": "rate limit exceeded" } } },
            {}
        ]);
        let usage = reading(&other, "the API key was refused");
        assert!(usage.error.as_deref().unwrap_or("").contains("cannot read"));
    }

    #[test]
    fn the_batch_may_come_keyed_by_index() {
        let keyed = json!({
            "0": { "result": { "data": { "json": { "totalBalance_mUsd": 2_000_000 } } } },
            "1": {}
        });
        let usage = reading(&keyed, "refused");
        assert_eq!(usage.windows.len(), 1);
        assert_eq!(usage.windows[0].detail.as_deref(), Some("2.00 USD"));
    }

    #[test]
    fn a_reply_that_is_not_a_batch_is_unreadable() {
        assert!(reading(&json!("nope"), "refused").error.is_some());
        assert!(reading(&json!([{}, {}]), "refused").error.is_some());
    }

    #[test]
    fn a_figure_may_arrive_as_a_string_and_a_boolean_is_not_one() {
        let reply = json!([
            { "result": { "data": { "json": { "totalBalance_mUsd": "2500000" } } } },
            { "result": { "data": { "json": { "subscription": {
                "currentPeriodUsageUsd": "2.5",
                "currentPeriodBaseCreditsUsd": "10",
                "currentPeriodBonusCreditsUsd": true
            } } } } }
        ]);
        let usage = reading(&reply, "refused");
        assert_eq!(usage.windows[0].percent_used, Some(25.0));
        assert_eq!(usage.windows[1].detail.as_deref(), Some("2.50 USD"));
    }
}
