//! DevPass, LLM Gateway's subscription: the billing cycle's plan credits, the
//! premium models' weekly allowance, and — where the key has one — the key's
//! own spending limit. Every allowance and every amount used is stated by the
//! service, in dollars of credit.
//!
//! Read with a regular LLM Gateway API key the user enters, from
//! `GET https://api.llmgateway.io/v1/key`.
//!
//! The cycle's end is not in the reply, so that allowance has no reset and no
//! length; neither is inferred. An allowance of zero is no allowance and is
//! left off rather than drawn full or empty.

use std::sync::Arc;

use serde_json::Value;

use super::{by_window_length, describe_reqwest_error, percent_from_fraction, Ctx, FetchFuture, Provider};
use crate::model::{parse_reset, ProviderUsage, UsageWindow};

const ENDPOINT: &str = "https://api.llmgateway.io/v1/key";

/// The plans the service names. Anything else is a reply this build cannot
/// read, which is said rather than guessed at.
const PLANS: [&str; 4] = ["none", "lite", "pro", "max"];

pub struct DevPass;

impl Provider for DevPass {
    fn id(&self) -> &'static str {
        "dev-pass"
    }

    fn name(&self) -> &'static str {
        "DevPass"
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
    const ID: &str = "dev-pass";
    const NAME: &str = "DevPass";

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

/// Amounts arrive as decimal strings, `"212.00"`. A missing, unreadable or
/// negative one is nothing.
fn amount(value: Option<&Value>) -> Option<f64> {
    let text = value?.as_str()?;
    let parsed = text.trim().parse::<f64>().ok()?;
    (parsed.is_finite() && parsed >= 0.0).then_some(parsed)
}

/// A window from an amount used and an allowance, both as DevPass wrote them.
/// `None` when either is missing or unreadable, or the allowance is zero.
fn window(
    label: &str,
    seconds: i64,
    used: Option<&Value>,
    limit: Option<&Value>,
    resets_at: Option<String>,
) -> Option<(i64, UsageWindow)> {
    let used = amount(used)?;
    let limit = amount(limit).filter(|limit| *limit > 0.0)?;
    Some((
        seconds,
        UsageWindow::new(label, Some(percent_from_fraction(used / limit))).with_reset(resets_at),
    ))
}

/// The mapping, kept apart from the request so a fixture can drive it.
fn reading(json: &Value) -> ProviderUsage {
    const ID: &str = "dev-pass";
    const NAME: &str = "DevPass";

    let Some(key) = json.get("data") else {
        return ProviderUsage::failed(ID, NAME, "bad reply: no key object");
    };
    let Some(plan) = key
        .get("devPlan")
        .and_then(|v| v.as_str())
        .filter(|plan| PLANS.contains(plan))
    else {
        return ProviderUsage::failed(ID, NAME, "bad reply: no plan this build knows");
    };

    let mut rows: Vec<(i64, UsageWindow)> = Vec::new();

    if plan != "none" {
        // Seven days from the first premium request — a stated length.
        if let Some(row) = window(
            "7d",
            7 * 86_400,
            key.get("devPlanPremiumCreditsUsed"),
            key.get("devPlanPremiumWeeklyLimit"),
            key.get("devPlanPremiumWeekResetsAt").and_then(parse_reset),
        ) {
            rows.push(row);
        }
        // The billing cycle's credits. No end is reported and no length is
        // claimed; thirty days only sorts it after the week.
        if let Some(row) = window(
            "Credits",
            30 * 86_400,
            key.get("devPlanCreditsUsed"),
            key.get("devPlanCreditsLimit"),
            None,
        ) {
            rows.push(row);
        }
    }

    // The key's own spending limit, against everything it has ever spent.
    // Never resets, so it has no clock and sorts last.
    if let Some(row) = window(
        "Spend",
        365 * 86_400,
        key.get("usage"),
        key.get("limit"),
        None,
    ) {
        rows.push(row);
    }

    let windows = by_window_length(rows);
    if windows.is_empty() {
        return ProviderUsage::failed(
            ID,
            NAME,
            if plan == "none" {
                "this key is pay as you go and has no spending limit"
            } else {
                "no allowance in the reply"
            },
        );
    }

    // "none" is no plan worth naming, so the card carries none.
    let plan = (plan != "none").then(|| title(plan));
    ProviderUsage::ok(ID, NAME, windows).with_plan(plan)
}

