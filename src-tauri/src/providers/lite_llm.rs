//! LiteLLM, a self-hosted proxy: the budget on the user a virtual key belongs
//! to, and the budget on its team, each as spend against a limit the proxy
//! states.
//!
//! Read with the virtual key and the address the reader names, from three of
//! LiteLLM's own routes: `GET /key/info` names the key's user and team, then
//! `GET /user/info?user_id=…` — or, for a key with a team and no user,
//! `GET /team/info?team_id=…` — carries the spend and the budget. The key goes
//! nowhere but that address. The shapes are second-hand — taken from CodexBar's
//! LiteLLM provider and its tests, not from a captured reply — and the fixtures
//! below say so.
//!
//! **A key and an address are both required, and this port has no settings
//! window to type them into.** They arrive from `PULSEWIN_LITELLM_KEY` and
//! `PULSEWIN_LITELLM_BASE_URL`, or from one
//! `%APPDATA%\PulseWin\litellm.json` holding `apiKey` and `baseUrl`; the
//! address may be the proxy's root or the `/v1` base a client is configured
//! with, and these routes sit beside either. `is_configured` asks for both —
//! a key with nowhere to send it is not a credential — and asks only from disk,
//! never over the network. The address itself is checked before the key is
//! attached to it: see `super::gateway_url`.
//!
//! **The team's budget is drawn first**, because it is the one the proxy
//! enforces on the key. A user or team with no `max_budget` has spend and
//! nothing to measure it against, and is left off. The ids the proxy answers
//! with are checked against the ones `/key/info` named, so another user's or
//! another team's budget is never drawn as this key's.
//!
//! A deployment that will not let a key read its own information (403, 404) has
//! no budget this app can read: the reference falls back to a month's spend
//! report there, which is spend with no limit behind it, and this app has
//! nowhere to show that.

use std::sync::Arc;

use serde_json::Value;

use super::{percent_from_fraction, Ctx, FetchFuture, Provider};
use crate::credentials;
use crate::model::{parse_reset, ProviderUsage, UsageWindow};

const KEY_INFO_PATH: &str = "/key/info";
const USER_INFO_PATH: &str = "/user/info";
const TEAM_INFO_PATH: &str = "/team/info";

pub struct LiteLlm;

impl Provider for LiteLlm {
    fn id(&self) -> &'static str {
        "litellm"
    }

    fn name(&self) -> &'static str {
        "LiteLLM"
    }

    /// The key **and** the address the fetch needs. See the module docs.
    fn is_configured(&self) -> bool {
        super::gateway_configured(self.id())
    }

    fn fetch(&self, ctx: Arc<Ctx>) -> FetchFuture {
        Box::pin(async move { fetch_inner(ctx).await })
    }
}

async fn fetch_inner(ctx: Arc<Ctx>) -> ProviderUsage {
    const ID: &str = "litellm";
    const NAME: &str = "LiteLLM";

    let Some(key) = super::provider_key(ID) else {
        return ProviderUsage::failed(ID, NAME, super::missing_key(ID, ""));
    };
    let Some(address) = super::provider_base_url(ID) else {
        return ProviderUsage::failed(ID, NAME, super::missing_address(ID));
    };
    let Some(key_info_url) = super::gateway_url(&address, KEY_INFO_PATH, &["/v1"]) else {
        return ProviderUsage::failed(ID, NAME, super::refused_address());
    };

    let (status, body) = match reply(&ctx, &key_info_url, &key, None).await {
        Ok(reply) => reply,
        Err(problem) => return ProviderUsage::failed(ID, NAME, problem),
    };

    // A deployment that will not let a key read itself is not a refused key:
    // there is simply no budget this app can read, and saying "refused" would
    // send the reader to replace a key that is fine.
    if status == reqwest::StatusCode::FORBIDDEN || status == reqwest::StatusCode::NOT_FOUND {
        return ProviderUsage::failed(
            ID,
            NAME,
            "no limits reported — this deployment does not let a key read its own information",
        );
    }
    if !status.is_success() {
        return ProviderUsage::failed(ID, NAME, problem(status));
    }

    let key_info: Value = match serde_json::from_str(&body) {
        Ok(v) => v,
        Err(e) => return ProviderUsage::failed(ID, NAME, format!("bad JSON: {e}")),
    };

    super::claude_code::debug_dump(ID, &key_info);

    let Some(identity) = identity(&key_info) else {
        return ProviderUsage::failed(ID, NAME, "no key information in response");
    };

    let (path, query) = match (identity.user.as_deref(), identity.team.as_deref()) {
        (Some(user), _) => (USER_INFO_PATH, ("user_id", user)),
        (None, Some(team)) => (TEAM_INFO_PATH, ("team_id", team)),
        // A key bound to neither — a master key — has no budget of its own.
        (None, None) => {
            return ProviderUsage::failed(
                ID,
                NAME,
                "no limits reported — this key belongs to neither a user nor a team",
            )
        }
    };

    let Some(url) = super::gateway_url(&address, path, &["/v1"]) else {
        return ProviderUsage::failed(ID, NAME, super::refused_address());
    };

    let (status, body) = match reply(&ctx, &url, &key, Some(query)).await {
        Ok(reply) => reply,
        Err(problem) => return ProviderUsage::failed(ID, NAME, problem),
    };
    if !status.is_success() {
        return ProviderUsage::failed(ID, NAME, problem(status));
    }

    let json: Value = match serde_json::from_str(&body) {
        Ok(v) => v,
        Err(e) => return ProviderUsage::failed(ID, NAME, format!("bad JSON: {e}")),
    };

    super::claude_code::debug_dump(ID, &json);

    match identity.user.as_deref() {
        Some(user) => reading_user_info(&json, user, identity.team.as_deref()),
        None => match identity.team.as_deref() {
            Some(team) => reading_team_info(&json, team),
            None => ProviderUsage::failed(ID, NAME, "no limits reported"),
        },
    }
}

