//! Alibaba Cloud's Coding Plan (Model Studio / Bailian): a five-hour, a weekly
//! and a billing-month allowance, each reported as an amount used out of an
//! amount the plan grants.
//!
//! Read with the plan's API key, sent to the console route the Model Studio
//! page itself asks: `POST /data/api.json?action=…queryCodingPlanInstanceInfoV2`
//! on the international console first and, when that one does not know the key
//! or the plan, on the China mainland console. Nothing is chosen in Settings:
//! the key belongs to one site, and whichever site answers for it is the one
//! read. The shape is second-hand — taken from CodexBar's Alibaba provider and
//! its tests, not from a captured reply — and the fixtures below say so.
//!
//! **Only figures the console states.** A window needs both its used amount
//! and its total; a missing or non-positive total leaves that window off rather
//! than drawn at zero. A reset time already in the past is dropped rather than
//! moved forward by five hours, which would be a guess.
//!
//! Some accounts get `ConsoleNeedLogin` for a key: the route wants a console
//! session instead. That is said as a refused key, the nearest shared reason,
//! and is not retried on the other site, which would refuse it the same way.
//!
//! This file also carries [`console`]: the envelope reader the original shares
//! between this service, the Token Plan and Qwen Cloud, which answer through
//! the same gateway in the same shapes.

use std::sync::Arc;
use std::time::Duration;

use chrono::{DateTime, Utc};
use serde_json::{Map, Value};

use super::{
    by_window_length, describe_reqwest_error, percent_from_fraction, Ctx, FetchFuture, Provider,
};
use crate::model::{ProviderUsage, UsageWindow};

const ID: &str = "alibaba-coding-plan";
const NAME: &str = "Alibaba Coding Plan";

/// The console route's own name for this call, as its page sends it.
const ACTION: &str = "zeldaEasy.broadscope-bailian.codingPlan.queryCodingPlanInstanceInfoV2";

/// A request's own timeout, as the original sets it — shorter than the client's,
/// because two sites are asked in turn.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(15);

pub struct AlibabaCodingPlan;

impl Provider for AlibabaCodingPlan {
    fn id(&self) -> &'static str {
        ID
    }

    fn name(&self) -> &'static str {
        NAME
    }

    /// The key the fetch reads, and nothing else.
    fn is_configured(&self) -> bool {
        super::provider_key(self.id()).is_some()
    }

    fn fetch(&self, ctx: Arc<Ctx>) -> FetchFuture {
        Box::pin(async move { fetch_inner(ctx).await })
    }
}

/// One of the two consoles the plan is sold on. The key is sent to the
/// console's own host and nowhere else.
struct Site {
    origin: &'static str,
    region_id: &'static str,
    commodity_code: &'static str,
    page: &'static str,
}

const INTERNATIONAL: Site = Site {
    origin: "https://modelstudio.console.alibabacloud.com",
    region_id: "ap-southeast-1",
    commodity_code: "sfm_codingplan_public_intl",
    page: "https://modelstudio.console.alibabacloud.com/ap-southeast-1/?tab=coding-plan#/efm/coding_plan",
};

const CHINA_MAINLAND: Site = Site {
    origin: "https://bailian.console.aliyun.com",
    region_id: "cn-beijing",
    commodity_code: "sfm_codingplan_public_cn",
    page: "https://bailian.console.aliyun.com/cn-beijing/?tab=model#/efm/coding_plan",
};

impl Site {
    fn endpoint(&self) -> String {
        format!(
            "{}/data/api.json?action={ACTION}&product=broadscope-bailian\
             &api=queryCodingPlanInstanceInfoV2&currentRegionId={}",
            self.origin, self.region_id
        )
    }
}

/// What one site's answer means: a reading, a reason worth asking the other
/// site about, or a reason that ends it.
enum Answer {
    Usage(ProviderUsage),
    AskOtherSite(Reason),
    Final(Reason),
}

/// The shared reasons this service can report, in the original's own set.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Reason {
    /// There is a key, and the service refused it.
    ApiKeyRefused,
    RateLimited,
    ServerError,
    /// Nothing answered at all. Carries the transport's own words, which are
    /// the only account of why there is.
    Unreachable(String),
    UnreadableReply,
    /// The console answered: this account has no plan with limits.
    NoPlan,
    NoLimitsReported,
}

impl Reason {
    fn message(&self) -> String {
        match self {
            Reason::ApiKeyRefused => "the API key was refused".to_string(),
            Reason::RateLimited => "rate limited, try again shortly".to_string(),
            Reason::ServerError => "the service returned an error".to_string(),
            Reason::Unreachable(detail) => detail.clone(),
            Reason::UnreadableReply => "unreadable reply".to_string(),
            Reason::NoPlan => "this account has no plan with usage limits".to_string(),
            Reason::NoLimitsReported => "no limits reported".to_string(),
        }
    }
}

