//! IBM Bob: the month's Bobcoins against each team's budget.
//!
//! Read with a key the user pastes, in two steps, from the admin routes Bob's
//! own tools call:
//!
//! 1. `GET https://api.us-east.bob.ibm.com/admin/v1/profile` lists the
//!    subscriptions the key can see, each with its teams, the user's id in it,
//!    and the regional host that serves it.
//! 2. `GET https://api.<region>/admin/v1/teams/<team>/users/<user>` for each
//!    team gives the Bobcoins used and the budget.
//!
//! **The key goes to IBM Bob's hosts and nowhere else.** The regional host
//! comes from the reply, so one that is not `bob.ibm.com` or under it — or that
//! carries anything but a bare host, which is how another host is hidden behind
//! one — is refused before anything is sent.
//!
//! **One ring, only when every team has a budget.** Bobcoins are counted per
//! month; a team with no budget is unlimited, and adding its usage to the
//! others' budgets would make a fraction nobody reported. So the sum is drawn
//! only when every team states one. The period is a billing month, so its
//! length is a sort key rather than a measurement.
//!
//! The shape is second-hand — taken from the reference implementation and its
//! tests, not from a captured reply.

use std::sync::Arc;

use serde_json::Value;

use super::{describe_reqwest_error, percent_from_fraction, Ctx, FetchFuture, Provider};
use crate::model::{parse_reset, ProviderUsage, UsageWindow};

const HOME: &str = "https://api.us-east.bob.ibm.com";

pub struct IbmBob;

impl Provider for IbmBob {
    fn id(&self) -> &'static str {
        "ibm-bob"
    }

    fn name(&self) -> &'static str {
        "IBM Bob"
    }

    /// The key the fetch reads, and nothing else.
    fn is_configured(&self) -> bool {
        super::provider_key(self.id()).is_some()
    }

    fn fetch(&self, ctx: Arc<Ctx>) -> FetchFuture {
        Box::pin(async move { fetch_inner(ctx).await })
    }
}

/// One team's month, as the two replies between them state it.
#[derive(Debug, Clone, PartialEq)]
struct Team {
    used: f64,
    budget: Option<f64>,
    plan: Option<String>,
    resets_at: Option<String>,
}

/// A subscription the key can see, and the teams to ask about under it.
struct Instance {
    instance_id: String,
    user_id: String,
    plan: Option<String>,
    resets_at: Option<String>,
    /// The regional host, already checked against `bob.ibm.com`.
    base: String,
    teams: Vec<TeamRef>,
}

struct TeamRef {
    id: String,
    budget_limit: Option<f64>,
}

async fn fetch_inner(ctx: Arc<Ctx>) -> ProviderUsage {
    const ID: &str = "ibm-bob";
    const NAME: &str = "IBM Bob";

    let key = match super::provider_key(ID) {
        Some(key) => key,
        None => return ProviderUsage::failed(ID, NAME, super::missing_key(ID, "")),
    };

    let profile = match ask(&ctx, &format!("{HOME}/admin/v1/profile"), &key, &[]).await {
        Ok(json) => json,
        Err(message) => return ProviderUsage::failed(ID, NAME, message),
    };

    let instances = match instances(&profile) {
        Ok(instances) => instances,
        Err(reason) => return ProviderUsage::failed(ID, NAME, reason),
    };

    let mut teams: Vec<Team> = Vec::new();
    for instance in &instances {
        for team in &instance.teams {
            let url = format!(
                "{}/admin/v1/teams/{}/users/{}",
                instance.base,
                encode_segment(&team.id),
                encode_segment(&instance.user_id),
            );
            let headers = [
                ("x-instance-id", instance.instance_id.as_str()),
                ("x-team-id", team.id.as_str()),
            ];
            let reply = match ask(&ctx, &url, &key, &headers).await {
                Ok(json) => json,
                Err(message) => return ProviderUsage::failed(ID, NAME, message),
            };
            teams.push(team_of(
                &reply,
                team.budget_limit,
                instance.plan.as_deref(),
                instance.resets_at.as_deref(),
            ));
        }
    }

    reading(&teams)
}

