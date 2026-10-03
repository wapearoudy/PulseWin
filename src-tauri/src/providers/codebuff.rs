//! Codebuff, the coding agent: its credit allowance, the credits left, and —
//! for the CLI's own login — the weekly rate limit.
//!
//! Read with a key the reader supplies (`PULSEWIN_CODEBUFF_KEY`, or
//! `%APPDATA%\PulseWin\codebuff.json`), or failing that the login the
//! `codebuff` CLI (formerly `manicode`) saved in
//! `~/.config/manicode/credentials.json` — the path the original reads.
//! `is_configured` is true when either is there, and makes no network call. The
//! two are refused in different words, because the remedy differs.
//!
//! - `POST https://www.codebuff.com/api/v1/usage` — the credits used, the
//!   quota, the balance and the next reset. A POST because that is how the CLI
//!   asks; it carries only a fingerprint id and changes nothing.
//! - `GET https://www.codebuff.com/api/user/subscription` — the tier and the
//!   weekly limit. Asked only with the CLI's login, as CodexBar does: an API
//!   key reads the credits alone. Best effort — its failure leaves the credits
//!   standing.
//!
//! The shapes are second-hand — taken from CodexBar's Codebuff provider and its
//! tests, not from captured replies — and the fixtures below say so.
//!
//! **The credits left have no money field of their own here.** The original
//! carries them as a credit balance beside the windows; this port's result has
//! only windows, so they ride as a window with no fraction and the figure on
//! its detail line — the same shape the balance-only providers use. Codebuff's
//! credits are not dollars, so nothing is called one.

use std::path::PathBuf;
use std::sync::Arc;

use serde_json::Value;

use super::{percent_from_fraction, Ctx, FetchFuture, Provider};
use crate::credentials;
use crate::model::{ProviderUsage, UsageWindow};

const USAGE_ENDPOINT: &str = "https://www.codebuff.com/api/v1/usage";
const SUBSCRIPTION_ENDPOINT: &str = "https://www.codebuff.com/api/user/subscription";

/// What the CLI sends in the body: nothing but a name for the caller.
const USAGE_BODY: &str = r#"{"fingerprintId":"pulse-usage"}"#;

/// Anything past this is milliseconds: as seconds it would be the year 33658.
const MILLISECONDS_ABOVE: f64 = 10_000_000_000.0;

pub struct Codebuff;

impl Provider for Codebuff {
    fn id(&self) -> &'static str {
        "codebuff"
    }

    fn name(&self) -> &'static str {
        "Codebuff"
    }

    /// A key the reader supplied, or the login the `codebuff` CLI saved. The
    /// file only has to hold a token to count; whether the service still takes
    /// it is what the fetch finds out.
    fn is_configured(&self) -> bool {
        super::provider_key(self.id()).is_some() || saved_token().is_some()
    }

    fn fetch(&self, ctx: Arc<Ctx>) -> FetchFuture {
        Box::pin(async move { fetch_inner(ctx).await })
    }
}

fn credentials_path() -> Option<PathBuf> {
    credentials::home_relative(&[".config", "manicode", "credentials.json"])
        .into_iter()
        .next()
}

/// The token the CLI saved: `default.authToken`, or a top-level `authToken`.
/// Only read, never written.
fn saved_token() -> Option<String> {
    let json = credentials::read_json(&credentials_path()?)?;
    credentials::dig_first_str(&json, &["default.authToken", "authToken"])
}