/// One request with the key on it.
///
/// The body is returned whatever the status was, because two of these routes
/// carry meaning in it — `/key/info` says "no budget you can read" with a 403
/// or a 404, which `classify` would otherwise read as a refused key.
///
/// Sent on the gateway client, so a redirect cannot move the key to another
/// host after the address has been checked: a 3xx comes back as a 3xx and is
/// read as the credential being turned away.
async fn reply(
    ctx: &Ctx,
    url: &str,
    key: &str,
    query: Option<(&str, &str)>,
) -> Result<(reqwest::StatusCode, String), String> {
    let mut request = ctx
        .gateway_client
        .get(url)
        .header("Authorization", format!("Bearer {key}"))
        .header("Accept", "application/json");

    if let Some((name, value)) = query {
        request = request.query(&[(name, value)]);
    }

    let response = request.send().await.map_err(|e| {
        format!("request failed: {}", super::describe_reqwest_error(&e))
    })?;

    let status = response.status();
    let body = response
        .text()
        .await
        .map_err(|e| format!("cannot read body: {e}"))?;

    Ok((status, body))
}

/// What a status means apart from the request, which is the reference's
/// `ProfileHTTP.classify` and nothing else.
fn problem(status: reqwest::StatusCode) -> String {
    if status.is_redirection()
        || status == reqwest::StatusCode::UNAUTHORIZED
        || status == reqwest::StatusCode::FORBIDDEN
    {
        "the proxy refused the key".to_string()
    } else if status == reqwest::StatusCode::TOO_MANY_REQUESTS {
        "rate limited, try again shortly".to_string()
    } else {
        "the proxy returned an error".to_string()
    }
}

/// The user and the team `/key/info` names.
struct Identity {
    user: Option<String>,
    team: Option<String>,
}

/// Blank names are none: an empty string is a field the proxy filled in with
/// nothing, not an account called "".
fn identity(json: &Value) -> Option<Identity> {
    let info = json.get("info")?;

    let named = |path: &str| {
        credentials::dig_str(info, path)
            .map(|text| text.trim().to_string())
            .filter(|text| !text.is_empty())
    };

    Some(Identity {
        user: named("user_id"),
        team: named("team_id"),
    })
}

/// The user's own budget, and its team's.
///
/// The team first, and **not sorted**: the reference puts the enforced pool
/// ahead of the member's own, and a period is not what decides which matters.
fn reading_user_info(json: &Value, user: &str, team: Option<&str>) -> ProviderUsage {
    const ID: &str = "litellm";
    const NAME: &str = "LiteLLM";

    let Some(user_budget) = json.get("user_info") else {
        return ProviderUsage::failed(ID, NAME, "no budget in response");
    };

    // Whichever id the proxy answered with has to be the one asked about.
    let answered = credentials::dig_str(user_budget, "user_id")
        .or_else(|| credentials::dig_str(json, "user_id"));
    if answered.is_some_and(|answered| answered != user) {
        return ProviderUsage::failed(ID, NAME, "the reply is not for this key's user");
    }

    let mut windows = Vec::new();

    if let Some(team) = team {
        if let Some(found) = json
            .get("teams")
            .and_then(Value::as_array)
            .and_then(|teams| {
                teams
                    .iter()
                    .find(|budget| credentials::dig_str(budget, "team_id").as_deref() == Some(team))
            })
        {
            if let Some(window) = budget_window(found, true) {
                windows.push(window);
            }
        }
    }

    if let Some(window) = budget_window(user_budget, false) {
        windows.push(window);
    }

    if windows.is_empty() {
        return ProviderUsage::failed(ID, NAME, "no limits reported");
    }

    ProviderUsage::ok(ID, NAME, windows)
}

