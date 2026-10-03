//! Reads the Grok account's usage.
//!
//! The same borrowing the two CLI routes do: Grok Build's CLI stores an OIDC
//! login in `~/.grok/auth.json`, and this asks the CLI's own proxy with it —
//! `GET cli-chat-proxy.grok.com/v1/billing?format=credits`. Nothing is held
//! here: the file belongs to whichever account the CLI is signed in to.
//!
//! **What comes back is the account's pool, not the CLI's.** A paid Grok plan
//! spends one weekly pool across every Grok product — the web chat, Imagine,
//! voice, the API and Grok Build alike — and the reply's `productUsage`
//! breakdown is the proof: it lists `GrokChat` beside `GrokTasks`. So the ring
//! is what this xAI *account* has spent this week, which is why the provider is
//! called Grok rather than Grok Build. There is no per-product limit to show
//! instead; the breakdown is shares of the one pool, and drawing them as
//! separate windows would claim four limits where there is one.
//!
//! **An absent percentage means zero, and that is read off this reply rather
//! than assumed.** The payload is proto3 serialised to JSON with implicit
//! presence, so a zero is simply left out — visible in the same reply, where
//! the `GrokChat` entry carries a product name and no `usagePercent` at all
//! while `GrokTasks` carries one. Taking a missing figure as "no reading" would
//! blank the ring for the first hours of every week.
//!
//! Two caveats, both shared with the other borrowed routes. Neither endpoint is
//! public API — they are what the CLI itself calls, and can change without
//! notice. And the stored token lasts about six hours: the CLI renews it while
//! you use Grok, and nothing renews it for PulseWin, so an aged-out login is
//! reported rather than worked around.
//!
//! VERIFY ON A REAL MACHINE: set `PULSEWIN_GROK_TOKEN` to bypass the file, and
//! `PULSEWIN_DEBUG=1` to dump a payload that fails to parse.

use std::sync::Arc;
use std::time::Duration;

use chrono::{DateTime, Utc};
use serde_json::Value;

use super::{describe_reqwest_error, percent_from_scale, Ctx, FetchFuture, Provider};
use crate::credentials;
use crate::model::{parse_reset, ProviderUsage, UsageWindow};

const BILLING_URL: &str = "https://cli-chat-proxy.grok.com/v1/billing?format=credits";

/// The plan's name comes from here, not from the billing reply.
const SETTINGS_URL: &str = "https://cli-chat-proxy.grok.com/v1/settings";

pub struct Grok;

impl Provider for Grok {
    fn id(&self) -> &'static str {
        "grok"
    }

    fn name(&self) -> &'static str {
        "Grok"
    }

    /// A login the CLI left that has **not aged out**.
    ///
    /// The file merely existing is not enough and the difference is the whole
    /// message: `select_login` picks the freshest unexpired entry, and an
    /// aged-out one draws no card at all because there is nothing to read with
    /// — the same verdict the fetch would reach before it made a request.
    fn is_configured(&self) -> bool {
        matches!(resolve_login(), Login::Usable(_))
    }

    fn fetch(&self, ctx: Arc<Ctx>) -> FetchFuture {
        Box::pin(async move { fetch_inner(ctx, None).await })
    }
}

/// What the CLI wrote at its last `grok login`.
enum Login {
    None,
    Expired,
    Usable(String),
}

