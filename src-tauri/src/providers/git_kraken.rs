//! GitKraken AI: the account's AI credits used out of its allowance, and the
//! organization's shared pool beside it where there is one.
//!
//! Read with an access token the user copies by hand from gitkraken.dev —
//! GitKraken issues no API key for this — from the route its own usage page and
//! GitLens call: `GET https://api.gitkraken.dev/v1/ai-tasks/usage`. How the
//! token is obtained is the reader's business: a browser's developer tools is
//! where it is copied from, and what is pasted is the same string the original's
//! own field holds.
//!
//! **Credits, not a week.** The reference implementation calls the allowance
//! weekly; the reply states only `resetsOn`, so no length is claimed. A limit
//! of `-1` (unlimited) or `0` (no allowance) is a statement with no fraction in
//! it, and draws nothing. The organization picker the reference offers
//! (`gk-org-id`) has no setting here, so the token's default organization is
//! the one read.

use std::sync::Arc;

use serde_json::Value;

use super::{describe_reqwest_error, percent_from_fraction, Ctx, FetchFuture, Provider};
use crate::model::{parse_reset, ProviderUsage, UsageWindow};

const ENDPOINT: &str = "https://api.gitkraken.dev/v1/ai-tasks/usage";

pub struct GitKraken;

impl Provider for GitKraken {
    fn id(&self) -> &'static str {
        "gitkraken"
    }

    fn name(&self) -> &'static str {
        "GitKraken AI"
    }

    /// The token the fetch reads, and nothing else.
    fn is_configured(&self) -> bool {
        super::provider_key(self.id()).is_some()
    }

    fn fetch(&self, ctx: Arc<Ctx>) -> FetchFuture {
        Box::pin(async move { fetch_inner(ctx).await })
    }
}

