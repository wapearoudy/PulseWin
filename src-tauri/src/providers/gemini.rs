//! Gemini CLI quota.
//!
//! Auth: reuses `~/.gemini/oauth_creds.json`. Google access tokens last about
//! an hour, so PulseWin refreshes using the stored refresh token and caches the
//! renewed access token in-process (it never writes back to Google's file).
//!
//! VERIFY ON A REAL MACHINE: `cloudcode-pa.googleapis.com` is the private
//! endpoint the Gemini CLI talks to. OAuth client configuration is read from
//! the installed Gemini CLI, credential JSON, or explicit environment overrides.
//! No OAuth client credentials are bundled with PulseWin.

use std::io::Read;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};
use tokio::sync::Mutex;

use super::{Ctx, FetchFuture, Provider};
use crate::credentials::{self, gemini_credential_paths};
use crate::model::{parse_reset, ProviderUsage, UsageWindow};

const TOKEN_URL: &str = "https://oauth2.googleapis.com/token";
const QUOTA_URL: &str = "https://cloudcode-pa.googleapis.com/v1internal:retrieveUserQuota";

/// Renewed tokens, keyed by refresh token, so a 30-second refresh loop does not
/// hammer Google's token endpoint.
static TOKEN_CACHE: Mutex<Option<CachedToken>> = Mutex::const_new(None);

struct CachedToken {
    refresh_token: String,
    access_token: String,
    expires_at: chrono::DateTime<chrono::Utc>,
}

pub struct GeminiCli;

impl Provider for GeminiCli {
    fn id(&self) -> &'static str {
        "gemini-cli"
    }

    fn name(&self) -> &'static str {
        "Gemini CLI"
    }

    /// One of the files the Gemini CLI writes. Whether the access token inside
    /// still has life in it — and whether the refresh token can renew it — is
    /// the fetch's business, and its failure says which.
    fn is_configured(&self) -> bool {
        credentials::first_existing(&gemini_credential_paths()).is_some()
    }

    fn fetch(&self, ctx: Arc<Ctx>) -> FetchFuture {
        Box::pin(async move { fetch_inner(ctx).await })
    }
}

struct GeminiCreds {
    access_token: Option<String>,
    refresh_token: Option<String>,
    /// Milliseconds since epoch, as Gemini CLI writes it.
    expiry_ms: Option<i64>,
    oauth_client: Option<OAuthClient>,
}

struct OAuthClient {
    id: String,
    secret: String,
}

fn client_from_json(json: &Value) -> Option<OAuthClient> {
    // Keep the pair from one configuration; never mix two different clients.
    for prefix in ["", "installed.", "web."] {
        if let (Some(id), Some(secret)) = (
            credentials::dig_str(json, &format!("{prefix}client_id")),
            credentials::dig_str(json, &format!("{prefix}client_secret")),
        ) {
            return Some(OAuthClient { id, secret });
        }
    }
    None
}

/// Read static string assignments only. Do not execute the CLI's JavaScript.
fn js_constant(source: &str, name: &str) -> Option<String> {
    for (offset, _) in source.match_indices(name) {
        if source[..offset].chars().next_back().is_some_and(|c| c.is_alphanumeric() || c == '_' || c == '$' || c == '.') {
            continue;
        }
        let Some(tail) = source[offset + name.len()..].trim_start().strip_prefix('=') else { continue };
        let tail = tail.trim_start();
        let Some(quote @ ('\'' | '"')) = tail.chars().next() else { continue };
        let Some(end) = tail[1..].find(quote) else { continue };
        let value = &tail[1..end + 1];
        let suffix = tail[end + 2..].trim_start();
        if !value.is_empty() && value.len() <= 2048 && value.is_ascii()
            && !value.chars().any(|c| c == '\\' || c.is_control())
            && (suffix.starts_with(';') || suffix.starts_with(',') || suffix.is_empty()) {
            return Some(value.to_string());
        }
    }
    None
}

fn installed_cli_client(paths: &[PathBuf]) -> Option<OAuthClient> {
    const LIMIT: u64 = 16 * 1024 * 1024;
    for path in paths {
        let Ok(file) = std::fs::File::open(path) else { continue };
        let mut source = String::new();
        if file.take(LIMIT + 1).read_to_string(&mut source).is_err() || source.len() as u64 > LIMIT {
            continue;
        }
        if let (Some(id), Some(secret)) = (
            js_constant(&source, "OAUTH_CLIENT_ID"),
            js_constant(&source, "OAUTH_CLIENT_SECRET"),
        ) {
            return Some(OAuthClient { id, secret });
        }
    }
    None
}