pub(crate) async fn fetch_inner(ctx: Arc<Ctx>, explicit: Option<serde_json::Value>) -> ProviderUsage {
    const ID: &str = "grok";
    const NAME: &str = "Grok";

    let login=if let Some(document)=explicit {match account_token(&document) {Ok(token)=>Login::Usable(token),Err(_)=>Login::Expired}} else {resolve_login()};
    let token = match login {
        Login::None => {
            return ProviderUsage::failed(
                ID,
                NAME,
                "no Grok login found — run `grok login` once (looked for ~/.grok/auth.json)",
            )
        }
        Login::Expired => {
            return ProviderUsage::failed(
                ID,
                NAME,
                "the stored Grok login has expired — run `grok login` again to renew it",
            )
        }
        Login::Usable(token) => token,
    };

    let response = request(&ctx, BILLING_URL, &token)
        .timeout(Duration::from_secs(15))
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
            (401, " — the Grok login expired; run `grok login` again"),
            (403, " — the Grok login expired; run `grok login` again"),
            (429, " — rate limited, try again shortly"),
        ];
        return ProviderUsage::failed(ID, NAME, super::http_failure(status, hints));
    }

    let json: Value = match serde_json::from_str(&body) {
        Ok(v) => v,
        Err(e) => return ProviderUsage::failed(ID, NAME, format!("bad JSON: {e}")),
    };

    super::claude_code::debug_dump(ID, &json);

    let Some(config) = json.get("config") else {
        return ProviderUsage::failed(ID, NAME, "no billing config in response");
    };

    let Some(window) = window_from(config, Utc::now()) else {
        return ProviderUsage::failed(ID, NAME, "no billing period in response");
    };

    // `prepaidBalance` and `onDemandCap` are both denominated in a unit the
    // reply never names, and the cap is an allowance rather than a balance —
    // so no money line is invented from either.
    ProviderUsage::ok(ID, NAME, vec![window])
        .with_plan(plan(&ctx, &token).await)
        .with_account(Some("CLI login".to_string()))
}

/// Authorization plus the header the CLI sends.
///
/// Without `x-xai-token-auth` the proxy answers the enterprise credit shape
/// instead, whose `monthlyLimit` is zero on a personal plan — a denominator of
/// nothing, and no percentage at all.
fn request(ctx: &Ctx, url: &str, token: &str) -> reqwest::RequestBuilder {
    ctx.client
        .get(url)
        .header("Authorization", format!("Bearer {token}"))
        .header("x-xai-token-auth", "xai-grok-cli")
        .header("Accept", "application/json")
}

/// A second call, because the billing reply does not name the plan.
///
/// Optional enrichment on a short budget: the usage figures are already in hand
/// by the time this runs, and a stalled settings call must not hold them back.
async fn plan(ctx: &Ctx, token: &str) -> Option<String> {
    let response = request(ctx, SETTINGS_URL, token)
        .timeout(Duration::from_secs(4))
        .send()
        .await
        .ok()?;

    if !response.status().is_success() {
        return None;
    }

    let json: Value = serde_json::from_str(&response.text().await.ok()?).ok()?;
    credentials::dig_str(&json, "subscription_tier_display")
        .map(|tier| tier.trim().to_string())
        .filter(|tier| !tier.is_empty())
}

fn auth_path() -> Option<std::path::PathBuf> {
    credentials::home_relative(&[".grok", "auth.json"])
        .into_iter()
        .next()
}

pub(crate) fn account_token(document:&Value)->Result<String,String> {
    // Imported CLI documents retain the issuer entries and their real expiry.
    if document.as_object().is_some_and(|entries|entries.values().any(|v|v.get("key").is_some())) {
        return match select_login(document,Utc::now()) {Login::Usable(token)=>Ok(token),Login::Expired=>Err("Grok 登录已过期，请重新导入登录。".into()),Login::None=>Err("没有找到 Grok 登录。".into())};
    }
    crate::accounts::token(document).ok_or_else(||"没有找到 Grok Token。".into())
}
fn resolve_login() -> Login {
    // The port's way in for a machine whose CLI file lives somewhere else, and
    // the only route that does not need the CLI installed at all.
    if let Some(token) = credentials::env_override("grok", "token") {
        return Login::Usable(token);
    }

    if let Some(token)=crate::settings::read_credential("grok").api_key {return Login::Usable(token);}
    let Some(path) = auth_path() else {
        return Login::None;
    };
    let Some(root) = credentials::read_json(&path) else {
        return Login::None;
    };

    select_login(&root, Utc::now())
}

