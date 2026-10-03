//! V2EX's AI Chat allowance, read with a Personal Access Token.
//!
//! One documented route, `GET https://edge.v2ex.com/api/v2/chat/quota`, and —
//! unusually for this provider set — a published help page behind it
//! (`https://edge.v2ex.com/help/quota`) that states the rules the numbers
//! follow.
//!
//! ```json
//! { "success": true, "result": {
//!     "active": false, "total_tokens": 8020000, "used_tokens": 0,
//!     "remaining_tokens": 8020000, "used_percent": 0,
//!     "period_start": 0, "period_end": 0,
//!     "extra_usage": { "pack_count": 1, "total_tokens": 12000000,
//!                      "used_tokens": 34897, "remaining_tokens": 11965103 } } }
//! ```
//!
//! **The window does not run until it is used.** V2EX starts a five-hour window
//! when it receives the next message, not on a clock — so a reading with
//! `active: false` reports the size of the allowance that *would* be granted
//! and `period_start` / `period_end` of zero. That is a complete answer and not
//! a fault: the ring is drawn at whatever has been spent (nothing), and no reset
//! time and no length are claimed, so the card does not count down to a moment
//! V2EX has not promised. Asking this route never starts a window.
//!
//! The extra pack (`extra_usage`, 额外用量) is a second allowance with its own
//! stated size, no expiry, and no window at all — it is only spent once the five
//! hours' worth is gone. It is reported as a window of its own rather than
//! folded into the one above, because a reader whose window is spent but whose
//! pack is full is not out of quota, and a single ring saying 100% would tell
//! them they were.

use std::sync::Arc;

use serde_json::Value;

use super::{describe_reqwest_error, percent_from_fraction, Ctx, FetchFuture, Provider};
use crate::model::{parse_reset, ProviderUsage, UsageWindow};

const ENDPOINT: &str = "https://edge.v2ex.com/api/v2/chat/quota";

/// V2EX's own wording: "每 5 小时为一个配额窗口".
const WINDOW_SECONDS: i64 = 5 * 3_600;

/// The window's heading, taken from the length V2EX states rather than from the
/// key the reply happens to arrive under.
fn window_label() -> String {
    match WINDOW_SECONDS {
        18_000 => "5h".to_string(),
        other => super::humanize_window_seconds(other),
    }
}

pub struct V2ex;

impl Provider for V2ex {
    fn id(&self) -> &'static str {
        "v2ex"
    }

    fn name(&self) -> &'static str {
        "V2EX"
    }

    /// The Personal Access Token the fetch reads, and nothing else.
    fn is_configured(&self) -> bool {
        super::provider_key(self.id()).is_some()
    }

    fn fetch(&self, ctx: Arc<Ctx>) -> FetchFuture {
        Box::pin(async move { fetch_inner(ctx).await })
    }
}

async fn fetch_inner(ctx: Arc<Ctx>) -> ProviderUsage {
    const ID: &str = "v2ex";
    const NAME: &str = "V2EX";

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
            (401, " — the Personal Access Token was refused"),
            (403, " — the Personal Access Token was refused"),
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
    const ID: &str = "v2ex";
    const NAME: &str = "V2EX";

    // V2EX answers 200 and says no in the body. The route is scoped to the
    // token, so the one thing it can refuse is the token.
    if json.get("success").and_then(|v| v.as_bool()) == Some(false) {
        return ProviderUsage::failed(ID, NAME, "the Personal Access Token was refused");
    }

    let Some(quota) = json.get("result") else {
        return ProviderUsage::failed(ID, NAME, "bad reply: no quota object");
    };

    let windows = windows_from(quota);
    if windows.is_empty() {
        return ProviderUsage::failed(ID, NAME, "no quota windows in response");
    }

    ProviderUsage::ok(ID, NAME, windows)
}

/// The window, and the pack when one has been bought.
///
/// The fraction is worked out from V2EX's own `used_tokens` over its own
/// `total_tokens` rather than from `used_percent`, which is the same figure
/// rounded to a whole number — the panel does its own rounding and would
/// otherwise round a rounding.
fn windows_from(quota: &Value) -> Vec<UsageWindow> {
    let mut windows = Vec::new();
    let is_active = quota.get("active").and_then(|v| v.as_bool()).unwrap_or(false);

    if let Some(fraction) = fraction(
        number(quota.get("used_tokens")),
        number(quota.get("total_tokens")),
    ) {
        // **Only while a window is running.** `period_end` is zero otherwise,
        // and 1970 drawn as a reset time is a countdown that has already
        // expired — so the five hours, which are real but have not started, are
        // not claimed either.
        let resets_at = if is_active {
            epoch(quota.get("period_end"))
        } else {
            None
        };

        windows.push(
            UsageWindow::new(window_label(), Some(percent_from_fraction(fraction)))
                .with_reset(resets_at)
                .with_detail(Some(if is_active {
                    "window running".to_string()
                } else {
                    "starts with your next message".to_string()
                })),
        );
    }

    // The 加油包: tokens bought on top, spent only after the window's are gone,
    // and with no expiry.
    if let Some(extra) = quota.get("extra_usage") {
        let packs = extra.get("pack_count").and_then(|v| v.as_i64()).unwrap_or(0);
        if packs > 0 {
            if let Some(fraction) = fraction(
                number(extra.get("used_tokens")),
                number(extra.get("total_tokens")),
            ) {
                // The pack never expires, so there is no length and nothing to
                // count down to.
                windows.push(
                    UsageWindow::new("Top-up", Some(percent_from_fraction(fraction)))
                        .with_kind(crate::model::WindowKind::TopUp)
                        .with_exhausted(number(extra.get("remaining_tokens"))==Some(0.0)).with_detail(
                        Some(format!(
                            "{packs} pack{} bought",
                            if packs == 1 { "" } else { "s" }
                        )),
                    ),
                );
            }
        }
    }

    windows
}

