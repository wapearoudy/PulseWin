//! The OpenCode Go plan's limits.
//!
//! Two places the key can come from, in this order:
//!
//! 1. **A key the user entered**, which wins — someone who typed a key meant
//!    that one to be used, otherwise a stale key left behind by OpenCode would
//!    quietly override a deliberate choice.
//! 2. **What OpenCode saved for itself** in `~/.local/share/opencode/auth.json`,
//!    which is the same borrowing Claude Code and Codex get, and means anyone
//!    already signed in there has nothing to configure.
//!
//! The endpoint is `GET /zen/go/v1/usage`, which is not documented — OpenCode's
//! own docs describe only the model endpoints — so it can change without
//! notice, exactly like the two undocumented routes the CLIs use.
//!
//! Subscription entitlement is distinct from authentication. A valid key for
//! another workspace produces an EntitlementError, not a refused-key verdict.

use std::path::PathBuf;
use std::sync::Arc;

use serde_json::Value;

use super::{describe_reqwest_error, percent_from_scale, Ctx, FetchFuture, Provider};
use crate::credentials;
use crate::model::{parse_reset, ProviderUsage, UsageWindow, WindowKind};

const USAGE_URL: &str = "https://opencode.ai/zen/go/v1/usage";

pub struct OpenCodeGo;

impl Provider for OpenCodeGo {
    fn id(&self) -> &'static str {
        "opencode-go"
    }

    fn name(&self) -> &'static str {
        "OpenCode Go"
    }

    /// Either key the fetch accepts: one entered for this provider, or the
    /// login the OpenCode CLI stored for itself.
    fn is_configured(&self) -> bool {
        super::provider_key(self.id()).is_some() || stored_key().is_some()
    }

    fn fetch(&self, ctx: Arc<Ctx>) -> FetchFuture {
        Box::pin(async move { fetch_inner(ctx).await })
    }
}

/// Where OpenCode keeps its own login.
///
/// The original reads `~/.local/share/opencode/auth.json` and nothing else. The
/// two `%APPDATA%`/`%LOCALAPPDATA%` candidates are this port's addition for a
/// Windows-native install of the same tool; the order keeps the original's path
/// first, so a machine with both reads the file the original would have read.
pub(crate) fn auth_paths() -> Vec<PathBuf> {
    auth_paths_from(credentials::home_dir(), credentials::config_dir(), credentials::data_local_dir(), std::env::var_os("XDG_DATA_HOME").map(PathBuf::from))
}

fn auth_paths_from(home: Option<PathBuf>, config: Option<PathBuf>, local: Option<PathBuf>, xdg: Option<PathBuf>) -> Vec<PathBuf> {
    let mut paths = Vec::new();
    // OpenCode honours XDG_DATA_HOME even on Windows. If explicitly set, its
    // data store precedes the default and the Windows compatibility paths.
    if let Some(xdg) = xdg.filter(|p| p.is_absolute()) { paths.push(xdg.join("opencode/auth.json")); }
    if let Some(home) = &home { paths.push(home.join(".local/share/opencode/auth.json")); }
    if let Some(config) = config { paths.push(config.join("opencode/auth.json")); }
    if let Some(home) = home { paths.push(home.join(".config/opencode/auth.json")); }
    if let Some(local) = local { paths.push(local.join("opencode/auth.json")); }
    let mut seen = std::collections::HashSet::new();
    paths.retain(|p| seen.insert(p.clone()));
    paths
}

struct ResolvedKey { key: String, source: String }

fn resolve_key() -> Option<ResolvedKey> {
    if let Some(key) = credentials::env_override("opencode-go", "key") {
        return Some(ResolvedKey { key, source: "环境变量：OpenCode Go API key".into() });
    }
    if let Some(key) = super::provider_key("opencode-go") {
        return Some(ResolvedKey { key: key.trim().into(), source: "PulseWin 设置中保存的 API key".into() });
    }
    stored_key()
}

fn key_from_auth(json: &Value) -> Option<String> {
    let entry = json.get("opencode-go")?;
    if entry.get("type").and_then(Value::as_str).is_some_and(|kind| kind != "api") { return None; }
    entry.get("key")?.as_str().map(str::trim).filter(|key| !key.is_empty()).map(str::to_owned)
}