async fn fetch_inner(ctx: Arc<Ctx>) -> ProviderUsage {
    let key = match super::provider_key(ID) {
        Some(key) => key,
        None => return ProviderUsage::failed(ID, NAME, super::missing_key(ID, "")),
    };

    let now = Utc::now();

    let first = ask(&ctx, &INTERNATIONAL, &key, now).await;
    let first_reason = match first {
        Answer::Usage(usage) => return usage,
        Answer::Final(reason) => return ProviderUsage::failed(ID, NAME, reason.message()),
        Answer::AskOtherSite(reason) => reason,
    };

    match ask(&ctx, &CHINA_MAINLAND, &key, now).await {
        Answer::Usage(usage) => usage,
        // A site that refused the key says less than one that took it and
        // found nothing, so the refusal is only reported when both refused.
        Answer::Final(reason) | Answer::AskOtherSite(reason) => {
            let reported = if reason == Reason::ApiKeyRefused {
                first_reason
            } else {
                reason
            };
            ProviderUsage::failed(ID, NAME, reported.message())
        }
    }
}

async fn ask(ctx: &Ctx, site: &Site, key: &str, now: DateTime<Utc>) -> Answer {
    let body = serde_json::json!({
        "queryCodingPlanInstanceInfoRequest": { "commodityCode": site.commodity_code }
    });

    let response = ctx
        .client
        .post(site.endpoint())
        .timeout(REQUEST_TIMEOUT)
        .header("Content-Type", "application/json")
        .header("Accept", "application/json")
        // The three places the console looks for a key, as the page's own
        // request carries it.
        .header("Authorization", format!("Bearer {key}"))
        .header("x-api-key", key)
        .header("X-DashScope-API-Key", key)
        .header("Origin", site.origin)
        .header("Referer", site.page)
        .json(&body)
        .send()
        .await;

    let response = match response {
        Ok(response) => response,
        Err(e) => {
            return Answer::Final(Reason::Unreachable(format!(
                "request failed: {}",
                describe_reqwest_error(&e)
            )))
        }
    };

    let status = response.status().as_u16();
    let text = match response.text().await {
        Ok(text) => text,
        Err(e) => return Answer::Final(Reason::Unreachable(format!("cannot read body: {e}"))),
    };

    // Not this site's key, or not this site's route: the other one may know it.
    if matches!(status, 401 | 403 | 404) {
        return Answer::AskOtherSite(if status == 404 {
            Reason::ServerError
        } else {
            Reason::ApiKeyRefused
        });
    }

    // `ProfileHTTP.classify`, for the statuses that are left: a redirect is
    // never followed, and arrives here as the credential being turned away.
    if !(200..300).contains(&status) {
        return Answer::Final(match status {
            300..=399 => Reason::ApiKeyRefused,
            429 => Reason::RateLimited,
            _ => Reason::ServerError,
        });
    }

    answer(&text, now)
}

// ---------------------------------------------------------------------------
// Reading the reply
// ---------------------------------------------------------------------------

fn answer(text: &str, now: DateTime<Utc>) -> Answer {
    let Ok(raw) = serde_json::from_str::<Value>(text) else {
        return Answer::Final(Reason::UnreadableReply);
    };
    let tree = console::expanded(&raw);
    let Some(root) = tree.as_object() else {
        return Answer::Final(Reason::UnreadableReply);
    };

    if let Some(refusal) = refusal(root) {
        return refusal;
    }

    let instances: Option<Vec<&Map<String, Value>>> = console::first_in(
        &["codingPlanInstanceInfos", "coding_plan_instance_infos"],
        root,
        true,
    )
    .and_then(Value::as_array)
    .map(|items| items.iter().filter_map(Value::as_object).collect());

    let chosen = instances.as_ref().and_then(|list| active_instance(list, now));
    let chosen_is_active = chosen.map(|one| activity(one, now) > 0).unwrap_or(false);

    // Several plans on one account: the active one's figures, and never an
    // expired one's standing in for them.
    let quota = match chosen {
        Some(instance) if quota_info(instance).is_some() => quota_info(instance),
        Some(_) if instances.as_ref().map(Vec::len).unwrap_or(0) > 1 && chosen_is_active => None,
        _ => quota_info(root),
    };

    let windows = quota.map(|quota| windows(quota, now)).unwrap_or_default();
    if windows.is_empty() {
        // A list of plans that is empty, or holds only lapsed ones, is the
        // console saying there is no plan — not that its figures are missing.
        if let Some(list) = &instances {
            if list.iter().all(|instance| activity(instance, now) < 0) {
                return Answer::AskOtherSite(Reason::NoPlan);
            }
        }
        return Answer::AskOtherSite(Reason::NoLimitsReported);
    }

    let plan = chosen
        .and_then(instance_plan_name)
        .or_else(|| plan_name(root));
    Answer::Usage(ProviderUsage::ok(ID, NAME, windows).with_plan(plan))
}

