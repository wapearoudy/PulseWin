//! Devin's daily and weekly quota, read from the endpoint its account page
//! calls: `GET https://app.devin.ai/api/<org>/billing/quota/usage`.
//!
//! **The credential is a pasted one.** The reference implementation prefers the
//! browser: it reads `auth1_session` out of a Chromium browser's `localStorage`
//! for `app.devin.ai` (`Auth/ChromiumLocalStorage.swift`) and falls back to the
//! plan the Devin/Windsurf app cached in its SQLite `state.vscdb`. PulseWin
//! reads neither — one is a Chromium profile, the other SQLite — so the
//! credential here is the live endpoint's own, pasted: a bearer token and, when
//! the account has more than one, the organization the quota is scoped to. The
//! original accepts exactly the same paste, which is what its route picker's
//! "endpoint" arm is for.
//!
//! `PULSEWIN_DEVIN_TOKEN` (or `%APPDATA%\PulseWin\devin.json` with `apiKey`)
//! holds it, in any of the shapes the original documents:
//!
//! ```text
//! auth1_… my-team
//! Authorization: Bearer auth1_… org_1a2b3c
//! auth1_… https://app.devin.ai/org/my-team/settings
//! ```
//!
//! A value copied out of the browser's `localStorage` — the JSON object holding
//! `token`/`access_token` and its organization — is read too, because that is
//! the shape this credential is actually stored in on the machine and the shape
//! somebody will paste.
//!
//! **The reply reports what has been used** (`daily_percentage`,
//! `weekly_percentage`), where the app's cached row reports what is left. This
//! reads the endpoint and nothing else: the row names no organization, and the
//! endpoint is scoped to one, so the two can be different allowances.

use std::sync::Arc;

use serde_json::Value;

use super::{describe_reqwest_error, percent_from_fraction, Ctx, FetchFuture, Provider};
use crate::model::{parse_reset, ProviderUsage, UsageWindow};

const ID: &str = "devin";
const NAME: &str = "Devin";

const HOST: &str = "https://app.devin.ai";

pub struct Devin;

impl Provider for Devin {
    fn id(&self) -> &'static str {
        ID
    }

    fn name(&self) -> &'static str {
        NAME
    }

    /// Whether a pasted credential that names a token is on the machine. No
    /// network call.
    fn is_configured(&self) -> bool {
        super::pasted::credential(ID)
            .and_then(|pasted| Credential::parse(&pasted))
            .is_some()
    }

    fn fetch(&self, ctx: Arc<Ctx>) -> FetchFuture {
        Box::pin(async move { fetch_inner(ctx).await })
    }
}

/// A bearer token, and the organization the quota path is scoped by.
struct Credential {
    token: String,
    /// Already normalised to the path segment it belongs in — `org/<slug>` or
    /// `organizations/<id>`. Nil when nothing was given, which the endpoint
    /// refuses; it is a separate reason from a missing token so the message can
    /// say which half is missing.
    organization: Option<String>,
}

impl Credential {
    /// The pasted value, in any of the shapes the original accepts.
    fn parse(pasted: &str) -> Option<Self> {
        let mut text = pasted.trim().to_string();
        if text.is_empty() {
            return None;
        }

        // What somebody copies out of `localStorage` is the object the session
        // is stored in, not one value from it.
        if let Ok(Value::Object(map)) = serde_json::from_str::<Value>(&text) {
            if let Some(token) = map
                .get("token")
                .or_else(|| map.get("access_token"))
                .and_then(Value::as_str)
                .filter(|token| token.len() > 20)
            {
                let organization = ["organization", "org", "orgId", "organizationId"]
                    .iter()
                    .find_map(|key| map.get(*key).and_then(Value::as_str))
                    .or_else(|| map.get("devin_primary_org_id").and_then(Value::as_str))
                    .and_then(normalize_organization);
                return Some(Self {
                    token: token.to_string(),
                    organization,
                });
            }
        }

        // A whole header line, as copied out of a browser's network tab.
        if let Some((name, rest)) = text.split_once(':') {
            if name.trim().eq_ignore_ascii_case("authorization") {
                text = rest.trim().to_string();
            }
        }
        if let Some(rest) = strip_prefix_ignoring_case(&text, "bearer ") {
            text = rest.trim().to_string();
        }

        let mut parts = text.split_whitespace();
        let token = parts.next()?.to_string();
        if token.is_empty() {
            return None;
        }
        let organization = parts.next().and_then(normalize_organization);
        Some(Self { token, organization })
    }

