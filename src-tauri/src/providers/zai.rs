//! The GLM Coding Plan's limits, from Zhipu's quota endpoint.
//!
//! **Two providers, one service.** Z.ai and BigModel are the same company's
//! international and mainland storefronts, answering the same JSON at the same
//! path on different hosts — but they are separate accounts with separate keys,
//! and a key for one is refused by the other. The original gives each a ring,
//! because someone with only the mainland plan should not have to know that an
//! international one exists to configure their own.
//!
//! `GET {host}/api/monitor/usage/quota/limit`, with the key as a bearer token.
//! Undocumented, like most of the routes here, and it can change without
//! notice.
//!
//! **The reply wraps its payload in a status of its own** — `success` and
//! `code`, both of which have to say 200 even when HTTP did. A refused key
//! comes back as HTTP 200 with `success: false`, so reading only the status
//! line would report an empty plan rather than a bad key.
//!
//! VERIFY ON A REAL MACHINE: both hosts are live services PulseWin cannot test
//! from here. Set `PULSEWIN_DEBUG=1` to dump a payload that fails to parse.

use std::sync::Arc;

use serde_json::Value;

use super::{describe_reqwest_error, percent_from_fraction, by_window_length, Ctx, FetchFuture, Provider};
use crate::credentials;
use crate::model::{parse_reset, ProviderUsage, UsageWindow};

const QUOTA_PATH: &str = "/api/monitor/usage/quota/limit";

/// The two hosts. `api.z.ai` is the international one; mainland accounts live
/// on BigModel and are not reachable there.
const INTERNATIONAL_HOST: &str = "https://api.z.ai";
const MAINLAND_HOST: &str = "https://open.bigmodel.cn";

/// Z.ai, the international storefront.
pub struct Zai;

/// Zhipu (the original's `.glmCoding`), the mainland storefront. Its id is
/// kebab-cased where the original's raw value is `glmCoding`; nothing on disk
/// keys off it here, and the account it names is a different one from Z.ai's.
pub struct Zhipu;

impl Provider for Zai {
    fn id(&self) -> &'static str {
        "zai"
    }

    fn name(&self) -> &'static str {
        "z.ai"
    }

    /// The key for the international storefront only. The mainland tool's file
    /// is **deliberately not consulted**: it is a different account, and
    /// counting it as a way in would draw a card for a plan this machine does
    /// not have — the same reason the fetch never sends that key here.
    fn is_configured(&self) -> bool {
        super::provider_key(self.id()).is_some()
    }

    fn fetch(&self, ctx: Arc<Ctx>) -> FetchFuture {
        Box::pin(async move {
            fetch_inner(ctx, "zai", "z.ai", INTERNATIONAL_HOST, false).await
        })
    }
}

impl Provider for Zhipu {
    fn id(&self) -> &'static str {
        "glm-coding"
    }

    fn name(&self) -> &'static str {
        "Zhipu"
    }

    /// Either key the fetch accepts: one entered for this provider, or the one
    /// a mainland tool has already written to `~/.coding-relay/glm-api-key`.
    fn is_configured(&self) -> bool {
        super::provider_key(self.id()).is_some() || stored_mainland_key().is_some()
    }

    fn fetch(&self, ctx: Arc<Ctx>) -> FetchFuture {
        Box::pin(async move {
            fetch_inner(ctx, "glm-coding", "Zhipu", MAINLAND_HOST, true).await
        })
    }
}