async fn fetch_inner(ctx: Arc<Ctx>) -> ProviderUsage {
    const ID: &str = "codebuff";
    const NAME: &str = "Codebuff";

    // The pasted key first: it is the one somebody chose on purpose.
    let (token, from_cli) = match super::provider_key(ID) {
        Some(key) => (key, false),
        None => match saved_token() {
            Some(token) => (token, true),
            None => {
                return ProviderUsage::failed(
                    ID,
                    NAME,
                    super::missing_key(
                        ID,
                        ", or sign in with the codebuff CLI so ~/.config/manicode/credentials.json exists",
                    ),
                )
            }
        },
    };

    let refused = if from_cli {
        "the saved Codebuff login is stale — run a codebuff command to renew it"
    } else {
        "the API key was refused"
    };

    // Both in flight together, as the reference asks them: the subscription is
    // an enrichment, and waiting for it in turn would hold the credits back.
    let usage = usage_reply(&ctx, &token, refused);
    let subscription = subscription_body(&ctx, &token, from_cli);

    let (usage, subscription) = futures::future::join(usage, subscription).await;

    let body = match usage {
        Ok(body) => body,
        Err(problem) => return ProviderUsage::failed(ID, NAME, problem),
    };

    let json: Value = match serde_json::from_str(&body) {
        Ok(v) => v,
        Err(e) => return ProviderUsage::failed(ID, NAME, format!("bad JSON: {e}")),
    };

    super::claude_code::debug_dump(ID, &json);

    reading(&json, subscription.as_ref())
}

/// The credits reply, or why there is none.
async fn usage_reply(ctx: &Ctx, token: &str, refused: &str) -> Result<String, String> {
    let response = ctx
        .client
        .post(USAGE_ENDPOINT)
        .header("Authorization", format!("Bearer {token}"))
        .header("Accept", "application/json")
        .header("Content-Type", "application/json")
        .body(USAGE_BODY)
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
            (401, refused),
            (403, refused),
            (429, " — rate limited, try again shortly"),
        ];
        return Err(super::http_failure(status, hints));
    }

    Ok(body)
}

/// The subscription reply when the login is the CLI's own.
///
/// Best effort: an API key does not read it at all, and a failure of any kind
/// leaves the credits standing rather than failing the card.
async fn subscription_body(ctx: &Ctx, token: &str, from_cli: bool) -> Option<Value> {
    if !from_cli {
        return None;
    }

    let response = ctx
        .client
        .get(SUBSCRIPTION_ENDPOINT)
        .header("Authorization", format!("Bearer {token}"))
        .header("Accept", "application/json")
        .send()
        .await
        .ok()?;

    if !response.status().is_success() {
        return None;
    }

    serde_json::from_str(&response.text().await.ok()?).ok()
}

/// The credits, the weekly limit and what is left.
fn reading(json: &Value, subscription: Option<&Value>) -> ProviderUsage {
    const ID: &str = "codebuff";
    const NAME: &str = "Codebuff";

    if !json.is_object() {
        return ProviderUsage::failed(ID, NAME, "unreadable reply");
    }

    let mut windows: Vec<(i64, UsageWindow)> = Vec::new();

    // Credits used out of the quota, both as Codebuff reports them. A missing
    // or zero quota is left off — never drawn as spent.
    let used = number(json.get("usage")).or_else(|| number(json.get("used")));
    let quota = number(json.get("quota")).or_else(|| number(json.get("limit")));
    if let (Some(used), Some(quota)) = (used, quota) {
        if quota > 0.0 {
            windows.push((
                30 * 86_400,
                UsageWindow::new("Credits", Some(percent_from_fraction(used / quota)))
                    // No period is stated, only the next reset.
                    .with_reset(date(json.get("next_quota_reset"))),
            ));
        }
    }

    // The weekly limit, when the subscription reply has one.
    let mut plan = None;
    if let Some(reply) = subscription {
        let details = reply.get("subscription");
        let rate_limit = reply.get("rateLimit");

        let weekly_used = rate_limit
            .and_then(|limit| number(limit.get("weeklyUsed")))
            .or_else(|| rate_limit.and_then(|limit| number(limit.get("used"))));
        let weekly_limit = rate_limit
            .and_then(|limit| number(limit.get("weeklyLimit")))
            .or_else(|| rate_limit.and_then(|limit| number(limit.get("limit"))));

        if let (Some(weekly_used), Some(weekly_limit)) = (weekly_used, weekly_limit) {
            if weekly_limit > 0.0 {
                windows.push((
                    7 * 86_400,
                    UsageWindow::new(
                        "7d",
                        Some(percent_from_fraction(weekly_used / weekly_limit)),
                    )
                    .with_reset(
                        date(rate_limit.and_then(|limit| limit.get("weeklyResetsAt"))),
                    ),
                ));
            }
        }

        plan = [
            details.and_then(|details| details.get("displayName")),
            reply.get("displayName"),
            details.and_then(|details| details.get("tier")),
            reply.get("tier"),
        ]
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(|text| text.trim().to_string())
        .find(|text| !text.is_empty())
        .map(|text| {
            // "pro" is a name, not a sentence.
            let mut chars = text.chars();
            match chars.next() {
                Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
                None => text,
            }
        });
    }

    let mut windows = super::by_window_length(windows);

    // Credits, not money: the number alone, as a balance in the provider's own
    // unit.
    let remaining =
        number(json.get("remainingBalance")).or_else(|| number(json.get("remaining")));
    if let Some(remaining) = remaining {
        windows.push(super::balance_window("Balance", credits(remaining)));
    }

    if windows.is_empty() {
        return ProviderUsage::failed(ID, NAME, "no limits reported");
    }

    ProviderUsage::ok(ID, NAME, windows).with_plan(plan)
}