    /// The internal id, where the organization was given as one. It also
    /// travels as a header, which is how the service resolves an account with
    /// more than one organization on it.
    fn internal_id(&self) -> Option<&str> {
        self.organization
            .as_deref()
            .and_then(|organization| organization.strip_prefix("organizations/"))
    }

    /// The paths to try, in order. The API is undocumented and the shape of
    /// this one segment is the part that varies, so a path that is not found is
    /// tried again as the others rather than reported as "no quota".
    fn paths(&self) -> Vec<String> {
        let Some(organization) = self.organization.as_deref() else {
            return Vec::new();
        };
        let mut paths = vec![organization.to_string()];
        if let Some(internal) = self.internal_id() {
            paths.insert(0, internal.to_string());
        }
        if let Some(slug) = organization.strip_prefix("org/") {
            paths.push(slug.to_string());
            if is_internal_id(slug) {
                paths.push(format!("organizations/{slug}"));
            }
        }

        let mut seen: Vec<String> = Vec::new();
        paths
            .into_iter()
            .filter(|path| {
                if seen.contains(path) {
                    false
                } else {
                    seen.push(path.clone());
                    true
                }
            })
            .map(|path| format!("{path}/billing/quota/usage"))
            .collect()
    }
}

/// A slug, an internal `org_…` id, or any `app.devin.ai` URL carrying one,
/// reduced to the path segment the API wants.
fn normalize_organization(raw: &str) -> Option<String> {
    let mut value = raw.trim().to_string();
    if value.is_empty() {
        return None;
    }

    if let Ok(url) = reqwest::Url::parse(&value) {
        let host = url.host_str().unwrap_or("").to_lowercase();
        if host == "devin.ai" || host.ends_with(".devin.ai") {
            let segments: Vec<&str> = url
                .path()
                .split('/')
                .filter(|segment| !segment.is_empty())
                .collect();
            if segments.len() >= 2 && (segments[0] == "org" || segments[0] == "organizations") {
                value = format!("{}/{}", segments[0], segments[1]);
            }
        }
    }

    let value = value.trim_matches('/').trim();
    if value.is_empty() {
        return None;
    }
    if value.starts_with("org/") || value.starts_with("organizations/") {
        return Some(value.to_string());
    }
    // `org_` and `org-` are the internal id's prefixes; anything else is the
    // slug that appears in the address bar.
    Some(if is_internal_id(value) {
        format!("organizations/{value}")
    } else {
        format!("org/{value}")
    })
}

fn is_internal_id(value: &str) -> bool {
    value.starts_with("org_") || value.starts_with("org-")
}

fn strip_prefix_ignoring_case<'a>(text: &'a str, prefix: &str) -> Option<&'a str> {
    text.get(..prefix.len())
        .filter(|head| head.eq_ignore_ascii_case(prefix))?;
    text.get(prefix.len()..)
}

// ---------------------------------------------------------------------------
// The request
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
enum Refusal {
    Refused,
    RateLimited,
    ServerError,
    Unreachable(String),
}

impl Refusal {
    fn message(&self) -> String {
        match self {
            Refusal::Refused => {
                "HTTP 401/403 — the token was refused, or it does not reach this organization"
                    .to_string()
            }
            Refusal::RateLimited => "HTTP 429 — rate limited, try again shortly".to_string(),
            Refusal::ServerError => "the service returned an error".to_string(),
            Refusal::Unreachable(why) => format!("request failed: {why}"),
        }
    }
}

