//! The MiniMax Coding Plan's limits, from its token-plan endpoint.
//!
//! **Two providers, one service**, for the same reason as the GLM plan:
//! `api.minimax.io` and `api.minimaxi.com` are one company's international and
//! mainland storefronts, and a key for one is refused by the other. CodexBar
//! models this as a region switch inside one provider; each gets its own ring
//! here.
//!
//! `GET {host}/v1/token_plan/remains` with the key as a bearer token, falling
//! back to the older coding-plan path. Undocumented, and it can change without
//! notice.
//!
//! Three things about the reply are worth knowing before changing anything:
//!
//! - **It reports what is *left*, not what is gone.** `current_*_remaining_percent`
//!   at 96 means 4% spent. The inversion happens here so everything downstream
//!   stays in terms of what has been used.
//! - **Numbers arrive as strings or as numbers, interchangeably.** The same
//!   field is `"96"` in one reply and `75` in another, so every figure goes
//!   through a reader that takes either (`super::dig_number`).
//! - **Lanes exist that are not part of the subscription.** They come back with
//!   status 3, zero counts and 100% remaining — a video lane on a plan with no
//!   video. Read literally that is a ring pinned at 0% for a thing the account
//!   cannot use, so they are left out.

use std::sync::Arc;

use serde_json::Value;

use super::{by_window_length, describe_reqwest_error, dig_number, percent_from_scale, Ctx, FetchFuture, Provider};
use crate::credentials;
use crate::model::{parse_reset, ProviderUsage, UsageWindow};

/// The mainland service is a different host *and* a different account.
const INTERNATIONAL_HOST: &str = "https://api.minimax.io";
const MAINLAND_HOST: &str = "https://api.minimaxi.com";

/// The current path first, then the one it replaced. A plan that answers 404 on
/// the first is not a failure, it is an older account.
const PATHS: &[&str] = &[
    "/v1/token_plan/remains",
    "/v1/api/openplatform/coding_plan/remains",
];

pub struct MiniMax;

pub struct MiniMaxCn;

impl Provider for MiniMax {
    fn id(&self) -> &'static str {
        "minimax"
    }

    fn name(&self) -> &'static str {
        "MiniMax"
    }

    /// The key the fetch reads, and nothing else.
    fn is_configured(&self) -> bool {
        super::provider_key(self.id()).is_some()
    }

    fn fetch(&self, ctx: Arc<Ctx>) -> FetchFuture {
        Box::pin(async move { fetch_inner(ctx, "minimax", "MiniMax", INTERNATIONAL_HOST).await })
    }
}

impl Provider for MiniMaxCn {
    fn id(&self) -> &'static str {
        "minimax-cn"
    }

    fn name(&self) -> &'static str {
        "MiniMax CN"
    }

    /// The key the fetch reads. A key for one platform is refused by the other,
    /// so the two are separate credentials and each is asked about separately.
    fn is_configured(&self) -> bool {
        super::provider_key(self.id()).is_some()
    }

    fn fetch(&self, ctx: Arc<Ctx>) -> FetchFuture {
        Box::pin(async move { fetch_inner(ctx, "minimax-cn", "MiniMax CN", MAINLAND_HOST).await })
    }
}

enum Attempt {
    Success(ProviderUsage),
    NotFound,
    Failed(String),
}

async fn fetch_inner(ctx: Arc<Ctx>, id: &'static str, name: &'static str, host: &str) -> ProviderUsage {
    let key = match super::provider_key(id) {
        Some(key) => key,
        None => return ProviderUsage::failed(id, name, super::missing_key(id, "")),
    };

    // **Both paths are tried on any failure**, which is what the reference
    // does — an account on the older plan answers 401 on the current path, not
    // 404, so stopping at the first refusal would strand it.
    //
    // The **first** reason is kept, not the last: a refusal from the current
    // path is what the user can act on, and it should not be masked by a server
    // error from a path their plan does not use.
    let mut first_problem: Option<String> = None;

    for path in PATHS {
        match attempt(&ctx, id, name, &format!("{host}{path}"), &key).await {
            Attempt::Success(usage) => return usage,
            Attempt::NotFound => continue,
            Attempt::Failed(reason) => first_problem = first_problem.or(Some(reason)),
        }
    }

    ProviderUsage::failed(
        id,
        name,
        first_problem.unwrap_or_else(|| "the service could not be reached".to_string()),
    )
}