async fn fetch_inner(ctx: Arc<Ctx>) -> ProviderUsage {
    const ID: &str = "gitkraken";
    const NAME: &str = "GitKraken AI";

    let pasted = match super::provider_key(ID) {
        Some(key) => key,
        None => return ProviderUsage::failed(ID, NAME, super::missing_key(ID, "")),
    };

    // People copy the whole header value, "Bearer" and all.
    let Some(token) = token(&pasted) else {
        return ProviderUsage::failed(
            ID,
            NAME,
            super::missing_key(ID, " — the pasted value was not a single bearer token"),
        );
    };

    let response = ctx
        .client
        .get(ENDPOINT)
        .header("Authorization", format!("Bearer {token}"))
        .header("Accept", "application/json")
        // GitKraken's API asks every caller to name itself.
        .header("Client-Name", "Pulse")
        .header("Client-Version", env!("CARGO_PKG_VERSION"))
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
            (401, " — the access token was refused"),
            (403, " — the access token was refused"),
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

/// The token alone, without a leading "Bearer", or `None` if nothing usable is
/// left. Anything but a single word after the scheme is not a token.
fn token(pasted: &str) -> Option<String> {
    let mut words = pasted.split_whitespace();
    let first = words.next()?;
    let rest: Vec<&str> = if first.eq_ignore_ascii_case("bearer") {
        words.collect()
    } else {
        std::iter::once(first).chain(words).collect()
    };
    match rest.as_slice() {
        [only] => Some((*only).to_string()),
        _ => None,
    }
}

/// Used out of a positive limit. `-1` is "unlimited" and `0` is "no
/// allowance": neither is a denominator.
fn fraction(used: Option<f64>, limit: Option<f64>) -> Option<f64> {
    let used = used.filter(|used| used.is_finite() && *used >= 0.0)?;
    let limit = limit.filter(|limit| limit.is_finite() && *limit > 0.0)?;
    Some(used / limit)
}

/// The mapping, kept apart from the request so a fixture can drive it.
fn reading(json: &Value) -> ProviderUsage {
    const ID: &str = "gitkraken";
    const NAME: &str = "GitKraken AI";

    let Some(payload) = json.get("data") else {
        return ProviderUsage::failed(ID, NAME, "bad reply: no data object");
    };
    if payload.get("used").is_none() {
        return ProviderUsage::failed(ID, NAME, "bad reply: no figure used");
    }

    let resets_at = payload.get("resetsOn").and_then(parse_reset);

    let mut windows: Vec<UsageWindow> = Vec::new();
    if let Some(used) = fraction(
        payload.get("used").and_then(Value::as_f64),
        payload.get("limit").and_then(Value::as_f64),
    ) {
        windows.push(window("Credits", used, resets_at.clone()));
    }

    let organization = payload.get("organization");
    if let Some(used) = fraction(
        organization.and_then(|o| o.get("used")).and_then(Value::as_f64),
        organization
            .and_then(|o| o.get("limit"))
            .and_then(Value::as_f64),
    ) {
        windows.push(window("Shared Credits", used, resets_at));
    }

    if windows.is_empty() {
        return ProviderUsage::failed(ID, NAME, "no credit allowance in the reply");
    }

    ProviderUsage::ok(ID, NAME, windows)
}

/// Both windows share one clock: the reply states a reset and nothing about the
/// period, so seven days is a sort key and not a claim.
fn window(label: &str, used: f64, resets_at: Option<String>) -> UsageWindow {
    UsageWindow::new(label, Some(percent_from_fraction(used))).with_reset(resets_at)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn fixture() -> Value {
        json!({
            "data": {
                "used": 120,
                "limit": 400,
                "resetsOn": "2026-10-09T00:00:00Z",
                "organization": { "used": 30, "limit": 100 }
            }
        })
    }

    #[test]
    fn reads_the_personal_and_organization_pools() {
        let usage = reading(&fixture());
        let labels: Vec<&str> = usage.windows.iter().map(|w| w.label.as_str()).collect();
        assert_eq!(labels, vec!["Credits", "Shared Credits"]);
        assert_eq!(usage.windows[0].percent_used, Some(30.0));
        assert_eq!(usage.windows[1].percent_used, Some(30.0));
        assert_eq!(
            usage.windows[1].resets_at.as_deref(),
            Some("2026-10-09T00:00:00Z")
        );
    }

    #[test]
    fn a_pool_past_its_limit_clamps_to_a_full_ring() {
        let reply = json!({ "data": { "used": 500, "limit": 400 } });
        assert_eq!(reading(&reply).windows[0].percent_used, Some(100.0));
    }

    /// `-1` says unlimited and `0` says no allowance: neither is a denominator,
    /// so neither pool is drawn rather than drawn full or empty.
    #[test]
    fn an_unlimited_or_empty_limit_is_not_a_denominator() {
        let unlimited = json!({ "data": { "used": 10, "limit": -1 } });
        assert!(reading(&unlimited).error.is_some());

        let empty = json!({ "data": { "used": 10, "limit": 0 } });
        assert!(reading(&empty).error.is_some());

        // …and one pool missing its limit does not take the other down.
        let mixed = json!({
            "data": { "used": 10, "limit": 40, "organization": { "used": 1, "limit": -1 } }
        });
        let usage = reading(&mixed);
        assert_eq!(usage.windows.len(), 1);
        assert_eq!(usage.windows[0].label, "Credits");
    }

    #[test]
    fn a_reply_without_the_figures_is_unreadable() {
        assert!(reading(&json!({})).error.is_some());
        assert!(reading(&json!({ "data": { "limit": 10 } })).error.is_some());
        assert!(reading(&json!({ "data": { "used": -1, "limit": 10 } }))
            .error
            .is_some());
    }

    #[test]
    fn the_pasted_value_may_carry_the_scheme() {
        assert_eq!(token("Bearer abc123").as_deref(), Some("abc123"));
        assert_eq!(token("bearer abc123").as_deref(), Some("abc123"));
        assert_eq!(token("abc123").as_deref(), Some("abc123"));
        assert_eq!(token("  abc123  ").as_deref(), Some("abc123"));
        // Two words after the scheme are not one token.
        assert_eq!(token("Bearer abc 123"), None);
        assert_eq!(token(""), None);
    }
}
