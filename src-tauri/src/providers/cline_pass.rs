//! ClinePass, Cline's subscription: a five-hour, a weekly and a monthly limit,
//! each reported as a percentage by the service itself.
//!
//! Read with a key the user enters, from the endpoint Cline's own app calls:
//! `GET https://api.cline.bot/api/v1/users/me/plan/usage-limits`. The key is
//! pasted into Settings, which is the whole credential — the original reads the
//! same field and nothing from a browser.
//!
//! Only the three named limits are read. A limit type this build does not know
//! is left off rather than guessed at: its length is stated nowhere, so neither
//! its name nor its window clock could be said honestly.

use std::sync::Arc;

use serde_json::Value;

use super::{
    by_window_length, describe_reqwest_error, percent_from_scale, Ctx, FetchFuture, Provider,
};
use crate::model::{parse_reset, ProviderUsage, UsageWindow};

const ENDPOINT: &str = "https://api.cline.bot/api/v1/users/me/plan/usage-limits";

pub struct ClinePass;

impl Provider for ClinePass {
    fn id(&self) -> &'static str {
        "cline-pass"
    }

    fn name(&self) -> &'static str {
        "ClinePass"
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
    const ID: &str = "cline-pass";
    const NAME: &str = "ClinePass";

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

/// The limits Cline names, with the length each one is.
///
/// `monthly` is a billing month rather than a fixed thirty days, so its length
/// is only a sort key and is not claimed — which is why the third member of the
/// tuple is there at all.
fn shape(kind: &str) -> Option<(&'static str, i64)> {
    match kind {
        "five_hour" => Some(("5h", 5 * 3_600)),
        "weekly" => Some(("7d", 7 * 86_400)),
        "monthly" => Some(("Monthly", 30 * 86_400)),
        _ => None,
    }
}

/// The mapping, kept apart from the request so a fixture can drive it.
fn reading(json: &Value) -> ProviderUsage {
    const ID: &str = "cline-pass";
    const NAME: &str = "ClinePass";

    if json.get("success").and_then(|v| v.as_bool()) != Some(true) {
        return ProviderUsage::failed(ID, NAME, "bad reply: the service did not report success");
    }

    let Some(limits) = json
        .get("data")
        .and_then(|data| data.get("limits"))
        .and_then(|v| v.as_array())
    else {
        return ProviderUsage::failed(ID, NAME, "bad reply: no limits");
    };

    let mut rows: Vec<(i64, UsageWindow)> = Vec::new();
    for limit in limits {
        let Some(kind) = limit.get("type").and_then(|v| v.as_str()) else {
            continue;
        };
        let Some((label, seconds)) = shape(kind) else {
            continue;
        };
        let Some(percent) = limit
            .get("percentUsed")
            .and_then(|v| v.as_f64())
            .filter(|percent| percent.is_finite() && *percent >= 0.0)
        else {
            continue;
        };

        rows.push((
            seconds,
            UsageWindow::new(label, Some(percent_from_scale(percent)))
                .with_reset(limit.get("resetsAt").and_then(parse_reset)),
        ));
    }

    let windows = by_window_length(rows);
    if windows.is_empty() {
        return ProviderUsage::failed(ID, NAME, "no plan limits in the reply");
    }

    ProviderUsage::ok(ID, NAME, windows)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// The reply's shape: a success flag and the named limits beneath it.
    fn fixture() -> Value {
        json!({
            "success": true,
            "data": {
                "limits": [
                    { "type": "weekly", "percentUsed": 12.5, "resetsAt": "2026-10-08T00:00:00Z" },
                    { "type": "five_hour", "percentUsed": 61.0, "resetsAt": "2026-10-02T05:00:00Z" },
                    { "type": "monthly", "percentUsed": 4.0, "resetsAt": "2026-11-01T00:00:00Z" }
                ]
            }
        })
    }

    #[test]
    fn reads_the_three_named_limits_shortest_first() {
        let usage = reading(&fixture());
        let labels: Vec<&str> = usage.windows.iter().map(|w| w.label.as_str()).collect();
        assert_eq!(labels, vec!["5h", "7d", "Monthly"]);
        assert_eq!(usage.windows[0].percent_used, Some(61.0));
        assert_eq!(usage.windows[1].percent_used, Some(12.5));
        assert_eq!(usage.windows[2].percent_used, Some(4.0));
    }

    #[test]
    fn the_resets_are_carried_through() {
        let usage = reading(&fixture());
        assert_eq!(
            usage.windows[0].resets_at.as_deref(),
            Some("2026-10-02T05:00:00Z")
        );
        assert_eq!(
            usage.windows[2].resets_at.as_deref(),
            Some("2026-11-01T00:00:00Z")
        );
    }

    /// A limit type this build does not know has no stated length, so it is
    /// left off rather than named or clocked by guesswork.
    #[test]
    fn an_unknown_limit_type_is_left_off() {
        let reply = json!({
            "success": true,
            "data": { "limits": [
                { "type": "daily", "percentUsed": 90.0 },
                { "type": "five_hour", "percentUsed": 10.0 }
            ] }
        });
        let usage = reading(&reply);
        assert_eq!(usage.windows.len(), 1);
        assert_eq!(usage.windows[0].label, "5h");
    }

    #[test]
    fn a_percentage_past_its_ceiling_clamps_to_a_full_ring() {
        let reply = json!({
            "success": true,
            "data": { "limits": [ { "type": "weekly", "percentUsed": 130.0 } ] }
        });
        assert_eq!(reading(&reply).windows[0].percent_used, Some(100.0));
    }

    #[test]
    fn a_negative_percentage_is_not_a_reading() {
        let reply = json!({
            "success": true,
            "data": { "limits": [ { "type": "weekly", "percentUsed": -1.0 } ] }
        });
        assert!(reading(&reply).error.is_some());
    }

    #[test]
    fn a_reply_that_is_not_successful_or_has_no_limits_is_unreadable() {
        assert!(reading(&json!({ "success": false, "data": { "limits": [] } }))
            .error
            .is_some());
        assert!(reading(&json!({ "data": { "limits": [] } })).error.is_some());
        assert!(reading(&json!({ "success": true })).error.is_some());
        assert!(reading(&json!({})).error.is_some());
    }
}