/// A key bound to a team and no user: the team's own budget is the whole
/// reading.
fn reading_team_info(json: &Value, team: &str) -> ProviderUsage {
    const ID: &str = "litellm";
    const NAME: &str = "LiteLLM";

    let Some(budget) = json.get("team_info") else {
        return ProviderUsage::failed(ID, NAME, "no budget in response");
    };

    let answered = credentials::dig_str(budget, "team_id")
        .or_else(|| credentials::dig_str(json, "team_id"));
    if answered.is_some_and(|answered| answered != team) {
        return ProviderUsage::failed(ID, NAME, "the reply is not for this key's team");
    }

    let Some(window) = budget_window(budget, true) else {
        return ProviderUsage::failed(ID, NAME, "no limits reported");
    };

    ProviderUsage::ok(ID, NAME, vec![window])
}

/// A budget's spend against its limit.
///
/// Spend with no limit beside it is a figure nothing can be a fraction of, so
/// it is left off rather than drawn against a denominator nobody stated. The
/// team's pool is its own kind of allowance and keeps its own heading, so the
/// two rows are told apart and never read as one.
fn budget_window(budget: &Value, team: bool) -> Option<UsageWindow> {
    let spend = credentials::dig_f64(budget, "spend").filter(|v| v.is_finite() && *v >= 0.0)?;
    let limit = credentials::dig_f64(budget, "max_budget").filter(|v| v.is_finite() && *v > 0.0)?;

    let label = if team {
        "Team".to_string()
    } else {
        period(credentials::dig_str(budget, "budget_duration").as_deref())
    };

    Some(
        UsageWindow::new(label, Some(percent_from_fraction(spend / limit)))
            .with_reset(budget.get("budget_reset_at").and_then(parse_reset)),
    )
}

/// LiteLLM's `budget_duration`: a count and a unit — `30s`, `12h`, `7d`, `1mo`.
///
/// Seconds, minutes, hours, days and weeks are a stated length and are named
/// from it; a month is a calendar month and is named as one only when it is
/// exactly one, because `3mo` is a quarter and calling it "Monthly" would state
/// a period the proxy did not. No duration at all is a budget that never turns
/// over, and is named as plain spend.
fn period(duration: Option<&str>) -> String {
    let unstated = || "Spend".to_string();

    let Some(text) = duration
        .map(|text| text.trim().to_lowercase())
        .filter(|text| !text.is_empty())
    else {
        return unstated();
    };

    let digits: String = text.chars().take_while(char::is_ascii_digit).collect();
    let Ok(count) = digits.parse::<i64>() else {
        return unstated();
    };
    if count <= 0 {
        return unstated();
    }

    let seconds = match &text[digits.len()..] {
        "mo" => {
            return if count == 1 {
                "Monthly".to_string()
            } else {
                unstated()
            }
        }
        "s" => count,
        "m" => count * 60,
        "h" => count * 3_600,
        "d" => count * 86_400,
        "w" => count * 7 * 86_400,
        _ => return unstated(),
    };

    label_for_seconds(seconds)
}

