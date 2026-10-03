//! Notion AI's usage allowance on a Business or Enterprise workspace: a rolling
//! window (six hours when this was written) and the billing period, each
//! reported as credits used out of a limit.
//!
//! Read from the two internal endpoints Notion's own Settings → Notion AI →
//! Usage page calls, both on `app.notion.com`: `POST /api/v3/getSpaces` for the
//! workspaces this account can see, then `POST /api/v3/getCreditRateLimitStatus`
//! for the chosen one. Neither is a public API.
//!
//! **The credential is a pasted cookie.** The original imports Notion's session
//! out of a Chromium browser's cookie store through the macOS login keychain,
//! which PulseWin cannot, so the `Cookie` header copied out of a signed-in
//! request is the credential here. Only `token_v2` is kept — it is the session,
//! and without it every call is a 401.
//!
//! **Which workspace.** This port has no picker for one either, so it takes the
//! first workspace on a Business or Enterprise plan, or the first there is. A
//! plan without an allowance answers `not_applicable`, which is "no plan with
//! usage limits" — an answer, not an outage.
//!
//! **Redirects are not followed**, so a signed-out session arrives as the
//! redirect to the sign-in page rather than as the page.
//!
//! The shapes are second-hand — taken from the original and its fixtures, not
//! from a captured reply.

use std::sync::Arc;

use chrono::{DateTime, Utc};
use serde_json::{Map, Value};

use super::{describe_reqwest_error, percent_from_fraction, Ctx, FetchFuture, Provider};
use crate::model::{ProviderUsage, UsageWindow};

const ID: &str = "notion-ai";
const NAME: &str = "Notion AI";

const BASE: &str = "https://app.notion.com";

/// The session, and the only cookie the API is given.
const COOKIES: [&str; 1] = ["token_v2"];

/// The two plans whose workspaces have an AI allowance.
const PLANS_WITH_AN_ALLOWANCE: [&str; 2] = ["business", "enterprise"];

pub struct NotionAi;

impl Provider for NotionAi {
    fn id(&self) -> &'static str {
        ID
    }

    fn name(&self) -> &'static str {
        NAME
    }

    /// Whether a pasted cookie is on the machine. No network call.
    fn is_configured(&self) -> bool {
        super::pasted::cookie(ID).is_some()
    }

    fn fetch(&self, ctx: Arc<Ctx>) -> FetchFuture {
        Box::pin(async move { fetch_inner(ctx).await })
    }
}