/// One request, and the JSON it answered with.
///
/// The routes all answer JSON, so anything else — a status, a body that will
/// not parse — is a failure of the whole fetch: half a Bobcoin total would be
/// a fraction of an account drawn as if it were the account.
async fn ask(
    ctx: &Ctx,
    url: &str,
    key: &str,
    headers: &[(&str, &str)],
) -> Result<Value, String> {
    let mut request = ctx
        .client
        .get(url)
        .header("Authorization", authorization(key))
        .header("Accept", "application/json");
    for (name, value) in headers {
        request = request.header(*name, *value);
    }

    let response = request
        .send()
        .await
        .map_err(|e| format!("request failed: {}", describe_reqwest_error(&e)))?;

    let status = response.status();
    let body = response
        .text()
        .await
        .map_err(|e| format!("cannot read body: {e}"))?;

    if !status.is_success() {
        let hints: &[(u16, &str)] = &[
            (401, " — the API key was refused"),
            (403, " — the API key was refused"),
            (429, " — rate limited, try again shortly"),
        ];
        return Err(super::http_failure(status, hints));
    }

    serde_json::from_str(&body).map_err(|e| format!("bad JSON: {e}"))
}

/// What the key is sent as: a JWT from a Bob sign-in as a bearer token,
/// anything else as an IBM API key.
fn authorization(key: &str) -> String {
    if is_jwt(key) {
        format!("Bearer {key}")
    } else {
        format!("Apikey {key}")
    }
}

/// Whether a pasted value is a signed-in Bob's JWT rather than an API key.
///
/// Three parts, and the middle one has to be a JSON **object**: a key with two
/// dots in it is still a key, and reading it as a bearer token would send it as
/// one and be refused.
fn is_jwt(token: &str) -> bool {
    let parts: Vec<&str> = token.split('.').collect();
    if parts.len() != 3 {
        return false;
    }
    match base64url(parts[1]) {
        Some(bytes) => matches!(serde_json::from_slice::<Value>(&bytes), Ok(Value::Object(_))),
        None => false,
    }
}

/// The bytes a base64url payload names, either alphabet.
///
/// A second copy of the decoder `nous_portal` already has, deliberately: this
/// one answers "is this a JSON object?", which is a different question from
/// that one's `exp` claim, and a shared helper would have to be told which of
/// the two lenient rules it was applying.
fn base64url(text: &str) -> Option<Vec<u8>> {
    fn digit(byte: u8) -> Option<u32> {
        match byte {
            b'A'..=b'Z' => Some(u32::from(byte - b'A')),
            b'a'..=b'z' => Some(u32::from(byte - b'a') + 26),
            b'0'..=b'9' => Some(u32::from(byte - b'0') + 52),
            b'-' | b'+' => Some(62),
            b'_' | b'/' => Some(63),
            _ => None,
        }
    }

    let mut out = Vec::new();
    let mut accumulator: u32 = 0;
    let mut bits: u32 = 0;
    for byte in text.bytes() {
        if byte == b'=' {
            break;
        }
        accumulator = (accumulator << 6) | digit(byte)?;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((accumulator >> bits) as u8);
        }
    }
    Some(out)
}

/// The regional host a subscription names, as an https base URL, or `None` for
/// one the key must not be sent to. No region named is the home host.
fn regional_host(domain: Option<&str>) -> Option<String> {
    let domain = domain.map(str::trim).filter(|domain| !domain.is_empty());
    let Some(domain) = domain else {
        return Some(HOME.to_string());
    };

    let lower = domain.to_lowercase();
    let host = if lower.starts_with("api.") {
        lower
    } else {
        format!("api.{lower}")
    };

    // A bare host and nothing else: no path, port, user or query to hide
    // another host behind. The character test is what rules those out, and the
    // host comparison below is what rules out a name that only normalizes to
    // one — a trailing dot, or a Unicode label that becomes `xn--…`.
    if !host
        .chars()
        .all(|c| c.is_alphanumeric() || c == '.' || c == '-')
    {
        return None;
    }
    if host != "bob.ibm.com" && !host.ends_with(".bob.ibm.com") {
        return None;
    }

    let url = reqwest::Url::parse(&format!("https://{host}")).ok()?;
    if url.host_str() != Some(host.as_str()) {
        return None;
    }
    Some(format!("https://{host}"))
}

