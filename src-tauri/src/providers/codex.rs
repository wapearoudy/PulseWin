//! OpenAI Codex CLI quota.
//!
//! Auth: reuses `~/.codex/auth.json`, written by `codex login`.
//!
//! VERIFY ON A REAL MACHINE: `chatgpt.com/backend-api/wham/usage` is a private
//! endpoint. Set `PULSEWIN_DEBUG=1` to dump the payload if parsing fails.

use std::sync::Arc;

use serde_json::Value;

use super::{Ctx, FetchFuture, Provider};
use crate::credentials::{self, codex_credential_paths};
use crate::model::{parse_reset, ProviderUsage, ResetCredits, UsageWindow};

const USAGE_URL: &str = "https://chatgpt.com/backend-api/wham/usage";

const TOKEN_PATHS: &[&str] = &[
    "tokens.access_token",
    "access_token",
    "OPENAI_API_KEY",
    "api_key",
];

const ACCOUNT_ID_PATHS: &[&str] = &["tokens.account_id", "account_id"];

pub struct Codex;

impl Provider for Codex {
    fn id(&self) -> &'static str {
        "codex"
    }

    fn name(&self) -> &'static str {
        "Codex"
    }

    /// The login the fetch reads: an env token, or `~/.codex/auth.json` as
    /// `codex login` last wrote it.
    fn is_configured(&self) -> bool {
        credentials::env_override(self.id(), "token").is_some()
            || credentials::first_existing(&codex_credential_paths()).is_some()
    }

    fn fetch(&self, ctx: Arc<Ctx>) -> FetchFuture {
        Box::pin(async move { fetch_inner(ctx, None).await })
    }
}

pub(crate) async fn fetch_inner(ctx: Arc<Ctx>, explicit: Option<serde_json::Value>) -> ProviderUsage {
    const ID: &str = "codex";
    const NAME: &str = "Codex";

    let paths = codex_credential_paths();
    let resolved=if let Some(document)=&explicit {crate::accounts::token(document).map(|token|(token,std::path::PathBuf::from("<account>"))).ok_or_else(||"账号凭据无效".to_string())} else {credentials::resolve_token(ID, &paths, TOKEN_PATHS)};
    let (token, source) = match resolved {
        Ok(v) => v,
        Err(e) => return ProviderUsage::failed(ID, NAME, e),
    };

    // The account id is optional; some builds omit it.
    let account_id = if let Some(document)=&explicit {credentials::dig_first_str(document,&["accountId","tokens.account_id","account_id"])} else {credentials::read_json(&source).and_then(|j| credentials::dig_first_str(&j, &["accountId",ACCOUNT_ID_PATHS[0],ACCOUNT_ID_PATHS[1]]))};

    let mut request = ctx
        .client
        .get(USAGE_URL)
        .header("Authorization", format!("Bearer {token}"))
        .header("Accept", "application/json")
        .header("OpenAI-Beta", "codex-1");

    if let Some(account_id) = account_id.filter(|a| !a.is_empty()) {
        request = request.header("chatgpt-account-id", account_id);
    }

    let response = match request.send().await {
        Ok(r) => r,
        Err(e) => return ProviderUsage::failed(ID, NAME, format!("request failed: {}", super::describe_reqwest_error(&e))),
    };

    let status = response.status();
    let body = match response.text().await {
        Ok(t) => t,
        Err(e) => return ProviderUsage::failed(ID, NAME, format!("cannot read body: {e}")),
    };

    if !status.is_success() {
        let hint = match status.as_u16() {
            401 | 403 => " — run `codex login` to refresh",
            404 => " — usage endpoint not available for this account type",
            _ => "",
        };
        return ProviderUsage::failed(ID, NAME, format!("HTTP {status}{hint}"));
    }

    let json: Value = match serde_json::from_str(&body) {
        Ok(v) => v,
        Err(e) => return ProviderUsage::failed(ID, NAME, format!("bad JSON: {e}")),
    };

    super::claude_code::debug_dump(ID, &json);

    let windows = extract_windows(&json);
    if windows.is_empty() {
        return ProviderUsage::failed(ID, NAME, "no quota windows in response");
    }

    let plan = credentials::dig_first_str(&json, &["plan_type", "plan", "subscription_plan"]);

    let mut usage = ProviderUsage::ok(ID, NAME, windows)
        .with_plan(plan)
        .with_account(Some(source_label(&source)));
    if explicit.is_none() && same_cli_login(&source) && reset_credits_enabled(&crate::preferences::load()) {
        usage.reset_credits = read_reset_credits().await;
    }
    usage
}

