//! LLM API Key Proxy, a self-hosted gateway: the quota groups it reports for
//! the upstream accounts behind it, each as a percentage left and a reset.
//!
//! Read with the key the reader supplies and the address they name, from the
//! proxy's own statistics route: `GET <address>/v1/quota-stats`, bearer key.
//! The key goes to that address and nowhere else.
//!
//! **A key and an address are both required, and this port has no settings
//! window to type them into.** They arrive from `PULSEWIN_LLM_PROXY_KEY` and
//! `PULSEWIN_LLM_PROXY_BASE_URL`, or from one
//! `%APPDATA%\PulseWin\llm-proxy.json` holding `apiKey` and `baseUrl`; the
//! address may be the service root or the `/v1` base a client is configured
//! with, and the route sits beside either. `is_configured` asks for both,
//! because a key with nowhere to send it is not a credential — and it asks
//! only from disk, never over the network, so an unused gateway costs nothing.
//! The address itself is then checked before the key is attached to it: see
//! `super::gateway_url`.
//!
//! **Each group is its own row.** The reference folds every group into one
//! figure — the lowest remainder anywhere — which is a number no upstream
//! account reported. Here each group keeps its own, scoped by the upstream's
//! name and the group's. None states its length, so each is an allowance with a
//! reset and no claimed period, and its seconds are a sort key and nothing
//! else.
//!
//! Request counts, token counts and the approximate cost are spend with no
//! limit behind them, which this app has nowhere to show, and are left off.

use std::sync::Arc;

use serde_json::Value;

use super::{by_window_length, percent_from_fraction, Ctx, FetchFuture, Provider};
use crate::model::{parse_reset, ProviderUsage, UsageWindow};

const QUOTA_PATH: &str = "/v1/quota-stats";

/// Every group's sort key. All the same, because none of them states a
/// period — the number exists so the rows have an order at all.
const SORT_ONLY_SECONDS: i64 = 86_400;

pub struct LlmProxy;

