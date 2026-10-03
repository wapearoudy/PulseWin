//! Bifrost, a self-hosted AI gateway: the budgets its governance puts on one
//! virtual key — dollars spent out of dollars allowed, per period.
//!
//! Read with the virtual key the reader supplies and the address they name,
//! from the gateway's own governance route:
//! `GET <address>/api/governance/virtual-keys/quota`, key in `x-bf-vk`. The key
//! goes to that address and nowhere else. The shape is second-hand — taken from
//! CodexBar's Bifrost plugin, not from a captured reply — and the fixture below
//! says so.
//!
//! **A key and an address are both required, and this port has no settings
//! window to type them into.** They arrive from `PULSEWIN_BIFROST_KEY` and
//! `PULSEWIN_BIFROST_BASE_URL`, or from one
//! `%APPDATA%\PulseWin\bifrost.json` holding `apiKey` and `baseUrl`; people
//! paste the OpenAI-compatible base, which ends in `/v1`, and the route sits
//! beside that rather than under it. `is_configured` asks for both — a key with
//! nowhere to send it is not a credential — and asks only from disk, never over
//! the network. The address itself is checked before the key is attached to it:
//! see `super::gateway_url`.
//!
//! **Budgets only.** The key's rate limits are left off: a token limit and a
//! request limit over the same period would both be headed "1h", and nothing
//! here can tell the reader which is which without a unit label this port does
//! not have. The per-model spend breakdown is spend with no limit and has
//! nowhere to go either.
//!
//! **A reset is stated only where it follows from the reply.** A period counted
//! in hours or less is a fixed length, so the next turn-over is the last one
//! plus the length; a period counted in days or longer may be aligned to the
//! calendar, and the reply does not say, so no reset is claimed for it.

use std::sync::Arc;

use chrono::{DateTime, Utc};
use serde_json::Value;

use super::{by_window_length, percent_from_fraction, Ctx, FetchFuture, Provider};
use crate::model::{parse_reset, ProviderUsage, UsageWindow};

const QUOTA_PATH: &str = "/api/governance/virtual-keys/quota";

pub struct Bifrost;

impl Provider for Bifrost {
    fn id(&self) -> &'static str {
        "bifrost"
    }

    fn name(&self) -> &'static str {
        "Bifrost"
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
    const ID: &str = "bifrost";
    const NAME: &str = "Bifrost";

    let Some(key) = super::provider_key(ID) else {
        return ProviderUsage::failed(ID, NAME, super::missing_key(ID, ""));
    };
    let Some(address) = super::provider_base_url(ID) else {
        return ProviderUsage::failed(ID, NAME, super::missing_address(ID));
    };
    let Some(url) = super::gateway_url(&address, QUOTA_PATH, &["/v1"]) else {
        return ProviderUsage::failed(ID, NAME, super::refused_address());
    };

    // The gateway's own header, not a bearer token, and the gateway client so a
    // redirect cannot move the key to another host once the address is checked.
    let response = ctx
        .gateway_client
        .get(url)
        .header("x-bf-vk", key)
        .header("Accept", "application/json")
        .send()
        .await;

    let response = match response {
        Ok(r) => r,
        Err(e) => {
            return ProviderUsage::failed(
                ID,
                NAME,
                format!("request failed: {}", super::describe_reqwest_error(&e)),
            )
        }
    };

    let status = response.status();
    let body = match response.text().await {
        Ok(t) => t,
        Err(e) => return ProviderUsage::failed(ID, NAME, format!("cannot read body: {e}")),
    };

    if !status.is_success() {
        if status.is_redirection() {
            return ProviderUsage::failed(ID, NAME, "the gateway redirected the request — the key was not accepted");
        }
        let hints: &[(u16, &str)] = &[
            (401, " — the gateway refused the key"),
            (403, " — the gateway refused the key"),
            (429, " — rate limited, try again shortly"),
        ];
        return ProviderUsage::failed(ID, NAME, super::http_failure(status, hints));
    }

    let json: Value = match serde_json::from_str(&body) {
        Ok(v) => v,
        Err(e) => return ProviderUsage::failed(ID, NAME, format!("bad JSON: {e}")),
    };

    super::claude_code::debug_dump(ID, &json);

    reading(&json, Utc::now())
}