pub fn reset_credits_enabled(prefs: &crate::preferences::Preferences) -> bool {
    prefs.enabled_providers.iter().any(|id| id == "codex")
        && prefs.account_appearance.get("codex").and_then(|appearance| appearance.card_parts.as_ref())
            .is_some_and(|parts| parts.iter().any(|part| part == "resetCredits"))
}

fn same_cli_login(source: &std::path::Path) -> bool {
    // A token pasted into PulseWin or set in the environment may correspond
    // to another account. Never attach the CLI's credits to that reading.
    let home = std::env::var_os("CODEX_HOME").map(std::path::PathBuf::from)
        .or_else(|| credentials::home_dir().map(|home| home.join(".codex")));
    home.is_some_and(|home| home.join("auth.json") == source)
}

async fn read_reset_credits() -> Option<ResetCredits> {
    if std::env::args().any(|arg| arg == "--native-smoke") { return None; }
    let binary = crate::window_starter::locate("codex", &[])?;
    let folder = crate::settings::directory()?.join("codex-usage");
    std::fs::create_dir_all(&folder).ok()?;
    let reply = crate::window_starter::codex_read_rpc(
        &binary, &folder, "account/rateLimits/read", None, std::time::Duration::from_secs(5),
    ).await?;
    parse_reset_credits(reply.get("result")?)
}

fn parse_reset_credits(value: &Value) -> Option<ResetCredits> {
    let block = value.get("rateLimitResetCredits")?.as_object()?;
    let listed = block.get("credits").and_then(Value::as_array);
    let active = || listed.into_iter().flatten().filter(|entry| entry["status"] == "available");
    let available = block.get("availableCount").and_then(Value::as_u64)
        .or_else(|| listed.map(|_| active().count() as u64))?;
    let next_expiry = active().filter_map(|entry| entry.get("expiresAt").and_then(parse_reset)).min();
    Some(ResetCredits { available, next_expiry })
}

fn source_label(source: &std::path::Path) -> String {
    if source.to_string_lossy().starts_with("<env:") {
        "env token".to_string()
    } else if source.to_string_lossy()=="<account>" {
        "独立登录".to_string()
    } else {
        "CLI login".to_string()
    }
}

fn extract_windows(json: &Value) -> Vec<UsageWindow> {
    let root = json.get("rate_limit").unwrap_or(json);
    let mut out = Vec::new();

    for key in ["primary_window", "secondary_window"] {
        let Some(node) = root.get(key) else { continue };
        if node.is_null() {
            continue;
        }

        let used = credentials::dig_first_f64(
            node,
            &["used_percent", "utilization", "percent_used", "used"],
        );

        let resets_at = node
            .get("reset_at")
            .or_else(|| node.get("resets_at"))
            .and_then(parse_reset);

        let label = credentials::dig_first_f64(node, &["limit_window_seconds"])
            .map(label_for_window)
            .unwrap_or_else(|| key.replace('_', " "));

        let detail = credentials::dig_first_f64(node, &["limit_window_seconds"])
            .map(|secs| format!("window {}", humanize_seconds(secs)));

        if used.is_none() && resets_at.is_none() {
            continue;
        }

        out.push(
            // Codex reports percentage points, including values at or below 1.
            UsageWindow::new(label, used.map(|value| value.clamp(0.0, 100.0)))
                .with_id(Some(key.to_owned()))
                .with_reset(resets_at)
                .with_duration(credentials::dig_first_f64(node, &["limit_window_seconds"]))
                .with_exhausted(node.get("limit_reached").and_then(Value::as_bool).unwrap_or(false))
                .with_detail(detail),
        );
    }

    out
}