/// How much of a stated allowance is gone, or none where one was not stated. A
/// total of zero is not an allowance to divide by.
fn fraction(used: Option<f64>, total: Option<f64>) -> Option<f64> {
    let total = total.filter(|total| total.is_finite() && *total > 0.0)?;
    let used = used.filter(|used| used.is_finite())?;
    Some(((used / total)).clamp(0.0, 1.0))
}

fn number(value: Option<&Value>) -> Option<f64> {
    super::dig_number(value)
}

/// Unix seconds, and **zero is not a date**: it is what this route reports when
/// there is no window, and 1 January 1970 shown as a reset is a countdown that
/// ran out decades ago.
fn epoch(value: Option<&Value>) -> Option<String> {
    let seconds = number(value).filter(|seconds| seconds.is_finite() && *seconds > 0.0)?;
    parse_reset(&serde_json::json!(seconds))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// The reply's shape, with no window running yet and one pack bought.
    fn fixture() -> Value {
        json!({
            "success": true,
            "result": {
                "active": false,
                "total_tokens": 8_020_000,
                "used_tokens": 0,
                "remaining_tokens": 8_020_000,
                "used_percent": 0,
                "period_start": 0,
                "period_end": 0,
                "extra_usage": {
                    "pack_count": 1,
                    "total_tokens": 12_000_000,
                    "used_tokens": 34_897,
                    "remaining_tokens": 11_965_103
                }
            }
        })
    }

    #[test]
    fn a_window_that_has_not_started_claims_no_reset() {
        let usage = reading(&fixture());
        assert_eq!(usage.windows[0].label, "5h");
        assert_eq!(usage.windows[0].percent_used, Some(0.0));
        // `period_end` is zero, and 1970 drawn as a reset is a countdown that
        // has already expired.
        assert!(usage.windows[0].resets_at.is_none());
        assert_eq!(
            usage.windows[0].detail.as_deref(),
            Some("starts with your next message")
        );
    }

    #[test]
    fn a_running_window_states_when_it_resets() {
        let mut reply = fixture();
        reply["result"]["active"] = json!(true);
        reply["result"]["used_tokens"] = json!(2_005_000);
        reply["result"]["period_end"] = json!(1_790_000_000);

        let usage = reading(&reply);
        assert_eq!(usage.windows[0].percent_used, Some(25.0));
        assert_eq!(usage.windows[0].resets_at.as_deref(), Some("2026-09-21T14:13:20Z"));
        assert_eq!(usage.windows[0].detail.as_deref(), Some("window running"));
    }

    #[test]
    fn the_pack_is_its_own_allowance_rather_than_folded_in() {
        let usage = reading(&fixture());
        assert_eq!(usage.windows.len(), 2);
        assert_eq!(usage.windows[1].label, "Top-up");
        assert!(usage.windows[1].resets_at.is_none());
        assert_eq!(usage.windows[1].detail.as_deref(), Some("1 pack bought"));
    }

    #[test]
    fn no_pack_means_no_second_window() {
        let reply = json!({
            "success": true,
            "result": { "active": true, "total_tokens": 100, "used_tokens": 10,
                        "extra_usage": { "pack_count": 0, "total_tokens": 0 } }
        });
        assert_eq!(reading(&reply).windows.len(), 1);
    }

    #[test]
    fn a_total_of_zero_is_not_an_allowance_to_divide_by() {
        assert_eq!(fraction(Some(5.0), Some(0.0)), None);
        assert_eq!(fraction(Some(5.0), None), None);
        assert_eq!(fraction(None, Some(100.0)), None);
    }

    #[test]
    fn a_token_the_route_refuses_arrives_as_a_good_http_status() {
        let reply = json!({ "success": false, "message": "invalid token" });
        let usage = reading(&reply);
        assert!(usage.error.is_some());
        assert!(usage.error.unwrap().contains("refused"));
    }

    #[test]
    fn a_used_figure_over_the_total_is_a_full_ring_not_more() {
        let reply = json!({
            "success": true,
            "result": { "active": true, "total_tokens": 100, "used_tokens": 250,
                        "period_end": 1_790_000_000 }
        });
        assert_eq!(reading(&reply).windows[0].percent_used, Some(100.0));
    }
}