/// The subscriptions the profile reply names, or the reason it cannot be read.
///
/// A region the key must not be sent to is a failure of the whole fetch rather
/// than a subscription to skip: the reply named a host, and ignoring it would
/// read the other subscriptions as if the account were only those.
fn instances(profile: &Value) -> Result<Vec<Instance>, &'static str> {
    let Some(list) = profile.get("instances").and_then(Value::as_array) else {
        return Err("bad reply: no subscriptions in the reply");
    };

    let mut out = Vec::new();
    for instance in list {
        let Some(user_id) = text(instance, "user_id").filter(|user| !user.is_empty()) else {
            continue;
        };
        let base = regional_host(text(instance, "region_domain"))
            .ok_or("bad reply: a region outside bob.ibm.com")?;

        let teams = instance
            .get("teams")
            .and_then(Value::as_array)
            .map(|teams| {
                teams
                    .iter()
                    .filter_map(|team| {
                        let id = text(team, "id").filter(|id| !id.is_empty())?;
                        Some(TeamRef {
                            id: id.to_string(),
                            budget_limit: number(team.get("budget_limit")),
                        })
                    })
                    .collect()
            })
            .unwrap_or_default();

        out.push(Instance {
            instance_id: text(instance, "instance_id").unwrap_or("").to_string(),
            user_id: user_id.to_string(),
            plan: text(instance, "plan_name").map(str::to_string),
            // `refresh_at` has shipped as Unix seconds and as an ISO string.
            resets_at: instance.get("refresh_at").and_then(parse_reset),
            base,
            teams,
        });
    }
    Ok(out)
}

/// One team's reply, with the team's own limit as the fallback: the admin route
/// states the budget on the team and only sometimes repeats it on the member.
fn team_of(
    reply: &Value,
    fallback_limit: Option<f64>,
    plan: Option<&str>,
    resets_at: Option<&str>,
) -> Team {
    Team {
        // A reply with no figure at all is not zero: it is a figure nobody
        // stated, which the reader below leaves out.
        used: number(reply.get("usage")).unwrap_or(f64::NAN),
        budget: number(reply.get("budget_limit")).or(fallback_limit),
        plan: plan.map(str::to_string),
        resets_at: resets_at.map(str::to_string),
    }
}

/// The mapping, kept apart from the requests so a fixture can drive it.
fn reading(teams: &[Team]) -> ProviderUsage {
    const ID: &str = "ibm-bob";
    const NAME: &str = "IBM Bob";

    if teams.is_empty() {
        return ProviderUsage::failed(ID, NAME, "no subscription this key can see");
    }

    let mut plans: Vec<String> = teams
        .iter()
        .filter_map(|team| team.plan.as_deref())
        .map(str::trim)
        .filter(|plan| !plan.is_empty())
        .map(str::to_string)
        .collect();
    plans.sort();
    plans.dedup();
    let plan = if plans.is_empty() {
        None
    } else {
        Some(plans.join(", "))
    };

    // One ring, only when every team states a budget: a team without one is
    // unlimited, and adding its usage to the others' budgets would make a
    // fraction nobody reported.
    let mut total_budget = 0.0;
    for team in teams {
        let Some(budget) = team.budget else {
            return ProviderUsage::failed(ID, NAME, "no limits reported");
        };
        if !budget.is_finite() || budget < 0.0 || !team.used.is_finite() || team.used < 0.0 {
            return ProviderUsage::failed(ID, NAME, "no limits reported");
        }
        total_budget += budget;
    }
    if !(total_budget > 0.0) {
        return ProviderUsage::failed(ID, NAME, "no limits reported");
    }

    let used: f64 = teams.iter().map(|team| team.used).sum();
    // The sooner of the subscriptions' refreshes.
    let resets_at = teams
        .iter()
        .filter_map(|team| team.resets_at.as_deref())
        .filter_map(|stamp| chrono::DateTime::parse_from_rfc3339(stamp).ok())
        .min()
        .map(|at| {
            at.with_timezone(&chrono::Utc)
                .to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
        });

    let window = UsageWindow::new(
        "Monthly",
        Some(percent_from_fraction(used / total_budget)),
    )
    .with_reset(resets_at);

    ProviderUsage::ok(ID, NAME, vec![window]).with_plan(plan)
}