fn cli_client_paths() -> Vec<PathBuf> {
    let mut prefixes = Vec::new();
    if let Some(appdata) = std::env::var_os("APPDATA") {
        prefixes.push(PathBuf::from(appdata).join("npm"));
    }
    if let Some(prefix) = std::env::var_os("NPM_CONFIG_PREFIX") {
        prefixes.push(PathBuf::from(prefix));
    }
    if let Some(path) = std::env::var_os("PATH") {
        for prefix in std::env::split_paths(&path) {
            if ["gemini.cmd", "gemini.ps1", "gemini"].iter().any(|name| prefix.join(name).is_file()) {
                prefixes.push(prefix);
            }
        }
    }
    prefixes.sort();
    prefixes.dedup();
    prefixes.into_iter().filter(|p| p.is_absolute()).flat_map(|prefix| {
        [
            "node_modules/@google/gemini-cli/node_modules/@google/gemini-cli-core/dist/src/code_assist/oauth2.js",
            "node_modules/@google/gemini-cli-core/dist/src/code_assist/oauth2.js",
            "node_modules/@google/gemini-cli/dist/index.js",
            "lib/node_modules/@google/gemini-cli/node_modules/@google/gemini-cli-core/dist/src/code_assist/oauth2.js",
            "lib/node_modules/@google/gemini-cli/dist/index.js",
        ].map(|relative| prefix.join(relative))
    }).collect()
}

fn resolve_oauth_client(creds: &GeminiCreds) -> Result<OAuthClient, String> {
    let env_value = |name| std::env::var(name).ok().filter(|value| !value.trim().is_empty());
    match (env_value("PULSEWIN_GEMINI_CLIENT_ID"), env_value("PULSEWIN_GEMINI_CLIENT_SECRET")) {
        (Some(id), Some(secret)) => return Ok(OAuthClient { id, secret }),
        (None, None) => (),
        _ => return Err("Set both PULSEWIN_GEMINI_CLIENT_ID and PULSEWIN_GEMINI_CLIENT_SECRET".into()),
    }
    if let Some(client) = &creds.oauth_client {
        return Ok(OAuthClient { id: client.id.clone(), secret: client.secret.clone() });
    }
    installed_cli_client(&cli_client_paths()).ok_or_else(||
        "Gemini token expired: run the Gemini CLI to renew its login, or install it so PulseWin can read its OAuth client configuration; alternatively set PULSEWIN_GEMINI_CLIENT_ID and PULSEWIN_GEMINI_CLIENT_SECRET".into())
}

async fn fetch_inner(ctx: Arc<Ctx>) -> ProviderUsage {
    const ID: &str = "gemini-cli";
    const NAME: &str = "Gemini CLI";

    let creds = match load_creds() {
        Ok(c) => c,
        Err(e) => return ProviderUsage::failed(ID, NAME, e),
    };

    let token = match ensure_access_token(&ctx, &creds).await {
        Ok(t) => t,
        Err(e) => return ProviderUsage::failed(ID, NAME, e),
    };

    let response = ctx
        .client
        .post(QUOTA_URL)
        .header("Authorization", format!("Bearer {token}"))
        .header("Content-Type", "application/json")
        .header("Accept", "application/json")
        .json(&json!({}))
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
            401 | 403 => " — run the Gemini CLI once to refresh its login",
            404 => " — no Code Assist subscription on this account",
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
        return ProviderUsage::failed(ID, NAME, "no quota buckets in response");
    }

    let plan = credentials::dig_first_str(&json, &["currentTier.name", "tier", "plan"]);

    ProviderUsage::ok(ID, NAME, windows)
        .with_plan(plan)
        .with_account(Some("Google login".to_string()))
}

fn load_creds() -> Result<GeminiCreds, String> {
    let paths = gemini_credential_paths();
    let path = credentials::first_existing(&paths).ok_or_else(|| {
        format!(
            "no Gemini credentials found (looked for: {})",
            paths
                .iter()
                .map(|p| p.display().to_string())
                .collect::<Vec<_>>()
                .join(", ")
        )
    })?;

    let json = credentials::read_json(&path).ok_or_else(|| {
        format!(
            "could not parse {} — run the Gemini CLI and sign in first",
            path.display()
        )
    })?;

    Ok(GeminiCreds {
        access_token: credentials::dig_first_str(&json, &["access_token", "accessToken"]),
        refresh_token: credentials::dig_first_str(&json, &["refresh_token", "refreshToken"]),
        // Gemini CLI stores expiry as epoch milliseconds.
        expiry_ms: credentials::dig_first_f64(&json, &["expiry_date", "expiryDate"])
            .map(|v| v as i64),
        oauth_client: client_from_json(&json),
    })
}