/// The budgets of one virtual key, shortest period first.
///
/// The key's own budgets come first, then those it has per upstream provider
/// and per model, each scoped by the name the gateway gives it — a budget with
/// no name of its own is the key's whole allowance, and is headed without one.
fn reading(json: &Value, now: DateTime<Utc>) -> ProviderUsage {
    const ID: &str = "bifrost";
    const NAME: &str = "Bifrost";

    let Some(reply) = json.as_object() else {
        return ProviderUsage::failed(ID, NAME, "unreadable reply");
    };

    let mut scoped: Vec<(Option<String>, &Value)> = Vec::new();

    for budget in budgets_of(reply.get("budgets")) {
        scoped.push((None, budget));
    }
    for config in budgets_of(reply.get("provider_configs")) {
        let scope = clean(config.get("provider"));
        for budget in budgets_of(config.get("budgets")) {
            scoped.push((scope.clone(), budget));
        }
    }
    for config in budgets_of(reply.get("model_configs")) {
        // A model config that names no model is about the provider it belongs
        // to, and reads the same way.
        let scope = clean(config.get("model_name")).or_else(|| clean(config.get("provider")));
        for budget in budgets_of(config.get("budgets")) {
            scoped.push((scope.clone(), budget));
        }
    }

    let windows = by_window_length(
        scoped
            .into_iter()
            .filter_map(|(scope, budget)| window_for(budget, scope.as_deref(), now))
            .collect(),
    );

    if windows.is_empty() {
        // An inactive key with nothing on it is a key the gateway will not
        // honour, which is a different thing from a key with no limits.
        return if reply.get("is_active").and_then(Value::as_bool) == Some(false) {
            ProviderUsage::failed(ID, NAME, "the gateway turned this key away")
        } else {
            ProviderUsage::failed(ID, NAME, "no limits reported")
        };
    }

    ProviderUsage::ok(ID, NAME, windows)
}

/// A JSON array of objects, wherever it is absent or the wrong shape.
fn budgets_of(value: Option<&Value>) -> Vec<&Value> {
    value
        .and_then(Value::as_array)
        .map(|items| items.iter().collect())
        .unwrap_or_default()
}

/// One budget as a window, or `None` where nothing can be drawn from it.
fn window_for(
    budget: &Value,
    scope: Option<&str>,
    now: DateTime<Utc>,
) -> Option<(i64, UsageWindow)> {
    // A budget the gateway did not name is not one it will answer for.
    clean(Some(budget.get("id")?))?;

    let used = super::dig_number(budget.get("current_usage"))
        .filter(|used| used.is_finite() && *used >= 0.0)?;
    let base = super::dig_number(budget.get("max_limit")).filter(|base| base.is_finite())?;

    // A temporary raise the gateway reports on top of the budget counts while
    // it is in force: for good, or for cycles it says remain.
    let mut limit = base;
    if let Some(extra) = super::dig_number(budget.get("override_amount")).filter(|extra| *extra > 0.0)
    {
        let mode = budget.get("override_mode").and_then(Value::as_str);
        let cycles = super::dig_number(budget.get("override_cycles_remaining")).unwrap_or(0.0);
        if mode == Some("forever") || (mode == Some("cycles") && cycles > 0.0) {
            limit += extra;
        }
    }

    if limit <= 0.0 {
        return None;
    }

    let period = Period::parse(budget.get("reset_duration").and_then(Value::as_str));

    // Only a fixed-length period says when the next one starts; see the module
    // docs.
    let resets_at = if period.is_fixed {
        next_reset(budget.get("last_reset"), period.seconds, now)
    } else {
        None
    };

    let label = match scope {
        Some(scope) => format!("{} {scope}", period.label),
        None => period.label.clone(),
    };

    Some((
        period.seconds,
        UsageWindow::new(label, Some(percent_from_fraction(used / limit))).with_reset(resets_at),
    ))
}

/// The reset that follows the one the gateway reports, when it is still ahead.
fn next_reset(last_reset: Option<&Value>, period_seconds: i64, now: DateTime<Utc>) -> Option<String> {
    if period_seconds <= 0 {
        return None;
    }

    let last = last_reset.and_then(parse_reset)?;
    let last = DateTime::parse_from_rfc3339(&last).ok()?.with_timezone(&Utc);
    let next = last + chrono::Duration::seconds(period_seconds);

    (next > now).then(|| next.to_rfc3339_opts(chrono::SecondsFormat::Secs, true))
}