/// The file is keyed by issuer and client id rather than by account, and may
/// hold more than one entry, so the freshest unexpired one is taken.
///
/// An entry that has aged out is kept as evidence: "signed in, and the login has
/// gone stale" is a different instruction from "never signed in", and the
/// difference is the whole message.
///
/// No expiry stated is not the same as expired — the token is simply undated,
/// and the service is the one that decides. It is kept as a last resort and
/// **cannot outrank a dated one**: treating an undated entry as valid forever
/// meant a file holding an undated entry before a perfectly good dated one used
/// the wrong token. So an undated entry loses to any live dated one, and only
/// wins when nothing dated is usable.
fn select_login(root: &Value, now: DateTime<Utc>) -> Login {
    let Some(entries) = root.as_object() else {
        return Login::None;
    };

    let mut saw_entry = false;
    let mut undated: Option<String> = None;
    let mut best: Option<(String, DateTime<Utc>)> = None;

    for entry in entries.values() {
        let Some(token) = entry.get("key").and_then(|v| v.as_str()) else {
            continue;
        };
        if token.is_empty() {
            continue;
        }
        saw_entry = true;

        let Some(expiry) = entry.get("expires_at").and_then(parse_reset) else {
            undated.get_or_insert_with(|| token.to_string());
            continue;
        };
        let Ok(expiry) = DateTime::parse_from_rfc3339(&expiry) else {
            continue;
        };
        let expiry = expiry.with_timezone(&Utc);
        if expiry <= now {
            continue;
        }
        if best
            .as_ref()
            .map(|(_, current)| expiry > *current)
            .unwrap_or(true)
        {
            best = Some((token.to_string(), expiry));
        }
    }

    if let Some((token, _)) = best {
        return Login::Usable(token);
    }
    if let Some(token) = undated {
        return Login::Usable(token);
    }
    if saw_entry {
        return Login::Expired;
    }
    Login::None
}

/// The one window the pool amounts to.
///
/// Both ends of the period are stated, so the length is measured from them
/// rather than assumed from the period's name. `currentPeriod` is preferred
/// over the flat `billingPeriod*` pair because it is the one that says which
/// period is *current*; the flat pair is the fallback for a reply that omits it.
fn window_from(config: &Value, now: DateTime<Utc>) -> Option<UsageWindow> {
    let period = config.get("currentPeriod");
    let start = period
        .and_then(|p| p.get("start"))
        .or_else(|| config.get("billingPeriodStart"))
        .and_then(parse_reset)
        .and_then(|s| DateTime::parse_from_rfc3339(&s).ok())
        .map(|dt| dt.with_timezone(&Utc))?;

    let end = period
        .and_then(|p| p.get("end"))
        .or_else(|| config.get("billingPeriodEnd"))
        .and_then(parse_reset)
        .and_then(|s| DateTime::parse_from_rfc3339(&s).ok())
        .map(|dt| dt.with_timezone(&Utc))?;

    if end <= start {
        return None;
    }

    let seconds = (end - start).num_seconds();

    // An omitted percentage is a zero the serialiser dropped — but only inside
    // a period that is actually running. A reply describing a period that has
    // ended says nothing about what has been spent in the one that followed it,
    // and reading that as 0% would report an empty pool as a full one.
    let percent = match config.get("creditUsagePercent").and_then(|v| v.as_f64()) {
        Some(percent) => percent,
        None if now >= start && now <= end => 0.0,
        None => return None,
    };

    Some(
        UsageWindow::new(label_for(seconds), Some(percent_from_scale(percent)))
            .with_reset(Some(end.to_rfc3339_opts(chrono::SecondsFormat::Secs, true))),
    )
}

