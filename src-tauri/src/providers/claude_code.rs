//! Claude Code (Anthropic) quota.
//!
//! Auth: reuses the OAuth token the Claude Code CLI already stored at
//! `~/.claude/.credentials.json`, so the user never pastes anything.
//!
//! VERIFY ON A REAL MACHINE: Anthropic's OAuth usage endpoint and its field
//! names are undocumented and change without notice. If the card shows an
//! error, run with `PULSEWIN_DEBUG=1` to dump the raw JSON (see `debug_dump`).

use std::sync::Arc;

use serde_json::Value;

use super::{Ctx, FetchFuture, Provider};
use crate::credentials::{self, claude_credential_paths};
use crate::model::{parse_reset, ProviderUsage, UsageWindow};

const USAGE_URL: &str = "https://api.anthropic.com/api/oauth/usage";
const OAUTH_BETA: &str = "oauth-2025-04-20";

/// Dotted paths that may hold the OAuth access token.
const TOKEN_PATHS: &[&str] = &[
    "claudeAiOauth.accessToken",
    "claudeAiOauth.access_token",
    "accessToken",
    "access_token",
];

pub struct ClaudeCode;

impl Provider for ClaudeCode {
    fn id(&self) -> &'static str {
        "claude-code"
    }

    fn name(&self) -> &'static str {
        "Claude Code"
    }

    /// The login the fetch reads: an env token, or one of the files the Claude
    /// Code CLI writes. A file that exists but holds no usable token still
    /// counts as present — "installed and signed out" is a card the reader
    /// needs, and `login_hint` on the failure is what tells them so.
    fn is_configured(&self) -> bool {
        credentials::env_override(self.id(), "token").is_some()
            || credentials::first_existing(&claude_credential_paths()).is_some()
    }

    fn fetch(&self, ctx: Arc<Ctx>) -> FetchFuture {
        Box::pin(async move { fetch_inner(ctx, None).await })
    }
}

pub(crate) async fn fetch_inner(ctx: Arc<Ctx>, explicit: Option<serde_json::Value>) -> ProviderUsage {
    const ID: &str = "claude-code";
    const NAME: &str = "Claude Code";

    let resolved=if let Some(document)=explicit { crate::accounts::token(&document).map(|token|(token,std::path::PathBuf::from("<account>"))).ok_or_else(||"账号凭据无效".to_string()) } else {credentials::resolve_token(ID, &claude_credential_paths(), TOKEN_PATHS)};
    let (token, source) = match resolved
    {
        Ok(v) => v,
        Err(e) => return ProviderUsage::failed(ID, NAME, format!("{e}{}", login_hint())),
    };

    let response = ctx
        .client
        .get(USAGE_URL)
        .header("Authorization", format!("Bearer {token}"))
        .header("anthropic-beta", OAUTH_BETA)
        .header("Accept", "application/json")
        .send()
        .await;

    let response = match response {
        Ok(r) => r,
        Err(e) => return ProviderUsage::failed(ID, NAME, format!("request failed: {}", super::describe_reqwest_error(&e))),
    };

    let status = response.status();
    let body = match response.text().await {
        Ok(t) => t,
        Err(e) => return ProviderUsage::failed(ID, NAME, format!("cannot read body: {e}")),
    };

    if !status.is_success() {
        // 401 means the CLI token expired and only the CLI can refresh it.
        let hint = match status.as_u16() {
            401 | 403 => " — open Claude Code once to refresh its login",
            429 => " — rate limited, try again shortly",
            _ => "",
        };
        return ProviderUsage::failed(ID, NAME, format!("HTTP {status}{hint}"));
    }

    let json: Value = match serde_json::from_str(&body) {
        Ok(v) => v,
        Err(e) => return ProviderUsage::failed(ID, NAME, format!("bad JSON: {e}")),
    };

    debug_dump(ID, &json);

    let windows = extract_windows(&json);
    if windows.is_empty() {
        return ProviderUsage::failed(ID, NAME, "no quota windows in response");
    }

    let plan = json
        .get("plan")
        .or_else(|| json.get("subscription_type"))
        .and_then(|v| v.as_str())
        .map(str::to_string);

    let plain = if source.starts_with("<env:") {
        "env token".to_string()
    } else if source.to_string_lossy()=="<account>" {
        "独立登录".to_string()
    } else {
        "CLI login".to_string()
    };

    ProviderUsage::ok(ID, NAME, windows)
        .with_plan(plan)
        .with_account(Some(plain))
}