/// A failure the console reports inside a 200.
fn refusal(tree: &Map<String, Value>) -> Option<Answer> {
    let message = console::first_string_in(&["statusMessage", "status_msg", "message", "msg"], tree)
        .unwrap_or_default()
        .to_lowercase();

    if let Some(status) = console::first_int_in(&["statusCode", "status_code", "code"], tree) {
        if status != 0 && status != 200 {
            if status == 401
                || status == 403
                || message.contains("api key")
                || message.contains("unauthorized")
            {
                return Some(Answer::AskOtherSite(Reason::ApiKeyRefused));
            }
            return Some(Answer::Final(Reason::ServerError));
        }
    }

    // `ConsoleNeedLogin`: this route wants a signed-in console, not a key.
    let code = console::first_string_in(&["code", "status", "statusCode"], tree)
        .unwrap_or_default()
        .to_lowercase();
    if code.contains("login") || message.contains("login") || message.contains("log in") {
        return Some(Answer::Final(Reason::ApiKeyRefused));
    }
    None
}

/// One of the three allowances, with the keys the console has used for it.
struct Allowance {
    label: &'static str,
    seconds: i64,
    used: &'static [&'static str],
    total: &'static [&'static str],
    reset: &'static [&'static str],
}

/// The three allowances the console names. A billing month is not a stated
/// length, so its thirty days are a sort key only.
const ALLOWANCES: [Allowance; 3] = [
    Allowance {
        label: "5h",
        seconds: 5 * 3_600,
        used: &["per5HourUsedQuota", "perFiveHourUsedQuota"],
        total: &["per5HourTotalQuota", "perFiveHourTotalQuota"],
        reset: &["per5HourQuotaNextRefreshTime", "perFiveHourQuotaNextRefreshTime"],
    },
    Allowance {
        label: "7d",
        seconds: 7 * 86_400,
        used: &["perWeekUsedQuota"],
        total: &["perWeekTotalQuota"],
        reset: &["perWeekQuotaNextRefreshTime"],
    },
    Allowance {
        label: "Monthly",
        seconds: 30 * 86_400,
        used: &["perBillMonthUsedQuota", "perMonthUsedQuota"],
        total: &["perBillMonthTotalQuota", "perMonthTotalQuota"],
        reset: &["perBillMonthQuotaNextRefreshTime", "perMonthQuotaNextRefreshTime"],
    },
];

const QUOTA_KEYS: [&str; 6] = [
    "per5HourUsedQuota",
    "per5HourTotalQuota",
    "perWeekUsedQuota",
    "perWeekTotalQuota",
    "perBillMonthUsedQuota",
    "perBillMonthTotalQuota",
];

fn windows(quota: &Map<String, Value>, now: DateTime<Utc>) -> Vec<UsageWindow> {
    let mut rows: Vec<(i64, UsageWindow)> = Vec::new();

    for allowance in &ALLOWANCES {
        let Some(used) = console::number(first_value(allowance.used, quota)) else {
            continue;
        };
        let Some(total) = console::number(first_value(allowance.total, quota)) else {
            continue;
        };
        if used < 0.0 || total <= 0.0 {
            continue;
        }

        let reset = console::date(first_value(allowance.reset, quota)).filter(|at| *at > now);

        rows.push((
            allowance.seconds,
            UsageWindow::new(allowance.label, Some(percent_from_fraction(used / total)))
                .with_reset(reset.map(stamp)),
        ));
    }

    by_window_length(rows)
}

/// The first of `keys` the object holds — present-but-null included, which is
/// then read as nothing by the parser for that field.
fn first_value<'a>(keys: &[&str], object: &'a Map<String, Value>) -> Option<&'a Value> {
    keys.iter().find_map(|key| object.get(*key))
}

fn quota_info(value: &Map<String, Value>) -> Option<&Map<String, Value>> {
    if let Some(named) = console::first_in(
        &["codingPlanQuotaInfo", "coding_plan_quota_info"],
        value,
        false,
    ) {
        if let Some(object) = named.as_object() {
            return Some(object);
        }
    }
    console::first_object_in(value, true, &|object| {
        QUOTA_KEYS.iter().any(|key| object.contains_key(*key))
    })
}

/// The instance that is running, or the first when none says either way.
///
/// Ties keep the **first**: Swift's `max(by:)` only replaces on a strict
/// increase, and the list's own order is the console's.
fn active_instance<'a>(
    instances: &[&'a Map<String, Value>],
    now: DateTime<Utc>,
) -> Option<&'a Map<String, Value>> {
    let mut best: Option<&Map<String, Value>> = None;
    for instance in instances {
        let better = match best {
            Some(current) => activity(instance, now) > activity(current, now),
            None => true,
        };
        if better {
            best = Some(instance);
        }
    }

    match best {
        Some(instance) if activity(instance, now) > 0 => Some(instance),
        _ => instances.first().copied(),
    }
}

