//! Cursor quota.
//!
//! Cursor keeps its session in a SQLite `state.vscdb` rather than a JSON file,
//! and its usage API is private and cookie-authenticated. PulseWin therefore
//! cannot silently reuse a login the way it can for the CLI tools: the user
//! supplies the session cookie once via
//! `PULSEWIN_CURSOR_COOKIE` or `%APPDATA%\PulseWin\cursor.json`.
//!
//! `cursor.json` shape: `{ "cookie": "WorkosCursorSessionToken=user_xxx%3A%3Ajwt" }`
//!
//! VERIFY ON A REAL MACHINE: Cursor's usage endpoints are undocumented and have
//! changed repeatedly upstream.

use std::sync::Arc;

use serde_json::Value;

use super::{Ctx, FetchFuture, Provider};
use crate::credentials::{self, cursor_state_db_paths};
use crate::model::{parse_reset, ProviderUsage, UsageWindow};

const USAGE_URL_TEMPLATE: &str = "https://www.cursor.com/api/usage?user={user}";

pub struct Cursor;

impl Provider for Cursor {
    fn id(&self) -> &'static str {
        "cursor"
    }

    fn name(&self) -> &'static str {
        "Cursor"
    }

    /// The session cookie the fetch reads: `PULSEWIN_CURSOR_COOKIE`, or the
    /// `cookie` field of `%APPDATA%\PulseWin\cursor.json`.
    ///
    /// The SQLite store Cursor keeps its own login in is **not** evidence here:
    /// this port cannot read it, so a machine whose only Cursor login is that
    /// file has nothing PulseWin can fetch with.
    fn is_configured(&self) -> bool {
        if credentials::env_override(self.id(), "cookie").is_some() {
            return true;
        }
        config_path()
            .and_then(|path| credentials::read_json(&path))
            .and_then(|json| credentials::dig_str(&json, "cookie"))
            .is_some()
    }

    fn fetch(&self, ctx: Arc<Ctx>) -> FetchFuture {
        Box::pin(async move { fetch_inner(ctx).await })
    }
}

struct CursorAuth {
    cookie: String,
    user_id: Option<String>,
}