enum Answer {
    Reply(String),
    Failed(Refusal),
}

async fn fetch_inner(ctx: Arc<Ctx>) -> ProviderUsage {
    let Some(credential) = super::pasted::credential(ID).and_then(|pasted| Credential::parse(&pasted))
    else {
        return ProviderUsage::failed(ID, NAME, super::pasted::needed(ID, "Devin token"));
    };

    let paths = credential.paths();
    if paths.is_empty() {
        return ProviderUsage::failed(
            ID,
            NAME,
            "the pasted credential names no organization — the endpoint is scoped to one, so \
             put it after the token (`auth1_… my-team`)",
        );
    }

    let mut last = Refusal::Unreachable("no path was tried".to_string());
    for path in &paths {
        match get(&ctx, path, &credential).await {
            // A reply is the answer, whatever it says: a refused token is
            // refused at every spelling of the path, so the next one is not
            // asked. Only a path that was not found is worth trying again.
            Answer::Reply(body) => return reading(&body),
            Answer::Failed(Refusal::ServerError) => last = Refusal::ServerError,
            Answer::Failed(refusal) => return ProviderUsage::failed(ID, NAME, refusal.message()),
        }
    }

    ProviderUsage::failed(ID, NAME, last.message())
}

async fn get(ctx: &Ctx, path: &str, credential: &Credential) -> Answer {
    let mut request = ctx
        .client
        .get(format!("{HOST}/api/{path}"))
        .header("Authorization", format!("Bearer {}", credential.token))
        .header("Accept", "application/json");

    // How the service picks between organizations on one account. Sent only
    // where the internal id is what was given — a slug is not one.
    if let Some(internal) = credential.internal_id() {
        request = request.header("x-cog-org-id", internal);
    }

    let response = match request.send().await {
        Ok(response) => response,
        Err(e) => return Answer::Failed(Refusal::Unreachable(describe_reqwest_error(&e))),
    };

    let status = response.status();
    let body = match response.text().await {
        Ok(body) => body,
        Err(e) => return Answer::Failed(Refusal::Unreachable(format!("cannot read body: {e}"))),
    };

    match status.as_u16() {
        200 => Answer::Reply(body),
        401 | 403 => Answer::Failed(Refusal::Refused),
        429 => Answer::Failed(Refusal::RateLimited),
        _ => Answer::Failed(Refusal::ServerError),
    }
}

// ---------------------------------------------------------------------------
// The reply
// ---------------------------------------------------------------------------

