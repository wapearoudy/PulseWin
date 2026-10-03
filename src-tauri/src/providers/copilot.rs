//! GitHub Copilot quota.
//!
//! Auth: reuses the OAuth token Copilot's editor extension already stored in
//! `hosts.json` (VS Code / VS Code Insiders %APPDATA%), or a `gh` CLI token.
//!
//! VERIFY ON A REAL MACHINE: `copilot_internal/user` requires the editor
//! identity headers below; GitHub has tightened these over time.

use std::sync::Arc;

use serde_json::Value;

use super::{normalize_percent, Ctx, FetchFuture, Provider};
use crate::credentials::{self, copilot_credential_paths};
use crate::model::{parse_reset, ProviderUsage, UsageWindow};

const USAGE_URL: &str = "https://api.github.com/copilot_internal/user";

/// `hosts.json` is keyed by host; take the first entry (normally "github.com").
const HOST_KEYS: &[&str] = &["github.com", "ghe.com"];

pub struct Copilot;

impl Provider for Copilot {
    fn id(&self) -> &'static str {
        "copilot"
    }

    fn name(&self) -> &'static str {
        "GitHub Copilot"
    }

    /// The token the fetch reads: `PULSEWIN_COPILOT_TOKEN`, or one of the hosts
    /// files the editor extensions and the `gh` CLI write.
    fn is_configured(&self) -> bool {
        credentials::env_override(self.id(), "token").is_some()
            || credentials::first_existing(&copilot_credential_paths()).is_some()
    }

    fn fetch(&self, ctx: Arc<Ctx>) -> FetchFuture {
        Box::pin(async move { fetch_inner(ctx).await })
    }
}