/// Named from the length the reply gave, not from its `type` string: the enum
/// is theirs to rename, the two timestamps are arithmetic. A period that is
/// neither of the two familiar lengths is still shown, under a heading built
/// from its own duration.
fn label_for(seconds: i64) -> String {
    match seconds {
        518_400..=691_200 => "7d".to_string(),
        2_332_800..=2_764_800 => "Monthly".to_string(),
        other => super::humanize_window_seconds(other),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn at(text: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(text).unwrap().with_timezone(&Utc)
    }

    /// The CLI's file, keyed by issuer and client id.
    fn auth_fixture() -> Value {
        json!({
            "https://auth.x.ai::b1a00492-073a-47ea-816f-4c329264a828": {
                "key": "stale-token",
                "expires_at": "2026-01-01T00:00:00Z"
            },
            "https://accounts.x.ai/sign-in": {
                "key": "fresh-token",
                "expires_at": "2026-10-01T23:00:00Z"
            }
        })
    }

    #[test]
    fn takes_the_freshest_unexpired_entry() {
        match select_login(&auth_fixture(), at("2026-10-01T12:00:00Z")) {
            Login::Usable(token) => assert_eq!(token, "fresh-token"),
            _ => panic!("expected a usable login"),
        }
    }

    #[test]
    fn an_aged_out_entry_is_evidence_rather_than_absence() {
        match select_login(&auth_fixture(), at("2027-01-01T00:00:00Z")) {
            Login::Expired => {}
            _ => panic!("expected an expired login"),
        }
    }

    #[test]
    fn an_undated_token_cannot_outrank_a_dated_one() {
        let root = json!({
            "first": { "key": "undated" },
            "second": { "key": "dated", "expires_at": "2026-10-01T23:00:00Z" }
        });
        match select_login(&root, at("2026-10-01T12:00:00Z")) {
            Login::Usable(token) => assert_eq!(token, "dated"),
            _ => panic!("expected the dated token"),
        }
    }

    #[test]
    fn an_undated_token_is_used_when_nothing_dated_is_left() {
        let root = json!({ "only": { "key": "undated" } });
        match select_login(&root, at("2026-10-01T12:00:00Z")) {
            Login::Usable(token) => assert_eq!(token, "undated"),
            _ => panic!("expected the undated token to be the last resort"),
        }
    }

    #[test]
    fn an_empty_file_is_no_login_at_all() {
        match select_login(&json!({}), at("2026-10-01T12:00:00Z")) {
            Login::None => {}
            _ => panic!("expected no login"),
        }
    }

    #[test]
    fn reads_the_pool_from_the_current_period() {
        let config = json!({
            "currentPeriod": { "start": "2026-09-28T00:00:00+00:00",
                               "end": "2026-10-05T00:00:00+00:00" },
            "creditUsagePercent": 37.5
        });
        let window = window_from(&config, at("2026-10-01T12:00:00Z")).unwrap();
        assert_eq!(window.label, "7d");
        assert_eq!(window.percent_used, Some(37.5));
        assert_eq!(window.resets_at.as_deref(), Some("2026-10-05T00:00:00Z"));
    }

    #[test]
    fn an_omitted_percentage_inside_a_running_period_is_zero() {
        let config = json!({
            "currentPeriod": { "start": "2026-09-28T00:00:00Z", "end": "2026-10-05T00:00:00Z" }
        });
        let window = window_from(&config, at("2026-10-01T12:00:00Z")).unwrap();
        assert_eq!(window.percent_used, Some(0.0));
    }

    #[test]
    fn an_ended_period_with_no_percentage_reports_nothing() {
        // Reading that as 0% would report an empty pool as a full one.
        let config = json!({
            "currentPeriod": { "start": "2026-09-01T00:00:00Z", "end": "2026-09-08T00:00:00Z" }
        });
        assert!(window_from(&config, at("2026-10-01T12:00:00Z")).is_none());
    }

    #[test]
    fn the_flat_period_pair_is_the_fallback() {
        let config = json!({
            "billingPeriodStart": "2026-09-28T00:00:00Z",
            "billingPeriodEnd": "2026-10-05T00:00:00Z",
            "creditUsagePercent": 0.0
        });
        let window = window_from(&config, at("2026-10-01T12:00:00Z")).unwrap();
        assert_eq!(window.percent_used, Some(0.0));
    }

    #[test]
    fn a_period_that_is_neither_familiar_length_is_still_shown() {
        let config = json!({
            "currentPeriod": { "start": "2026-09-30T00:00:00Z", "end": "2026-10-02T00:00:00Z" },
            "creditUsagePercent": 10.0
        });
        let window = window_from(&config, at("2026-10-01T12:00:00Z")).unwrap();
        assert_eq!(window.label, "2d");
    }

    #[test]
    fn a_month_long_pool_is_named_monthly() {
        assert_eq!(label_for(2_592_000), "Monthly");
        assert_eq!(label_for(604_800), "7d");
    }
}