/// The mapping, kept apart from the request so a fixture can drive it.
///
/// Measured shape of the live reply:
///
/// ```json
/// { "daily_percentage": 2, "weekly_percentage": 1,
///   "daily_reset_at": "2026-09-14T00:00:00-08:00",
///   "weekly_reset_at": "2026-09-20T00:00:00-08:00",
///   "hide_daily_quota": false, "has_quota_allocation": true,
///   "is_quota_plan": true, "overage_balance": 10 }
/// ```
fn reading(body: &str) -> ProviderUsage {
    let reply: Value = match serde_json::from_str(body) {
        Ok(value @ Value::Object(_)) => value,
        Ok(_) => return ProviderUsage::failed(ID, NAME, "the reply is not a quota object"),
        Err(e) => return ProviderUsage::failed(ID, NAME, format!("bad JSON: {e}")),
    };

    let daily = figure(&reply, "daily_percentage");
    let weekly = figure(&reply, "weekly_percentage");
    let balance = figure(&reply, "overage_balance")
        .or_else(|| figure(&reply, "overage_balance_cents").map(|cents| cents / 100.0));

    // Neither window and no money is not a reply, whatever it parsed as.
    if daily.is_none() && weekly.is_none() && balance.is_none() {
        return ProviderUsage::failed(ID, NAME, "the reply could not be read");
    }

    let hidden = |key: &str| reply.get(key).and_then(Value::as_bool).unwrap_or(false);
    // Absent means yes: only an explicit `false` says there is no allowance,
    // and a field that stops being sent must not silently empty the rings.
    let allocated = reply
        .get("has_quota_allocation")
        .and_then(Value::as_bool)
        .unwrap_or(true);

    let mut windows: Vec<UsageWindow> = Vec::new();
    if allocated {
        if !hidden("hide_daily_quota") {
            if let Some(used) = daily {
                windows.push(
                    UsageWindow::new("Daily", Some(percent_from_fraction(used / 100.0)))
                        .with_reset(date(reply.get("daily_reset_at"))),
                );
            }
        }
        if !hidden("hide_weekly_quota") {
            if let Some(used) = weekly {
                windows.push(
                    UsageWindow::new("7d", Some(percent_from_fraction(used / 100.0)))
                        .with_reset(date(reply.get("weekly_reset_at"))),
                );
            }
        }
    }

    if let Some(balance) = balance.filter(|balance| balance.is_finite()) {
        windows.push(super::balance_window(
            "Balance",
            format!("{balance:.2} USD"),
        ));
    }

    if windows.is_empty() {
        return ProviderUsage::failed(ID, NAME, "no limits reported in the reply");
    }

    let plan = ["plan_name", "planName", "plan", "tier"]
        .iter()
        .find_map(|key| reply.get(*key).and_then(Value::as_str))
        .map(str::trim)
        .filter(|plan| !plan.is_empty())
        .map(str::to_string);

    ProviderUsage::ok(ID, NAME, windows).with_plan(plan)
}

fn figure(value: &Value, key: &str) -> Option<f64> {
    super::dig_number(value.get(key)).filter(|figure| figure.is_finite())
}

