//! Aixy, an AI gateway: the budgets that apply to one key — set on the key, its
//! user, team, project or organization — each as an amount used and a limit in
//! US dollars that the gateway states.
//!
//! Read with the project API key the user enters, from the hosted gateway's
//! `GET https://api.aixy-gateway.com/v1/usage`. The key is pasted into Settings,
//! which is the whole credential.
//!
//! **Only budgets whose balance the gateway knows are drawn.** One marked
//! unavailable has a limit and no figure against it, and is left off rather
//! than drawn at zero. A hard budget's use is what was spent plus what is
//! reserved for requests in flight, because that is what the gateway enforces
//! against; a monitor-only budget's is what was spent.
//!
//! Budgets overlap and are never summed. Where two share a period, the one that
//! binds is drawn: an enforced budget before a monitored one, then the one
//! nearest its limit. Their scope — key, team, project — is not a model name
//! and is not put on the row.
//!
//! Left out on purpose: the last seven days' attributed spend, which Aixy
//! itself says may be estimated or partial and which has no limit beside it.

use std::sync::Arc;

use serde_json::Value;

use super::{by_window_length, describe_reqwest_error, percent_from_fraction, Ctx, FetchFuture, Provider};
use crate::model::{parse_reset, ProviderUsage, UsageWindow};

const ENDPOINT: &str = "https://api.aixy-gateway.com/v1/usage";

/// The periods Aixy names, with the length each is. A month is a calendar month
/// and a lifetime budget never turns over, so both lengths are only sort keys —
/// which is why a lifetime window takes no reset whatever the reply says.
const PERIODS: [(&str, &str, i64); 4] = [
    ("daily", "Daily", 86_400),
    ("weekly", "Weekly", 7 * 86_400),
    ("monthly", "Monthly", 30 * 86_400),
    ("lifetime", "Lifetime", 365 * 86_400),
];

pub struct Aixy;

impl Provider for Aixy {
    fn id(&self) -> &'static str {
        "aixy"
    }

    fn name(&self) -> &'static str {
        "Aixy"
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
    const ID: &str = "aixy";
    const NAME: &str = "Aixy";

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

/// An amount, written as a number or as a decimal string.
fn amount(value: Option<&Value>) -> Option<f64> {
    match value? {
        Value::String(text) => text
            .trim()
            .parse::<f64>()
            .ok()
            .filter(|value| value.is_finite()),
        other => other.as_f64().filter(|value| value.is_finite()),
    }
}

fn text<'a>(value: &'a Value, key: &str) -> Option<&'a str> {
    value.get(key).and_then(Value::as_str)
}

/// One budget, if it is this key's, has a known balance and a limit.
fn budget(budget: &Value, key_id: &str, project_id: Option<&str>) -> Option<(i64, UsageWindow, bool)> {
    let (period, label, seconds) = {
        let interval = text(budget, "interval")?;
        PERIODS
            .iter()
            .find(|(name, _, _)| *name == interval)
            .map(|(name, label, seconds)| (*name, *label, *seconds))?
    };

    let applies_to = budget.get("appliesTo").and_then(|v| v.as_array())?;
    if applies_to.is_empty() {
        return None;
    }
    // Every target has to be this key's, or the budget is somebody else's.
    let mine = applies_to.iter().all(|target| {
        text(target, "apiKeyId") == Some(key_id)
            && text(target, "projectId") == project_id
    });
    if !mine {
        return None;
    }

    let limit = amount(budget.get("limitUsd")).filter(|limit| *limit > 0.0)?;

    let hard = match text(budget, "enforcement") {
        Some("hard") => true,
        Some("monitor") => false,
        _ => return None,
    };

    let used = if hard {
        let availability = budget.get("availability")?;
        if text(availability, "status") != Some("available") {
            return None;
        }
        let spent = amount(availability.get("spentUsd"))?;
        let reserved = amount(availability.get("reservedUsd"))?;
        spent + reserved
    } else {
        if text(budget, "spendStatus") != Some("available") {
            return None;
        }
        amount(budget.get("spendUsd"))?
    };

    if !used.is_finite() || used < 0.0 {
        return None;
    }

    let fraction = used / limit;
    let window = UsageWindow::new(label, Some(percent_from_fraction(fraction)))
        .with_detail(Some(format!("{used:.2} / {limit:.2} USD")))
        // A lifetime budget has no reset, whatever the reply says.
        .with_reset(if period == "lifetime" {
            None
        } else {
            budget.get("resetsAt").and_then(parse_reset)
        });

    Some((seconds, window, hard))
}