async fn attempt(
    ctx: &Ctx,
    id: &'static str,
    name: &'static str,
    url: &str,
    key: &str,
) -> Attempt {
    let response = ctx
        .client
        .get(url)
        .header("Authorization", format!("Bearer {key}"))
        .header("Accept", "application/json")
        .send()
        .await;

    let response = match response {
        Ok(r) => r,
        Err(e) => return Attempt::Failed(format!("request failed: {}", describe_reqwest_error(&e))),
    };

    let status = response.status();
    let body = match response.text().await {
        Ok(t) => t,
        Err(e) => return Attempt::Failed(format!("cannot read body: {e}")),
    };

    if !status.is_success() {
        return match status.as_u16() {
            404 => Attempt::NotFound,
            401 | 403 => Attempt::Failed("the API key was refused".to_string()),
            429 => Attempt::Failed("rate limited, try again shortly".to_string()),
            _ => Attempt::Failed(super::http_failure(status, &[])),
        };
    }

    let json: Value = match serde_json::from_str(&body) {
        Ok(v) => v,
        Err(e) => return Attempt::Failed(format!("bad JSON: {e}")),
    };

    super::claude_code::debug_dump(id, &json);

    // The service's own verdict, which is not the HTTP status: a refused key
    // comes back as a perfectly good 200 with a non-zero status here.
    if let Some(verdict) = status_verdict(json.get("base_resp")) {
        return Attempt::Failed(verdict);
    }

    let payload = payload_of(&json);
    let windows = windows_from(&payload);
    if windows.is_empty() {
        return Attempt::Failed("no quota windows in response".to_string());
    }

    let plan = first_string(
        &payload,
        &[
            "current_subscribe_title",
            "plan_name",
            "combo_title",
            "current_plan_title",
        ],
    );

    Attempt::Success(ProviderUsage::ok(id, name, windows).with_plan(plan))
}

/// **Not every non-zero status is a bad key**, and saying so sends the user to
/// check a credential that is fine. 1004 is the credential one; the rest are
/// the service having a bad day. Returns none when the service reports no
/// problem at all.
fn status_verdict(base_resp: Option<&Value>) -> Option<String> {
    let status = dig_number(base_resp.and_then(|b| b.get("status_code"))).unwrap_or(0.0);
    if status == 0.0 {
        return None;
    }

    let said = base_resp
        .and_then(|b| b.get("status_msg"))
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_lowercase();

    let credential = status == 1004.0
        || ["token", "auth", "login", "cookie", "credential"]
            .iter()
            .any(|word| said.contains(word));

    Some(if credential {
        "the API key was refused".to_string()
    } else {
        "the service returned an error".to_string()
    })
}

/// **The payload is not always wrapped.** Some replies put `data` at the root
/// instead, and the reference decoder takes either — most of its own captured
/// replies are the unwrapped shape. Reading only `data` reported a perfectly
/// good account as "no limits reported".
fn payload_of(json: &Value) -> Value {
    json.get("data").cloned().unwrap_or_else(|| json.clone())
}

/// Internal so the mapping can be driven against captured replies: the field
/// names are undocumented and the inversion below is the whole feature.
fn windows_from(payload: &Value) -> Vec<UsageWindow> {
    let Some(models) = payload.get("model_remains").and_then(|v| v.as_array()) else {
        return Vec::new();
    };

    let mut rows: Vec<(i64, UsageWindow)> = Vec::new();

    for model in models {
        let name = model
            .get("model_name")
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty());

        // "general" is the plan itself rather than a model, so it is left
        // unscoped — a row reading "5h general" says nothing.
        let scope = name.filter(|n| n.to_lowercase() != "general");

        if let Some(row) = interval(model, scope) {
            rows.push(row);
        }
        if let Some(row) = weekly(model, scope) {
            rows.push(row);
        }
    }

    by_window_length(rows)
}

/// The short window. Its length is measured from the timestamps rather than
/// assumed: they are the only statement of it the reply makes, and a window
/// with no length can be neither named nor sorted — so it is dropped rather
/// than given an invented one.
fn interval(model: &Value, scope: Option<&str>) -> Option<(i64, UsageWindow)> {
    if is_unavailable(
        dig_number(model.get("current_interval_status")),
        dig_number(model.get("current_interval_total_count")),
        dig_number(model.get("current_interval_remaining_percent")),
    ) {
        return None;
    }

    let used = spent(
        dig_number(model.get("current_interval_remaining_percent")),
        dig_number(model.get("current_interval_total_count")),
        dig_number(model.get("current_interval_usage_count")),
    )?;

    let start = dig_number(model.get("start_time"))?;
    let end = dig_number(model.get("end_time"))?;
    if end <= start {
        return None;
    }

    // Sub-second intervals would floor to zero and read as "0-hour limit".
    let seconds = ((end - start) / 1000.0) as i64;
    if seconds <= 0 {
        return None;
    }

    let window = UsageWindow::new(scoped_label(&label_for(seconds), scope), Some(used))
        .with_reset(parse_reset(&serde_json::json!(end)));

    Some((seconds, window))
}