/// A key already sitting on this machine, for the mainland plan only.
///
/// The relay and console tools that set GLM up write the key to a one-line
/// file, so anyone already using it configures nothing. **Never consulted for
/// the international route**: they are separate accounts, and quietly sending a
/// BigModel key to `api.z.ai` would report a refused key for a plan the user
/// does not have.
///
/// Only the first readable line is taken, and it is taken **carefully**. The
/// original's note is worth keeping: `split(separator: "\n")` does not cut a
/// CRLF file at all in Swift, so a file written on Windows yielded the whole
/// thing as the "key" — and a header value containing a newline is silently
/// discarded, so the request went out with no `Authorization` at all, came back
/// 401, and was reported as a refused key about a key that was correct. Rust's
/// `lines()` splits CRLF properly, which is the behaviour the original wanted.
fn stored_mainland_key() -> Option<String> {
    let candidates: &[&[&str]] = &[
        &[".coding-relay", "glm-api-key"],
        &[".config", "bigmodel", "api_key"],
        &[".config", "zhipu", "api_key"],
    ];

    for parts in candidates {
        let path = credentials::home_relative(parts).into_iter().next()?;
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue;
        };
        let key = text.lines().next().unwrap_or("").trim().to_string();
        if !key.is_empty() {
            return Some(key);
        }
    }
    None
}

async fn fetch_inner(
    ctx: Arc<Ctx>,
    id: &'static str,
    name: &'static str,
    host: &str,
    mainland: bool,
) -> ProviderUsage {
    // What the user entered wins, so a stale file cannot quietly override a
    // deliberate choice — the same order OpenCode Go's two sources take.
    let key = match super::provider_key(id) {
        Some(key) => key,
        None if mainland => match stored_mainland_key() {
            Some(key) => key,
            None => {
                return ProviderUsage::failed(
                    id,
                    name,
                    super::missing_key(
                        id,
                        ", or sign in with a mainland tool that writes ~/.coding-relay/glm-api-key",
                    ),
                )
            }
        },
        None => return ProviderUsage::failed(id, name, super::missing_key(id, "")),
    };

    let url = format!("{host}{QUOTA_PATH}");
    let response = ctx
        .client
        .get(&url)
        .header("Authorization", format!("Bearer {key}"))
        .header("Accept", "application/json")
        .send()
        .await;

    let response = match response {
        Ok(r) => r,
        Err(e) => {
            return ProviderUsage::failed(
                id,
                name,
                format!("request failed: {}", describe_reqwest_error(&e)),
            )
        }
    };

    let status = response.status();
    let body = match response.text().await {
        Ok(t) => t,
        Err(e) => return ProviderUsage::failed(id, name, format!("cannot read body: {e}")),
    };

    if !status.is_success() {
        let hints: &[(u16, &str)] = &[
            (401, " — the API key was refused"),
            (403, " — the API key was refused"),
            (429, " — rate limited, try again shortly"),
        ];
        return ProviderUsage::failed(id, name, super::http_failure(status, hints));
    }

    let json: Value = match serde_json::from_str(&body) {
        Ok(v) => v,
        Err(e) => return ProviderUsage::failed(id, name, format!("bad JSON: {e}")),
    };

    super::claude_code::debug_dump(id, &json);

    // The envelope's own verdict. A key the service refuses arrives here as a
    // perfectly good HTTP 200, so this is the only place it can be seen.
    let success = json.get("success").and_then(|v| v.as_bool());
    let code = json.get("code").and_then(|v| v.as_i64());
    if success != Some(true) || code != Some(200) {
        return ProviderUsage::failed(id, name, problem(&json).message());
    }

    let limits = json
        .get("data")
        .and_then(|d| d.get("limits"))
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();

    let windows = windows_from(&limits);
    if windows.is_empty() {
        return ProviderUsage::failed(id, name, "no quota windows in response");
    }

    ProviderUsage::ok(id, name, windows).with_plan(plan_label(&json))
}

/// What the envelope's refusal actually was.
///
/// **The keywords used to be English only, and the mainland host answers in
/// Chinese**, so nothing ever matched and everything fell through to the code —
/// which only knew HTTP's numbers. Measured against the live endpoint: a key of
/// the wrong shape gets `401 令牌已过期或验证不正确`, a well-formed key that the
/// host does not recognise gets `1000 身份验证失败。`, and a missing header gets
/// `1001`. Only the first was mapped, so the common case — a key from the
/// *other* region, which is the right shape and the wrong account — was
/// reported as "the service returned an error" and sent people looking for an
/// outage.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Problem {
    /// The key works and the account has no running coding plan.
    NoCodingPlan,
    KeyRefused,
    RateLimited,
    ServerError,
}

