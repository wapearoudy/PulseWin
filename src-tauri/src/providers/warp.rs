//! Warp: the plan's credits for the period, and any add-on credits bought or
//! granted on top.
//!
//! Read with a key the user pastes, from the read-only GraphQL query Warp's own
//! app sends: `GetRequestLimitInfo`, posted to
//! `https://app.warp.dev/graphql/v2`. The API still calls credits "requests".
//!
//! - **The plan's credits**: `requestsUsedSinceLastRefresh` of `requestLimit`,
//!   both stated, refilling at `nextRefreshTime`. The period's length is not
//!   stated, so it is not claimed. An unlimited plan has no limit to draw.
//! - **Add-on credits**: each grant states what it was and what is left, so the
//!   pack is their sum, spent after the plan's and never reset. Its soonest
//!   expiry is a fact about the pack rather than a reset — a pack *stops
//!   existing*, which is why the row carries no reset and the date rides in the
//!   detail line instead.
//!
//! **The client names itself as Warp's Windows client.** The endpoint's edge
//! limiter answers 429 to a client that does not identify itself, so these
//! headers are load-bearing; the original sends `macOS` because it is one, and
//! claiming that here would be a lie the limiter is entitled to act on.
//!
//! The shape is second-hand — taken from the reference implementation and its
//! tests, not from a captured reply.

use std::sync::Arc;

use chrono::{DateTime, Utc};
use serde_json::{json, Value};

use super::{describe_reqwest_error, percent_from_fraction, Ctx, FetchFuture, Provider};
use crate::model::{parse_reset, ProviderUsage, UsageWindow};

const ENDPOINT: &str = "https://app.warp.dev/graphql/v2?op=GetRequestLimitInfo";

const QUERY: &str = r#"query GetRequestLimitInfo($requestContext: RequestContext!) {
  user(requestContext: $requestContext) {
    __typename
    ... on UserOutput {
      user {
        requestLimitInfo { isUnlimited nextRefreshTime requestLimit requestsUsedSinceLastRefresh }
        bonusGrants { requestCreditsGranted requestCreditsRemaining expiration }
        workspaces { bonusGrantsInfo { grants { requestCreditsGranted requestCreditsRemaining expiration } } }
      }
    }
  }
}"#;

pub struct Warp;

impl Provider for Warp {
    fn id(&self) -> &'static str {
        "warp"
    }

    fn name(&self) -> &'static str {
        "Warp"
    }

    /// The key the fetch reads, and nothing else.
    fn is_configured(&self) -> bool {
        super::provider_key(self.id()).is_some()
    }

    fn fetch(&self, ctx: Arc<Ctx>) -> FetchFuture {
        Box::pin(async move { fetch_inner(ctx).await })
    }
}

async fn fetch_inner(ctx: Arc<Ctx>) -> ProviderUsage {
    const ID: &str = "warp";
    const NAME: &str = "Warp";

    let key = match super::provider_key(ID) {
        Some(key) => key,
        None => return ProviderUsage::failed(ID, NAME, super::missing_key(ID, "")),
    };

    let response = build(&ctx.client, &key).send().await;

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
            (401, " — the API key was refused"),
            (403, " — the API key was refused"),
            (429, " — rate limited, try again shortly"),
        ];
        return ProviderUsage::failed(ID, NAME, super::http_failure(status, hints));
    }

    let json: Value = match serde_json::from_str(&body) {
        Ok(v) => v,
        Err(e) => return ProviderUsage::failed(ID, NAME, format!("bad JSON: {e}")),
    };

    super::claude_code::debug_dump(ID, &json);

    reading_at(&json, Utc::now())
}

/// The query as Warp's app posts it, key and client headers included.
fn build(client: &reqwest::Client, key: &str) -> reqwest::RequestBuilder {
    let version = os_version();

    let mut request = client
        .post(ENDPOINT)
        .header("Authorization", format!("Bearer {key}"))
        .header("Content-Type", "application/json")
        .header("Accept", "application/json")
        .header("x-warp-client-id", "warp-app")
        .header("x-warp-os-category", OS_NAME)
        .header("x-warp-os-name", OS_NAME)
        .header("User-Agent", "Warp/1.0");
    if let Some(version) = &version {
        request = request.header("x-warp-os-version", version);
    }

    let mut os_context = serde_json::Map::new();
    os_context.insert("category".to_string(), json!(OS_NAME));
    os_context.insert("name".to_string(), json!(OS_NAME));
    if let Some(version) = version {
        os_context.insert("version".to_string(), json!(version));
    }

    request.json(&json!({
        "operationName": "GetRequestLimitInfo",
        "query": QUERY,
        "variables": {
            "requestContext": {
                "clientContext": {},
                "osContext": os_context,
            },
        },
    }))
}