/// The weekly window. Unlike the one above this one names its own length, so it
/// survives a reply that omits the timestamps.
fn weekly(model: &Value, scope: Option<&str>) -> Option<(i64, UsageWindow)> {
    if is_unavailable(
        dig_number(model.get("current_weekly_status")),
        dig_number(model.get("current_weekly_total_count")),
        dig_number(model.get("current_weekly_remaining_percent")),
    ) {
        return None;
    }

    let used = spent(
        dig_number(model.get("current_weekly_remaining_percent")),
        dig_number(model.get("current_weekly_total_count")),
        dig_number(model.get("current_weekly_usage_count")),
    )?;

    let seconds = 7 * 86_400;
    let window = UsageWindow::new(scoped_label("7d", scope), Some(used))
        .with_reset(dig_number(model.get("weekly_end_time")).and_then(|ms| parse_reset(&serde_json::json!(ms))));

    Some((seconds, window))
}

/// A lane the schema has but this subscription does not.
///
/// Status 3 with nothing issued and nothing spent is how the service says "not
/// part of your plan" — a video lane on a plan with no video. Taken at face
/// value it draws a ring pinned at 0% for something the account cannot use at
/// all. The same shape covers a lane that is genuinely unlimited, which has no
/// percentage worth showing either.
fn is_unavailable(status: Option<f64>, total: Option<f64>, remaining_percent: Option<f64>) -> bool {
    status == Some(3.0) && total.unwrap_or(0.0) == 0.0 && remaining_percent.unwrap_or(0.0) >= 100.0
}

/// How much of the lane is gone, 0..100, or none when the reply says nothing
/// usable about it.
///
/// **Everything here is stated as what is *left*.** A percentage of 96 is 4
/// spent — and the counts are the same way round despite their name:
/// `current_interval_usage_count` is the **remaining** quota, not the used one.
/// Reading it as a spend inverts every figure on the card.
///
/// The percentage is preferred because it is what the service intends to be
/// read; the counts are the fallback, and are the only thing the older endpoint
/// returns.
fn spent(remaining_percent: Option<f64>, total: Option<f64>, left: Option<f64>) -> Option<f64> {
    if let Some(remaining_percent) = remaining_percent {
        return Some(percent_from_scale(100.0 - remaining_percent));
    }
    let total = total.filter(|t| *t > 0.0)?;
    let left = left?;
    Some(percent_from_scale(((total - left) / total) * 100.0))
}

fn label_for(seconds: i64) -> String {
    match seconds {
        18_000 => "5h".to_string(),
        604_800 => "7d".to_string(),
        2_592_000 => "Monthly".to_string(),
        other => super::humanize_window_seconds(other),
    }
}

/// The port's labels are one string where the original's are a kind plus a
/// scope, so a scoped window reads "5h MiniMax-M2".
fn scoped_label(base: &str, scope: Option<&str>) -> String {
    match scope {
        Some(scope) => format!("{base} {scope}"),
        None => base.to_string(),
    }
}