/// ISO 8601, epoch seconds, or epoch milliseconds — all three appear in the
/// accounts this API serves.
fn date(value: Option<&Value>) -> Option<String> {
    let value = value?;
    if let Some(text) = value.as_str() {
        if let Some(stamp) = parse_reset(value) {
            return Some(stamp);
        }
        return super::stamp_from_epoch_seconds(text.trim().parse::<f64>().ok()?);
    }
    // The original's own line: a figure past 10,000,000,000 in seconds is
    // really milliseconds.
    let raw = super::dig_number(Some(value)).filter(|raw| *raw > 0.0)?;
    super::stamp_from_epoch_seconds(if raw > 10_000_000_000.0 {
        raw / 1_000.0
    } else {
        raw
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn reads_the_two_percentages_the_endpoint_reports_as_used() {
        let reply = json!({
            "daily_percentage": 2,
            "weekly_percentage": 1,
            "daily_reset_at": "2026-09-14T00:00:00-08:00",
            "weekly_reset_at": 1_789_891_200,
            "has_quota_allocation": true,
            "overage_balance": 10
        })
        .to_string();

        let usage = reading(&reply);
        let labels: Vec<&str> = usage.windows.iter().map(|w| w.label.as_str()).collect();
        assert_eq!(labels, vec!["Daily", "7d", "Balance"]);
        assert_eq!(usage.windows[0].percent_used, Some(2.0));
        assert_eq!(usage.windows[1].percent_used, Some(1.0));
        assert_eq!(
            usage.windows[0].resets_at.as_deref(),
            Some("2026-09-14T08:00:00Z")
        );
        // The balance is money and draws no ring.
        assert_eq!(usage.windows[2].percent_used, None);
        assert_eq!(usage.windows[2].detail.as_deref(), Some("10.00 USD"));
    }

    #[test]
    fn a_hidden_or_unallocated_quota_draws_nothing() {
        let hidden = json!({
            "daily_percentage": 2,
            "weekly_percentage": 3,
            "hide_daily_quota": true
        })
        .to_string();
        let usage = reading(&hidden);
        assert_eq!(usage.windows.len(), 1);
        assert_eq!(usage.windows[0].label, "7d");

        let no_allocation = json!({
            "daily_percentage": 2,
            "weekly_percentage": 1,
            "has_quota_allocation": false
        })
        .to_string();
        // The percentages beside a plan with no allowance are not a reading.
        assert!(reading(&no_allocation).error.is_some());
    }

    #[test]
    fn a_plan_name_is_the_replys_own() {
        let reply = json!({ "weekly_percentage": 4, "plan_name": "Pro" }).to_string();
        assert_eq!(reading(&reply).plan.as_deref(), Some("Pro"));
    }

    #[test]
    fn a_reply_with_nothing_in_it_is_unreadable() {
        assert!(reading(&json!({ "is_quota_plan": true }).to_string()).error.is_some());
        assert!(reading("[1, 2]").error.is_some());
        assert!(reading("not json").error.is_some());
    }

    #[test]
    fn a_pasted_credential_is_a_token_and_an_organization() {
        let bare = Credential::parse("auth1_abcdefghijklmnopqrstuvwxyz").unwrap();
        assert_eq!(bare.token, "auth1_abcdefghijklmnopqrstuvwxyz");
        assert_eq!(bare.organization, None);

        let two = Credential::parse("  auth1_abcdefghijklmnopqrstuvwxyz  my-team ").unwrap();
        assert_eq!(two.organization.as_deref(), Some("org/my-team"));

        let header = Credential::parse("Authorization: Bearer auth1_abcdefghijklmnopqrstuvwxyz org_1a2b3c")
            .unwrap();
        assert_eq!(header.token, "auth1_abcdefghijklmnopqrstuvwxyz");
        assert_eq!(header.organization.as_deref(), Some("organizations/org_1a2b3c"));

        let url = Credential::parse(
            "auth1_abcdefghijklmnopqrstuvwxyz https://app.devin.ai/org/my-team/settings",
        )
        .unwrap();
        assert_eq!(url.organization.as_deref(), Some("org/my-team"));

        assert!(Credential::parse("   ").is_none());
    }

    #[test]
    fn a_value_copied_out_of_local_storage_is_read_too() {
        let stored = json!({
            "token": "auth1_abcdefghijklmnopqrstuvwxyz",
            "userId": "user-1",
            "organization": "org-0123456789abcdef0123456789abcdef"
        })
        .to_string();

        let credential = Credential::parse(&stored).unwrap();
        assert_eq!(credential.token, "auth1_abcdefghijklmnopqrstuvwxyz");
        assert_eq!(
            credential.organization.as_deref(),
            Some("organizations/org-0123456789abcdef0123456789abcdef")
        );
    }

    #[test]
    fn the_paths_try_every_spelling_of_the_segment() {
        let internal =
            Credential::parse("auth1_abcdefghijklmnopqrstuvwxyz org_1a2b3c").unwrap();
        assert_eq!(
            internal.paths(),
            vec![
                "org_1a2b3c/billing/quota/usage",
                "organizations/org_1a2b3c/billing/quota/usage"
            ]
        );

        let slug = Credential::parse("auth1_abcdefghijklmnopqrstuvwxyz my-team").unwrap();
        assert_eq!(
            slug.paths(),
            vec!["org/my-team/billing/quota/usage", "my-team/billing/quota/usage"]
        );

        let none = Credential::parse("auth1_abcdefghijklmnopqrstuvwxyz").unwrap();
        assert!(none.paths().is_empty());
    }

    #[test]
    fn a_reset_is_read_as_a_stamp_seconds_or_milliseconds() {
        assert_eq!(
            date(Some(&json!("2026-09-14T00:00:00Z"))).as_deref(),
            Some("2026-09-14T00:00:00Z")
        );
        assert_eq!(
            date(Some(&json!(1_789_891_200))).as_deref(),
            Some("2026-09-20T08:00:00Z")
        );
        assert_eq!(
            date(Some(&json!(1_789_891_200_000i64))).as_deref(),
            Some("2026-09-20T08:00:00Z")
        );
        assert_eq!(date(Some(&json!(""))), None);
        assert_eq!(date(Some(&json!(0))), None);
    }
}