/// How sure the instance is about being active: stated outright, implied by an
/// end date still ahead, or said to have lapsed.
fn activity(instance: &Map<String, Value>, now: DateTime<Utc>) -> i32 {
    let status = instance
        .get("status")
        .or_else(|| instance.get("instanceStatus"))
        .and_then(Value::as_str)
        .map(str::to_uppercase);

    if let Some(status) = status {
        if status == "VALID" || status == "ACTIVE" {
            return 3;
        }
        const LAPSED: [&str; 6] = [
            "EXPIRED",
            "INVALID",
            "INACTIVE",
            "DISABLED",
            "TERMINATED",
            "STOPPED",
        ];
        if LAPSED.contains(&status.as_str()) {
            return -1;
        }
    }

    if let Some(active) = instance
        .get("isActive")
        .or_else(|| instance.get("active"))
        .and_then(Value::as_bool)
    {
        return if active { 3 } else { -1 };
    }

    let end = ["endTime", "periodEndTime", "expireTime", "expirationTime"]
        .iter()
        .find_map(|key| console::date(instance.get(*key)));

    if end.map(|end| end > now).unwrap_or(false) {
        1
    } else {
        0
    }
}

fn instance_plan_name(instance: &Map<String, Value>) -> Option<String> {
    [
        "planName",
        "plan_name",
        "instanceName",
        "instance_name",
        "packageName",
        "package_name",
    ]
    .iter()
    .find_map(|key| console::string(instance.get(*key)))
}

fn plan_name(tree: &Map<String, Value>) -> Option<String> {
    console::first_string_in(
        &["planName", "plan_name", "packageName", "package_name"],
        tree,
    )
}

fn stamp(at: DateTime<Utc>) -> String {
    at.to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
}

// ---------------------------------------------------------------------------
// The console's envelopes
// ---------------------------------------------------------------------------

/// Alibaba's console replies nest, and sometimes carry the real reply as a JSON
/// string inside a field. Unwrapped here so a reading sees one tree.
///
/// The original's `AlibabaConsoleJSON`, shared with the Token Plan and Qwen
/// Cloud, which answer through the same console gateway in the same envelopes.
pub(super) mod console {
    use chrono::{DateTime, Local, NaiveDate, NaiveDateTime, TimeZone, Utc};
    use serde_json::{Map, Value};

    /// Every JSON string that holds JSON, replaced by what it holds.
    pub(crate) fn expanded(value: &Value) -> Value {
        match value {
            Value::String(text) => {
                let trimmed = text.trim();
                if trimmed.starts_with('{') || trimmed.starts_with('[') {
                    if let Ok(inner) = serde_json::from_str::<Value>(trimmed) {
                        return expanded(&inner);
                    }
                }
                value.clone()
            }
            Value::Object(map) => Value::Object(
                map.iter()
                    .map(|(key, nested)| (key.clone(), expanded(nested)))
                    .collect(),
            ),
            Value::Array(items) => Value::Array(items.iter().map(expanded).collect()),
            _ => value.clone(),
        }
    }