impl Provider for LlmProxy {
    fn id(&self) -> &'static str {
        "llm-proxy"
    }

    fn name(&self) -> &'static str {
        "LLM API Key Proxy"
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
    const ID: &str = "llm-proxy";
    const NAME: &str = "LLM API Key Proxy";

    let Some(key) = super::provider_key(ID) else {
        return ProviderUsage::failed(ID, NAME, super::missing_key(ID, ""));
    };
    let Some(address) = super::provider_base_url(ID) else {
        return ProviderUsage::failed(ID, NAME, super::missing_address(ID));
    };
    let Some(url) = super::gateway_url(&address, QUOTA_PATH, &["/v1"]) else {
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

    // A redirect is one of the ways a credential is turned away, not a route to
    // follow; the rest are the ordinary refusals.
    if !status.is_success() {
        if status.is_redirection() {
            return ProviderUsage::failed(ID, NAME, "the proxy redirected the request — the key was not accepted");
        }
        let hints: &[(u16, &str)] = &[
            (401, " — the proxy refused the key"),
            (403, " — the proxy refused the key"),
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

/// The reply's groups, upstream by upstream.
///
/// `quota_groups` arrives keyed by the group's name *or* as a plain list, so
/// both are read — and a `quota_groups` that is neither is left out rather than
/// failing the whole reply, which is what the reference does. Upstreams and
/// named groups are walked in name order, because a dictionary has no order of
/// its own and the rail would otherwise reshuffle between refreshes.
fn reading(json: &Value) -> ProviderUsage {
    const ID: &str = "llm-proxy";
    const NAME: &str = "LLM API Key Proxy";

    let Some(providers) = json.get("providers").and_then(|v| v.as_object()) else {
        return ProviderUsage::failed(ID, NAME, "no upstreams in response");
    };

    let mut upstreams: Vec<(&String, &Value)> = providers.iter().collect();
    upstreams.sort_by(|a, b| a.0.cmp(b.0));

    let mut rows: Vec<(i64, UsageWindow)> = Vec::new();

    for (upstream, stats) in upstreams {
        for (name, group) in groups_of(stats) {
            // A remainder outside its own scale is not a reading.
            let Some(left) = group
                .get("remaining_percent")
                .and_then(Value::as_f64)
                .filter(|left| left.is_finite() && (0.0..=100.0).contains(left))
            else {
                continue;
            };

            // The upstream and group names are the proxy's own identifiers —
            // "gemini_cli", "claude-sonnet" — so they are never translated.
            // "default" is the proxy saying it has no name for the group, so it
            // is left off rather than printed.
            let scope = match name.filter(|name| name != "default") {
                Some(name) => format!("{upstream} · {name}"),
                None => upstream.clone(),
            };

            let window = UsageWindow::new(
                format!("Credits {scope}"),
                Some(percent_from_fraction((100.0 - left) / 100.0)),
            )
            .with_reset(
                group
                    .get("reset_time")
                    .and_then(parse_reset),
            );

            rows.push((SORT_ONLY_SECONDS, window));
        }
    }

    if rows.is_empty() {
        return ProviderUsage::failed(ID, NAME, "no limits reported");
    }

    ProviderUsage::ok(ID, NAME, by_window_length(rows))
}

/// The groups under one upstream, as `(name, group)` pairs in the order they
/// are drawn: named groups sorted by name, an unnamed list in its own order.
fn groups_of(stats: &Value) -> Vec<(Option<String>, &Value)> {
    match stats.get("quota_groups") {
        Some(Value::Object(named)) => {
            let mut entries: Vec<(&String, &Value)> = named.iter().collect();
            entries.sort_by(|a, b| a.0.cmp(b.0));
            entries
                .into_iter()
                .map(|(name, group)| (Some(name.clone()), group))
                .collect()
        }
        Some(Value::Array(list)) => list.iter().map(|group| (None, group)).collect(),
        // Absent, or a shape the proxy does not send: nothing from this
        // upstream, rather than nothing from the reply.
        _ => Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// The reply's shape, as CodexBar's plugin and its tests describe it.
    fn fixture() -> Value {
        json!({
            "providers": {
                "gemini_cli": {
                    "quota_groups": {
                        "default": { "remaining_percent": 80.0, "reset_time": "2026-10-02T00:00:00Z" },
                        "claude-sonnet": { "remaining_percent": 25.5 }
                    }
                },
                "openai": {
                    "quota_groups": [
                        { "remaining_percent": 100.0 },
                        { "remaining_percent": 0.0, "reset_time": "2026-10-03T00:00:00Z" }
                    ]
                }
            }
        })
    }

    #[test]
    fn each_group_is_its_own_row_counted_from_what_is_left() {
        let usage = reading(&fixture());
        let labels: Vec<&str> = usage.windows.iter().map(|w| w.label.as_str()).collect();

        // Named groups sorted, then the unnamed list in its own order; the
        // "default" group is the upstream with no group name on it.
        assert_eq!(
            labels,
            vec![
                "Credits gemini_cli · claude-sonnet",
                "Credits gemini_cli",
                "Credits openai",
                "Credits openai",
            ]
        );

        // A remainder is turned into what has gone: 80% left is 20% used.
        assert_eq!(usage.windows[0].percent_used, Some(74.5));
        assert_eq!(usage.windows[1].percent_used, Some(20.0));
        assert_eq!(usage.windows[2].percent_used, Some(0.0));
        assert_eq!(usage.windows[3].percent_used, Some(100.0));
    }

    #[test]
    fn a_reset_comes_across_and_no_length_is_claimed() {
        let usage = reading(&fixture());
        // The named group sorts ahead of the default one, and only the default
        // group states a reset.
        assert!(usage.windows[0].resets_at.is_none());
        assert_eq!(
            usage.windows[1].resets_at.as_deref(),
            Some("2026-10-02T00:00:00Z")
        );
        assert_eq!(
            usage.windows[3].resets_at.as_deref(),
            Some("2026-10-03T00:00:00Z")
        );
        // None of the groups states a period, so no heading claims one: every
        // row is the same kind of allowance, whatever its reset is.
        assert!(usage.windows.iter().all(|w| w.label.starts_with("Credits ")));
        assert!(usage.windows.iter().all(|w| !w.label.contains("limit")));
    }

    #[test]
    fn a_remainder_outside_its_own_scale_is_not_a_reading() {
        let reply = json!({
            "providers": {
                "a": { "quota_groups": {
                    "high": { "remaining_percent": 120.0 },
                    "low": { "remaining_percent": -5.0 },
                    "text": { "remaining_percent": "80" },
                    "good": { "remaining_percent": 50.0 }
                } }
            }
        });
        let usage = reading(&reply);
        assert_eq!(usage.windows.len(), 1);
        assert_eq!(usage.windows[0].percent_used, Some(50.0));
    }

    #[test]
    fn an_upstream_with_a_malformed_groups_field_is_left_out_rather_than_failing() {
        let reply = json!({
            "providers": {
                "broken": { "quota_groups": "not groups" },
                "whole": { "quota_groups": { "only": { "remaining_percent": 10.0 } } }
            }
        });
        let usage = reading(&reply);
        assert_eq!(usage.windows.len(), 1);
        assert_eq!(usage.windows[0].label, "Credits whole · only");
        assert_eq!(usage.windows[0].percent_used, Some(90.0));
    }

    #[test]
    fn a_reply_with_nobody_in_it_is_no_limits_rather_than_a_ring_at_zero() {
        assert!(reading(&json!({ "providers": {} })).error.is_some());
        assert!(reading(&json!({})).error.is_some());
        // Upstreams, but nothing usable among them.
        let reply = json!({ "providers": { "a": { "quota_groups": {} } } });
        assert!(reading(&reply).error.is_some());
    }
}