async fn fetch_inner(ctx: Arc<Ctx>) -> ProviderUsage {
    const ID: &str = "copilot";
    const NAME: &str = "GitHub Copilot";

    let token = match resolve_copilot_token() {
        Ok(t) => t,
        Err(e) => return ProviderUsage::failed(ID, NAME, e),
    };

    let response = ctx
        .client
        .get(USAGE_URL)
        .header("Authorization", format!("token {token}"))
        .header("Accept", "application/json")
        .header("X-GitHub-Api-Version", "2022-11-28")
        // GitHub rejects requests that do not look like an editor.
        .header("Editor-Version", "vscode/1.95.0")
        .header("Editor-Plugin-Version", "copilot-chat/0.22.0")
        .header("User-Agent", "GitHubCopilotChat/0.22.0")
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
        let hint = match status.as_u16() {
            401 | 403 => " — sign in to Copilot in your editor again",
            404 => " — this account has no Copilot subscription",
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

    let plan = credentials::dig_first_str(&json, &["copilot_plan", "plan", "sku"]);
    let account = credentials::dig_first_str(&json, &["login", "user.login", "email"]);

    ProviderUsage::ok(ID, NAME, windows)
        .with_plan(plan)
        .with_account(account)
}

/// Read the editor-stored token, tolerating the several shapes `hosts.json`
/// has had over time.
fn resolve_copilot_token() -> Result<String, String> {
    if let Some(v) = credentials::env_override("copilot", "token") {
        return Ok(v);
    }

    let paths = copilot_credential_paths();
    let path = credentials::first_existing(&paths)
        .ok_or_else(|| format!("no Copilot token found (looked for: {})", join_paths(&paths)))?;

    // The gh CLI stores a YAML file; scan it line-wise rather than pulling in a
    // YAML parser for one field.
    if path.extension().map(|e| e == "yml").unwrap_or(false) {
        let text = std::fs::read_to_string(&path).map_err(|e| e.to_string())?;
        for line in text.lines() {
            let line = line.trim();
            if let Some(rest) = line.strip_prefix("oauth_token:") {
                let token = rest.trim().trim_matches('"').trim_matches('\'');
                if !token.is_empty() {
                    return Ok(token.to_string());
                }
            }
        }
        return Err(format!("no oauth_token in {}", path.display()));
    }

    let json = credentials::read_json(&path)
        .ok_or_else(|| format!("could not parse {}", path.display()))?;

    // Shape A: { "github.com": { "oauth_token": "..." } }
    for key in HOST_KEYS {
        if let Some(token) = credentials::dig_str(&json, &format!("{key}.oauth_token")) {
            return Ok(token);
        }
    }
    // Shape B / C: a flat token field, or any host key we did not anticipate.
    if let Some(token) = credentials::dig_first_str(&json, &["oauth_token", "token", "access_token"])
    {
        return Ok(token);
    }
    if let Value::Object(map) = &json {
        for value in map.values() {
            if let Some(token) = value.get("oauth_token").and_then(|v| v.as_str()) {
                if !token.is_empty() {
                    return Ok(token.to_string());
                }
            }
        }
    }

    Err(format!("no oauth_token field in {}", path.display()))
}

fn join_paths(paths: &[std::path::PathBuf]) -> String {
    paths
        .iter()
        .map(|p| p.display().to_string())
        .collect::<Vec<_>>()
        .join(", ")
}

/// Copilot reports *remaining* quota, so we invert it into "used".
fn extract_windows(json: &Value) -> Vec<UsageWindow> {
    let mut out = Vec::new();

    let Some(snapshots) = json.get("quota_snapshots").and_then(|v| v.as_object()) else {
        return out;
    };

    for (key, node) in snapshots {
        let unlimited = node
            .get("unlimited")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        if unlimited {
            out.push(UsageWindow::new(label_for(key), Some(0.0)).with_detail(Some("unlimited".into())));
            continue;
        }

        let percent_used = node
            .get("percent_remaining")
            .and_then(|v| v.as_f64())
            .map(|remaining| normalize_percent(100.0 - remaining));

        let remaining = node.get("remaining").and_then(|v| v.as_f64());
        let entitlement = node.get("entitlement").and_then(|v| v.as_f64());

        let detail = match (remaining, entitlement) {
            (Some(rem), Some(total)) => Some(format!("{} / {} left", rem.round(), total.round())),
            _ => None,
        };

        let resets_at = node
            .get("reset_at")
            .or_else(|| node.get("quota_reset_date"))
            .or_else(|| json.get("quota_reset_date"))
            .and_then(parse_reset);

        if percent_used.is_none() && detail.is_none() {
            continue;
        }

        out.push(
            UsageWindow::new(label_for(key), percent_used)
                .with_reset(resets_at)
                .with_detail(detail),
        );
    }

    // Stable order: premium first, then chat, then completions.
    out.sort_by_key(|w| match w.label.as_str() {
        "Premium" => 0,
        "Chat" => 1,
        "Completions" => 2,
        _ => 3,
    });
    out
}

fn label_for(key: &str) -> String {
    match key {
        "premium_interactions" => "Premium".to_string(),
        "chat" => "Chat".to_string(),
        "completions" => "Completions".to_string(),
        other => {
            let mut chars = other.chars();
            match chars.next() {
                Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
                None => other.to_string(),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn inverts_percent_remaining_into_used() {
        let payload = json!({
            "copilot_plan": "individual",
            "quota_snapshots": {
                "premium_interactions": {
                    "percent_remaining": 75.0,
                    "remaining": 225,
                    "entitlement": 300,
                    "unlimited": false
                }
            }
        });
        let windows = extract_windows(&payload);
        assert_eq!(windows.len(), 1);
        assert_eq!(windows[0].label, "Premium");
        assert_eq!(windows[0].percent_used, Some(25.0));
        assert_eq!(windows[0].detail.as_deref(), Some("225 / 300 left"));
    }

    #[test]
    fn unlimited_is_zero_used() {
        let payload = json!({
            "quota_snapshots": { "chat": { "unlimited": true } }
        });
        let windows = extract_windows(&payload);
        assert_eq!(windows[0].percent_used, Some(0.0));
        assert_eq!(windows[0].detail.as_deref(), Some("unlimited"));
    }

    #[test]
    fn sorts_premium_first() {
        let payload = json!({
            "quota_snapshots": {
                "completions": { "percent_remaining": 50.0 },
                "premium_interactions": { "percent_remaining": 50.0 },
                "chat": { "percent_remaining": 50.0 }
            }
        });
        let labels: Vec<String> = extract_windows(&payload).into_iter().map(|w| w.label).collect();
        assert_eq!(labels, vec!["Premium", "Chat", "Completions"]);
    }

    #[test]
    fn missing_snapshots_yields_nothing() {
        assert!(extract_windows(&json!({ "copilot_plan": "business" })).is_empty());
    }
}