async fn fetch_inner(ctx: Arc<Ctx>) -> ProviderUsage {
    const ID: &str = "opencode-go";
    const NAME: &str = "OpenCode Go";

    let credential = match resolve_key() {
        Some(key) => key,
        None => {
            return ProviderUsage::failed(
                ID,
                NAME,
                super::missing_key(
                    ID,
                    ", or sign in with the OpenCode CLI so ~/.local/share/opencode/auth.json exists",
                ),
            )
        }
    };

    let response = ctx
        .gateway_client
        .get(USAGE_URL)
        .bearer_auth(&credential.key)
        .header("Accept", "application/json")
        .send()
        .await;

    let response = match response {
        Ok(r) => r,
        Err(e) => {
            let mut usage = ProviderUsage::failed(
                ID,
                NAME,
                format!("request failed: {}", describe_reqwest_error(&e)),
            );
            usage.source=Some(credential.source);return usage;
        }
    };

    let status = response.status();
    let body = match response.text().await {
        Ok(t) => t,
        Err(e) => {let mut usage=ProviderUsage::failed(ID, NAME, format!("cannot read body: {e}"));usage.source=Some(credential.source);return usage;},
    };

    let mut usage = response_usage(status, &body);
    usage.source = Some(credential.source);
    usage
}

fn response_usage(status: reqwest::StatusCode, body: &str) -> ProviderUsage {
    const ID: &str = "opencode-go";
    const NAME: &str = "OpenCode Go";
    if !status.is_success() {
        let failure = serde_json::from_str::<Value>(body).ok();
        if status.as_u16() == 403 && failure.as_ref().and_then(|v| v.pointer("/error/type")).and_then(Value::as_str) == Some("EntitlementError") {
            return ProviderUsage::failed(ID, NAME, "当前密钥所属账号或工作区未检测到 OpenCode Go 订阅。请核对订阅账号，在 OpenCode 控制台复制该账号的 API key，并在连接设置中保存后重新检查。");
        }
        let hints: &[(u16, &str)] = &[
            (401, " — API key 无效或已失效，请在连接设置中检查"),
            (403, " — 服务拒绝访问，请检查账号权限或网络；不表示用量为零"),
            (429, " — 请求过于频繁，请稍后重试"),
        ];
        return ProviderUsage::failed(ID, NAME, super::http_failure(status, hints));
    }

    let json: Value = match serde_json::from_str(body) {
        Ok(v) => v,
        Err(e) => return ProviderUsage::failed(ID, NAME, format!("bad JSON: {e}")),
    };

    super::claude_code::debug_dump(ID, &json);

    let windows = windows_from(&json.get("usage").cloned().unwrap_or(Value::Null));
    if windows.is_empty() {
        return ProviderUsage::failed(ID, NAME, "no quota windows in response");
    }

    // The reply carries limits and nothing else — no plan name, no balance —
    // so neither is invented here.
    ProviderUsage::ok(ID, NAME, windows)
}

/// The key `opencode` wrote when it signed in.
fn stored_key() -> Option<ResolvedKey> {
    for path in auth_paths() {
        let Some(json) = credentials::read_json(&path) else {
            continue;
        };
        if let Some(key) = key_from_auth(&json) {
            return Some(ResolvedKey { key, source: format!("OpenCode CLI：{}", path.display()) });
        }
    }
    None
}

/// Shortest window first, which is the order the other providers' limits arrive
/// in and the order they matter in — the one about to bite leads.
fn windows_from(usage: &Value) -> Vec<UsageWindow> {
    // The reply calls the short window "rolling" and never says how long it
    // runs, but it is the five-hour one — measured, the reset it reports lands
    // five hours out. So it is named as such rather than by the key it arrives
    // under.
    [
        ("rolling", "5h", 5 * 3_600),
        ("weekly", "7d", 7 * 86_400),
        ("monthly", "Monthly", 30 * 86_400),
    ]
    .into_iter()
    .filter_map(|(key, label, seconds)| window_from(usage.get(key), key, label, seconds))
    .collect()
}