    /// The first object, the outer one before what it holds, that `matches`.
    pub(crate) fn first_object_in<'a, F>(
        map: &'a Map<String, Value>,
        into_arrays: bool,
        matches: &F,
    ) -> Option<&'a Map<String, Value>>
    where
        F: Fn(&Map<String, Value>) -> bool,
    {
        if matches(map) {
            return Some(map);
        }
        map.values()
            .find_map(|nested| first_object(nested, into_arrays, matches))
    }

    fn first_object<'a, F>(
        value: &'a Value,
        into_arrays: bool,
        matches: &F,
    ) -> Option<&'a Map<String, Value>>
    where
        F: Fn(&Map<String, Value>) -> bool,
    {
        match value {
            Value::Object(map) => first_object_in(map, into_arrays, matches),
            Value::Array(items) if into_arrays => items
                .iter()
                .find_map(|nested| first_object(nested, into_arrays, matches)),
            _ => None,
        }
    }

    /// The value of the first of `keys`, in the first object that holds one.
    pub(crate) fn first_in<'a>(
        keys: &[&str],
        map: &'a Map<String, Value>,
        into_arrays: bool,
    ) -> Option<&'a Value> {
        let object = first_object_in(map, into_arrays, &|object| {
            keys.iter().any(|key| object.contains_key(*key))
        })?;
        keys.iter().find_map(|key| object.get(*key))
    }

    /// The first non-empty string any of `keys` names, anywhere in the tree.
    pub(crate) fn first_string_in(keys: &[&str], map: &Map<String, Value>) -> Option<String> {
        let object = first_object_in(map, true, &|object| {
            keys.iter().any(|key| string(object.get(*key)).is_some())
        })?;
        string_in(keys, object)
    }

    /// The first of `keys` anywhere in the tree that is a whole number.
    pub(crate) fn first_int_in(keys: &[&str], map: &Map<String, Value>) -> Option<i64> {
        let object = first_object_in(map, true, &|object| {
            keys.iter().any(|key| whole(object.get(*key)).is_some())
        })?;
        keys.iter().find_map(|key| whole(object.get(*key)))
    }

    /// The same two readers over an object already in hand.
    pub(crate) fn string_in(keys: &[&str], object: &Map<String, Value>) -> Option<String> {
        keys.iter().find_map(|key| string(object.get(*key)))
    }

    /// A number, or a string that is one. **Never a boolean**, which a JSON
    /// parser would otherwise hand over as 0 or 1.
    pub(crate) fn number(value: Option<&Value>) -> Option<f64> {
        let parsed = match value? {
            Value::Number(number) => number.as_f64()?,
            Value::String(text) => text.trim().parse::<f64>().ok()?,
            _ => return None,
        };
        parsed.is_finite().then_some(parsed)
    }

    /// A number that is a whole one, the way Swift's `Int(exactly:)` reads one.
    fn whole(value: Option<&Value>) -> Option<i64> {
        let number = number(value)?;
        if number.fract() != 0.0 || number < i64::MIN as f64 || number > i64::MAX as f64 {
            return None;
        }
        Some(number as i64)
    }

    /// A trimmed, non-empty string.
    pub(crate) fn string(value: Option<&Value>) -> Option<String> {
        let text = value?.as_str()?.trim();
        if text.is_empty() {
            None
        } else {
            Some(text.to_string())
        }
    }

    /// Epoch seconds or milliseconds, ISO 8601, or the console's own
    /// `yyyy-MM-dd HH:mm[:ss]`.
    pub(crate) fn date(value: Option<&Value>) -> Option<DateTime<Utc>> {
        if let Some(epoch) = number(value) {
            if epoch > 0.0 {
                let seconds = if epoch >= 1_000_000_000_000.0 {
                    epoch / 1_000.0
                } else {
                    epoch
                };
                return DateTime::from_timestamp(seconds as i64, 0);
            }
        }

        let text = string(value)?;
        if let Some(at) = DateTime::parse_from_rfc3339(text.trim())
            .ok()
            .map(|at| at.with_timezone(&Utc))
        {
            return Some(at);
        }

        for format in ["%Y-%m-%d %H:%M:%S", "%Y-%m-%d %H:%M", "%Y-%m-%d"] {
            if let Ok(naive) = NaiveDateTime::parse_from_str(&text, format) {
                return in_local_zone(naive);
            }
            if let Ok(day) = NaiveDate::parse_from_str(&text, format) {
                return in_local_zone(day.and_hms_opt(0, 0, 0)?);
            }
        }
        None
    }

    /// A stamp with no zone is read in the machine's own, as the original's own
    /// formatter reads it: the console writes the reader's local time.
    fn in_local_zone(naive: NaiveDateTime) -> Option<DateTime<Utc>> {
        Local
            .from_local_datetime(&naive)
            .earliest()
            .map(|at| at.with_timezone(&Utc))
    }

    /// What a reply that is not a reading says went wrong, read the way the
    /// console's own page reads it.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub(crate) enum Failure {
        /// The console wants a sign-in, or refused the one it was given.
        SignedOut,
        /// Anything else the console says failed.
        Failed,
    }

    pub(crate) fn failure(tree: &Map<String, Value>) -> Option<Failure> {
        let frame = first_object_in(tree, true, &|object| {
            ["successResponse", "success", "Success"]
                .iter()
                .any(|key| object.get(*key).and_then(Value::as_bool) == Some(false))
        });

        const CODES: [&str; 5] = ["errorCode", "code", "Code", "status", "statusCode"];
        const MESSAGES: [&str; 5] = ["errorMsg", "message", "Message", "msg", "statusMessage"];

        let code = frame
            .and_then(|frame| string_in(&CODES, frame))
            .or_else(|| first_string_in(&CODES, tree));
        let message = frame
            .and_then(|frame| string_in(&MESSAGES, frame))
            .or_else(|| first_string_in(&MESSAGES, tree));
        let said = [code, message]
            .into_iter()
            .flatten()
            .map(|text| text.to_lowercase())
            .collect::<Vec<String>>()
            .join(" ");

        const SIGN_IN: [&str; 6] = [
            "needlogin",
            "login",
            "tokenerror",
            "request has expired",
            "refresh page",
            "请求已经过期",
        ];
        // A workspace the account may not use is a permission, not a session:
        // signing in again would change nothing.
        const REFUSED: [&str; 8] = [
            "notauthorised",
            "notauthorized",
            "not authorised",
            "not authorized",
            "unauthorised",
            "unauthorized",
            "access denied",
            "forbidden",
        ];

        if SIGN_IN.iter().any(|phrase| said.contains(phrase)) {
            return Some(Failure::SignedOut);
        }
        if !said.contains("workspace.notauthori") && REFUSED.iter().any(|phrase| said.contains(phrase))
        {
            return Some(Failure::SignedOut);
        }
        if frame.is_some() {
            return Some(Failure::Failed);
        }

        if let Some(status) =
            first_int_in(&["statusCode", "status_code", "code", "httpStatusCode"], tree)
        {
            if status != 0 && status != 200 {
                return Some(if status == 401 || status == 403 {
                    Failure::SignedOut
                } else {
                    Failure::Failed
                });
            }
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn at(text: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(text).unwrap().with_timezone(&Utc)
    }

    /// A console reply's shape, as CodexBar's tests describe it.
    fn fixture() -> Value {
        json!({
            "statusCode": 200,
            "codingPlanInstanceInfos": [
                { "instanceId": "i-1", "status": "VALID", "planName": "Coding Plan Pro",
                  "codingPlanQuotaInfo": {
                      "per5HourUsedQuota": 40, "per5HourTotalQuota": 100,
                      "per5HourQuotaNextRefreshTime": 1_800_000_000_000i64,
                      "perWeekUsedQuota": "250", "perWeekTotalQuota": "1000",
                      "perBillMonthUsedQuota": 900, "perBillMonthTotalQuota": 0
                  } }
            ]
        })
    }

    fn reading_of(reply: &Value, now: DateTime<Utc>) -> ProviderUsage {
        match answer(&reply.to_string(), now) {
            Answer::Usage(usage) => usage,
            _ => panic!("expected a reading"),
        }
    }

    #[test]
    fn reads_the_three_allowances_the_console_states() {
        let usage = reading_of(&fixture(), at("2026-10-02T00:00:00Z"));

        // The month's total is zero, which is no total: that window is left off
        // rather than drawn at zero.
        let labels: Vec<&str> = usage.windows.iter().map(|w| w.label.as_str()).collect();
        assert_eq!(labels, vec!["5h", "7d"]);
        assert_eq!(usage.windows[0].percent_used, Some(40.0));
        // The weekly figures arrive as strings, the way the console sometimes
        // sends them.
        assert_eq!(usage.windows[1].percent_used, Some(25.0));
        assert_eq!(usage.plan.as_deref(), Some("Coding Plan Pro"));
    }

    #[test]
    fn a_reset_already_past_is_dropped_rather_than_moved_forward() {
        let usage = reading_of(&fixture(), at("2026-10-02T00:00:00Z"));
        // 1_800_000_000_000 ms is 2027-01-15T08:00:00Z.
        assert_eq!(
            usage.windows[0].resets_at.as_deref(),
            Some("2027-01-15T08:00:00Z")
        );

        // The same reply read a year later: the stamp is stale, and a stale
        // stamp is not a new window.
        let usage = reading_of(&fixture(), at("2028-01-01T00:00:00Z"));
        assert!(usage.windows[0].resets_at.is_none());
    }

    /// A plan that is running is read; one that has lapsed is not, and never
    /// stands in for a running one.
    #[test]
    fn the_active_instance_is_the_one_read() {
        let reply = json!({
            "codingPlanInstanceInfos": [
                { "status": "EXPIRED", "planName": "Lapsed",
                  "codingPlanQuotaInfo": { "per5HourUsedQuota": 100, "per5HourTotalQuota": 100 } },
                { "status": "VALID", "planName": "Running",
                  "codingPlanQuotaInfo": { "per5HourUsedQuota": 25, "per5HourTotalQuota": 100 } }
            ]
        });

        let usage = reading_of(&reply, at("2026-10-02T00:00:00Z"));
        assert_eq!(usage.windows[0].percent_used, Some(25.0));
        assert_eq!(usage.plan.as_deref(), Some("Running"));
    }

    #[test]
    fn a_list_of_only_lapsed_plans_is_no_plan() {
        let lapsed = json!({
            "codingPlanInstanceInfos": [ { "status": "EXPIRED", "instanceStatus": "INVALID" } ]
        });
        assert!(matches!(
            answer(&lapsed.to_string(), Utc::now()),
            Answer::AskOtherSite(Reason::NoPlan)
        ));

        // An empty list says the same thing: Swift's `allSatisfy` is true of
        // nothing at all.
        let empty = json!({ "codingPlanInstanceInfos": [] });
        assert!(matches!(
            answer(&empty.to_string(), Utc::now()),
            Answer::AskOtherSite(Reason::NoPlan)
        ));

        // No list at all is the other answer: figures missing, not a plan.
        assert!(matches!(
            answer(&json!({ "data": {} }).to_string(), Utc::now()),
            Answer::AskOtherSite(Reason::NoLimitsReported)
        ));
    }

    /// A refusal inside a 200 is the only place a key the console will not take
    /// can be seen.
    #[test]
    fn a_console_that_wants_a_sign_in_is_a_refused_key() {
        let signed_out = json!({ "success": false, "code": "ConsoleNeedLogin" });
        assert!(matches!(
            answer(&signed_out.to_string(), Utc::now()),
            Answer::Final(Reason::ApiKeyRefused)
        ));

        let other_site = json!({ "statusCode": 401, "statusMessage": "Unauthorized" });
        assert!(matches!(
            answer(&other_site.to_string(), Utc::now()),
            Answer::AskOtherSite(Reason::ApiKeyRefused)
        ));

        let broken = json!({ "statusCode": 500, "statusMessage": "internal error" });
        assert!(matches!(
            answer(&broken.to_string(), Utc::now()),
            Answer::Final(Reason::ServerError)
        ));

        // A reply that is not JSON at all, and one that is not an object.
        assert!(matches!(
            answer("<html>", Utc::now()),
            Answer::Final(Reason::UnreadableReply)
        ));
        assert!(matches!(
            answer("[1, 2]", Utc::now()),
            Answer::Final(Reason::UnreadableReply)
        ));
    }

    #[test]
    fn the_console_may_nest_its_reply_or_hand_it_over_as_a_string() {
        let nested = json!({
            "data": { "result": { "codingPlanInstanceInfos": [
                { "status": "ACTIVE",
                  "codingPlanQuotaInfo": { "perWeekUsedQuota": 5, "perWeekTotalQuota": 10 } }
            ] } }
        });
        assert_eq!(reading_of(&nested, Utc::now()).windows[0].percent_used, Some(50.0));

        // The real reply inside a string field, as the console sometimes sends
        // it.
        let inner = json!({
            "codingPlanInstanceInfos": [
                { "status": "VALID",
                  "codingPlanQuotaInfo": { "per5HourUsedQuota": 1, "per5HourTotalQuota": 4 } }
            ]
        });
        let wrapped = json!({ "data": inner.to_string() });
        assert_eq!(reading_of(&wrapped, Utc::now()).windows[0].percent_used, Some(25.0));
    }

    #[test]
    fn the_first_of_equal_instances_is_the_active_one() {
        let holding = [
            json!({ "instanceId": "a", "endTime": "2030-01-01T00:00:00Z" }),
            json!({ "instanceId": "b", "endTime": "2031-01-01T00:00:00Z" }),
        ];
        let list: Vec<&Map<String, Value>> = holding
            .iter()
            .filter_map(|value| value.as_object())
            .collect();

        // Both are implied-active by an end date still ahead; the first wins.
        let chosen = active_instance(&list, at("2026-10-02T00:00:00Z")).unwrap();
        assert_eq!(chosen.get("instanceId").and_then(Value::as_str), Some("a"));

        // None saying either way is the first as well.
        let quiet = [json!({ "instanceId": "a" }), json!({ "instanceId": "b" })];
        let list: Vec<&Map<String, Value>> = quiet
            .iter()
            .filter_map(|value| value.as_object())
            .collect();
        let chosen = active_instance(&list, at("2026-10-02T00:00:00Z")).unwrap();
        assert_eq!(chosen.get("instanceId").and_then(Value::as_str), Some("a"));

        assert!(active_instance(&[], at("2026-10-02T00:00:00Z")).is_none());
    }

    /// A lapsed instance that is still the only one is not run through the
    /// quota it holds.
    #[test]
    fn a_lapsed_instance_is_looked_past_rather_than_read() {
        let reply = json!({
            "codingPlanInstanceInfos": [
                { "status": "VALID", "instanceId": "live",
                  "codingPlanQuotaInfo": { "per5HourUsedQuota": 1, "per5HourTotalQuota": 2 } },
                { "status": "EXPIRED", "instanceId": "gone",
                  "codingPlanQuotaInfo": { "perWeekUsedQuota": 9, "perWeekTotalQuota": 10 } }
            ]
        });
        let usage = reading_of(&reply, at("2026-10-02T00:00:00Z"));
        assert_eq!(usage.windows.len(), 1);
        assert_eq!(usage.windows[0].label, "5h");
    }

    #[test]
    fn a_window_needs_both_figures_and_a_positive_total() {
        let quota = json!({
            "per5HourUsedQuota": 10,
            "perWeekUsedQuota": 10, "perWeekTotalQuota": 0,
            "perBillMonthUsedQuota": -1, "perBillMonthTotalQuota": 10
        });
        // The five-hour window states a used amount and no total at all; the
        // week's total is zero and the month's used amount is negative.
        assert!(windows(quota.as_object().unwrap(), Utc::now()).is_empty());
    }

    #[test]
    fn the_sites_route_is_the_one_the_console_page_asks() {
        assert_eq!(
            INTERNATIONAL.endpoint(),
            "https://modelstudio.console.alibabacloud.com/data/api.json\
             ?action=zeldaEasy.broadscope-bailian.codingPlan.queryCodingPlanInstanceInfoV2\
             &product=broadscope-bailian&api=queryCodingPlanInstanceInfoV2\
             &currentRegionId=ap-southeast-1"
        );
        assert!(CHINA_MAINLAND
            .endpoint()
            .starts_with("https://bailian.console.aliyun.com/data/api.json"));

        // A site sends its key to its own host and its own page, and no other.
        assert!(INTERNATIONAL.page.starts_with(INTERNATIONAL.origin));
        assert!(CHINA_MAINLAND.page.starts_with(CHINA_MAINLAND.origin));
    }

    #[test]
    fn a_reason_says_what_happened() {
        assert_eq!(Reason::ApiKeyRefused.message(), "the API key was refused");
        assert_eq!(Reason::NoLimitsReported.message(), "no limits reported");
        assert_eq!(
            Reason::Unreachable("request failed: dns".to_string()).message(),
            "request failed: dns"
        );
    }

    mod envelopes {
        use super::*;
        use crate::providers::alibaba_coding_plan::console;
        use chrono::TimeZone;

        /// The object a fixture holds, as the readers take one.
        fn as_map(value: Value) -> Map<String, Value> {
            match value {
                Value::Object(map) => map,
                other => panic!("expected an object, got {other}"),
            }
        }

        #[test]
        fn a_number_may_arrive_as_a_string_and_never_as_a_boolean() {
            assert_eq!(console::number(Some(&json!(96.0))), Some(96.0));
            assert_eq!(console::number(Some(&json!("96"))), Some(96.0));
            assert_eq!(console::number(Some(&json!(" 75 "))), Some(75.0));
            assert_eq!(console::number(Some(&json!(true))), None);
            assert_eq!(console::number(Some(&json!("a lot"))), None);
            assert_eq!(console::number(None), None);
        }

        #[test]
        fn a_date_is_read_from_every_shape_the_console_writes() {
            // Epoch seconds, epoch milliseconds, ISO 8601 either way.
            assert_eq!(
                console::date(Some(&json!(1_800_000_000))),
                Some(at("2027-01-15T08:00:00Z"))
            );
            assert_eq!(
                console::date(Some(&json!(1_800_000_000_000i64))),
                Some(at("2027-01-15T08:00:00Z"))
            );
            assert_eq!(
                console::date(Some(&json!("2027-01-15T08:00:00Z"))),
                Some(at("2027-01-15T08:00:00Z"))
            );
            // Fractional seconds are kept, as the original's own formatter
            // keeps them; they are dropped when the window is stamped.
            assert_eq!(
                console::date(Some(&json!("2027-01-15T08:00:00.500Z")))
                    .unwrap()
                    .timestamp_millis(),
                1_800_000_000_500
            );

            // The console's own stamp, read in this machine's zone, as the
            // original's formatter reads it.
            let local = console::date(Some(&json!("2027-01-15 08:00:00"))).unwrap();
            assert_eq!(
                local,
                chrono::Local
                    .with_ymd_and_hms(2027, 1, 15, 8, 0, 0)
                    .unwrap()
                    .with_timezone(&Utc)
            );
            let day = console::date(Some(&json!("2027-01-15"))).unwrap();
            assert_eq!(
                day,
                chrono::Local
                    .with_ymd_and_hms(2027, 1, 15, 0, 0, 0)
                    .unwrap()
                    .with_timezone(&Utc)
            );

            // Nothing at all, a zero epoch, and a stamp that is not one.
            assert_eq!(console::date(Some(&json!(0))), None);
            assert_eq!(console::date(Some(&json!("not a date"))), None);
            assert_eq!(console::date(None), None);
        }

        #[test]
        fn the_first_holder_of_a_key_is_the_one_read() {
            // The object itself is read before what it holds.
            let tree = as_map(json!({ "statusCode": 200, "nested": { "statusCode": 401 } }));
            assert_eq!(console::first_int_in(&["statusCode"], &tree), Some(200));

            let said = as_map(json!({ "msg": " ConsoleNeedLogin " }));
            assert_eq!(
                console::first_string_in(&["message", "msg"], &said),
                Some("ConsoleNeedLogin".to_string())
            );

            // An array is descended into, unless the caller says not to.
            let listed = as_map(json!({ "items": [ { "code": 7 } ] }));
            assert_eq!(console::first_int_in(&["code"], &listed), Some(7));
            assert!(console::first_in(&["code"], &listed, false).is_none());

            // A fraction is not a whole number, and a boolean is not a number.
            let fractional = as_map(json!({ "code": 4.5 }));
            assert_eq!(console::first_int_in(&["code"], &fractional), None);
            let boolean = as_map(json!({ "code": true }));
            assert_eq!(console::first_int_in(&["code"], &boolean), None);
        }

        #[test]
        fn a_failed_frame_and_a_sign_in_are_told_apart() {
            use crate::providers::alibaba_coding_plan::console::Failure;

            assert_eq!(
                console::failure(&as_map(json!({ "success": false }))),
                Some(Failure::Failed)
            );
            assert_eq!(
                console::failure(&as_map(json!({ "code": "ConsoleNeedLogin" }))),
                Some(Failure::SignedOut)
            );
            assert_eq!(
                console::failure(&as_map(json!({ "statusCode": 403 }))),
                Some(Failure::SignedOut)
            );
            assert_eq!(
                console::failure(&as_map(json!({ "statusCode": 500 }))),
                Some(Failure::Failed)
            );
            // A workspace the account may not use is a permission: not a
            // session, and not a failure of any kind.
            assert_eq!(
                console::failure(&as_map(json!({ "code": "workspace.notauthorized" }))),
                None
            );
            assert_eq!(console::failure(&as_map(json!({ "statusCode": 200 }))), None);
        }
    }
}