/// A budget's `reset_duration`, as Bifrost writes it: a Go duration such as
/// `1h` or `1h30m`, or a count of days, weeks, months, quarters or years
/// (`1d`, `1w`, `1M`, `1Q`, `1Y`).
struct Period {
    /// The heading, and nothing else: what the row is named.
    label: String,
    /// The sort key, and — when `is_fixed` — the length the next reset follows
    /// from.
    seconds: i64,
    /// Counted in hours or less, so its next reset follows from the last.
    is_fixed: bool,
}

impl Period {
    /// A budget with no length this can state: plain spend, sorted by
    /// `sort_key`.
    fn unstated(sort_key: i64) -> Self {
        Self {
            label: "Spend".to_string(),
            seconds: sort_key,
            is_fixed: false,
        }
    }

    fn parse(raw: Option<&str>) -> Self {
        let text = raw.map(str::trim).unwrap_or("");

        if let Some(seconds) = fixed_seconds(text) {
            // Named by its length only when that is a whole number of hours:
            // "30m" headed as an hour would be a claim. The reset still
            // follows from the last one, because the length is fixed all the
            // same.
            if seconds % 3_600 != 0 {
                return Self {
                    label: "Spend".to_string(),
                    seconds,
                    is_fixed: true,
                };
            }
            return Self {
                label: label_for_seconds(seconds),
                seconds,
                is_fixed: true,
            };
        }

        let count = text
            .chars()
            .next_back()
            .zip(text.len().checked_sub(1).and_then(|len| text.get(..len)))
            .and_then(|(last, digits)| Some((last, digits.parse::<i64>().ok()?)))
            .filter(|(_, count)| *count > 0 && *count < 1_000);

        let Some((last, count)) = count else {
            return Self::unstated(30 * 86_400);
        };

        match (last, count) {
            ('d', 1) => Self {
                label: "24h".to_string(),
                seconds: 86_400,
                is_fixed: false,
            },
            ('d', 7) | ('w', 1) => Self {
                label: "7d".to_string(),
                seconds: 7 * 86_400,
                is_fixed: false,
            },
            ('d', _) => Self {
                label: super::humanize_window_seconds(count * 86_400),
                seconds: count * 86_400,
                is_fixed: false,
            },
            ('w', _) => Self {
                label: super::humanize_window_seconds(count * 7 * 86_400),
                seconds: count * 7 * 86_400,
                is_fixed: false,
            },
            // A month is not a fixed length: a name and a sort key only.
            ('M', 1) => Self {
                label: "Monthly".to_string(),
                seconds: 30 * 86_400,
                is_fixed: false,
            },
            // Several months, a quarter or a year: a length nothing here can
            // name without claiming a number of days.
            ('M', _) => Self::unstated(count * 30 * 86_400),
            ('Q', _) => Self::unstated(count * 90 * 86_400),
            ('Y', _) => Self::unstated(count * 365 * 86_400),
            _ => Self::unstated(30 * 86_400),
        }
    }
}

/// The heading for a fixed length: the familiar ones keep the names the rest of
/// the port uses, and anything else is headed by its own duration. Every length
/// that reaches here is a whole number of hours, so the heading never rounds.
fn label_for_seconds(seconds: i64) -> String {
    match seconds {
        18_000 => "5h".to_string(),
        86_400 => "24h".to_string(),
        other => super::humanize_window_seconds(other),
    }
}

/// `1h`, `90m`, `1h30m`: whole seconds, or `None` if it is not one.
///
/// A trailing `ms` is refused rather than read as minutes, which is what makes
/// this a duration parser and not a prefix match.
fn fixed_seconds(text: &str) -> Option<i64> {
    const UNITS: [(&str, f64); 3] = [("h", 3_600.0), ("m", 60.0), ("s", 1.0)];

    let mut rest = text;
    let mut total = 0.0;
    let mut matched = false;

    while !rest.is_empty() {
        let digits: String = rest
            .chars()
            .take_while(|c| c.is_ascii_digit() || *c == '.')
            .collect();
        if digits.is_empty() {
            return None;
        }
        let value: f64 = digits.parse().ok()?;
        rest = &rest[digits.len()..];

        let unit = UNITS
            .iter()
            .find(|(unit, _)| rest.starts_with(unit) && !rest.starts_with("ms"))?;
        rest = &rest[unit.0.len()..];

        total += value * unit.1;
        matched = true;
    }

    if !matched || !total.is_finite() || total < 1.0 || total >= f64::from(i32::MAX) {
        return None;
    }
    Some(total.round() as i64)
}