/// The heading for a length the proxy stated.
///
/// The three familiar lengths keep the names the rest of the port uses for
/// them; anything else is headed by its own duration, **never below an hour** —
/// which is the finest unit anything here states, and the reason `30s` reads as
/// an hour rather than as nothing at all. That floor is the original's, from
/// `UsageWindow.name`.
fn label_for_seconds(seconds: i64) -> String {
    match seconds {
        18_000 => "5h".to_string(),
        86_400 => "24h".to_string(),
        604_800 => "7d".to_string(),
        other if other >= 86_400 && other % 86_400 == 0 => {
            format!("{}d", other / 86_400)
        }
        other => format!("{}h", ((other as f64 / 3_600.0).round() as i64).max(1)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// A budget as LiteLLM keeps it on a user or a team.
    fn budget(spend: f64, limit: f64, duration: Option<&str>) -> Value {
        let mut value = json!({ "spend": spend, "max_budget": limit });
        if let Some(duration) = duration {
            value["budget_duration"] = json!(duration);
        }
        value
    }

    #[test]
    fn names_the_user_and_the_team_the_key_belongs_to() {
        let named = identity(&json!({ "info": { "user_id": "u-1", "team_id": " t-9 " } })).unwrap();
        assert_eq!(named.user.as_deref(), Some("u-1"));
        assert_eq!(named.team.as_deref(), Some("t-9"));

        // Blank is none, and no `info` at all is not an identity.
        let blank = identity(&json!({ "info": { "user_id": "  ", "team_id": "" } })).unwrap();
        assert!(blank.user.is_none() && blank.team.is_none());
        assert!(identity(&json!({})).is_none());
    }

    #[test]
    fn draws_the_teams_pool_before_the_members_own() {
        let reply = json!({
            "user_id": "u-1",
            "user_info": budget(5.0, 10.0, Some("5h")),
            "teams": [ { "team_id": "t-9", "spend": 300.0, "max_budget": 1000.0, "budget_duration": "1mo" } ]
        });

        let usage = reading_user_info(&reply, "u-1", Some("t-9"));
        let labels: Vec<&str> = usage.windows.iter().map(|w| w.label.as_str()).collect();
        assert_eq!(labels, vec!["Team", "5h"]);
        assert_eq!(usage.windows[0].percent_used, Some(30.0));
        assert_eq!(usage.windows[1].percent_used, Some(50.0));
    }

    #[test]
    fn a_budget_with_no_limit_is_left_off_rather_than_drawn_against_nothing() {
        let reply = json!({
            "user_id": "u-1",
            "user_info": { "spend": 4.0 },
            "teams": [ { "team_id": "t-9", "spend": 3.0, "max_budget": 0.0 } ]
        });
        assert!(reading_user_info(&reply, "u-1", Some("t-9")).error.is_some());

        // The team is unreadable but the user's own budget is not: it is still
        // a reading.
        let reply = json!({
            "user_id": "u-1",
            "user_info": budget(5.0, 10.0, None),
            "teams": [ { "team_id": "t-9", "spend": 3.0 } ]
        });
        let usage = reading_user_info(&reply, "u-1", Some("t-9"));
        assert_eq!(usage.windows.len(), 1);
        assert_eq!(usage.windows[0].label, "Spend");
    }

    #[test]
    fn another_users_budget_is_never_drawn_as_this_keys() {
        let reply = json!({
            "user_id": "u-2",
            "user_info": budget(5.0, 10.0, None)
        });
        assert!(reading_user_info(&reply, "u-1", None).error.is_some());

        // Answered by `user_info` rather than by the envelope.
        let reply = json!({
            "user_info": { "user_id": "u-2", "spend": 5.0, "max_budget": 10.0 }
        });
        assert!(reading_user_info(&reply, "u-1", None).error.is_some());

        // No id at all is not a contradiction: the proxy simply did not say.
        let reply = json!({ "user_info": budget(5.0, 10.0, None) });
        assert!(reading_user_info(&reply, "u-1", None).error.is_none());
    }

    #[test]
    fn a_key_with_a_team_and_no_user_reads_the_teams_budget() {
        let reply = json!({
            "team_id": "t-9",
            "team_info": budget(300.0, 1000.0, Some("1mo"))
        });
        let usage = reading_team_info(&reply, "t-9");
        assert_eq!(usage.windows.len(), 1);
        assert_eq!(usage.windows[0].label, "Team");
        assert_eq!(usage.windows[0].percent_used, Some(30.0));

        assert!(reading_team_info(&reply, "t-1").error.is_some());
        assert!(reading_team_info(&json!({ "team_id": "t-9" }), "t-9").error.is_some());
    }

    #[test]
    fn a_reset_is_carried_across_when_the_proxy_states_one() {
        let reply = json!({
            "user_id": "u-1",
            "user_info": {
                "spend": 5.0, "max_budget": 10.0,
                "budget_reset_at": "2026-10-05T00:00:00Z"
            }
        });
        let usage = reading_user_info(&reply, "u-1", None);
        assert_eq!(
            usage.windows[0].resets_at.as_deref(),
            Some("2026-10-05T00:00:00Z")
        );
    }

    #[test]
    fn a_duration_is_named_by_its_own_length() {
        assert_eq!(period(Some("5h")), "5h");
        assert_eq!(period(Some("1d")), "24h");
        assert_eq!(period(Some("7d")), "7d");
        assert_eq!(period(Some("1w")), "7d");
        assert_eq!(period(Some("12h")), "12h");
        assert_eq!(period(Some("1mo")), "Monthly");
        // A quarter is not a month, and nothing may claim it is.
        assert_eq!(period(Some("3mo")), "Spend");
        assert_eq!(period(Some("30s")), "1h");
        assert_eq!(period(Some("")), "Spend");
        assert_eq!(period(None), "Spend");
        assert_eq!(period(Some("0d")), "Spend");
        assert_eq!(period(Some("forever")), "Spend");
    }
}