fn text<'a>(value: &'a Value, key: &str) -> Option<&'a str> {
    value.get(key).and_then(Value::as_str)
}

/// A figure, as a number and never as a string: an IBM admin reply that quotes
/// its Bobcoins is not this reply.
fn number(value: Option<&Value>) -> Option<f64> {
    value.and_then(Value::as_f64).filter(|value| value.is_finite())
}

/// One path segment, escaped. The team and user ids come out of the first
/// reply, so a `/` in either must not become a step further up the route.
fn encode_segment(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for byte in text.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~') {
            out.push(byte as char);
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// Second-hand, from the original's fixture: two subscriptions, two
    /// regions, and `refresh_at` in both of the shapes it has shipped in.
    fn profile() -> Value {
        json!({
            "instances": [
                {
                    "instance_id": "instance-one",
                    "instance_name": "Personal",
                    "user_id": "user-one",
                    "plan_name": "Pro+",
                    "refresh_at": 1_790_812_800,
                    "region_domain": "us-east.bob.ibm.com",
                    "teams": [ { "id": "team-one", "name": "Solo", "budget_limit": 40 } ]
                },
                {
                    "instance_id": "instance-two",
                    "name": "Work",
                    "user_id": "user-two",
                    "plan_name": "Enterprise",
                    "refresh_at": "2026-10-05T00:00:00.000Z",
                    "region_domain": "api.eu-de.bob.ibm.com",
                    "teams": [ { "id": "team-two", "name": "Platform", "budget_limit": 160 } ]
                }
            ]
        })
    }

    #[test]
    fn reads_every_teams_bobcoins_against_every_teams_budget() {
        let teams = vec![
            team_of(&json!({ "usage": 10 }), Some(40.0), Some("Pro+"), Some("2026-10-01T00:00:00Z")),
            team_of(
                &json!({ "usage": 25, "budget_limit": 160 }),
                Some(160.0),
                Some("Enterprise"),
                Some("2026-10-05T00:00:00Z"),
            ),
        ];

        let usage = reading(&teams);
        assert_eq!(usage.windows.len(), 1);
        assert_eq!(usage.windows[0].label, "Monthly");
        assert_eq!(usage.windows[0].percent_used, Some(17.5));
        // The sooner of the two subscriptions' refreshes.
        assert_eq!(
            usage.windows[0].resets_at.as_deref(),
            Some("2026-10-01T00:00:00Z")
        );
        assert_eq!(usage.plan.as_deref(), Some("Enterprise, Pro+"));
    }

    /// The limit is the member's when the team route repeats it, and the
    /// team's own when it does not.
    #[test]
    fn the_team_limit_is_the_fallback() {
        let member = team_of(&json!({ "usage": 10 }), Some(40.0), None, None);
        assert_eq!(member.budget, Some(40.0));

        let repeated = team_of(&json!({ "usage": 10, "budget_limit": 25 }), Some(40.0), None, None);
        assert_eq!(repeated.budget, Some(25.0));

        let neither = team_of(&json!({ "usage": 10 }), None, None, None);
        assert_eq!(neither.budget, None);
    }

    /// A team without a budget is unlimited, and nothing is made up for it.
    #[test]
    fn a_team_without_a_budget_is_unlimited() {
        let teams = vec![
            Team { used: 10.0, budget: Some(40.0), plan: Some("Pro+".into()), resets_at: None },
            Team { used: 25.0, budget: None, plan: Some("Enterprise".into()), resets_at: None },
        ];
        assert!(reading(&teams).error.is_some());
    }

    /// Figures that aren't figures are left off, and nothing left is no limits.
    #[test]
    fn figures_that_are_not_figures_are_left_off() {
        for team in [
            Team { used: -1.0, budget: Some(40.0), plan: None, resets_at: None },
            Team { used: 5.0, budget: Some(0.0), plan: None, resets_at: None },
            Team { used: 5.0, budget: Some(-40.0), plan: None, resets_at: None },
            // A reply with no `usage` at all.
            team_of(&json!({}), Some(40.0), None, None),
        ] {
            assert!(reading(&[team.clone()]).error.is_some(), "read {team:?}");
        }
    }

    #[test]
    fn a_key_that_sees_no_subscription_has_no_plan() {
        let usage = reading(&[]);
        assert!(usage.error.as_deref().unwrap().contains("no subscription"));
    }

    #[test]
    fn reads_the_subscriptions_the_profile_names() {
        let instances = instances(&profile()).unwrap();
        assert_eq!(instances.len(), 2);
        assert_eq!(instances[0].base, "https://api.us-east.bob.ibm.com");
        assert_eq!(instances[0].user_id, "user-one");
        assert_eq!(instances[0].instance_id, "instance-one");
        assert_eq!(instances[0].teams[0].id, "team-one");
        assert_eq!(instances[0].teams[0].budget_limit, Some(40.0));
        // Unix seconds on one, an ISO string on the other, read as one shape.
        assert_eq!(
            instances[0].resets_at.as_deref(),
            Some("2026-10-01T00:00:00Z")
        );
        assert_eq!(
            instances[1].resets_at.as_deref(),
            Some("2026-10-05T00:00:00Z")
        );
        assert_eq!(instances[1].base, "https://api.eu-de.bob.ibm.com");
    }

    /// A subscription with no user in it has no team route to ask.
    #[test]
    fn a_subscription_without_a_user_is_skipped() {
        let reply = json!({ "instances": [ { "instance_id": "i", "teams": [ { "id": "t" } ] } ] });
        assert!(instances(&reply).unwrap().is_empty());
        // A reply with no subscriptions at all is not a reply this build reads.
        assert!(instances(&json!({})).is_err());
    }

    #[test]
    fn regional_hosts() {
        assert_eq!(
            regional_host(Some("us-east.bob.ibm.com")).as_deref(),
            Some("https://api.us-east.bob.ibm.com")
        );
        // One that already names the API host is not prefixed twice.
        assert_eq!(
            regional_host(Some("api.eu-de.bob.ibm.com")).as_deref(),
            Some("https://api.eu-de.bob.ibm.com")
        );
        assert_eq!(regional_host(None).as_deref(), Some(HOME));
        assert_eq!(regional_host(Some("  ")).as_deref(), Some(HOME));
        // Case is not a different host.
        assert_eq!(
            regional_host(Some("US-EAST.BOB.IBM.COM")).as_deref(),
            Some("https://api.us-east.bob.ibm.com")
        );
    }

    /// Every one of these writes a host that is not IBM's while reading, to an
    /// eye, like one.
    #[test]
    fn hosts_that_only_look_like_ibms() {
        for domain in [
            "evil.example/x.bob.ibm.com",
            "bob.ibm.com.evil.example",
            "x@evil.example",
            "evil.example?next=.bob.ibm.com",
            "evil.example#.bob.ibm.com",
            "evil.example@us-east.bob.ibm.com",
            "us-east.bob.ibm.com:443",
            "ü.bob.ibm.com",
        ] {
            assert!(regional_host(Some(domain)).is_none(), "accepted {domain}");
        }
    }

    /// The region is checked before the key is sent anywhere, so an untrusted
    /// one fails the fetch rather than being skipped.
    #[test]
    fn an_untrusted_region_fails_the_fetch() {
        let reply = json!({ "instances": [ {
            "instance_id": "i", "user_id": "u", "region_domain": "evil.example",
            "teams": [ { "id": "t" } ]
        } ] });
        assert!(instances(&reply).is_err());
    }

    #[test]
    fn a_key_is_an_ibm_api_key_and_a_jwt_is_a_bearer_token() {
        assert_eq!(authorization("plain-key"), "Apikey plain-key");
        let jwt = "header.eyJzdWIiOiJ1c2VyIn0.signature";
        assert_eq!(authorization(jwt), format!("Bearer {jwt}"));
        // Two dots are not a JWT when the middle is not a JSON object.
        assert_eq!(authorization("a.b.c"), "Apikey a.b.c");
        assert_eq!(authorization("header.bm90IGpzb24.signature"), "Apikey header.bm90IGpzb24.signature");
    }

    /// The ids come out of the first reply, so neither may carry a path step.
    #[test]
    fn ids_are_escaped_into_one_path_segment() {
        assert_eq!(encode_segment("team-one"), "team-one");
        assert_eq!(encode_segment("a/b"), "a%2Fb");
        assert_eq!(encode_segment("a b?c#d"), "a%20b%3Fc%23d");
    }
}
