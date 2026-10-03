//! Kimi Code's limits, from its own documented usage endpoint.
//!
//! Reached with a key the user pastes into Settings — the same arrangement as
//! OpenCode Go, and without that provider's fallback to a credential another
//! tool stored: nothing on this machine holds a Kimi Code key.
//!
//! The reply has **two kinds of limit in it and they are not the same figure**:
//!
//! - `limits[]` — windows the service actually times, each stating a duration
//!   and a unit (300 minutes, say). These are read as they are given.
//! - `usage` — the weekly allowance. The reply gives it a reset time and no
//!   length, and the reset can land anywhere inside the week since the window
//!   rolls, so the length is not inferable from it — it is named from what the
//!   plan actually is.
//!
//! Every count arrives as a *string*, and `detail` reports what is left rather
//! than what is spent, so both are converted here and everything downstream
//! stays in whole numbers of what is gone.
//!
//! VERIFY ON A REAL MACHINE: `api.kimi.com/coding/v1/usages` is undocumented
//! outside Kimi's own CLI. Set `PULSEWIN_DEBUG=1` to dump a payload.

use std::sync::Arc;

use serde_json::Value;

use super::{describe_reqwest_error, percent_from_fraction, by_window_length, Ctx, FetchFuture, Provider};
use crate::credentials;
use crate::model::{parse_reset, ProviderUsage, UsageWindow};

const USAGE_URL: &str = "https://api.kimi.com/coding/v1/usages";

pub struct KimiCode;

impl Provider for KimiCode {
    fn id(&self) -> &'static str {
        "kimi-code"
    }

    fn name(&self) -> &'static str {
        "Kimi Code"
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
    const ID: &str = "kimi-code";
    const NAME: &str = "Kimi Code";

    let key = match super::provider_key(ID) {
        Some(key) => key,
        None => return ProviderUsage::failed(ID, NAME, super::missing_key(ID, "")),
    };

    let response = ctx
        .client
        .get(USAGE_URL)
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

    let windows = windows_from(&json);
    if windows.is_empty() {
        return ProviderUsage::failed(ID, NAME, "no quota windows in response");
    }

    let plan = plan_name(credentials::dig_str(&json, "user.membership.level"));

    // `totalQuota` comes back empty and `parallel.limit` is how many requests
    // may run at once, which is not a balance — so no account line either.
    ProviderUsage::ok(ID, NAME, windows).with_plan(plan)
}

/// The timed windows first, named by the length the service states, then the
/// weekly allowance, which the reply carries separately.
///
/// The original sorts by `windowSeconds`; a rolling weekly allowance is
/// therefore placed after any shorter timed limit rather than before it.
fn windows_from(json: &Value) -> Vec<UsageWindow> {
    let mut rows: Vec<(i64, UsageWindow)> = Vec::new();

    if let Some(limits) = json.get("limits").and_then(|v| v.as_array()) {
        for limit in limits {
            let Some(seconds) = duration_of(limit.get("window")) else {
                continue;
            };
            if let Some(window) = window_from(limit.get("detail"), seconds, true) {
                rows.push((seconds, window));
            }
        }
    }

    // The weekly allowance states a reset and no length. Seven days is what
    // sorts it; nothing downstream may treat it as a stated length, which is
    // why the original marks it `reportsLength: false` — this port's
    // `UsageWindow` has no such flag and never divides by a window, so the only
    // thing the number does here is order the row.
    if let Some(window) = window_from(json.get("usage"), 7 * 86_400, false) {
        rows.push((7 * 86_400, window));
    }

    by_window_length(rows)
}

fn window_from(detail: Option<&Value>, seconds: i64, is_limit_lane: bool) -> Option<UsageWindow> {
    let detail = detail?;

    let limit = number(detail.get("limit"))?;
    if limit <= 0.0 {
        return None;
    }

    // `used` when it is given, otherwise what the limit and the remainder
    // imply. `limits[].detail` carries no `used` at all.
    let used = match number(detail.get("used")) {
        Some(used) => used,
        None => limit - number(detail.get("remaining"))?,
    };

    let label = label_for(seconds);
    let resets_at = detail.get("resetTime").and_then(parse_reset);

    let window = UsageWindow::new(label, Some(percent_from_fraction((used / limit).clamp(0.0, 1.0))))
        .with_reset(resets_at)
        // `limits[]` can hold two windows of the same length — the shape the
        // other providers' per-model limits take — and this port has no window
        // id to tell them apart with, so the row says which lane it is.
        .with_detail(if is_limit_lane {
            Some(format!("{} / {} limit", used.round(), limit.round()))
        } else {
            None
        });

    Some(window)
}