impl Problem {
    fn message(self) -> String {
        match self {
            // The original has a case of its own for this, because the remedy
            // is to buy a plan rather than to check a key.
            Problem::NoCodingPlan => {
                "this account has no coding plan — the key works, the subscription does not".to_string()
            }
            Problem::KeyRefused => "the API key was refused".to_string(),
            Problem::RateLimited => "rate limited, try again shortly".to_string(),
            Problem::ServerError => "the service returned an error".to_string(),
        }
    }
}

fn problem(json: &Value) -> Problem {
    let said = json
        .get("msg")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_lowercase();

    // **Checked before anything else, because the code is useless here.** A
    // working key on an account with no running subscription answers `500` —
    // the vendor's generic number — with this sentence, and 500 alone would say
    // the service broke. The phrase is embedded in English on both hosts.
    if said.contains("coding plan") {
        return Problem::NoCodingPlan;
    }

    // Matched as text because the code list cannot be complete: this is one
    // vendor's private numbering, and it is not published in full. The Chinese
    // words are the same sentences the mainland host sends.
    const AUTH_WORDS: &[&str] = &[
        "token",
        "auth",
        "key",
        "unauthor",
        "forbidden",
        "credential",
        "身份验证",
        "鉴权",
        "认证",
        "令牌",
        "未授权",
        "无权限",
        "密钥",
    ];
    if AUTH_WORDS.iter().any(|word| said.contains(word)) {
        return Problem::KeyRefused;
    }

    match json.get("code").and_then(|v| v.as_i64()).unwrap_or(0) {
        // HTTP's numbers, which this envelope also uses.
        401 | 403 => Problem::KeyRefused,
        429 => Problem::RateLimited,
        // Zhipu's own 1000-series is authentication. 1000 and 1001 are
        // measured; the rest of the band is documented as the same family and
        // sending somebody to check their key is the better mistake.
        1000..=1099 => Problem::KeyRefused,
        _ => Problem::ServerError,
    }
}

/// The plan's name, under whichever of five keys this account's tier happens to
/// use. Passed through verbatim when unfamiliar — an unknown name still beats
/// no name.
fn plan_label(json: &Value) -> Option<String> {
    let data = json.get("data")?;
    credentials::dig_first_str(
        data,
        &["planName", "plan", "plan_type", "packageName", "level"],
    )
}

/// Shortest first, so a five-hour limit is read before a weekly one.
fn windows_from(limits: &[Value]) -> Vec<UsageWindow> {
    let rows = limits
        .iter()
        .enumerate()
        .filter_map(|(index, limit)| window_from(limit, index))
        .collect();
    by_window_length(rows)
}

fn window_from(limit: &Value, index: usize) -> Option<(i64, UsageWindow)> {
    // Only these three carry a quota. Anything else the service starts
    // reporting is left out rather than shown under a heading guessed at.
    let kind_code = credentials::dig_str(limit, "type")?;
    if !["TOKENS_LIMIT", "CREDIT_LIMIT", "TIME_LIMIT"].contains(&kind_code.as_str()) {
        return None;
    }

    let unit = credentials::dig_f64(limit, "unit")? as i64;
    let number = credentials::dig_f64(limit, "number")? as i64;

    let minutes = minutes_for(unit, number, &kind_code)?;
    let used = used_percent(limit)?;

    // The position is kept out of the label — the port has no window id — but
    // the original needs it for exactly the reason two limits can share a type
    // and a duration: without it, the rows are indistinguishable.
    let label = label_for(minutes, &kind_code);

    let seconds = minutes * 60;
    let window = UsageWindow::new(label, Some(used))
        .with_reset(credentials::dig_f64(limit, "nextResetTime").and_then(|ms| {
            parse_reset(&serde_json::json!(ms))
        }))
        .with_detail(Some(format!("limit {}", index + 1)));

    Some((seconds, window))
}

