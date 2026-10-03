//! ElevenLabs: the subscription's character credits for the current billing
//! period — how many have been used, out of how many, and when they reset.
//!
//! Read with a key the user enters, from ElevenLabs' own subscription route:
//! `GET https://api.elevenlabs.io/v1/user/subscription`, key in `xi-api-key`.
//! The key needs the `user_read` permission.
//!
//! **Credits, not a month.** The period follows the subscription's billing date
//! and the reply states only when it ends, so the allowance claims no length.
//! Voice slots are left off: they are a count of voices kept, not an allowance
//! spent over time, and no kind of window reads as one.

use std::sync::Arc;

use serde_json::Value;

use super::{describe_reqwest_error, percent_from_fraction, Ctx, FetchFuture, Provider};
use crate::model::{parse_reset, ProviderUsage, UsageWindow};

const ENDPOINT: &str = "https://api.elevenlabs.io/v1/user/subscription";

pub struct ElevenLabs;

impl Provider for ElevenLabs {
    fn id(&self) -> &'static str {
        "elevenlabs"
    }

    fn name(&self) -> &'static str {
        "ElevenLabs"
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
    const ID: &str = "elevenlabs";
    const NAME: &str = "ElevenLabs";

    let key = match super::provider_key(ID) {
        Some(key) => key,
        None => return ProviderUsage::failed(ID, NAME, super::missing_key(ID, "")),
    };

    // ElevenLabs takes its key in a header of its own rather than as a bearer.
    let response = ctx
        .client
        .get(ENDPOINT)
        .header("xi-api-key", key)
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
            (401, " — the API key was refused, or lacks the user_read permission"),
            (403, " — the API key was refused, or lacks the user_read permission"),
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
    const ID: &str = "elevenlabs";
    const NAME: &str = "ElevenLabs";

    let used = json
        .get("character_count")
        .and_then(|v| v.as_f64())
        .filter(|used| used.is_finite() && *used >= 0.0);
    // A limit of zero is no allowance at all, and is left off rather than drawn
    // as an empty ring.
    let limit = json
        .get("character_limit")
        .and_then(|v| v.as_f64())
        .filter(|limit| limit.is_finite() && *limit > 0.0);

    let (Some(used), Some(limit)) = (used, limit) else {
        return ProviderUsage::failed(ID, NAME, "no character allowance in response");
    };

    // The reset only counts when it is a real epoch value.
    let resets_at = json
        .get("next_character_count_reset_unix")
        .and_then(|v| v.as_f64())
        .filter(|seconds| *seconds > 0.0)
        .and_then(|seconds| parse_reset(&serde_json::json!(seconds)));

    let window = UsageWindow::new("Credits", Some(percent_from_fraction(used / limit)))
        .with_reset(resets_at)
        .with_detail(Some(format!("{} / {} characters", used.round(), limit.round())));

    ProviderUsage::ok(ID, NAME, vec![window]).with_plan(plan(json.get("tier")))
}

/// "creator" → "Creator", "growing_business" → "Growing Business". The tier is
/// ElevenLabs' own plan name, so it is left untranslated.
fn plan(tier: Option<&Value>) -> Option<String> {
    let tier = tier?.as_str()?.trim();
    if tier.is_empty() {
        return None;
    }

    Some(
        tier.replace('_', " ")
            .split(' ')
            .map(|word| {
                let mut chars = word.chars();
                match chars.next() {
                    Some(first) => first.to_uppercase().collect::<String>() + &chars.as_str().to_lowercase(),
                    None => String::new(),
                }
            })
            .collect::<Vec<_>>()
            .join(" "),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// The reply's shape: characters used, the allowance, and the epoch the
    /// billing period ends at.
    fn fixture() -> Value {
        json!({
            "tier": "creator",
            "character_count": 250_000,
            "character_limit": 1_000_000,
            "next_character_count_reset_unix": 1_790_000_000,
            "voice_limit": 30
        })
    }

    #[test]
    fn reads_the_periods_character_credits() {
        let usage = reading(&fixture());
        assert_eq!(usage.windows[0].label, "Credits");
        assert_eq!(usage.windows[0].percent_used, Some(25.0));
        assert_eq!(
            usage.windows[0].detail.as_deref(),
            Some("250000 / 1000000 characters")
        );
        assert_eq!(usage.plan.as_deref(), Some("Creator"));
    }

    #[test]
    fn the_period_states_only_when_it_ends() {
        let usage = reading(&fixture());
        // 1,790,000,000 is 2026-09-21T14:13:20Z.
        assert_eq!(usage.windows[0].resets_at.as_deref(), Some("2026-09-21T14:13:20Z"));

        let mut reply = fixture();
        reply["next_character_count_reset_unix"] = json!(0);
        assert!(reading(&reply).windows[0].resets_at.is_none());
    }

    #[test]
    fn a_spent_allowance_reads_full() {
        let mut reply = fixture();
        reply["character_count"] = json!(1_000_000);
        assert_eq!(reading(&reply).windows[0].percent_used, Some(100.0));
    }

    #[test]
    fn a_limit_of_zero_is_no_allowance_rather_than_an_empty_ring() {
        let mut reply = fixture();
        reply["character_limit"] = json!(0);
        assert!(reading(&reply).error.is_some());
    }

    #[test]
    fn a_reply_without_the_counts_is_unreadable() {
        assert!(reading(&json!({ "tier": "free" })).error.is_some());
    }

    #[test]
    fn the_tier_is_tidied_into_a_plan_name() {
        assert_eq!(plan(Some(&json!("growing_business"))).as_deref(), Some("Growing Business"));
        assert_eq!(plan(Some(&json!("  "))).as_deref(), None);
        assert_eq!(plan(None), None);
    }
}