async fn ensure_access_token(ctx: &Ctx, creds: &GeminiCreds) -> Result<String, String> {
    let still_valid = creds.expiry_ms.map(|expiry| {
        let now_ms = chrono::Utc::now().timestamp_millis();
        // Refresh a minute early rather than racing the expiry.
        expiry - now_ms > 60_000
    });

    if let (Some(token), Some(true)) = (creds.access_token.as_ref(), still_valid) {
        return Ok(token.clone());
    }
    if let Some(token) = creds.access_token.as_ref() {
        if still_valid.is_none() {
            // No expiry recorded; assume the token works and let 401 drive a refresh.
            return Ok(token.clone());
        }
    }

    let refresh_token = creds
        .refresh_token
        .as_ref()
        .ok_or("Gemini access token expired and no refresh token stored — run the Gemini CLI")?;

    {
        let cache = TOKEN_CACHE.lock().await;
        if let Some(cached) = cache.as_ref() {
            if &cached.refresh_token == refresh_token
                && cached.expires_at - chrono::Utc::now() > chrono::Duration::seconds(60)
            {
                return Ok(cached.access_token.clone());
            }
        }
    }

    let oauth_client = resolve_oauth_client(creds)?;

    let response = ctx
        .client
        .post(TOKEN_URL)
        .timeout(Duration::from_secs(15))
        .form(&[
            ("client_id", oauth_client.id.as_str()),
            ("client_secret", oauth_client.secret.as_str()),
            ("refresh_token", refresh_token.as_str()),
            ("grant_type", "refresh_token"),
        ])
        .send()
        .await
        .map_err(|e| format!("token refresh failed: {e}"))?;

    let status = response.status();
    let body = response
        .text()
        .await
        .map_err(|e| format!("cannot read token body: {e}"))?;

    if !status.is_success() {
        return Err(format!("token refresh returned HTTP {status}"));
    }

    let json: Value =
        serde_json::from_str(&body).map_err(|e| format!("bad token JSON: {e}"))?;

    let access = credentials::dig_str(&json, "access_token")
        .ok_or("token refresh response had no access_token")?;
    let expires_in = credentials::dig_f64(&json, "expires_in").unwrap_or(3600.0);

    {
        let mut cache = TOKEN_CACHE.lock().await;
        *cache = Some(CachedToken {
            refresh_token: refresh_token.clone(),
            access_token: access.clone(),
            expires_at: chrono::Utc::now() + chrono::Duration::seconds(expires_in as i64),
        });
    }

    Ok(access)
}