/// Distinguish "Claude Code was never installed" from "installed but signed
/// out" — the two need completely different fixes from the user.
fn login_hint() -> &'static str {
    let installed = credentials::home_relative(&[".claude"])
        .iter()
        .any(|p| p.is_dir())
        || credentials::home_relative(&[".claude.json"])
            .iter()
            .any(|p| p.is_file());

    if installed {
        " — Claude Code is installed but not signed in; run `claude` once to log in"
    } else {
        " — Claude Code does not appear to be installed on this machine"
    }
}

/// Newer responses nest each window under a key; older ones flatten it.
/// Accept both, and ignore windows that report no utilization at all.
fn extract_windows(json: &Value) -> Vec<UsageWindow> {
    const CANDIDATES: &[(&str, &str)] = &[
        ("five_hour", "5h"),
        ("seven_day", "7d"),
        ("seven_day_opus", "7d Opus"),
        ("seven_day_sonnet", "7d Sonnet"),
        ("seven_day_oauth_apps", "7d Apps"),
        ("monthly", "Monthly"),
    ];

    let root = json.get("usage").unwrap_or(json);
    let scoped = root.get("limits").and_then(Value::as_array).into_iter().flatten().filter_map(|limit| {
        let kind = limit.get("kind")?.as_str()?;
        let (label,seconds) = match kind {
            "session" => ("5h",18000.),
            "weekly_all" | "weekly_scoped" => ("7d",604800.),
            _ => return None,
        };
        let percent = credentials::dig_first_f64(limit,&["percent"])?;
        if !percent.is_finite() {return None;}
        let scope = limit.pointer("/scope/model/display_name").and_then(Value::as_str).map(str::to_owned);
        Some(UsageWindow::new(label,Some(percent))
            .with_id(Some(format!("claudeCode.{kind}.{}",scope.as_deref().unwrap_or("all"))))
            .with_scope(scope).with_duration(Some(seconds)).with_exhausted(is_spent(limit))
            .with_reset(limit.get("resets_at").and_then(parse_reset)))
    }).collect::<Vec<_>>();
    if !scoped.is_empty() {return scoped;}
    let mut out = Vec::new();

    for (key, label) in CANDIDATES {
        let Some(node) = root.get(*key) else { continue };
        if node.is_null() {
            continue;
        }

        // This protocol reports percent, including 0.25%. It is not a fraction.
        let used = credentials::dig_first_f64(node, &["utilization", "used_percent", "percent"])
            .filter(|value|value.is_finite());

        let resets_at = node
            .get("resets_at")
            .or_else(|| node.get("resetsAt"))
            .or_else(|| node.get("reset_at"))
            .and_then(parse_reset);

        if used.is_none() && resets_at.is_none() {
            continue;
        }

        let scope = match *key {
            "seven_day_opus" => Some("Opus".to_owned()),
            "seven_day_sonnet" => Some("Sonnet".to_owned()),
            _ => None,
        };
        let seconds = match *key {"five_hour"=>Some(18000.),"seven_day"|"seven_day_opus"|"seven_day_sonnet"|"seven_day_oauth_apps"=>Some(604800.),_=>None};
        out.push(UsageWindow::new(*label, used).with_id(Some(format!("claudeCode.{key}"))).with_scope(scope)
            .with_duration(seconds).with_exhausted(is_spent(node)).with_reset(resets_at));
    }

    out
}

fn is_spent(limit:&Value)->bool {
    if limit.get("locked_reason").is_some_and(|reason|!reason.is_null()) {return true;}
    limit.get("severity").and_then(Value::as_str).is_some_and(|severity|
        !["normal","ok","none","healthy","warning","warn"].contains(&severity.to_ascii_lowercase().as_str()))
}