/// How long the window runs.
///
/// The reply states a `unit` code and a `number` of them. An unrecognised unit
/// means the length cannot be read, and a window with no length can be neither
/// named nor sorted — so it is dropped rather than given an invented one.
fn minutes_for(unit: i64, number: i64, kind_code: &str) -> Option<i64> {
    // A monthly MCP allowance is reported as "1 minute", which is a marker
    // rather than a duration — taken literally it would sort above a five-hour
    // limit and claim to reset every minute.
    if kind_code == "TIME_LIMIT" && unit == 5 && number == 1 {
        return Some(30 * 24 * 60);
    }

    if number <= 0 {
        return None;
    }

    let multiplier = match unit {
        1 => 1440,
        3 => 60,
        5 => 1,
        6 => 10080,
        _ => return None,
    };
    Some(number * multiplier)
}

/// The window's heading, plus the MCP scope where the limit is that lane: a
/// different allowance from the coding quota, and saying so is the only way two
/// rows of the same length tell apart.
fn label_for(minutes: i64, kind_code: &str) -> String {
    let base = match minutes {
        300 => "5h".to_string(),
        10080 => "7d".to_string(),
        43200 => "Monthly".to_string(),
        other => humanize_minutes(other),
    };
    if kind_code == "TIME_LIMIT" {
        format!("{base} MCP")
    } else {
        base
    }
}

fn humanize_minutes(minutes: i64) -> String {
    if minutes % (24 * 60) == 0 {
        format!("{}d", minutes / (24 * 60))
    } else if minutes % 60 == 0 {
        format!("{}h", minutes / 60)
    } else {
        format!("{minutes}m")
    }
}