/// `retrieveUserQuota` returns `{ buckets: [{ modelId, remainingFraction, resetTime }] }`.
/// We surface the tightest bucket per model family.
fn extract_windows(json: &Value) -> Vec<UsageWindow> {
    let mut out = Vec::new();

    let buckets = json
        .get("buckets")
        .or_else(|| json.get("quotaBuckets"))
        .and_then(|v| v.as_array());

    let Some(buckets) = buckets else {
        return out;
    };

    for bucket in buckets {
        let label = credentials::dig_first_str(bucket, &["modelId", "model_id", "name"])
            .unwrap_or_else(|| "Quota".to_string());

        // `remainingFraction` is 1.0 = untouched, 0.0 = exhausted.
        let ratio = credentials::dig_first_f64(
            bucket,
            &["remainingFraction", "remaining_fraction", "remaining"],
        );

        let percent_used = ratio.map(|r| {
            let used = (1.0 - r) * 100.0;
            used.clamp(0.0, 100.0)
        });

        let resets_at = bucket
            .get("resetTime")
            .or_else(|| bucket.get("reset_time"))
            .and_then(parse_reset);

        if percent_used.is_none() && resets_at.is_none() {
            continue;
        }

        out.push(
            UsageWindow::new(short_label(&label), percent_used).with_reset(resets_at),
        );
    }

    // Tightest quota first.
    out.sort_by(|a, b| {
        b.percent_used
            .partial_cmp(&a.percent_used)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    out.truncate(4);
    out
}

/// Bucket ids are long (`gemini-2.5-pro-preview-06-05`); keep them readable.
fn short_label(model_id: &str) -> String {
    let trimmed = model_id
        .trim_start_matches("models/")
        .replace("gemini-", "");
    if trimmed.len() <= 16 {
        trimmed
    } else {
        trimmed.chars().take(15).collect::<String>() + "…"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn reads_multiline_cli_constants_without_executing_code() {
        let source = "const OAUTH_CLIENT_ID =\n 'fixture-client';\nconst OAUTH_CLIENT_SECRET = \"fixture-secret\";";
        assert_eq!(js_constant(source, "OAUTH_CLIENT_ID").as_deref(), Some("fixture-client"));
        assert_eq!(js_constant(source, "OAUTH_CLIENT_SECRET").as_deref(), Some("fixture-secret"));
        assert!(js_constant("const OAUTH_CLIENT_ID = readSecret();", "OAUTH_CLIENT_ID").is_none());
        assert!(js_constant("const OAUTH_CLIENT_ID = 'a' + 'b';", "OAUTH_CLIENT_ID").is_none());
        assert!(js_constant("const OTHER_OAUTH_CLIENT_ID = 'a';", "OAUTH_CLIENT_ID").is_none());
        assert!(js_constant("const OAUTH_CLIENT_ID = 'a\\n';", "OAUTH_CLIENT_ID").is_none());
    }

    #[test]
    fn json_oauth_client_requires_a_complete_pair_from_one_section() {
        let client = client_from_json(&json!({"installed":{"client_id":"fixture-client","client_secret":"fixture-secret"}})).unwrap();
        assert_eq!(client.id, "fixture-client");
        assert_eq!(client.secret, "fixture-secret");
        assert!(client_from_json(&json!({"client_id":"one","installed":{"client_secret":"two"}})).is_none());
    }

    #[test]
    fn reads_installed_cli_file_and_skips_incomplete_files() {
        let base = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../.test-data/gemini-oauth");
        let directory = base.join(format!("{}-{}", std::process::id(), chrono::Utc::now().timestamp_nanos_opt().unwrap()));
        std::fs::create_dir_all(&directory).unwrap();
        let incomplete = directory.join("incomplete.js");
        let complete = directory.join("oauth2.js");
        std::fs::write(&incomplete, "const OAUTH_CLIENT_ID = 'wrong';").unwrap();
        std::fs::write(&complete, "const OAUTH_CLIENT_ID = 'fixture-client'; const OAUTH_CLIENT_SECRET = 'fixture-secret';").unwrap();
        let client = installed_cli_client(&[directory.join("missing.js"), incomplete, complete]).unwrap();
        assert_eq!(client.id, "fixture-client");
        assert_eq!(client.secret, "fixture-secret");
        assert!(directory.is_absolute() && directory.starts_with(&base));
        std::fs::remove_dir_all(&directory).unwrap();
    }

    #[tokio::test]
    async fn valid_access_token_does_not_require_oauth_client_configuration() {
        let creds = GeminiCreds { access_token: Some("fixture-token".into()), refresh_token: None,
            expiry_ms: Some(chrono::Utc::now().timestamp_millis() + 3_600_000), oauth_client: None };
        let ctx = Ctx::new().unwrap();
        assert_eq!(ensure_access_token(&ctx, &creds).await.unwrap(), "fixture-token");
    }

    #[test]
    fn inverts_remaining_fraction() {
        let payload = json!({
            "buckets": [
                { "modelId": "gemini-2.5-pro", "remainingFraction": 0.25,
                  "resetTime": "2026-10-02T00:00:00Z" }
            ]
        });
        let windows = extract_windows(&payload);
        assert_eq!(windows.len(), 1);
        assert_eq!(windows[0].percent_used, Some(75.0));
        assert_eq!(windows[0].label, "2.5-pro");
        assert!(windows[0].resets_at.is_some());
    }

    #[test]
    fn full_bucket_is_zero_used() {
        let payload = json!({ "buckets": [{ "modelId": "m", "remainingFraction": 1.0 }] });
        assert_eq!(extract_windows(&payload)[0].percent_used, Some(0.0));
    }

    #[test]
    fn exhausted_bucket_is_hundred_used() {
        let payload = json!({ "buckets": [{ "modelId": "m", "remainingFraction": 0.0 }] });
        assert_eq!(extract_windows(&payload)[0].percent_used, Some(100.0));
    }

    #[test]
    fn sorts_tightest_first() {
        let payload = json!({
            "buckets": [
                { "modelId": "loose", "remainingFraction": 0.9 },
                { "modelId": "tight", "remainingFraction": 0.1 }
            ]
        });
        let windows = extract_windows(&payload);
        assert_eq!(windows[0].label, "tight");
    }

    #[test]
    fn missing_buckets_yields_nothing() {
        assert!(extract_windows(&json!({})).is_empty());
    }

    #[test]
    fn shortens_long_model_ids() {
        let long = short_label("models/gemini-2.5-pro-preview-06-05");
        assert!(long.chars().count() <= 16);
        assert!(long.ends_with('…'));
    }
}