/// A name the gateway gave, or none where it gave a blank.
fn clean(value: Option<&Value>) -> Option<String> {
    value
        .and_then(Value::as_str)
        .map(|text| text.trim().to_string())
        .filter(|text| !text.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn at(text: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(text).unwrap().with_timezone(&Utc)
    }

    /// The reply's shape, as CodexBar's plugin describes it.
    fn fixture() -> Value {
        json!({
            "is_active": true,
            "budgets": [
                { "id": "vk-1", "max_limit": 100.0, "current_usage": 25.0,
                  "reset_duration": "1M", "last_reset": "2026-10-01T00:00:00Z" }
            ],
            "provider_configs": [
                { "provider": "openai",
                  "budgets": [ { "id": "p-1", "max_limit": 20.0, "current_usage": 5.0,
                                 "reset_duration": "1h", "last_reset": "2026-10-02T00:30:00Z" } ] }
            ],
            "model_configs": [
                { "provider": "anthropic", "model_name": "claude-sonnet",
                  "budgets": [ { "id": "m-1", "max_limit": 5.0, "current_usage": 5.0,
                                 "reset_duration": "1d", "last_reset": "2026-10-01T00:00:00Z" } ] }
            ]
        })
    }

    #[test]
    fn reads_the_keys_budgets_scoped_shortest_first() {
        let usage = reading(&fixture(), at("2026-10-02T01:00:00Z"));
        let labels: Vec<&str> = usage.windows.iter().map(|w| w.label.as_str()).collect();

        assert_eq!(labels, vec!["1h openai", "24h claude-sonnet", "Monthly"]);
        assert_eq!(usage.windows[0].percent_used, Some(25.0));
        assert_eq!(usage.windows[1].percent_used, Some(100.0));
        assert_eq!(usage.windows[2].percent_used, Some(25.0));
    }

    #[test]
    fn a_reset_is_claimed_only_where_the_length_is_fixed() {
        let usage = reading(&fixture(), at("2026-10-02T01:00:00Z"));
        // The provider budget runs an hour from 00:30, so the next is 01:30.
        assert_eq!(
            usage.windows[0].resets_at.as_deref(),
            Some("2026-10-02T01:30:00Z")
        );
        // A day and a month may be aligned to the calendar, and the reply does
        // not say, so neither claims a reset.
        assert!(usage.windows[1].resets_at.is_none());
        assert!(usage.windows[2].resets_at.is_none());
    }

    #[test]
    fn a_reset_already_past_is_not_reported_as_the_next_one() {
        let reply = json!({
            "budgets": [ { "id": "b", "max_limit": 10.0, "current_usage": 1.0,
                           "reset_duration": "1h", "last_reset": "2026-10-01T00:00:00Z" } ]
        });
        assert!(reading(&reply, at("2026-10-02T01:00:00Z")).windows[0]
            .resets_at
            .is_none());
    }

    #[test]
    fn a_temporary_raise_counts_only_while_it_is_in_force() {
        let raised = |mode: &str, cycles: f64, extra: f64| {
            json!({ "budgets": [ { "id": "b", "max_limit": 10.0, "current_usage": 10.0,
                                   "reset_duration": "1M",
                                   "override_mode": mode,
                                   "override_cycles_remaining": cycles,
                                   "override_amount": extra } ] })
        };

        // 10 spent out of 10 + 10 is half.
        assert_eq!(
            reading(&raised("forever", 0.0, 10.0), at("2026-10-02T01:00:00Z")).windows[0]
                .percent_used,
            Some(50.0)
        );
        assert_eq!(
            reading(&raised("cycles", 2.0, 10.0), at("2026-10-02T01:00:00Z")).windows[0]
                .percent_used,
            Some(50.0)
        );
        // Cycles already spent, an unknown mode, and a raise of nothing: the
        // budget stands as stated.
        for spent in [
            raised("cycles", 0.0, 10.0),
            raised("once", 3.0, 10.0),
            raised("forever", 0.0, 0.0),
        ] {
            assert_eq!(
                reading(&spent, at("2026-10-02T01:00:00Z")).windows[0].percent_used,
                Some(100.0)
            );
        }
    }

    #[test]
    fn a_budget_nothing_can_be_drawn_from_is_left_out() {
        let reply = json!({
            "budgets": [
                { "max_limit": 10.0, "current_usage": 1.0, "reset_duration": "1M" },
                { "id": "zero", "max_limit": 0.0, "current_usage": 1.0, "reset_duration": "1M" },
                { "id": "negative", "max_limit": 10.0, "current_usage": -1.0, "reset_duration": "1M" },
                { "id": "good", "max_limit": 10.0, "current_usage": 1.0, "reset_duration": "1M" }
            ]
        });
        let usage = reading(&reply, at("2026-10-02T01:00:00Z"));
        assert_eq!(usage.windows.len(), 1);
        assert_eq!(usage.windows[0].percent_used, Some(10.0));
    }

    #[test]
    fn a_key_the_gateway_has_turned_off_is_said_so_rather_than_called_empty() {
        let inactive = json!({ "is_active": false, "budgets": [] });
        let usage = reading(&inactive, at("2026-10-02T01:00:00Z"));
        assert!(usage.error.as_deref().unwrap_or("").contains("turned this key away"));

        let active = json!({ "is_active": true, "budgets": [] });
        assert_eq!(
            reading(&active, at("2026-10-02T01:00:00Z")).error.as_deref(),
            Some("no limits reported")
        );
    }

    #[test]
    fn a_duration_is_read_as_the_gateway_writes_it() {
        let seconds = |text: &str| fixed_seconds(text);
        assert_eq!(seconds("1h"), Some(3_600));
        assert_eq!(seconds("90m"), Some(5_400));
        assert_eq!(seconds("1h30m"), Some(5_400));
        assert_eq!(seconds("45s"), Some(45));
        assert_eq!(seconds("0.5h"), Some(1_800));
        // A duration has to be one, and "100ms" is not 100 minutes.
        assert_eq!(seconds("100ms"), None);
        assert_eq!(seconds("forever"), None);
        assert_eq!(seconds("h"), None);
        assert_eq!(seconds(""), None);
    }

    #[test]
    fn a_period_is_named_by_what_the_gateway_stated() {
        let named = |text: &str| Period::parse(Some(text)).label;

        assert_eq!(named("1h"), "1h");
        assert_eq!(named("2h"), "2h");
        assert_eq!(named("5h"), "5h");
        assert_eq!(named("24h"), "24h");
        // Not a whole number of hours: a sort key, and no claim in the heading.
        assert_eq!(named("30m"), "Spend");
        assert_eq!(named("1h30m"), "Spend");

        assert_eq!(named("1d"), "24h");
        assert_eq!(named("7d"), "7d");
        assert_eq!(named("1w"), "7d");
        assert_eq!(named("2d"), "2d");
        assert_eq!(named("2w"), "14d");
        assert_eq!(named("1M"), "Monthly");
        // Nothing here can name a quarter, a year or several months without
        // claiming a number of days.
        assert_eq!(named("3M"), "Spend");
        assert_eq!(named("1Q"), "Spend");
        assert_eq!(named("1Y"), "Spend");
        assert_eq!(named("forever"), "Spend");
        assert_eq!(named(""), "Spend");

        // The sort keys still separate them.
        assert_eq!(Period::parse(Some("3M")).seconds, 90 * 86_400);
        assert_eq!(Period::parse(Some("1Y")).seconds, 365 * 86_400);
        assert_eq!(Period::parse(Some("forever")).seconds, 30 * 86_400);
        // A month, a quarter and a year are never a fixed length…
        assert!(!Period::parse(Some("1M")).is_fixed);
        assert!(!Period::parse(Some("1d")).is_fixed);
        // …but a duration in hours or less is, however it is named.
        assert!(Period::parse(Some("1h")).is_fixed);
        assert!(Period::parse(Some("30m")).is_fixed);
    }
}