const OS_NAME: &str = "Windows";

/// The version this Windows names itself as, for the edge limiter.
///
/// Read from the registry rather than invented: the header is part of how the
/// client identifies itself, and a made-up number is worse than an absent one,
/// which is why a registry that will not answer leaves the header off entirely.
#[cfg(windows)]
fn os_version() -> Option<String> {
    let key = winreg::RegKey::predef(winreg::enums::HKEY_LOCAL_MACHINE)
        .open_subkey(r"SOFTWARE\Microsoft\Windows NT\CurrentVersion")
        .ok()?;
    let version: String = key.get_value("CurrentVersion").ok()?;
    let version = version.trim().to_string();
    if version.is_empty() {
        None
    } else {
        Some(version)
    }
}

#[cfg(not(windows))]
fn os_version() -> Option<String> {
    None
}

/// The mapping, kept apart from the request so a fixture can drive it.
///
/// `now` is a parameter because the pack's expiry is only shown while it is
/// still ahead, and a test that read the clock would be a test of the calendar.
fn reading_at(json: &Value, now: DateTime<Utc>) -> ProviderUsage {
    const ID: &str = "warp";
    const NAME: &str = "Warp";

    // GraphQL answers a failed query with 200 and a list of errors.
    if let Some(errors) = json.get("errors").and_then(Value::as_array) {
        if !errors.is_empty() {
            return ProviderUsage::failed(ID, NAME, "the service reported an error");
        }
    }

    let Some(account) = json
        .get("data")
        .and_then(|data| data.get("user"))
        .and_then(|user| user.get("user"))
    else {
        return ProviderUsage::failed(ID, NAME, "bad reply: no account in the reply");
    };

    let Some(limit) = account.get("requestLimitInfo") else {
        return ProviderUsage::failed(ID, NAME, "bad reply: no request limit");
    };

    let mut windows: Vec<UsageWindow> = Vec::new();

    // An unlimited plan has no limit to draw, and neither has one stated as
    // zero: a fraction of nothing is not a reading.
    let unlimited = limit.get("isUnlimited").and_then(Value::as_bool) == Some(true);
    if !unlimited {
        let cap = number(limit.get("requestLimit")).filter(|cap| *cap > 0.0);
        let used = number(limit.get("requestsUsedSinceLastRefresh")).filter(|used| *used >= 0.0);
        if let (Some(cap), Some(used)) = (cap, used) {
            windows.push(
                UsageWindow::new("Credits", Some(percent_from_fraction(used / cap)))
                    .with_reset(limit.get("nextRefreshTime").and_then(parse_reset)),
            );
        }
    }

    // Every grant with both figures, the user's own and each workspace's.
    let mut grants: Vec<(f64, f64, Option<String>)> = Vec::new();
    for grant in grants_of(account) {
        let Some(granted) = number(grant.get("requestCreditsGranted")).filter(|v| *v > 0.0) else {
            continue;
        };
        let Some(left) = number(grant.get("requestCreditsRemaining")).filter(|v| *v >= 0.0) else {
            continue;
        };
        let expires = grant.get("expiration").and_then(parse_reset);
        grants.push((granted, left, expires));
    }

    let granted: f64 = grants.iter().map(|(granted, _, _)| *granted).sum();
    if granted > 0.0 {
        let left: f64 = grants.iter().map(|(_, left, _)| *left).sum();
        let mut detail = format!("{left:.0} / {granted:.0} credits");
        if let Some(expires) = soonest_expiry(&grants, now) {
            detail.push_str(&format!(" · expires {expires}"));
        }
        windows.push(
            UsageWindow::new(
                "Top-up",
                Some(percent_from_fraction((granted - left).max(0.0) / granted)),
            )
            // A pack is spent and never reset; only the expiry in the detail
            // line says when it stops existing.
            .with_detail(Some(detail)),
        );
    }

    if windows.is_empty() {
        return ProviderUsage::failed(ID, NAME, "no limits reported");
    }

    ProviderUsage::ok(ID, NAME, windows)
}