/// With `PULSEWIN_DEBUG=1`, print the raw payload so a user can report a
/// provider breakage without attaching a debugger.
pub(crate) fn debug_dump(provider: &str, json: &Value) {
    if std::env::var("PULSEWIN_DEBUG").map(|v| v == "1").unwrap_or(false) {
        eprintln!("[PulseWin:{provider}] raw usage payload:");
        eprintln!("{}", serde_json::to_string_pretty(json).unwrap_or_default());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn extracts_five_hour_and_seven_day() {
        let payload = json!({
            "five_hour": { "utilization": 37.5, "resets_at": "2026-10-01T12:00:00Z" },
            "seven_day": { "utilization": 82.0, "resets_at": null }
        });
        let windows = extract_windows(&payload);
        assert_eq!(windows.len(), 2);
        assert_eq!(windows[0].label, "5h");
        assert_eq!(windows[0].percent_used, Some(37.5));
        assert_eq!(windows[0].resets_at.as_deref(), Some("2026-10-01T12:00:00Z"));
        assert_eq!(windows[1].label, "7d");
        assert_eq!(windows[1].percent_used, Some(82.0));
    }

    #[test]
    fn unwraps_nested_usage_object() {
        let payload = json!({ "usage": { "five_hour": { "utilization": 10.0 } } });
        let windows = extract_windows(&payload);
        assert_eq!(windows.len(), 1);
        assert_eq!(windows[0].percent_used, Some(10.0));
    }

    #[test]
    fn skips_null_and_empty_windows() {
        let payload = json!({
            "five_hour": null,
            "seven_day": {},
            "seven_day_opus": { "utilization": 5.0 }
        });
        let windows = extract_windows(&payload);
        assert_eq!(windows.len(), 1);
        assert_eq!(windows[0].label, "7d Opus");
    }

    #[test]
    fn preserves_sub_one_percent_utilization_without_a_hundredfold_increase() {
        let payload = json!({ "five_hour": { "utilization": 0.25 } });
        let windows = extract_windows(&payload);
        assert_eq!(windows[0].percent_used, Some(0.25));
    }

    #[test]
    fn limits_take_precedence_and_preserve_real_model_scope_and_duration() {
        let windows=extract_windows(&json!({"five_hour":{"utilization":99},"limits":[
            {"kind":"session","percent":0.25,"severity":"normal"},
            {"kind":"weekly_scoped","percent":76,"severity":"warning","scope":{"model":{"display_name":"Sonnet"}}}
        ]}));
        assert_eq!(windows.len(),2);assert_eq!(windows[0].percent_used,Some(0.25));
        assert_eq!(windows[0].window_seconds,Some(18000.));
        assert_eq!(windows[1].scope.as_deref(),Some("Sonnet"));
        assert_eq!(windows[1].id.as_deref(),Some("claudeCode.weekly_scoped.Sonnet"));
        assert!(!windows[1].is_exhausted);
    }

    #[test]
    fn warning_is_not_a_block_but_locked_reason_and_unknown_severity_are() {
        for severity in ["normal","ok","none","healthy","warning","warn","WARNING"] {
            assert!(!is_spent(&json!({"severity":severity,"locked_reason":null})));
        }
        assert!(is_spent(&json!({"severity":"unexpected"})));
        assert!(is_spent(&json!({"severity":"normal","locked_reason":"blocked"})));
    }

    #[test]
    fn unknown_or_incomplete_scoped_limits_fall_back_without_inventing_a_window() {
        let windows=extract_windows(&json!({"limits":[{"kind":"future","percent":30},{"kind":"session"}],"five_hour":{"utilization":1}}));
        assert_eq!(windows.len(),1);assert_eq!(windows[0].percent_used,Some(1.));
    }

    #[test]
    fn empty_payload_yields_nothing() {
        assert!(extract_windows(&json!({})).is_empty());
    }
}
