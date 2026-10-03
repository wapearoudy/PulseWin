//! Hugging Face: the account's ZeroGPU quota — GPU time on ZeroGPU Spaces,
//! counted in seconds and reported with how much is left and when it resets.
//!
//! Read with an access token: one the user enters, or failing that the one
//! `hf auth login` saved in `~/.cache/huggingface/token`. Any token that can
//! read the account will do.
//!
//! `GET https://huggingface.co/api/spaces/zero-gpu/quota` — a documented Hub
//! endpoint.
//!
//! **What is left out, and why.** Inference Providers' month-to-date charges
//! (`/api/settings/billing/usage-v2`) are spend: the reply states no allowance,
//! and its "included" amount and spending limit do not make one this port could
//! draw without deciding what the limit is measured against. The prepaid credit
//! wallet is only on the website, behind a browser session and an identity
//! check against the token; one credential cannot be both.

use std::sync::Arc;

use serde_json::Value;

use super::{describe_reqwest_error, percent_from_fraction, Ctx, FetchFuture, Provider};
use crate::credentials;
use crate::model::{parse_reset, ProviderUsage, UsageWindow};

const ENDPOINT: &str = "https://huggingface.co/api/spaces/zero-gpu/quota";

/// Seconds since 1970 up to the year 4000, past which a figure is not a date.
const MAX_EPOCH_SECONDS: f64 = 64_092_211_200.0;

pub struct HuggingFace;

impl Provider for HuggingFace {
    fn id(&self) -> &'static str {
        "hugging-face"
    }

    fn name(&self) -> &'static str {
        "Hugging Face"
    }

    /// Either token the fetch accepts: one entered for this provider, or the
    /// one `hf auth login` saved.
    fn is_configured(&self) -> bool {
        super::provider_key(self.id()).is_some() || saved_token().is_some()
    }

    fn fetch(&self, ctx: Arc<Ctx>) -> FetchFuture {
        Box::pin(async move { fetch_inner(ctx).await })
    }
}

async fn fetch_inner(ctx: Arc<Ctx>) -> ProviderUsage {
    const ID: &str = "hugging-face";
    const NAME: &str = "Hugging Face";

    // The token somebody entered first: it is the one they chose on purpose.
    let entered = super::provider_key(ID);
    let from_file = entered.is_none();
    let token = match entered.or_else(saved_token) {
        Some(token) => token,
        None => {
            return ProviderUsage::failed(
                ID,
                NAME,
                super::missing_key(
                    ID,
                    ", or run `hf auth login` so ~/.cache/huggingface/token exists",
                ),
            )
        }
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
        // A token read out of Hugging Face's own file that the Hub turns away
        // is a stale login, not a key somebody typed — and the remedy differs.
        let hints: &[(u16, &str)] = if from_file {
            &[
                (401, " — the saved Hugging Face login is stale; run `hf auth login` again"),
                (403, " — the saved Hugging Face login is stale; run `hf auth login` again"),
                (429, " — rate limited, try again shortly"),
            ]
        } else {
            &[
                (401, " — the access token was refused"),
                (403, " — the access token was refused"),
                (429, " — rate limited, try again shortly"),
            ]
        };
        return ProviderUsage::failed(ID, NAME, super::http_failure(status, hints));
    }

    let json: Value = match serde_json::from_str(&body) {
        Ok(v) => v,
        Err(e) => return ProviderUsage::failed(ID, NAME, format!("bad JSON: {e}")),
    };

    super::claude_code::debug_dump(ID, &json);

    reading(&json).with_account(Some(if from_file {
        "hf auth login".to_string()
    } else {
        "access token".to_string()
    }))
}

/// The token `hf auth login` wrote: the file's first line, with the surrounding
/// quotes some shells leave on it stripped. Only read, never written.
fn saved_token() -> Option<String> {
    let path = credentials::home_relative(&[".cache", "huggingface", "token"])
        .into_iter()
        .next()?;
    let text = std::fs::read_to_string(&path).ok()?;
    let line = text.lines().next()?.trim().to_string();
    if line.is_empty() {
        return None;
    }

    let token = match (
        line.chars().next(),
        line.chars().last(),
        line.chars().count() >= 2,
    ) {
        (Some('"'), Some('"'), true) | (Some('\''), Some('\''), true) => {
            line[1..line.len() - 1].trim().to_string()
        }
        _ => line,
    };

    if token.is_empty() {
        None
    } else {
        Some(token)
    }
}