/// A figure: a finite, non-negative number, or one written as a string.
fn number(value: Option<&Value>) -> Option<f64> {
    super::dig_number(value).filter(|figure| figure.is_finite() && *figure >= 0.0)
}

/// Credits to at most one decimal place: `1234`, `1234.5`.
fn credits(amount: f64) -> String {
    let rounded = (amount * 10.0).round() / 10.0;
    if rounded.fract() == 0.0 {
        format!("{}", rounded as i64)
    } else {
        format!("{rounded:.1}")
    }
}

/// A reset as Codebuff writes it: ISO 8601, or seconds — or milliseconds —
/// since 1970, either as a number or in a string.
///
/// Its own threshold rather than `parse_reset`'s: this reply's figures are
/// known to pass ten billion well before the year 2286, which is where the
/// shared helper starts treating a number as milliseconds.
fn date(value: Option<&Value>) -> Option<String> {
    match value? {
        Value::String(text) => {
            let text = text.trim();
            if let Ok(at) = chrono::DateTime::parse_from_rfc3339(text) {
                return Some(stamp(at.with_timezone(&chrono::Utc)));
            }
            epoch(text.parse::<f64>().ok()?)
        }
        Value::Number(number) => epoch(number.as_f64()?),
        _ => None,
    }
}

fn epoch(value: f64) -> Option<String> {
    if !value.is_finite() || value <= 0.0 {
        return None;
    }
    let seconds = if value > MILLISECONDS_ABOVE {
        value / 1_000.0
    } else {
        value
    };
    Some(stamp(chrono::DateTime::from_timestamp(seconds as i64, 0)?))
}