fn label_for_window(seconds: f64) -> String {
    let hours = seconds / 3600.0;
    if (hours - 5.0).abs() < 0.5 {
        "5h".to_string()
    } else if (hours - 24.0).abs() < 1.0 {
        "24h".to_string()
    } else if (hours - 168.0).abs() < 2.0 {
        "7d".to_string()
    } else {
        humanize_seconds(seconds)
    }
}

fn humanize_seconds(seconds: f64) -> String {
    let hours = seconds / 3600.0;
    if hours >= 24.0 {
        format!("{}d", (hours / 24.0).round() as i64)
    } else {
        format!("{}h", hours.round() as i64)
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn reset_credits_require_a_reported_count_or_list() {
        use serde_json::json;
        assert!(super::parse_reset_credits(&json!({"rateLimits": {}})).is_none());
        assert!(super::parse_reset_credits(&json!({"rateLimitResetCredits": {}})).is_none());
        let empty = super::parse_reset_credits(&json!({"rateLimitResetCredits":{"availableCount":0}})).unwrap();
        assert_eq!((empty.available, empty.next_expiry), (0, None));
        let reading = super::parse_reset_credits(&json!({"rateLimitResetCredits":{"credits":[
            {"status":"used","expiresAt":1700000000},
            {"status":"available","expiresAt":1900000000},
            {"status":"available","expiresAt":1800000000}
        ]}})).unwrap();
        assert_eq!(reading.available, 2);
        assert_eq!(reading.next_expiry, super::parse_reset(&json!(1800000000)));
    }

    #[test]
    fn small_reported_percentages_are_not_fractions() {
        for used in [0.0, 0.5, 1.0, 37.0, 41.0, 100.0] {
            let windows = super::extract_windows(&serde_json::json!({"rate_limit": {
                "primary_window": {"used_percent": used, "limit_window_seconds": 18000},
                "secondary_window": {"used_percent": used, "limit_window_seconds": 604800}
            }}));
            assert_eq!(windows.len(), 2);
            assert_eq!(windows[0].percent_used, Some(used));
            assert_eq!(windows[1].percent_used, Some(used));
        }
    }
    #[test]
    fn retains_reported_window_duration_and_explicit_restriction() {
        let windows = super::extract_windows(&serde_json::json!({"rate_limit": {
            "primary_window": {"used_percent": 20, "limit_window_seconds": 18000, "limit_reached": true},
            "secondary_window": {"used_percent": 90}
        }}));
        assert_eq!(windows[0].window_seconds, Some(18000.0));
        assert!(windows[0].is_exhausted);
        assert_eq!(windows[1].window_seconds, None);
        assert!(!windows[1].is_exhausted);
    }
    use super::*;
    use serde_json::json;

    #[test]
    fn extracts_primary_and_secondary() {
        let payload = json!({
            "plan_type": "pro",
            "rate_limit": {
                "primary_window": {
                    "used_percent": 12.0,
                    "reset_at": 1790000000,
                    "limit_window_seconds": 18000
                },
                "secondary_window": {
                    "used_percent": 64.0,
                    "limit_window_seconds": 604800
                }
            }
        });
        let windows = extract_windows(&payload);
        assert_eq!(windows.len(), 2);
        assert_eq!(windows[0].label, "5h");
        assert_eq!(windows[0].percent_used, Some(12.0));
        assert!(windows[0].resets_at.is_some());
        assert_eq!(windows[1].label, "7d");
        assert_eq!(windows[1].percent_used, Some(64.0));
    }

    #[test]
    fn tolerates_missing_rate_limit_wrapper() {
        let payload = json!({ "primary_window": { "used_percent": 5.0 } });
        let windows = extract_windows(&payload);
        assert_eq!(windows.len(), 1);
        assert_eq!(windows[0].percent_used, Some(5.0));
    }

    #[test]
    fn skips_null_windows() {
        let payload = json!({ "rate_limit": { "primary_window": null } });
        assert!(extract_windows(&payload).is_empty());
    }

    #[test]
    fn labels_common_windows() {
        assert_eq!(label_for_window(18000.0), "5h");
        assert_eq!(label_for_window(86400.0), "24h");
        assert_eq!(label_for_window(604800.0), "7d");
    }
}