/// The user's grants and each workspace's, as one list.
fn grants_of(account: &Value) -> Vec<&Value> {
    let mut out: Vec<&Value> = account
        .get("bonusGrants")
        .and_then(Value::as_array)
        .map(|grants| grants.iter().collect())
        .unwrap_or_default();

    if let Some(workspaces) = account.get("workspaces").and_then(Value::as_array) {
        for workspace in workspaces {
            if let Some(grants) = workspace
                .get("bonusGrantsInfo")
                .and_then(|info| info.get("grants"))
                .and_then(Value::as_array)
            {
                out.extend(grants.iter());
            }
        }
    }
    out
}

/// The soonest a part of the pack stops existing, as a date.
///
/// The original shows this beside the balance rather than as a reset, and only
/// when it is still ahead: an expiry already past is not a coming one.
fn soonest_expiry(grants: &[(f64, f64, Option<String>)], now: DateTime<Utc>) -> Option<String> {
    grants
        .iter()
        .filter_map(|(_, _, expires)| expires.as_deref())
        .filter_map(|stamp| DateTime::parse_from_rfc3339(stamp).ok())
        .filter(|at| *at > now)
        .min()
        .map(|at| at.format("%Y-%m-%d").to_string())
}

fn number(value: Option<&Value>) -> Option<f64> {
    value.and_then(Value::as_f64).filter(|value| value.is_finite())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Before every expiry in the fixture.
    fn now() -> DateTime<Utc> {
        DateTime::parse_from_rfc3339("2026-09-25T00:00:00Z")
            .unwrap()
            .with_timezone(&Utc)
    }

    /// One header as it went out, whatever case it was spelled in.
    fn header(request: &reqwest::Request, name: &str) -> String {
        request
            .headers()
            .get(name)
            .unwrap_or_else(|| panic!("no {name} header"))
            .to_str()
            .unwrap()
            .to_string()
    }

    /// Second-hand, from the original's fixture: a plan with a refill, one
    /// grant on the user and one on a workspace, and a grant of nothing.
    fn fixture() -> Value {
        json!({
            "data": { "user": { "__typename": "UserOutput", "user": {
                "requestLimitInfo": {
                    "isUnlimited": false,
                    "nextRefreshTime": "2026-10-01T00:00:00Z",
                    "requestLimit": 2500,
                    "requestsUsedSinceLastRefresh": 625
                },
                "bonusGrants": [
                    { "requestCreditsGranted": 1000, "requestCreditsRemaining": 400,
                      "expiration": "2026-10-15T12:00:00Z" },
                    { "requestCreditsGranted": 0, "requestCreditsRemaining": 0, "expiration": null }
                ],
                "workspaces": [ { "bonusGrantsInfo": { "grants": [
                    { "requestCreditsGranted": 500, "requestCreditsRemaining": 500,
                      "expiration": "2026-12-01T00:00:00Z" }
                ] } } ]
            } } }
        })
    }

    #[test]
    fn reads_the_plans_credits_and_the_add_on_credits() {
        let usage = reading_at(&fixture(), now());
        let labels: Vec<&str> = usage.windows.iter().map(|w| w.label.as_str()).collect();
        assert_eq!(labels, vec!["Credits", "Top-up"]);

        let plan = &usage.windows[0];
        assert_eq!(plan.percent_used, Some(25.0));
        assert_eq!(plan.resets_at.as_deref(), Some("2026-10-01T00:00:00Z"));

        // The user's grant and the workspace's, added up: 1,500 granted, 900
        // left. A pack is spent and never reset, so it carries no reset.
        let add_on = &usage.windows[1];
        assert_eq!(add_on.percent_used, Some(40.0));
        assert!(add_on.resets_at.is_none());
        assert_eq!(
            add_on.detail.as_deref(),
            Some("900 / 1500 credits · expires 2026-10-15")
        );
    }

    /// An unlimited plan has no limit to draw.
    #[test]
    fn an_unlimited_plan_draws_nothing() {
        let reply = json!({ "data": { "user": { "__typename": "UserOutput", "user": {
            "requestLimitInfo": { "isUnlimited": true, "requestLimit": 0,
                                  "requestsUsedSinceLastRefresh": 12 } } } } });
        assert!(reading_at(&reply, now()).error.is_some());
    }

    /// A limit with no figure is left off, and nothing left is no limits.
    #[test]
    fn a_limit_with_no_figure_is_left_off() {
        for reply in [
            json!({ "data": { "user": { "user": { "requestLimitInfo": {
                "isUnlimited": false, "requestLimit": 0, "requestsUsedSinceLastRefresh": 0 } } } } }),
            json!({ "data": { "user": { "user": {
                "requestLimitInfo": { "requestLimit": 100, "requestsUsedSinceLastRefresh": -1 },
                "bonusGrants": [ { "requestCreditsGranted": 100,
                                   "requestCreditsRemaining": -5 } ] } } } }),
            json!({ "data": { "user": { "user": {
                "requestLimitInfo": { "requestLimit": 100 } } } } }),
            // A grant with no remainder and one of nothing are not packs.
            json!({ "data": { "user": { "user": { "requestLimitInfo": {},
                "bonusGrants": [ { "requestCreditsGranted": 100 } ] } } } }),
            json!({ "data": { "user": { "user": { "requestLimitInfo": {},
                "bonusGrants": [ { "requestCreditsGranted": 0,
                                   "requestCreditsRemaining": 0 } ] } } } }),
        ] {
            assert!(reading_at(&reply, now()).error.is_some(), "read {reply}");
        }
    }

    #[test]
    fn a_reply_without_the_limit_cannot_be_read() {
        for reply in [
            json!("not json"),
            json!({}),
            json!({ "data": { "user": { "__typename": "UserFacingError" } } }),
            json!({ "data": { "user": { "user": {} } } }),
        ] {
            assert!(reading_at(&reply, now()).error.is_some(), "read {reply}");
        }
    }

    /// GraphQL errors answered with a 200 are the service's.
    #[test]
    fn graphql_errors_answered_with_a_good_status_are_the_services() {
        let reply = json!({ "errors": [ { "message": "Something went wrong" } ], "data": null });
        let usage = reading_at(&reply, now());
        assert!(usage.error.as_deref().unwrap().contains("service"));
    }

    /// A pack whose grants state no expiry shows its figures and no date, and
    /// an expiry already past is not a coming one.
    #[test]
    fn only_a_coming_expiry_is_shown() {
        let undated = json!({ "data": { "user": { "user": {
            "requestLimitInfo": { "isUnlimited": true },
            "bonusGrants": [ { "requestCreditsGranted": 100,
                               "requestCreditsRemaining": 100 } ] } } } });
        let usage = reading_at(&undated, now());
        assert_eq!(usage.windows[0].detail.as_deref(), Some("100 / 100 credits"));

        let past = json!({ "data": { "user": { "user": {
            "requestLimitInfo": { "isUnlimited": true },
            "bonusGrants": [ { "requestCreditsGranted": 100, "requestCreditsRemaining": 10,
                               "expiration": "2026-09-01T00:00:00Z" } ] } } } });
        let usage = reading_at(&past, now());
        assert_eq!(usage.windows[0].detail.as_deref(), Some("10 / 100 credits"));
    }

    /// A pack past its own size is a ring at nothing used, not a negative.
    #[test]
    fn more_left_than_was_granted_is_nothing_used() {
        let reply = json!({ "data": { "user": { "user": {
            "requestLimitInfo": { "isUnlimited": true },
            "bonusGrants": [ { "requestCreditsGranted": 100,
                               "requestCreditsRemaining": 400 } ] } } } });
        assert_eq!(reading_at(&reply, now()).windows[0].percent_used, Some(0.0));
    }

    /// The query is a read, posted with the key and the client headers the
    /// edge asks for.
    #[test]
    fn the_request_is_the_query_warps_app_sends() {
        let client = reqwest::Client::new();
        let request = build(&client, "wk-key").build().unwrap();

        assert_eq!(request.method(), reqwest::Method::POST);
        assert_eq!(request.url().host_str(), Some("app.warp.dev"));
        assert_eq!(header(&request, "Authorization"), "Bearer wk-key");
        assert_eq!(header(&request, "User-Agent"), "Warp/1.0");
        assert_eq!(header(&request, "x-warp-client-id"), "warp-app");
        assert_eq!(header(&request, "x-warp-os-category"), "Windows");
        assert_eq!(header(&request, "x-warp-os-name"), "Windows");
        // The version is read from the machine and never invented, so this is
        // the one header that may be absent.
        assert_eq!(
            request
                .headers()
                .get("x-warp-os-version")
                .map(|value| value.to_str().unwrap().to_string()),
            os_version(),
        );

        let body = request.body().unwrap().as_bytes().unwrap();
        let object: Value = serde_json::from_slice(body).unwrap();
        assert_eq!(object["operationName"], "GetRequestLimitInfo");
        assert!(object["query"].as_str().unwrap().starts_with("query "));
        assert_eq!(
            object["variables"]["requestContext"]["osContext"]["category"],
            "Windows"
        );
    }
}