/// `lite` → `Lite`, as the original writes it.
fn title(plan: &str) -> String {
    let mut chars = plan.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
        None => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn fixture() -> Value {
        json!({
            "data": {
                "usage": "212.00",
                "limit": "1000.00",
                "devPlan": "pro",
                "devPlanCreditsUsed": "40.00",
                "devPlanCreditsLimit": "200.00",
                "devPlanPremiumCreditsUsed": "15.00",
                "devPlanPremiumWeeklyLimit": "60.00",
                "devPlanPremiumWeekResetsAt": "2026-10-06T12:00:00Z"
            }
        })
    }

    #[test]
    fn reads_the_three_allowances_shortest_first() {
        let usage = reading(&fixture());
        let labels: Vec<&str> = usage.windows.iter().map(|w| w.label.as_str()).collect();
        assert_eq!(labels, vec!["7d", "Credits", "Spend"]);
        assert_eq!(usage.windows[0].percent_used, Some(25.0));
        assert_eq!(usage.windows[1].percent_used, Some(20.0));
        assert_eq!(usage.windows[2].percent_used, Some(21.2));
        assert_eq!(usage.plan.as_deref(), Some("Pro"));
    }

    /// The week is a stated length and carries its reset; the cycle is not and
    /// carries none; the key's limit never turns over.
    #[test]
    fn only_the_week_has_a_reset() {
        let usage = reading(&fixture());
        assert_eq!(
            usage.windows[0].resets_at.as_deref(),
            Some("2026-10-06T12:00:00Z")
        );
        assert!(usage.windows[1].resets_at.is_none());
        assert!(usage.windows[2].resets_at.is_none());
    }

    #[test]
    fn a_pay_as_you_go_key_reports_only_its_own_limit() {
        let reply = json!({
            "data": { "devPlan": "none", "usage": "5.00", "limit": "50.00" }
        });
        let usage = reading(&reply);
        assert_eq!(usage.windows.len(), 1);
        assert_eq!(usage.windows[0].label, "Spend");
        assert_eq!(usage.windows[0].percent_used, Some(10.0));
        assert!(usage.plan.is_none());
    }

    #[test]
    fn a_pay_as_you_go_key_with_no_limit_has_no_reading() {
        let reply = json!({ "data": { "devPlan": "none", "usage": "5.00" } });
        let usage = reading(&reply);
        assert!(usage.error.is_some());
        assert!(usage
            .error
            .as_deref()
            .unwrap()
            .contains("pay as you go"));
    }

    /// An allowance of zero is no allowance — never a ring drawn full or empty.
    #[test]
    fn a_zero_allowance_is_left_off() {
        let reply = json!({
            "data": { "devPlan": "lite", "devPlanPremiumCreditsUsed": "0",
                      "devPlanPremiumWeeklyLimit": "0", "usage": "1.00", "limit": "10.00" }
        });
        let usage = reading(&reply);
        assert_eq!(usage.windows.len(), 1);
        assert_eq!(usage.windows[0].label, "Spend");
    }

    #[test]
    fn a_reply_this_build_cannot_read_is_unreadable() {
        assert!(reading(&json!({})).error.is_some());
        // A plan this build does not know is not guessed at.
        assert!(reading(&json!({ "data": { "devPlan": "enterprise" } }))
            .error
            .is_some());
        // A negative amount is not a figure.
        assert!(reading(&json!({
            "data": { "devPlan": "pro", "devPlanCreditsUsed": "-1", "devPlanCreditsLimit": "10" }
        }))
        .error
        .is_some());
        // A number where a decimal string belongs is not read either.
        assert!(reading(&json!({
            "data": { "devPlan": "pro", "devPlanCreditsUsed": 1, "devPlanCreditsLimit": "10" }
        }))
        .error
        .is_some());
    }

    #[test]
    fn a_spend_past_its_limit_clamps_to_a_full_ring() {
        let reply = json!({
            "data": { "devPlan": "none", "usage": "80.00", "limit": "50.00" }
        });
        assert_eq!(reading(&reply).windows[0].percent_used, Some(100.0));
    }
}