async fn fetch_inner(ctx: Arc<Ctx>) -> ProviderUsage {
    let Some(cookie) = super::pasted::cookie(ID).and_then(|header| super::pasted::keep(&header, &COOKIES))
    else {
        return ProviderUsage::failed(ID, NAME, super::pasted::needed(ID, "Notion session cookie"));
    };

    let spaces = match post(&ctx, "getSpaces", "{}", &cookie).await {
        Ok(body) => body,
        Err(reason) => return ProviderUsage::failed(ID, NAME, reason),
    };

    let Some(spaces) = workspaces(&spaces) else {
        return ProviderUsage::failed(ID, NAME, "the reply could not be read");
    };
    let Some(workspace) = choose(&spaces) else {
        return ProviderUsage::failed(ID, NAME, "this account has no plan with usage limits");
    };

    let status = format!(r#"{{"spaceId":"{}"}}"#, workspace.id);
    match post(&ctx, "getCreditRateLimitStatus", &status, &cookie).await {
        Ok(body) => reading(&body, workspace.tier(), Utc::now()),
        Err(reason) => ProviderUsage::failed(ID, NAME, reason),
    }
}

/// One POST, and what its status means.
async fn post(ctx: &Ctx, method: &str, body: &str, cookie: &str) -> Result<String, String> {
    // The client that refuses redirects: a `Cookie` header set by hand rides a
    // redirect to whatever host it names.
    let response = ctx
        .gateway_client
        .post(format!("{BASE}/api/v3/{method}"))
        .header("Content-Type", "application/json")
        .header("Accept", "application/json")
        .header("Origin", BASE)
        .header("Referer", format!("{BASE}/"))
        .header("Cookie", cookie)
        .body(body.to_string())
        .send()
        .await
        .map_err(|e| format!("request failed: {}", describe_reqwest_error(&e)))?;

    let status = response.status();
    let body = response
        .text()
        .await
        .map_err(|e| format!("cannot read body: {e}"))?;

    if status.is_redirection() {
        return Err(
            "HTTP 3xx — the session has expired; copy a fresh cookie from notion.com".to_string(),
        );
    }
    if !status.is_success() {
        let hints: &[(u16, &str)] = &[
            (401, " — the session has expired; copy a fresh cookie from notion.com"),
            (403, " — the session has expired; copy a fresh cookie from notion.com"),
            (429, " — rate limited, try again shortly"),
        ];
        return Err(super::http_failure(status, hints));
    }

    Ok(body)
}

// ---------------------------------------------------------------------------
// Workspaces
// ---------------------------------------------------------------------------

struct Workspace {
    id: String,
    /// The plan as Notion spells it — "business", "enterprise", "free".
    subscription_tier: Option<String>,
}

impl Workspace {
    fn has_allowance(&self) -> bool {
        self.subscription_tier
            .as_deref()
            .is_some_and(|tier| PLANS_WITH_AN_ALLOWANCE.contains(&tier.to_lowercase().as_str()))
    }

    /// The plan's name for the card, capitalised as Notion's own page does.
    fn tier(&self) -> Option<String> {
        let raw = self.subscription_tier.as_deref()?.trim();
        let mut chars = raw.chars();
        match chars.next() {
            Some(first) => Some(first.to_uppercase().collect::<String>() + chars.as_str()),
            None => None,
        }
    }
}

/// `getSpaces` is a record map keyed by user id, each holding its `notion_user`
/// and `space` records. Only a reply that names exactly one user is read:
/// taking whichever key came first could report another account's allowance.
/// `None` when it cannot be read; empty when it names the user and no
/// workspace.
fn workspaces(body: &str) -> Option<Vec<Workspace>> {
    let root: Map<String, Value> = match serde_json::from_str(body) {
        Ok(Value::Object(map)) => map,
        _ => return None,
    };

    let identified: Vec<&String> = root
        .keys()
        .filter(|key| {
            root.get(*key)
                .and_then(|container| container.get("notion_user"))
                .and_then(|users| users.get(*key))
                .and_then(record)
                .and_then(|fields| fields.get("id"))
                .and_then(Value::as_str)
                == Some(key.as_str())
        })
        .collect();

    let user = match identified.as_slice() {
        [only] => only.to_string(),
        // Older replies leave the id out of the record; one key is still one
        // user.
        [] if root.len() == 1 => root.keys().next()?.to_string(),
        _ => return None,
    };

    let container = root.get(&user)?;
    let empty = Map::new();
    let spaces = container
        .get("space")
        .and_then(Value::as_object)
        .unwrap_or(&empty);

    let mut keys: Vec<&String> = spaces.keys().collect();
    keys.sort();
    Some(
        keys.into_iter()
            .filter_map(|key| {
                let fields = record(spaces.get(key)?)?;
                Some(Workspace {
                    id: fields
                        .get("id")
                        .and_then(Value::as_str)
                        .unwrap_or(key)
                        .to_string(),
                    subscription_tier: fields
                        .get("subscription_tier")
                        .and_then(Value::as_str)
                        .map(str::to_string),
                })
            })
            .collect(),
    )
}

/// Records come as `{"value": {…}}` or, newer, `{"value": {"value": {…}}}`.
fn record(raw: &Value) -> Option<&Map<String, Value>> {
    let outer = raw.as_object()?;
    let Some(value) = outer.get("value").and_then(Value::as_object) else {
        return Some(outer);
    };
    Some(value.get("value").and_then(Value::as_object).unwrap_or(value))
}

/// The workspace whose plan has an allowance, or the first there is.
fn choose(workspaces: &[Workspace]) -> Option<&Workspace> {
    workspaces
        .iter()
        .find(|workspace| workspace.has_allowance())
        .or_else(|| workspaces.first())
}

// ---------------------------------------------------------------------------
// The allowance
// ---------------------------------------------------------------------------

/// The mapping, kept apart from the request so a fixture can drive it.
fn reading(body: &str, plan: Option<String>, now: DateTime<Utc>) -> ProviderUsage {
    let reply: Value = match serde_json::from_str(body) {
        Ok(value @ Value::Object(_)) => value,
        _ => return ProviderUsage::failed(ID, NAME, "the reply could not be read"),
    };

    if reply
        .get("status")
        .and_then(Value::as_str)
        .is_some_and(|status| status.to_lowercase() == "not_applicable")
    {
        return ProviderUsage::failed(ID, NAME, "this account has no plan with usage limits");
    }

    let rolling = reply.get("window");
    let billing = reply.get("billingPeriodWindow");
    // Every field is optional, so an unrelated body decodes as all empty.
    if rolling.is_none() && billing.is_none() {
        return ProviderUsage::failed(ID, NAME, "the reply could not be read");
    }

    let mut rows: Vec<(i64, UsageWindow)> = Vec::new();

    if let Some(rolling) = rolling {
        if let Some(percent) = fraction(rolling) {
            let (label, seconds) = length(rolling.get("window").and_then(Value::as_str));
            // The service states how long until the window turns over, not
            // when: the reset is that many seconds from now.
            let reset = super::dig_number(reply.get("resetsInSeconds"))
                .filter(|seconds| seconds.is_finite() && *seconds >= 0.0)
                .and_then(|seconds| {
                    now.checked_add_signed(chrono::Duration::milliseconds(
                        (seconds * 1_000.0) as i64,
                    ))
                })
                .map(|at| at.to_rfc3339_opts(chrono::SecondsFormat::Secs, true));
            rows.push((seconds, UsageWindow::new(label, Some(percent)).with_reset(reset)));
        }
    }

    if let Some(billing) = billing {
        if let Some(percent) = fraction(billing) {
            // A billing period is only as long as the month it falls in, and
            // Notion states only its end.
            let reset = super::dig_number(billing.get("periodEndMs"))
                .filter(|milliseconds| milliseconds.is_finite() && *milliseconds > 0.0)
                .and_then(super::stamp_from_epoch);
            rows.push((
                30 * 86_400,
                UsageWindow::new("Monthly", Some(percent)).with_reset(reset),
            ));
        }
    }

    if rows.is_empty() {
        return ProviderUsage::failed(ID, NAME, "no limits reported in the reply");
    }

    ProviderUsage::ok(ID, NAME, super::by_window_length(rows)).with_plan(plan)
}

/// Credits used over the stated limit. A missing or non-positive limit is
/// nothing to measure against, not 0%.
fn fraction(window: &Value) -> Option<f64> {
    let used = super::dig_number(window.get("used")).filter(|used| used.is_finite() && *used >= 0.0)?;
    let limit = super::dig_number(window.get("limit")).filter(|limit| limit.is_finite() && *limit > 0.0)?;
    Some(percent_from_fraction(used / limit))
}

/// Notion states the rolling window as a token — `6h`. A token that cannot be
/// read leaves the allowance named for what it is and with no length claimed;
/// the seconds are then only where the row sorts.
fn length(token: Option<&str>) -> (String, i64) {
    let unstated = || ("Credits".to_string(), 86_400);

    let Some(raw) = token.map(|token| token.trim().to_lowercase()) else {
        return unstated();
    };
    let Some(unit) = raw.chars().last() else {
        return unstated();
    };
    let Ok(value) = raw[..raw.len() - unit.len_utf8()].parse::<i64>() else {
        return unstated();
    };
    if value <= 0 {
        return unstated();
    }

    let seconds = match unit {
        'm' => value * 60,
        'h' => value * 3_600,
        'd' => value * 86_400,
        'w' => value * 7 * 86_400,
        _ => return unstated(),
    };

    // The familiar lengths get their own name; anything else is named by its
    // own number of minutes, hours or days.
    let label = match seconds {
        18_000 => "5h".to_string(),
        86_400 => "Daily".to_string(),
        604_800 => "7d".to_string(),
        other => super::humanize_window_seconds(other),
    };
    (label, seconds)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Second-hand, from the original's fixtures: the replies CodexBar's Notion
    /// provider describes, which is what these routes are read against.
    fn rate_limit() -> String {
        r#"{"status":"within_limit","window":{"creditType":"basic_ai_credits","scope":"per_user","window":"6h","used":42.5,"limit":100},"resetsInSeconds":12600,"billingPeriodWindow":{"creditType":"basic_ai_credits","scope":"per_user","cadence":"billing_period","used":18.0,"limit":100,"periodEndMs":1788000000000},"enforcement":"preview"}"#
            .to_string()
    }

    fn spaces() -> String {
        r#"{"aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee":{"notion_user":{"aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee":{"value":{"value":{"id":"aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee","email":"person@example.com","name":"Example Person"}}}},"space":{"00000000-7777-8888-9999-aaaaaaaaaaaa":{"value":{"value":{"id":"00000000-7777-8888-9999-aaaaaaaaaaaa","name":"Personal","plan_type":"personal","subscription_tier":"free"}}},"11111111-2222-3333-4444-555555555555":{"value":{"value":{"id":"11111111-2222-3333-4444-555555555555","name":"Acme","plan_type":"team","subscription_tier":"business"}}}}}}"#
            .to_string()
    }

    fn now() -> DateTime<Utc> {
        DateTime::from_timestamp(1_785_600_000, 0).unwrap()
    }

    #[test]
    fn reads_the_rolling_window_and_the_billing_period_as_used_over_limit() {
        let usage = reading(&rate_limit(), Some("Business".to_string()), now());

        assert_eq!(usage.plan.as_deref(), Some("Business"));
        let labels: Vec<&str> = usage.windows.iter().map(|w| w.label.as_str()).collect();
        assert_eq!(labels, vec!["6h", "Monthly"]);
        assert_eq!(usage.windows[0].percent_used, Some(42.5));
        // The service states how long until the window turns over, so the reset
        // is that many seconds from now.
        assert_eq!(
            usage.windows[0].resets_at.as_deref(),
            Some(super::super::stamp_from_epoch_seconds(1_785_612_600.0).unwrap().as_str())
        );
        // The billing period states its end, and not its length.
        assert_eq!(usage.windows[1].percent_used, Some(18.0));
        assert_eq!(
            usage.windows[1].resets_at.as_deref(),
            Some(super::super::stamp_from_epoch_seconds(1_788_000_000.0).unwrap().as_str())
        );
    }

    #[test]
    fn the_workspace_with_an_allowance_is_chosen_over_one_that_sorts_first() {
        let spaces = workspaces(&spaces()).unwrap();
        assert_eq!(spaces.len(), 2);
        let chosen = choose(&spaces).unwrap();
        assert_eq!(chosen.id, "11111111-2222-3333-4444-555555555555");
        assert_eq!(chosen.tier().as_deref(), Some("Business"));
    }

    #[test]
    fn with_no_workspace_on_a_paid_plan_the_first_one_is_asked() {
        let free = Workspace {
            id: "a".to_string(),
            subscription_tier: Some("free".to_string()),
        };
        let plus = Workspace {
            id: "b".to_string(),
            subscription_tier: Some("plus".to_string()),
        };
        let both = vec![free, plus];
        assert_eq!(choose(&both).unwrap().id, "a");
        assert!(choose(&[]).is_none());
        // A workspace with no plan at all is still a workspace.
        let unknown = vec![Workspace {
            id: "c".to_string(),
            subscription_tier: None,
        }];
        assert_eq!(choose(&unknown).unwrap().tier(), None);
    }

    #[test]
    fn a_reply_naming_two_users_is_refused_rather_than_guessed_between() {
        let two = r#"{"a":{"notion_user":{"a":{"value":{"id":"a"}}}},"b":{"notion_user":{"b":{"value":{"id":"b"}}}}}"#;
        assert!(workspaces(two).is_none());
        assert!(workspaces("[]").is_none());
        assert!(workspaces("not json").is_none());
    }

    #[test]
    fn an_older_reply_without_the_users_own_id_still_reads() {
        let legacy = r#"{"u":{"space":{"s":{"value":{"name":"Acme","subscription_tier":"enterprise"}}}}}"#;
        let spaces = workspaces(legacy).unwrap();
        assert_eq!(spaces.len(), 1);
        assert_eq!(spaces[0].id, "s");
        assert_eq!(spaces[0].subscription_tier.as_deref(), Some("enterprise"));
        assert_eq!(spaces[0].tier().as_deref(), Some("Enterprise"));
    }

    #[test]
    fn a_workspace_without_an_allowance_is_no_plan_not_an_outage() {
        let usage = reading(r#"{"status":"not_applicable"}"#, None, now());
        assert_eq!(
            usage.error.as_deref(),
            Some("this account has no plan with usage limits")
        );
    }

    #[test]
    fn a_reply_that_is_not_one_cannot_be_read() {
        for reply in ["not json", "{}", r#"{"status":"within_limit"}"#, "[]"] {
            assert!(reading(reply, None, now()).error.is_some(), "read {reply}");
        }
    }

    #[test]
    fn a_limit_of_nothing_or_a_figure_missing_is_left_off() {
        let reply = r#"{"window":{"window":"6h","used":4,"limit":0},"billingPeriodWindow":{"limit":100}}"#;
        assert!(reading(reply, None, now()).error.is_some());
    }

    #[test]
    fn the_rolling_windows_length_is_the_one_notion_states_or_none() {
        assert_eq!(length(Some("5h")), ("5h".to_string(), 5 * 3_600));
        assert_eq!(length(Some("24h")), ("Daily".to_string(), 86_400));
        assert_eq!(length(Some("1d")), ("Daily".to_string(), 86_400));
        assert_eq!(length(Some("7d")), ("7d".to_string(), 7 * 86_400));
        assert_eq!(length(Some("90m")), ("90m".to_string(), 5_400));
        assert_eq!(length(Some("1w")), ("7d".to_string(), 7 * 86_400));
        // A token that cannot be read claims no length and is named for what it
        // is; the day is only where the row sorts.
        assert_eq!(length(Some("soon")), ("Credits".to_string(), 86_400));
        assert_eq!(length(Some("")), ("Credits".to_string(), 86_400));
        assert_eq!(length(Some("0h")), ("Credits".to_string(), 86_400));
        assert_eq!(length(Some("h")), ("Credits".to_string(), 86_400));
        assert_eq!(length(None), ("Credits".to_string(), 86_400));
    }

    /// Only `token_v2` leaves the browser, and it is required.
    #[test]
    fn only_the_session_cookie_is_kept() {
        let kept = super::super::pasted::keep("notion_user_id=1; token_v2=abc; _ga=x", &COOKIES);
        assert_eq!(kept.as_deref(), Some("token_v2=abc"));
        assert_eq!(super::super::pasted::keep("notion_user_id=1", &COOKIES), None);
    }
}