/// The mapping, kept apart from the request so a fixture can drive it.
fn reading(json: &Value) -> ProviderUsage {
    const ID: &str = "aixy";
    const NAME: &str = "Aixy";

    if text(json, "object") != Some("key.usage") || text(json, "currency") != Some("USD") {
        return ProviderUsage::failed(ID, NAME, "bad reply: not a key usage in USD");
    }

    let Some(key_id) = json.get("key").and_then(|key| text(key, "id")) else {
        return ProviderUsage::failed(ID, NAME, "bad reply: no key id");
    };
    let project_id = json.get("key").and_then(|key| text(key, "projectId"));

    let Some(budgets) = json.get("budgets").and_then(|v| v.as_array()) else {
        return ProviderUsage::failed(ID, NAME, "bad reply: no budgets");
    };

    // One per period: the one that binds. An enforced budget before a monitored
    // one, then the one nearest its limit.
    let mut binding: Vec<(String, i64, bool, f64, UsageWindow)> = Vec::new();
    for entry in budgets {
        let Some((seconds, window, hard)) = budget(entry, key_id, project_id) else {
            continue;
        };
        let Some(period) = text(entry, "interval").map(str::to_string) else {
            continue;
        };
        let fraction = window.percent_used.unwrap_or(0.0);

        match binding.iter_mut().find(|(name, _, _, _, _)| *name == period) {
            Some(existing) => {
                // Enforced before monitored, then nearest its limit.
                let better = match (hard, existing.2) {
                    (true, false) => true,
                    (false, true) => false,
                    _ => fraction > existing.3,
                };
                if better {
                    existing.2 = hard;
                    existing.3 = fraction;
                    existing.4 = window;
                }
            }
            None => binding.push((period, seconds, hard, fraction, window)),
        }
    }

    let rows: Vec<(i64, UsageWindow)> = binding
        .into_iter()
        .map(|(_, seconds, _, _, window)| (seconds, window))
        .collect();

    let windows = by_window_length(rows);
    if windows.is_empty() {
        return ProviderUsage::failed(ID, NAME, "no budget windows in the reply");
    }

    ProviderUsage::ok(ID, NAME, windows)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn budget(id: &str, interval: &str, enforcement: &str, limit: &str) -> Value {
        let mut budget = json!({
            "id": id,
            "interval": interval,
            "enforcement": enforcement,
            "limitUsd": limit,
            "appliesTo": [ { "apiKeyId": "key_1", "projectId": "proj_1" } ],
            "resetsAt": "2026-10-03T00:00:00Z"
        });
        if enforcement == "hard" {
            budget["availability"] =
                json!({ "status": "available", "spentUsd": "10.00", "reservedUsd": "2.50" });
        } else {
            budget["spendStatus"] = json!("available");
            budget["spendUsd"] = json!("40.00");
        }
        budget
    }

    fn fixture() -> Value {
        json!({
            "object": "key.usage",
            "currency": "USD",
            "key": { "id": "key_1", "projectId": "proj_1" },
            "budgets": [
                budget("b1", "daily", "hard", "50.00"),
                budget("b2", "weekly", "monitor", "100.00"),
                budget("b3", "monthly", "hard", "200.00")
            ]
        })
    }

    #[test]
    fn reads_one_window_per_period_shortest_first() {
        let usage = reading(&fixture());
        let labels: Vec<&str> = usage.windows.iter().map(|w| w.label.as_str()).collect();
        assert_eq!(labels, vec!["Daily", "Weekly", "Monthly"]);
        // A hard budget's use is what was spent plus what is reserved.
        assert_eq!(usage.windows[0].percent_used, Some(25.0));
        // A monitor budget's is what was spent.
        assert_eq!(usage.windows[1].percent_used, Some(40.0));
        assert_eq!(usage.windows[2].percent_used, Some(6.25));
    }

    #[test]
    fn the_daily_and_weekly_resets_are_carried_through() {
        let usage = reading(&fixture());
        assert_eq!(
            usage.windows[0].resets_at.as_deref(),
            Some("2026-10-03T00:00:00Z")
        );
    }

    /// A lifetime budget never turns over, so it takes no reset whatever the
    /// reply says.
    #[test]
    fn a_lifetime_budget_has_no_reset() {
        let reply = json!({
            "object": "key.usage", "currency": "USD",
            "key": { "id": "key_1", "projectId": "proj_1" },
            "budgets": [ budget("b4", "lifetime", "hard", "1000.00") ]
        });
        let usage = reading(&reply);
        assert_eq!(usage.windows[0].label, "Lifetime");
        assert!(usage.windows[0].resets_at.is_none());
    }

    /// Where two budgets share a period, the enforced one is drawn even when a
    /// monitored one sits nearer its limit.
    #[test]
    fn an_enforced_budget_wins_over_a_monitored_one() {
        let reply = json!({
            "object": "key.usage", "currency": "USD",
            "key": { "id": "key_1", "projectId": "proj_1" },
            "budgets": [
                budget("hard", "weekly", "hard", "100.00"),
                budget("monitor", "weekly", "monitor", "50.00")
            ]
        });
        let usage = reading(&reply);
        assert_eq!(usage.windows.len(), 1);
        // The hard budget: 10 + 2.50 of 100, not the monitor's 40 of 50.
        assert_eq!(usage.windows[0].percent_used, Some(12.5));
    }

    #[test]
    fn the_nearest_its_limit_wins_between_two_of_a_kind() {
        let mut looser = budget("looser", "weekly", "hard", "100.00");
        looser["availability"] = json!({ "status": "available", "spentUsd": "1.00", "reservedUsd": "0" });
        let mut tighter = budget("tighter", "weekly", "hard", "100.00");
        tighter["availability"] = json!({ "status": "available", "spentUsd": "90.00", "reservedUsd": "0" });

        let reply = json!({
            "object": "key.usage", "currency": "USD",
            "key": { "id": "key_1", "projectId": "proj_1" },
            "budgets": [ looser, tighter ]
        });
        assert_eq!(reading(&reply).windows[0].percent_used, Some(90.0));
    }

    /// A budget whose balance the gateway does not know is left off rather than
    /// drawn at zero, and a budget for another key is not this key's.
    #[test]
    fn unavailable_and_foreign_budgets_are_left_off() {
        let mut unavailable = budget("u", "daily", "hard", "50.00");
        unavailable["availability"] = json!({ "status": "unavailable", "limitUsd": "50.00" });

        let mut foreign = budget("f", "weekly", "hard", "50.00");
        foreign["appliesTo"] = json!([ { "apiKeyId": "other", "projectId": "proj_1" } ]);

        let reply = json!({
            "object": "key.usage", "currency": "USD",
            "key": { "id": "key_1", "projectId": "proj_1" },
            "budgets": [ unavailable, foreign ]
        });
        assert!(reading(&reply).error.is_some());
    }

    #[test]
    fn a_reply_this_build_cannot_read_is_unreadable() {
        assert!(reading(&json!({})).error.is_some());
        // Not a key usage, or not in dollars.
        assert!(reading(&json!({ "object": "org.usage", "currency": "USD",
                                 "key": { "id": "k" }, "budgets": [] }))
        .error
        .is_some());
        assert!(reading(&json!({ "object": "key.usage", "currency": "EUR",
                                 "key": { "id": "k" }, "budgets": [] }))
        .error
        .is_some());
        assert!(reading(&json!({ "object": "key.usage", "currency": "USD" }))
            .error
            .is_some());
        // A limit of zero is not a denominator.
        let mut zero = budget("z", "daily", "hard", "0");
        zero["availability"] = json!({ "status": "available", "spentUsd": "0", "reservedUsd": "0" });
        assert!(reading(&json!({ "object": "key.usage", "currency": "USD",
                                 "key": { "id": "key_1", "projectId": "proj_1" },
                                 "budgets": [zero] }))
        .error
        .is_some());
    }

    #[test]
    fn a_budget_past_its_limit_clamps_to_a_full_ring() {
        let mut over = budget("over", "daily", "monitor", "10.00");
        over["spendUsd"] = json!("25.00");
        let reply = json!({
            "object": "key.usage", "currency": "USD",
            "key": { "id": "key_1", "projectId": "proj_1" },
            "budgets": [ over ]
        });
        assert_eq!(reading(&reply).windows[0].percent_used, Some(100.0));
    }

    #[test]
    fn an_amount_written_as_a_string_is_a_figure() {
        assert_eq!(amount(Some(&json!("12.50"))), Some(12.5));
        assert_eq!(amount(Some(&json!(12.5))), Some(12.5));
        assert_eq!(amount(Some(&json!("a lot"))), None);
        assert_eq!(amount(Some(&json!(true))), None);
        assert_eq!(amount(None), None);
    }
}