fn stamp(at: chrono::DateTime<chrono::Utc>) -> String {
    at.to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// The two replies as CodexBar's tests describe them.
    fn usage_fixture() -> Value {
        json!({
            "usage": 250.0,
            "quota": 1000.0,
            "remainingBalance": 750.0,
            "next_quota_reset": "2026-11-01T00:00:00Z"
        })
    }

    fn subscription_fixture() -> Value {
        json!({
            "subscription": { "displayName": "pro", "tier": "tier_2" },
            "rateLimit": {
                "weeklyUsed": 40.0,
                "weeklyLimit": 200.0,
                "weeklyResetsAt": "2026-10-08T00:00:00Z"
            }
        })
    }

    #[test]
    fn reads_the_credits_the_weekly_limit_and_the_balance() {
        let usage = reading(&usage_fixture(), Some(&subscription_fixture()));
        let labels: Vec<&str> = usage.windows.iter().map(|w| w.label.as_str()).collect();

        assert_eq!(labels, vec!["7d", "Credits", "Balance"]);
        assert_eq!(usage.windows[0].percent_used, Some(20.0));
        assert_eq!(
            usage.windows[0].resets_at.as_deref(),
            Some("2026-10-08T00:00:00Z")
        );
        assert_eq!(usage.windows[1].percent_used, Some(25.0));
        assert_eq!(
            usage.windows[1].resets_at.as_deref(),
            Some("2026-11-01T00:00:00Z")
        );
        assert_eq!(usage.windows[2].detail.as_deref(), Some("750"));
        assert_eq!(usage.windows[2].percent_used, None);
        assert_eq!(usage.plan.as_deref(), Some("Pro"));
    }

    #[test]
    fn an_api_key_reads_the_credits_alone() {
        let usage = reading(&usage_fixture(), None);
        let labels: Vec<&str> = usage.windows.iter().map(|w| w.label.as_str()).collect();
        assert_eq!(labels, vec!["Credits", "Balance"]);
        assert!(usage.plan.is_none());
    }

    #[test]
    fn a_quota_that_is_missing_or_zero_is_never_drawn_as_spent() {
        let no_quota = json!({ "usage": 250.0, "remainingBalance": 10.0 });
        let usage = reading(&no_quota, None);
        assert_eq!(usage.windows.len(), 1);
        assert_eq!(usage.windows[0].label, "Balance");

        let zero = json!({ "usage": 5.0, "quota": 0.0, "remainingBalance": 10.0 });
        assert_eq!(reading(&zero, None).windows.len(), 1);
    }

    #[test]
    fn the_alternate_field_names_are_read_too() {
        let reply = json!({ "used": 5.0, "limit": 10.0, "remaining": 5.0 });
        let usage = reading(&reply, None);
        assert_eq!(usage.windows[0].percent_used, Some(50.0));
        assert_eq!(usage.windows[1].detail.as_deref(), Some("5"));

        let rate = json!({
            "rateLimit": { "used": 5.0, "limit": 10.0 }
        });
        let usage = reading(&json!({ "remaining": 1.0 }), Some(&rate));
        let labels: Vec<&str> = usage.windows.iter().map(|w| w.label.as_str()).collect();
        assert_eq!(labels, vec!["7d", "Balance"]);
    }

    #[test]
    fn a_figure_may_arrive_as_a_string_and_never_negative() {
        let reply = json!({ "usage": "250", "quota": "1000", "remainingBalance": -1.0 });
        let usage = reading(&reply, None);
        assert_eq!(usage.windows.len(), 1);
        assert_eq!(usage.windows[0].percent_used, Some(25.0));
    }

    #[test]
    fn credits_are_shown_to_one_decimal_place_and_no_more() {
        assert_eq!(credits(750.0), "750");
        assert_eq!(credits(750.5), "750.5");
        assert_eq!(credits(750.44), "750.4");
        assert_eq!(credits(0.0), "0");
    }

    #[test]
    fn a_reset_is_read_as_a_time_or_as_a_count_of_seconds() {
        assert_eq!(
            date(Some(&json!("2026-11-01T00:00:00Z"))).as_deref(),
            Some("2026-11-01T00:00:00Z")
        );
        assert_eq!(
            date(Some(&json!("1759276800"))).as_deref(),
            Some("2025-10-01T00:00:00Z")
        );
        assert_eq!(
            date(Some(&json!(1_759_276_800_000i64))).as_deref(),
            Some("2025-10-01T00:00:00Z")
        );
        assert_eq!(date(Some(&json!(0))), None);
        assert_eq!(date(Some(&json!(true))), None);
        assert_eq!(date(None), None);
    }

    #[test]
    fn a_reply_that_is_not_an_object_is_unreadable_and_an_empty_one_is_no_limits() {
        assert!(reading(&json!([1, 2]), None).error.is_some());
        assert!(reading(&json!("nope"), None).error.is_some());
        assert!(reading(&json!({}), None).error.is_some());
    }

    #[test]
    fn the_plan_keeps_the_name_the_service_gave_and_only_lifts_its_first_letter() {
        let named = json!({ "subscription": { "tier": "max" } });
        assert_eq!(
            reading(&json!({ "remaining": 1.0 }), Some(&named))
                .plan
                .as_deref(),
            Some("Max")
        );

        // A display name wins over the tier, and a blank one is skipped.
        let both = json!({
            "subscription": { "displayName": "  ", "tier": "tier_2" },
            "displayName": "Codebuff Plus"
        });
        assert_eq!(
            reading(&json!({ "remaining": 1.0 }), Some(&both))
                .plan
                .as_deref(),
            Some("Codebuff Plus")
        );
    }
}