/// How much of the limit is gone, 0..100.
///
/// `percentage` is what the service intends to be read, but it is a whole
/// number — so a plan whose counts are also given is worked out from those
/// instead, which is finer. `remaining` is what is *left*, so the spend is the
/// difference; `currentValue` is the spend directly and wins when both are
/// present, since it is the one the service is counting up.
fn used_percent(limit: &Value) -> Option<f64> {
    let usage = credentials::dig_f64(limit, "usage");
    if let Some(usage) = usage.filter(|u| *u > 0.0) {
        let remaining = credentials::dig_f64(limit, "remaining");
        let current = credentials::dig_f64(limit, "currentValue");

        let used = match (remaining, current) {
            (Some(remaining), current) => (usage - remaining).max(current.unwrap_or(usage - remaining)),
            (None, Some(current)) => current,
            (None, None) => return None,
        };
        return Some(percent_from_fraction((used.min(usage)) / usage));
    }

    // **None rather than zero.** A limit that arrives with no figure at all is
    // not a limit at 0% — it is a limit whose reading is missing, and drawing a
    // full green ring for an account that may be out of quota is the one thing
    // this app is not allowed to do.
    let percentage = credentials::dig_f64(limit, "percentage")?;
    Some(percentage.clamp(0.0, 100.0))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// A captured reply's shape: the envelope, and the three limit kinds the
    /// service actually sends. The unit codes are the vendor's — 3 is hours,
    /// 5 is minutes, 6 is weeks — so a five-hour limit is `unit: 3, number: 5`.
    fn fixture() -> Value {
        json!({
            "success": true,
            "code": 200,
            "msg": "ok",
            "data": {
                "planName": "GLM Coding Pro",
                "limits": [
                    { "type": "TOKENS_LIMIT", "unit": 3, "number": 5,
                      "percentage": 12.0, "nextResetTime": 1790000000000i64 },
                    { "type": "TOKENS_LIMIT", "unit": 6, "number": 1,
                      "percentage": 64.0 },
                    { "type": "TIME_LIMIT", "unit": 5, "number": 1,
                      "percentage": 3.0 }
                ]
            }
        })
    }

    fn limits_of(payload: &Value) -> Vec<Value> {
        payload["data"]["limits"].as_array().cloned().unwrap_or_default()
    }

    #[test]
    fn names_the_windows_the_service_states() {
        let windows = windows_from(&limits_of(&fixture()));
        let labels: Vec<&str> = windows.iter().map(|w| w.label.as_str()).collect();
        assert_eq!(labels, vec!["5h", "7d", "Monthly MCP"]);
        assert_eq!(windows[0].percent_used, Some(12.0));
        assert_eq!(windows[1].percent_used, Some(64.0));
        assert_eq!(windows[2].percent_used, Some(3.0));
    }

    #[test]
    fn the_reset_is_epoch_milliseconds() {
        let windows = windows_from(&limits_of(&fixture()));
        // 1790000000000 ms is 2026-09-21T14:13:20Z.
        assert_eq!(windows[0].resets_at.as_deref(), Some("2026-09-21T14:13:20Z"));
        assert!(windows[1].resets_at.is_none());
    }

    #[test]
    fn counts_are_finer_than_the_whole_percentage() {
        let limits = vec![json!({
            "type": "TOKENS_LIMIT", "unit": 3, "number": 5,
            "percentage": 12.0, "usage": 1000.0, "remaining": 875.0
        })];
        // 125 of 1000 gone is 12.5%, which the whole number could not say.
        assert_eq!(windows_from(&limits)[0].percent_used, Some(12.5));
    }

    #[test]
    fn a_measured_spend_wins_over_the_remainder() {
        let limits = vec![json!({
            "type": "TOKENS_LIMIT", "unit": 3, "number": 5,
            "usage": 100.0, "remaining": 90.0, "currentValue": 40.0
        })];
        assert_eq!(windows_from(&limits)[0].percent_used, Some(40.0));
    }

    #[test]
    fn a_limit_with_no_figure_is_left_unread() {
        let limits = vec![json!({ "type": "CREDIT_LIMIT", "unit": 1, "number": 1 })];
        assert!(windows_from(&limits).is_empty());
    }

    #[test]
    fn an_unknown_unit_is_dropped_rather_than_named() {
        let limits = vec![json!({
            "type": "TOKENS_LIMIT", "unit": 99, "number": 3, "percentage": 5.0
        })];
        assert!(windows_from(&limits).is_empty());
    }

    #[test]
    fn an_unknown_type_is_left_out() {
        let limits = vec![json!({
            "type": "SOMETHING_LIMIT", "unit": 3, "number": 300, "percentage": 5.0
        })];
        assert!(windows_from(&limits).is_empty());
    }

    #[test]
    fn the_plan_comes_from_whichever_key_the_tier_uses() {
        assert_eq!(plan_label(&fixture()).as_deref(), Some("GLM Coding Pro"));
        assert_eq!(
            plan_label(&json!({ "data": { "level": "pro" } })).as_deref(),
            Some("pro")
        );
        assert!(plan_label(&json!({ "data": {} })).is_none());
    }

    #[test]
    fn a_refused_key_arrives_as_a_good_http_status() {
        let reply = json!({ "success": false, "code": 1000, "msg": "身份验证失败。" });
        assert_eq!(problem(&reply), Problem::KeyRefused);
        let reply = json!({ "success": false, "code": 401, "msg": "令牌已过期或验证不正确" });
        assert_eq!(problem(&reply), Problem::KeyRefused);
        let reply = json!({ "success": false, "code": 1001, "msg": "" });
        assert_eq!(problem(&reply), Problem::KeyRefused);
    }

    #[test]
    fn no_coding_plan_is_not_a_bad_key() {
        // A working key on an account with no subscription answers 500 with
        // this sentence, and 500 alone would say the service broke.
        let reply = json!({
            "success": false, "code": 500,
            "msg": "There is no coding plan subscription for this account"
        });
        assert_eq!(problem(&reply), Problem::NoCodingPlan);
        assert!(Problem::NoCodingPlan.message().contains("no coding plan"));
    }

    #[test]
    fn an_unrecognised_failure_is_the_services_not_the_keys() {
        let reply = json!({ "success": false, "code": 500, "msg": "internal error" });
        assert_eq!(problem(&reply), Problem::ServerError);
        let reply = json!({ "success": false, "code": 1302, "msg": "too many requests" });
        assert_eq!(problem(&reply), Problem::ServerError);
    }
}