async fn fetch_inner(ctx: Arc<Ctx>) -> ProviderUsage {
    const ID: &str = "cursor";
    const NAME: &str = "Cursor";

    let auth = match resolve_auth() {
        Ok(a) => a,
        Err(e) => return ProviderUsage::failed(ID, NAME, e),
    };

    let url = match &auth.user_id {
        Some(user) => USAGE_URL_TEMPLATE.replace("{user}", user),
        // Cursor accepts the session cookie alone on some deployments.
        None => "https://www.cursor.com/api/usage".to_string(),
    };

    let response = ctx
        .client
        .get(&url)
        .header("Cookie", &auth.cookie)
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
        let hint = match status.as_u16() {
            401 | 403 => " — the session cookie expired; copy a fresh one from cursor.com",
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

    let plan = credentials::dig_first_str(&json, &["plan", "membershipType", "subscriptionType"]);

    ProviderUsage::ok(ID, NAME, windows)
        .with_plan(plan)
        .with_account(auth.user_id)
}

fn config_path() -> Option<std::path::PathBuf> {
    credentials::config_relative(&["PulseWin", "cursor.json"])
        .into_iter()
        .next()
}

fn resolve_auth() -> Result<CursorAuth, String> {
    if let Some(cookie) = credentials::env_override("cursor", "cookie") {
        let user_id = user_from_cookie(&cookie);
        return Ok(CursorAuth { cookie, user_id });
    }

    if let Some(path) = config_path() {
        if let Some(json) = credentials::read_json(&path) {
            if let Some(cookie) = credentials::dig_str(&json, "cookie") {
                let user_id =
                    credentials::dig_str(&json, "userId").or_else(|| user_from_cookie(&cookie));
                return Ok(CursorAuth { cookie, user_id });
            }
        }
    }

    let db_hint = cursor_state_db_paths()
        .first()
        .map(|p| p.display().to_string())
        .unwrap_or_else(|| "%APPDATA%\\Cursor\\User\\globalStorage\\state.vscdb".to_string());

    let config_hint = config_path()
        .map(|p| p.display().to_string())
        .unwrap_or_else(|| "%APPDATA%\\PulseWin\\cursor.json".to_string());

    Err(format!(
        "Cursor needs a one-time session cookie. Set PULSEWIN_CURSOR_COOKIE, \
         or create {config_hint} with {{\"cookie\": \"...\"}}. \
         (Cursor's login lives in {db_hint}, which is a SQLite store PulseWin does not read.)"
    ))
}

/// Cursor's cookie is `WorkosCursorSessionToken=<user_id>::<jwt>`; the user id
/// is also what the usage endpoint wants in its `user` query parameter.
fn user_from_cookie(cookie: &str) -> Option<String> {
    let value = cookie
        .split(';')
        .map(str::trim)
        .find_map(|part| part.strip_prefix("WorkosCursorSessionToken="))?;
    let decoded = value.replace("%3A%3A", "::").replace("%3a%3a", "::");
    let user = decoded.split("::").next()?.trim();
    if user.is_empty() {
        None
    } else {
        Some(user.to_string())
    }
}

/// Cursor returns per-model request counts, e.g.
/// `{ "gpt-4": { "numRequests": 12, "maxRequestUsage": 500 }, "startOfMonth": "..." }`
fn extract_windows(json: &Value) -> Vec<UsageWindow> {
    let mut out = Vec::new();

    let start_of_month = json.get("startOfMonth").and_then(parse_reset);
    // Cursor reports the *next* reset as the start of the next billing month.
    let resets_at = start_of_month
        .as_deref()
        .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
        .and_then(|dt| dt.checked_add_months(chrono::Months::new(1)))
        .map(|dt| dt.to_rfc3339_opts(chrono::SecondsFormat::Secs, true));

    let Some(map) = json.as_object() else {
        return out;
    };

    for (key, node) in map {
        if key == "startOfMonth" || !node.is_object() {
            continue;
        }

        let used = credentials::dig_first_f64(node, &["numRequests", "requests", "used"]);
        let max = credentials::dig_first_f64(
            node,
            &["maxRequestUsage", "maxRequests", "limit", "entitlement"],
        );

        if used.is_none() && max.is_none() {
            continue;
        }

        let percent = match (used, max) {
            (Some(u), Some(m)) if m > 0.0 => Some(((u / m) * 100.0).clamp(0.0, 100.0)),
            _ => None,
        };

        let detail = match (used, max) {
            (Some(u), Some(m)) => Some(format!("{} / {} requests", u.round(), m.round())),
            (Some(u), None) => Some(format!("{} requests", u.round())),
            _ => None,
        };

        out.push(
            UsageWindow::new(label_for(key), percent)
                .with_reset(resets_at.clone())
                .with_detail(detail),
        );
    }

    // Busiest model first, so the ring shows the thing that will run out.
    out.sort_by(|a, b| {
        b.percent_used
            .partial_cmp(&a.percent_used)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    out.truncate(4);
    out
}

fn label_for(key: &str) -> String {
    match key {
        "gpt-4" => "GPT-4".to_string(),
        "gpt-3.5-turbo" => "GPT-3.5".to_string(),
        other => other.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn extracts_user_id_from_cookie() {
        let cookie = "WorkosCursorSessionToken=user_01ABC%3A%3Ajwt.token.here; other=1";
        assert_eq!(user_from_cookie(cookie).as_deref(), Some("user_01ABC"));
    }

    #[test]
    fn cookie_without_token_yields_none() {
        assert!(user_from_cookie("other=1").is_none());
    }

    #[test]
    fn computes_percent_from_requests() {
        let payload = json!({
            "gpt-4": { "numRequests": 125, "maxRequestUsage": 500 },
            "startOfMonth": "2026-10-01T00:00:00.000Z"
        });
        let windows = extract_windows(&payload);
        assert_eq!(windows.len(), 1);
        assert_eq!(windows[0].label, "GPT-4");
        assert_eq!(windows[0].percent_used, Some(25.0));
        assert_eq!(windows[0].detail.as_deref(), Some("125 / 500 requests"));
        assert!(windows[0].resets_at.is_some());
    }

    #[test]
    fn sorts_busiest_first_and_truncates() {
        let payload = json!({
            "a": { "numRequests": 1, "maxRequestUsage": 100 },
            "b": { "numRequests": 90, "maxRequestUsage": 100 },
            "c": { "numRequests": 50, "maxRequestUsage": 100 },
            "d": { "numRequests": 10, "maxRequestUsage": 100 },
            "e": { "numRequests": 5, "maxRequestUsage": 100 }
        });
        let windows = extract_windows(&payload);
        assert_eq!(windows.len(), 4);
        assert_eq!(windows[0].label, "b");
    }

    #[test]
    fn skips_non_object_and_start_of_month() {
        let payload = json!({ "startOfMonth": "2026-10-01T00:00:00Z", "note": "hi" });
        assert!(extract_windows(&payload).is_empty());
    }
}