/// The first of several names that carries a usable string. Which one a reply
/// uses varies by account, and reading only one leaves the card blank for
/// everybody else.
fn first_string(payload: &Value, keys: &[&str]) -> Option<String> {
    for key in keys {
        if let Some(text) = credentials::dig_str(payload, key) {
            let trimmed = text.trim().to_string();
            if !trimmed.is_empty() {
                return Some(trimmed);
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// A captured reply's shape: the plan's own window beside a per-model one,
    /// percentages reported as what is *left*.
    fn fixture() -> Value {
        json!({
            "base_resp": { "status_code": 0 },
            "model_remains": [
                {
                    "model_name": "general",
                    "current_interval_status": 1,
                    "current_interval_total_count": 100,
                    "current_interval_remaining_percent": 96,
                    "current_interval_usage_count": 96,
                    "start_time": 1780000000000i64,
                    "end_time": 1780018000000i64,
                    "current_weekly_status": 1,
                    "current_weekly_total_count": 1000,
                    "current_weekly_remaining_percent": 40,
                    "weekly_end_time": 1780500000000i64
                },
                {
                    "model_name": "MiniMax-M2",
                    "current_interval_status": 1,
                    "current_interval_total_count": 50,
                    "current_interval_remaining_percent": 10,
                    "start_time": 1780000000000i64,
                    "end_time": 1780018000000i64,
                    "current_weekly_status": 1,
                    "current_weekly_total_count": 500,
                    "current_weekly_remaining_percent": 0
                }
            ]
        })
    }

    #[test]
    fn a_percentage_left_is_inverted_into_what_is_gone() {
        let windows = windows_from(&fixture());
        let general_5h = windows.iter().find(|w| w.label == "5h").unwrap();
        assert_eq!(general_5h.percent_used, Some(4.0));
    }

    #[test]
    fn the_plan_itself_is_left_unscoped_and_a_model_is_not() {
        let labels: Vec<String> = windows_from(&fixture()).into_iter().map(|w| w.label).collect();
        assert!(labels.contains(&"5h".to_string()));
        assert!(labels.contains(&"5h MiniMax-M2".to_string()));
        assert!(labels.contains(&"7d".to_string()));
        assert!(labels.contains(&"7d MiniMax-M2".to_string()));
    }

    #[test]
    fn the_shortest_window_leads() {
        let windows = windows_from(&fixture());
        let labels: Vec<&str> = windows.iter().map(|w| w.label.as_str()).collect();
        assert!(labels[0].starts_with("5h"));
        assert!(labels[1].starts_with("5h"));
        assert!(labels[2].starts_with("7d"));
        assert!(labels[3].starts_with("7d"));
    }

    #[test]
    fn the_interval_length_is_measured_from_the_timestamps() {
        let windows = windows_from(&fixture());
        let general_5h = windows.iter().find(|w| w.label == "5h").unwrap();
        // end - start is 18,000,000 ms, which is five hours — and the reset is
        // the end of that measured window.
        assert!(general_5h.resets_at.is_some());
        assert_eq!(general_5h.label, "5h");
    }

    #[test]
    fn a_lane_the_plan_does_not_include_is_left_out() {
        let payload = json!({
            "model_remains": [ {
                "model_name": "video-01",
                "current_interval_status": 3,
                "current_interval_total_count": 0,
                "current_interval_remaining_percent": 100,
                "start_time": 1780000000000i64,
                "end_time": 1780018000000i64
            } ]
        });
        assert!(windows_from(&payload).is_empty());
    }

    #[test]
    fn the_counts_are_the_fallback_and_they_are_what_is_left_too() {
        let payload = json!({
            "model_remains": [ {
                "model_name": "general",
                "current_interval_total_count": 100,
                "current_interval_usage_count": 25,
                "start_time": 1780000000000i64,
                "end_time": 1780018000000i64
            } ]
        });
        let windows = windows_from(&payload);
        // 25 of 100 left is 75% gone, not 25%.
        assert_eq!(windows[0].percent_used, Some(75.0));
    }

    #[test]
    fn a_sub_second_interval_is_dropped_rather_than_read_as_zero_hours() {
        let payload = json!({
            "model_remains": [ {
                "model_name": "general",
                "current_interval_remaining_percent": 50,
                "start_time": 1780000000000i64,
                "end_time": 1780000000500i64
            } ]
        });
        assert!(windows_from(&payload).is_empty());
    }

    #[test]
    fn the_payload_may_arrive_unwrapped() {
        let wrapped = json!({ "data": { "model_remains": [] } });
        assert!(payload_of(&wrapped).get("model_remains").is_some());
        let plain = json!({ "model_remains": [] });
        assert!(payload_of(&plain).get("model_remains").is_some());
    }

    #[test]
    fn a_bad_key_is_the_services_own_verdict_not_the_http_status() {
        let refused = json!({ "base_resp": { "status_code": 1004, "status_msg": "invalid api key" } });
        assert_eq!(status_verdict(refused.get("base_resp")).as_deref(), Some("the API key was refused"));

        let said = json!({ "base_resp": { "status_code": 2001, "status_msg": "login expired" } });
        assert_eq!(status_verdict(said.get("base_resp")).as_deref(), Some("the API key was refused"));

        // Not every non-zero status is about the credential.
        let broken = json!({ "base_resp": { "status_code": 1002, "status_msg": "rate limit" } });
        assert_eq!(status_verdict(broken.get("base_resp")).as_deref(), Some("the service returned an error"));

        let fine = json!({ "base_resp": { "status_code": 0 } });
        assert!(status_verdict(fine.get("base_resp")).is_none());
    }

    #[test]
    fn the_plan_comes_from_whichever_title_the_account_uses() {
        let payload = json!({ "combo_title": "Coding Plan Pro" });
        assert_eq!(
            first_string(&payload, &["current_subscribe_title", "plan_name", "combo_title"]).as_deref(),
            Some("Coding Plan Pro")
        );
        assert!(first_string(&json!({}), &["plan_name"]).is_none());
    }
}