/// The mapping, kept apart from the request so a fixture can drive it.
fn reading(json: &Value) -> ProviderUsage {
    const ID: &str = "hugging-face";
    const NAME: &str = "Hugging Face";

    // `base` is the quota and `current` what is left of it, in seconds of GPU
    // time. An account with no quota — or figures that aren't ones — draws
    // nothing rather than a zero.
    let base = seconds(json.get("base"));
    let current = seconds(json.get("current"));

    let (Some(base), Some(current)) = (base, current) else {
        return ProviderUsage::failed(ID, NAME, "no ZeroGPU quota in response");
    };
    if base <= 0.0 {
        return ProviderUsage::failed(ID, NAME, "no ZeroGPU quota in response");
    }

    let used = (base - current).max(0.0);
    let window = UsageWindow::new("ZeroGPU", Some(percent_from_fraction(used / base)))
        // Hugging Face describes the quota as daily; the reply states only when
        // it resets, so the reset is shown and no length is claimed. The port's
        // window model has no `reportsLength`, and never divides by a window,
        // so nothing here counts down against a length.
        .with_reset(reset(json.get("resetsAt")))
        .with_detail(Some(format!(
            "{} / {} s of GPU time left",
            current.round(),
            base.round()
        )));

    ProviderUsage::ok(ID, NAME, vec![window])
}

fn seconds(value: Option<&Value>) -> Option<f64> {
    match value? {
        // A JSON boolean is not a number, and `true as f64` would be one.
        Value::Number(n) => n.as_f64().filter(|v| v.is_finite() && *v >= 0.0),
        _ => None,
    }
}

/// An ISO 8601 string, or seconds since 1970 within a sane range.
fn reset(value: Option<&Value>) -> Option<String> {
    let value = value?;
    match value {
        Value::String(_) => parse_reset(value),
        Value::Number(n) => n
            .as_f64()
            .filter(|seconds| seconds.is_finite() && *seconds > 0.0 && *seconds < MAX_EPOCH_SECONDS)
            .and_then(|seconds| parse_reset(&serde_json::json!(seconds))),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn reads_the_zerogpu_quota_as_seconds_of_gpu_time() {
        let reply = json!({
            "base": 18_000, "current": 4_500,
            "resetsAt": "2026-10-02T00:00:00Z"
        });
        let usage = reading(&reply);
        assert_eq!(usage.windows[0].label, "ZeroGPU");
        // 13,500 of 18,000 seconds gone is 75%.
        assert_eq!(usage.windows[0].percent_used, Some(75.0));
        assert_eq!(usage.windows[0].resets_at.as_deref(), Some("2026-10-02T00:00:00Z"));
    }

    #[test]
    fn an_epoch_reset_is_read_as_seconds() {
        let reply = json!({ "base": 100, "current": 50, "resetsAt": 1_790_000_000 });
        let usage = reading(&reply);
        assert_eq!(usage.windows[0].resets_at.as_deref(), Some("2026-09-21T14:13:20Z"));
    }

    #[test]
    fn an_exhausted_quota_reads_full() {
        let reply = json!({ "base": 100, "current": 0 });
        assert_eq!(reading(&reply).windows[0].percent_used, Some(100.0));
    }

    #[test]
    fn more_left_than_the_quota_is_not_a_negative_spend() {
        let reply = json!({ "base": 100, "current": 150 });
        assert_eq!(reading(&reply).windows[0].percent_used, Some(0.0));
    }

    #[test]
    fn an_account_with_no_quota_draws_nothing_rather_than_a_zero() {
        assert!(reading(&json!({ "base": 0, "current": 0 })).error.is_some());
        assert!(reading(&json!({ "current": 10 })).error.is_some());
        assert!(reading(&json!({ "base": true, "current": 10 })).error.is_some());
    }

    #[test]
    fn a_quote_wrapped_token_is_unwrapped() {
        // The file's shape is one line; some tools leave quotes on it.
        let line = "\"hf_abc123\"";
        let token = line[1..line.len() - 1].trim().to_string();
        assert_eq!(token, "hf_abc123");
    }
}