fn window_from(reported: Option<&Value>, id: &str, label: &str, seconds: i64) -> Option<UsageWindow> {
    let reported = reported?;
    let percent = reported.get("percent").and_then(|v| v.as_f64())?;

    // The provider's own verdict, not one inferred from the percentage.
    // Anything other than "ok" is treated as spent — erring towards "you're
    // blocked" is the safer way to be wrong. Keep the percentage and verdict
    // separate, just as the original does for a blocked low-percentage window.
    let status = reported
        .get("status")
        .and_then(|v| v.as_str())
        .unwrap_or("ok")
        .to_string();

    let detail = if status.to_lowercase() != "ok" {
        Some(status)
    } else {
        None
    };

    Some(
        UsageWindow::new(label, Some(percent_from_scale(percent)))
            .with_id(Some(id.into()))
            .with_kind(WindowKind::Limit)
            .with_duration(Some(seconds as f64))
            .with_exhausted(detail.is_some())
            .with_reset(reported.get("resetsAt").and_then(parse_reset))
            .with_detail(detail),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// A captured reply's shape: three windows, the percentage already what is
    /// *gone*.
    fn fixture() -> Value {
        json!({
            "usage": {
                "rolling": { "status": "ok", "percent": 12.5,
                             "resetsAt": "2026-10-01T17:00:00.000Z" },
                "weekly": { "status": "ok", "percent": 64.0 },
                "monthly": { "status": "limit_reached", "percent": 100.0 }
            }
        })
    }

    /// The windows live under `usage`, which is what the provider reads.
    fn usage_of(payload: &Value) -> Value {
        payload.get("usage").cloned().unwrap_or(Value::Null)
    }

    #[test]
    fn reads_the_three_windows_shortest_first() {
        let windows = windows_from(&usage_of(&fixture()));
        let labels: Vec<&str> = windows.iter().map(|w| w.label.as_str()).collect();
        assert_eq!(labels, vec!["5h", "7d", "Monthly"]);
        assert_eq!(windows[0].percent_used, Some(12.5));
        assert_eq!(windows[1].percent_used, Some(64.0));
        assert_eq!(windows[2].percent_used, Some(100.0));
    }

    #[test]
    fn a_millisecond_reset_stamp_still_parses() {
        let windows = windows_from(&usage_of(&fixture()));
        assert_eq!(windows[0].resets_at.as_deref(), Some("2026-10-01T17:00:00Z"));
    }

    #[test]
    fn the_status_is_the_providers_own_verdict() {
        let windows = windows_from(&usage_of(&fixture()));
        assert!(windows[0].detail.is_none());
        assert_eq!(windows[2].detail.as_deref(), Some("limit_reached"));
    }

    #[test]
    fn a_window_with_no_percentage_is_left_out() {
        let payload = json!({ "usage": { "weekly": { "status": "ok" } } });
        assert!(windows_from(&payload).is_empty());
    }

    #[test]
    fn a_missing_usage_object_yields_nothing() {
        assert!(windows_from(&json!({})).is_empty());
    }

    #[test]
    fn reads_the_key_opencode_stored_for_itself() {
        // The file's shape is `{ "opencode-go": { "key": "…" } }`.
        let json = json!({ "opencode-go": { "key": "sk-opencode" } });
        assert_eq!(
            credentials::dig_str(&json, "opencode-go.key").as_deref(),
            Some("sk-opencode")
        );
    }

    #[test]
    fn preserves_original_window_identity_duration_and_restriction() {
        let windows = windows_from(&usage_of(&json!({"usage": {
            "rolling": {"status":"rate-limited","percent":8},
            "weekly": {"status":"ok","percent":0.25},
            "monthly": {"status":"ok","percent":99.8}
        }})));
        assert_eq!(windows[0].id.as_deref(), Some("rolling"));
        assert_eq!(windows[0].window_seconds, Some(18_000.0));
        assert!(windows[0].is_exhausted, "a blocked window must stay blocked even below 100%");
        assert_eq!(windows[0].percent_used, Some(8.0));
        assert_eq!(windows[1].id.as_deref(), Some("weekly"));
        assert_eq!(windows[1].window_seconds, Some(604_800.0));
        assert_eq!(windows[1].percent_used, Some(0.25));
        assert!(!windows[1].is_exhausted);
        assert_eq!(windows[2].id.as_deref(), Some("monthly"));
        assert_eq!(windows[2].window_seconds, Some(2_592_000.0));
    }

    #[test]
    fn real_entitlement_reply_is_not_a_refused_key_or_zero_usage() {
        let usage = response_usage(reqwest::StatusCode::FORBIDDEN,
            r#"{"type":"error","error":{"type":"EntitlementError","message":"OpenCode Go subscription required."}}"#);
        assert!(usage.windows.is_empty());
        assert!(usage.error.as_deref().unwrap().contains("订阅"));
        assert!(!usage.error.as_deref().unwrap().contains("refused"));
        assert!(usage.configured);
        let refused = response_usage(reqwest::StatusCode::UNAUTHORIZED,
            r#"{"type":"error","error":{"type":"AuthError","message":"Unauthorized"}}"#);
        assert!(refused.error.as_deref().unwrap().contains("失效"));
        let denied = response_usage(reqwest::StatusCode::FORBIDDEN,"<html>blocked</html>");
        assert!(denied.error.as_deref().unwrap().contains("服务拒绝"));
        assert!(!denied.error.as_deref().unwrap().contains("订阅"));
    }

    #[test]
    fn successful_response_replays_the_original_fixture() {
        // Reconstructed upstream fixture (Apache-2.0), not a live account capture.
        let usage = response_usage(reqwest::StatusCode::OK,include_str!("../../tests/fixtures/opencode-go-normal.json"));
        assert!(usage.error.is_none());
        assert_eq!(usage.windows.len(),3);
        assert_eq!(usage.windows[0].percent_used,Some(42.5));
        assert_eq!(usage.windows[0].resets_at.as_deref(),Some("2026-09-26T18:00:00Z"));
        assert!(usage.account.is_none(),"an API key is not an account identity");
    }

    #[test]
    fn cli_credentials_accept_only_nonempty_string_api_keys() {
        assert_eq!(key_from_auth(&json!({"opencode-go":{"type":"api","key":"  sample  "}})).as_deref(),Some("sample"));
        assert_eq!(key_from_auth(&json!({"opencode-go":{"key":"legacy"}})).as_deref(),Some("legacy"));
        for entry in [json!({"type":"oauth","key":"different-auth"}),json!({"key":42}),json!({"key":"  "})] {
            assert!(key_from_auth(&json!({"opencode-go":entry})).is_none());
        }
    }

    #[test]
    fn explicit_xdg_location_precedes_legacy_paths_and_is_deduplicated() {
        let base=std::env::temp_dir();
        let paths=auth_paths_from(Some(base.join("home")),Some(base.join("config")),Some(base.join("local")),Some(base.join("xdg")));
        assert_eq!(paths[0],base.join("xdg/opencode/auth.json"));
        assert_eq!(paths[1],base.join("home/.local/share/opencode/auth.json"));
        assert_eq!(paths.len(),5);
    }

    #[tokio::test]
    #[ignore = "Explicit live smoke: GET only, uses this Windows user's OpenCode credential"]
    async fn live_usage_reports_readings_or_actionable_entitlement_failure() {
        assert!(OpenCodeGo.is_configured(),"a configured OpenCode Go account is required for the explicit live check");
        let usage=OpenCodeGo.fetch(Arc::new(Ctx::new().unwrap())).await;
        assert!(usage.source.is_some());
        if let Some(error)=usage.error {
            assert!(error.contains("订阅"),"live service did not return the expected entitlement classification");
            println!("OpenCode Go live GET: subscription entitlement required; credential source retained; no quota invented");
            assert!(usage.windows.is_empty());
        }else{
            assert!(!usage.windows.is_empty());
            assert!(usage.windows.iter().all(|window|window.id.is_some() && window.window_seconds.is_some()));
            println!("OpenCode Go live GET: {} reported windows; no key printed",usage.windows.len());
        }
    }
}