/// A window's length in seconds, or none for a unit that isn't recognised — a
/// window with no length can't be named or sorted, and inventing one would put
/// a figure under a heading that isn't true.
fn duration_of(window: Option<&Value>) -> Option<i64> {
    let window = window?;
    let duration = credentials::dig_f64(window, "duration")? as i64;
    if duration <= 0 {
        return None;
    }

    match credentials::dig_str(window, "timeUnit")?.as_str() {
        "TIME_UNIT_SECOND" => Some(duration),
        "TIME_UNIT_MINUTE" => Some(duration * 60),
        "TIME_UNIT_HOUR" => Some(duration * 3_600),
        "TIME_UNIT_DAY" => Some(duration * 86_400),
        _ => None,
    }
}

fn label_for(seconds: i64) -> String {
    match seconds {
        18_000 => "5h".to_string(),
        604_800 => "7d".to_string(),
        2_592_000 => "Monthly".to_string(),
        other => super::humanize_window_seconds(other),
    }
}

/// "LEVEL_INTERMEDIATE" → "Intermediate". An unfamiliar tier is passed through
/// tidied rather than blanked: an unknown name still beats none, and it is the
/// only clue left when a new tier appears.
fn plan_name(level: Option<String>) -> Option<String> {
    let level = level?;
    if level.is_empty() {
        return None;
    }

    let bare = level.strip_prefix("LEVEL_").unwrap_or(&level);
    Some(
        bare.split('_')
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

/// Every count arrives as a string, and some as a number.
fn number(value: Option<&Value>) -> Option<f64> {
    super::dig_number(value)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// A captured reply's shape, counts as strings the way the service sends
    /// them.
    fn fixture() -> Value {
        json!({
            "user": { "membership": { "level": "LEVEL_INTERMEDIATE" } },
            "usage": { "limit": "1000", "used": "250", "remaining": "750",
                       "resetTime": "2026-10-08T00:00:00.123456Z" },
            "limits": [
                { "window": { "duration": 300, "timeUnit": "TIME_UNIT_MINUTE" },
                  "detail": { "limit": "100", "remaining": "96",
                              "resetTime": "2026-10-01T05:00:00Z" } }
            ]
        })
    }

    #[test]
    fn reads_the_timed_window_and_the_weekly_allowance() {
        let windows = windows_from(&fixture());
        assert_eq!(windows.len(), 2);
        assert_eq!(windows[0].label, "5h");
        // The timed lane reports what is left, not what is spent.
        assert_eq!(windows[0].percent_used, Some(4.0));
        assert_eq!(windows[0].resets_at.as_deref(), Some("2026-10-01T05:00:00Z"));
        assert_eq!(windows[1].label, "7d");
        assert_eq!(windows[1].percent_used, Some(25.0));
    }

    #[test]
    fn a_sub_second_reset_stamp_still_parses() {
        let windows = windows_from(&fixture());
        assert_eq!(windows[1].resets_at.as_deref(), Some("2026-10-08T00:00:00Z"));
    }

    #[test]
    fn used_wins_over_the_remainder() {
        let payload = json!({
            "limits": [ { "window": { "duration": 1, "timeUnit": "TIME_UNIT_HOUR" },
                          "detail": { "limit": "100", "used": "80", "remaining": "50" } } ]
        });
        assert_eq!(windows_from(&payload)[0].percent_used, Some(80.0));
        assert_eq!(windows_from(&payload)[0].label, "1h");
    }

    #[test]
    fn a_zero_limit_is_no_window() {
        let payload = json!({
            "limits": [ { "window": { "duration": 300, "timeUnit": "TIME_UNIT_MINUTE" },
                          "detail": { "limit": "0", "remaining": "0" } } ]
        });
        assert!(windows_from(&payload).is_empty());
    }

    #[test]
    fn an_unrecognised_unit_is_dropped_rather_than_named() {
        let payload = json!({
            "limits": [ { "window": { "duration": 3, "timeUnit": "TIME_UNIT_FORTNIGHT" },
                          "detail": { "limit": "10", "remaining": "5" } } ]
        });
        assert!(windows_from(&payload).is_empty());
    }

    #[test]
    fn a_month_long_limit_is_named_monthly() {
        let payload = json!({
            "limits": [ { "window": { "duration": 30, "timeUnit": "TIME_UNIT_DAY" },
                          "detail": { "limit": "10", "remaining": "1" } } ]
        });
        assert_eq!(windows_from(&payload)[0].label, "Monthly");
    }

    #[test]
    fn the_plan_name_is_tidied_from_the_level_code() {
        assert_eq!(
            plan_name(Some("LEVEL_INTERMEDIATE".into())).as_deref(),
            Some("Intermediate")
        );
        assert_eq!(plan_name(Some("MAX".into())).as_deref(), Some("Max"));
        assert!(plan_name(None).is_none());
        assert!(plan_name(Some(String::new())).is_none());
    }
}
