//! Qoder's credits, read the way its own account page reads them.
//!
//! **The credential is a pasted cookie.** Qoder publishes no usage API. The
//! account page (`/account/usage`) asks
//! `GET /api/v2/me/usages/big_model_credits` with the signed-in cookies and
//! draws what comes back, so that session is the credential — and the original
//! takes it out of the browser's own store, which on macOS means Full Disk
//! Access or a keychain prompt. PulseWin cannot do either, so the `Cookie`
//! header copied out of a signed-in request is the credential here.
//!
//! **Two sites, two accounts.** `qoder.com` and `qoder.com.cn` are separate
//! sign-ins on separate hosts, and a session for one is never sent to the
//! other: which one is read is a choice, not a fallback —
//! `PULSEWIN_QODER_SITE` (or the `site` field of the same settings file),
//! `china` for the mainland one, `qoder.com` by default.
//!
//! **A deny list, where the shared helper is an allow list.** Qoder's session
//! cookie has no published name, so what the host set is kept and what a
//! third-party analytics script set is dropped. Alibaba's own (`cna`, `isg`,
//! `tfstk`) are **kept**: they look like tracking and some of them are, but the
//! same family carries the bot screening in front of the site, and dropping one
//! is how a session that works in the browser gets refused here. Whatever is
//! kept never leaves for any host but the one it came from.
//!
//! **Not the IDE's own figures.** The Qoder app caches a credit reading of its
//! own and exposes it over a local socket, but other monitors reading it found
//! it disagreeing with the billing page for paid accounts. This is the web
//! route, which is the one the page itself uses.
//!
//! **A reset already behind `now` is no reset.** A mainland trial account was
//! seen answering with a `nextResetAt` a month in the past beside 586 credits
//! it could still spend: the period stopped turning over and the date was left
//! where it was. The credits are real; the date is not, so the ring is drawn
//! without one. The packs' own end dates are not carried: this port's window
//! has nowhere to put an expiry, and a reset the service never stated is worse
//! than a line that is missing.
//!
//! Reply shape, from the account page's own request. Both spellings are
//! accepted per field: the mainland site's reply mixes them (`nextResetAt`
//! beside `total_quota`), and other readers recorded all-camelCase. The shapes
//! are second-hand — taken from the original and its fixtures, not from a
//! capture made here.

use std::sync::Arc;

use chrono::{DateTime, Utc};
use serde_json::Value;

use super::{describe_reqwest_error, percent_from_fraction, Ctx, FetchFuture, Provider};
use crate::model::{ProviderUsage, UsageWindow};
use crate::credentials;

const ID: &str = "qoder";
const NAME: &str = "Qoder";

/// Prefixes of cookies set by third-party analytics and advertising scripts
/// rather than by Qoder.
const ANALYTICS: [&str; 21] = [
    "_ga", "_gid", "_gat", "_gcl", "_fbp", "_fbc", "_clck", "_clsk", "_hj", "_uet", "_tt_", "_ttp",
    "ajs_", "amp_", "mp_", "hm_", "hmaccount", "intercom-", "__stripe", "_rdt", "_pin",
];

/// How long a `Cookie` header may be before it is not one somebody pasted.
const MAXIMUM_HEADER: usize = 32_768;

pub struct Qoder;

impl Provider for Qoder {
    fn id(&self) -> &'static str {
        ID
    }

    fn name(&self) -> &'static str {
        NAME
    }

    /// Whether a pasted cookie that survives the filter is on the machine. No
    /// network call.
    fn is_configured(&self) -> bool {
        super::pasted::cookie(ID).is_some_and(|pasted| normalize(&pasted).is_ok())
    }

    fn fetch(&self, ctx: Arc<Ctx>) -> FetchFuture {
        Box::pin(async move { fetch_inner(ctx).await })
    }
}

// ---------------------------------------------------------------------------
// The two sites
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Site {
    International,
    China,
}

impl Site {
    /// The account page's host, and the only host whose cookies are read.
    fn host(&self) -> &'static str {
        match self {
            Site::International => "qoder.com",
            Site::China => "qoder.com.cn",
        }
    }

    fn origin(&self) -> String {
        format!("https://{}", self.host())
    }

    fn usage_url(&self) -> String {
        format!("{}/api/v2/me/usages/big_model_credits", self.origin())
    }

    /// The page that makes this request, so the `Referer` is true rather than
    /// invented.
    fn account_page(&self) -> String {
        format!("{}/account/usage", self.origin())
    }

    fn named(typed: &str) -> Self {
        let typed = typed.trim().to_lowercase();
        if typed.contains("cn") || typed.contains("china") {
            Site::China
        } else {
            Site::International
        }
    }
}

/// Which site is read, from the two places this port has.
fn site() -> Site {
    if let Some(typed) = credentials::env_override(ID, "site") {
        return Site::named(&typed);
    }
    if let Some(path) = super::settings_path(ID) {
        if let Some(json) = credentials::read_json(&path) {
            if let Some(typed) = credentials::dig_str(&json, "site") {
                return Site::named(&typed);
            }
        }
    }
    Site::International
}

// ---------------------------------------------------------------------------
// The cookies
// ---------------------------------------------------------------------------

/// Why a pasted header is not one this will send.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CookieProblem {
    /// Nothing was pasted, or nothing was left once the `Cookie:` name was
    /// taken off.
    Missing,
    /// A control character, a length this will not forward, or nothing of the
    /// host's own left after the analytics were dropped.
    Invalid,
}

/// A `Cookie:` header reduced to the cookies worth sending.
///
/// Takes what a browser store hands over *or* what somebody pasted out of their
/// network tab, which is why the `Cookie:` prefix is tolerated and why each
/// value is checked rather than trusted: a header assembled from an arbitrary
/// string is a header injection if a value carries a newline.
fn normalize(input: &str) -> Result<String, CookieProblem> {
    if input
        .chars()
        .any(|c| !c.is_ascii() || c.is_ascii_control())
    {
        return Err(CookieProblem::Invalid);
    }

    let mut header = input.trim();
    if header.len() >= "cookie:".len() && header[.."cookie:".len()].eq_ignore_ascii_case("cookie:") {
        header = header["cookie:".len()..].trim();
    }
    if header.is_empty() {
        return Err(CookieProblem::Missing);
    }
    if header.len() > MAXIMUM_HEADER {
        return Err(CookieProblem::Invalid);
    }

    let mut kept: Vec<String> = Vec::new();
    let mut seen: Vec<&str> = Vec::new();

    for pair in header.split(';') {
        let Some((name, value)) = pair.split_once('=') else {
            continue;
        };
        let name = name.trim();
        let value = value.trim();
        if name.is_empty() || value.is_empty() {
            continue;
        }

        let lowered = name.to_lowercase();
        if ANALYTICS.iter().any(|prefix| lowered.starts_with(prefix)) {
            continue;
        }
        // A cookie this cannot pass on unaltered is one it drops, rather than
        // one that costs the whole session: an odd preference cookie is not a
        // reason to refuse the sign-in beside it.
        if value.contains(' ') || value.contains('\\') || name.contains(' ') {
            continue;
        }
        // A host-only row and a domain row for one name is normal in every
        // browser store; the first wins, as it does in any cookie header.
        if seen.contains(&name) {
            continue;
        }
        seen.push(name);
        kept.push(format!("{name}={value}"));
    }

    if kept.is_empty() {
        return Err(CookieProblem::Invalid);
    }
    Ok(kept.join("; "))
}

fn cookie_problem(problem: CookieProblem) -> String {
    match problem {
        CookieProblem::Missing => super::pasted::needed(ID, "Qoder session cookie"),
        CookieProblem::Invalid => format!(
            "the pasted cookie for {ID} carries no Qoder session — paste one `Cookie` header \
             from a signed-in {} request, on one line",
            site().host()
        ),
    }
}

// ---------------------------------------------------------------------------
// The request
// ---------------------------------------------------------------------------

async fn fetch_inner(ctx: Arc<Ctx>) -> ProviderUsage {
    let Some(pasted) = super::pasted::cookie(ID) else {
        return ProviderUsage::failed(ID, NAME, super::pasted::needed(ID, "Qoder session cookie"));
    };
    let header = match normalize(&pasted) {
        Ok(header) => header,
        Err(problem) => return ProviderUsage::failed(ID, NAME, cookie_problem(problem)),
    };

    let site = site();

    // The client that refuses redirects: a signed-out page answers this call by
    // redirecting it at the sign-in flow, and a `Cookie` header set by hand
    // rides a redirect to whatever host it names.
    let response = ctx
        .gateway_client
        .get(site.usage_url())
        .header("Cookie", header)
        .header("Accept", "application/json, text/plain, */*")
        .header("Origin", site.origin())
        .header("Referer", site.account_page())
        // What the page's own request carries. `Bx-V` belongs to the bot
        // screening in front of the site: a request that does not look like the
        // page it stands in for is the one that screening turns away.
        .header("X-Requested-With", "XMLHttpRequest")
        .header("Bx-V", "2.5.35")
        .header(
            "User-Agent",
            "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 \
             (KHTML, like Gecko) Chrome/143.0.0.0 Safari/537.36",
        )
        .send()
        .await;

    let response = match response {
        Ok(response) => response,
        Err(e) => {
            return ProviderUsage::failed(ID, NAME, format!("request failed: {}", describe_reqwest_error(&e)))
        }
    };

    let status = response.status();
    let body = match response.text().await {
        Ok(body) => body,
        Err(e) => return ProviderUsage::failed(ID, NAME, format!("cannot read body: {e}")),
    };

    if status.is_redirection() {
        return ProviderUsage::failed(ID, NAME, session_expired());
    }
    if !status.is_success() {
        return ProviderUsage::failed(
            ID,
            NAME,
            match status.as_u16() {
                401 | 403 => session_expired(),
                429 => super::http_failure(status, &[(429, " — rate limited, try again shortly")]),
                500..=599 => "the service reported an error".to_string(),
                _ => "the reply could not be read".to_string(),
            },
        );
    }

    let snapshot = match parse(&body) {
        Ok(snapshot) => snapshot,
        Err(reason) => return ProviderUsage::failed(ID, NAME, reason),
    };

    let windows = windows(&snapshot, Utc::now());
    // A complete answer: the account has no allowance to display.
    if windows.is_empty() {
        return ProviderUsage::failed(ID, NAME, "This Qoder account has no credits.");
    }

    ProviderUsage::ok(ID, NAME, windows)
}

fn session_expired() -> String {
    "Qoder's saved session expired. Sign in again in your browser.".to_string()
}

// ---------------------------------------------------------------------------
// The reply
// ---------------------------------------------------------------------------

/// One allowance: what has been used, and what the limit is.
///
/// The reply also states a remainder of its own. It is read for two things the
/// original does with it — it overrides the arithmetic when it says a pool is
/// spent, and it feeds the packs' expiry line — and this port's window carries
/// neither an exhaustion flag nor an expiry, so the share used here is
/// `usedValue` over `limitValue` and nothing else.
struct Pool {
    used: f64,
    limit: f64,
}

/// What one read of the account page's route returns.
struct Snapshot {
    /// The account's own credits: plan plus resource packs.
    personal: Pool,
    /// A team's shared pool. None off a team plan, and none when Qoder reports
    /// an empty placeholder — a pool of zero is not one anybody can spend.
    shared: Option<Pool>,
    resets_at: Option<DateTime<Utc>>,
}

fn parse(body: &str) -> Result<Snapshot, String> {
    let root: Value = serde_json::from_str(body).map_err(|_| unreadable())?;
    if !root.is_object() {
        return Err(unreadable());
    }

    let personal = root
        .get("totalQuota")
        .or_else(|| root.get("total_quota"))
        .and_then(summary)
        .and_then(pool)
        .ok_or_else(unreadable)?;

    let shared = match root.get("sharedQuota").or_else(|| root.get("shared_quota")) {
        None => None,
        Some(container) => {
            // An absent team pool is normal; an unreadable one is not proof
            // that no allowance remains. Only a valid zero pool is omitted.
            let pool = summary(container).and_then(pool).ok_or_else(unreadable)?;
            (pool.limit > 0.0).then_some(pool)
        }
    };

    Ok(Snapshot {
        personal,
        shared,
        resets_at: date(root.get("nextResetAt")).or_else(|| date(root.get("next_reset_at"))),
    })
}

/// The `quotaSummary` of a container, either spelling.
fn summary(container: &Value) -> Option<&Value> {
    container
        .get("quotaSummary")
        .or_else(|| container.get("quota_summary"))
}

/// A summary Qoder stated in full, or nothing. Negative figures are not a
/// reading anybody could have, so they are refused rather than clamped.
fn pool(summary: &Value) -> Option<Pool> {
    let figure = |keys: [&str; 2]| -> Option<f64> {
        keys.iter()
            .find_map(|key| super::dig_number(summary.get(*key)))
            .filter(|figure| figure.is_finite())
    };

    let used = figure(["usedValue", "used_value"]).filter(|used| *used >= 0.0)?;
    let limit = figure(["limitValue", "limit_value"]).filter(|limit| *limit >= 0.0)?;

    Some(Pool { used, limit })
}

/// ISO 8601 text or a Unix stamp, seconds or milliseconds. **Zero and garbage
/// are no date**: a reset drawn at 1970 is a countdown that ran out before
/// anybody signed up.
fn date(value: Option<&Value>) -> Option<DateTime<Utc>> {
    let value = value?;

    // Text first, as the original reads it: a stamp written as a string is an
    // ISO 8601 date, not a count of seconds. A string that is not one is no
    // date at all rather than a figure read out of it.
    if let Some(text) = value.as_str() {
        return DateTime::parse_from_rfc3339(text.trim())
            .ok()
            .map(|at| at.with_timezone(&Utc));
    }

    let raw = super::dig_number(Some(value)).filter(|raw| raw.is_finite() && *raw > 0.0)?;
    let seconds = if raw > 10_000_000_000.0 {
        raw / 1_000.0
    } else {
        raw
    };
    DateTime::from_timestamp(seconds as i64, 0)
}

fn unreadable() -> String {
    "the reply could not be read".to_string()
}

// ---------------------------------------------------------------------------
// The rings
// ---------------------------------------------------------------------------

/// The account's credits, and the team's pool beside them when there is one.
///
/// The fraction is Qoder's `usedValue` over its `limitValue` rather than its
/// `usagePercentage`, which is the same figure rounded to a whole number — the
/// panel rounds for itself and would otherwise round a rounding. A pool with a
/// limit of zero is **not drawn**: there is no allowance to divide by, and a
/// ring at 100% would say something was spent that was never granted. They are
/// **two rings, never one sum**.
fn windows(snapshot: &Snapshot, now: DateTime<Utc>) -> Vec<UsageWindow> {
    let mut windows: Vec<UsageWindow> = Vec::new();

    let reset = snapshot.resets_at.filter(|at| *at > now);
    if let Some(window) = pool_window(&snapshot.personal, "Credits", reset) {
        windows.push(window);
    }
    // The reset Qoder states is the account's. Whether a team's pool turns over
    // on the same day is not something the reply says, so it is not claimed.
    if let Some(shared) = &snapshot.shared {
        if let Some(window) = pool_window(shared, "Shared Credits", None) {
            windows.push(window);
        }
    }

    windows
}

/// Thirty days is a **sort key, not a reported length**: Qoder states when the
/// credits reset and never how long the period is — a trial runs a fortnight, a
/// plan a billing month — so nothing divides by it.
fn pool_window(pool: &Pool, label: &str, resets_at: Option<DateTime<Utc>>) -> Option<UsageWindow> {
    if pool.limit <= 0.0 {
        return None;
    }
    Some(
        UsageWindow::new(
            label,
            Some(percent_from_fraction(pool.used / pool.limit)),
        )
        .with_reset(
            resets_at.map(|at| at.to_rfc3339_opts(chrono::SecondsFormat::Secs, true)),
        ),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Sanitized from the shapes other monitors of this route recorded —
    /// camelCase today, snake_case in an earlier build.
    fn credits() -> String {
        r#"{"userId":"redacted","quotaKey":"big_model_credits","nextResetAt":"2024-09-01T00:00:00Z","status":"active","totalQuota":{"quotaSummary":{"usedValue":125,"limitValue":500,"remainingValue":375,"usagePercentage":25,"unit":"credit"},"quotaDetail":[]}}"#
            .to_string()
    }

    fn at(seconds: i64) -> DateTime<Utc> {
        DateTime::from_timestamp(seconds, 0).unwrap()
    }

    /// A day before the fixtures' reset, so their date is still ahead.
    fn before() -> DateTime<Utc> {
        at(1_725_148_800 - 86_400)
    }

    #[test]
    fn analytics_cookies_are_dropped_and_the_hosts_own_are_kept() {
        let kept = normalize("_ga=GA1.1.9; _gcl_au=1.1; qoder_session=abc; Hm_lvt_x=1; cna=dev1; locale=zh")
            .unwrap();
        assert_eq!(kept, "qoder_session=abc; cna=dev1; locale=zh");
    }

    #[test]
    fn a_pasted_header_is_taken_as_pasted() {
        assert_eq!(normalize("Cookie: sid=abc; lang=en").unwrap(), "sid=abc; lang=en");
    }

    /// A value carrying a newline would end the header and start whatever came
    /// after it as a fresh one.
    #[test]
    fn a_control_character_is_refused_not_forwarded() {
        assert_eq!(
            normalize("sid=abc\r\nX-Injected: 1"),
            Err(CookieProblem::Invalid)
        );
    }

    #[test]
    fn nothing_but_analytics_is_no_session() {
        assert_eq!(normalize("_ga=1; _gid=2"), Err(CookieProblem::Invalid));
        assert_eq!(normalize("Cookie: "), Err(CookieProblem::Missing));
    }

    /// The browser hands back a host-only and a domain row for one name; the
    /// first wins rather than the whole session being thrown away.
    #[test]
    fn a_repeated_name_keeps_the_first() {
        assert_eq!(normalize("sid=first; sid=second").unwrap(), "sid=first");
    }

    #[test]
    fn the_accounts_credits_with_qoders_own_reset() {
        let snapshot = parse(&credits()).unwrap();
        assert_eq!(snapshot.personal.used, 125.0);
        assert_eq!(snapshot.personal.limit, 500.0);
        assert!(snapshot.shared.is_none());
        assert_eq!(snapshot.resets_at, Some(at(1_725_148_800)));

        let windows = windows(&snapshot, before());
        assert_eq!(windows.len(), 1);
        assert_eq!(windows[0].label, "Credits");
        assert_eq!(windows[0].percent_used, Some(25.0));
        assert_eq!(windows[0].resets_at.as_deref(), Some("2024-09-01T00:00:00Z"));
    }

    /// The earlier build's field names, with a reset stated in milliseconds.
    #[test]
    fn the_snake_case_reply_reads_the_same() {
        let snake = r#"{"user_id":"redacted","quota_key":"big_model_credits","next_reset_at":1725148800000,"status":"active","total_quota":{"quota_summary":{"used_value":125,"limit_value":500,"remaining_value":375,"usage_percentage":25,"unit":"credit"},"quota_detail":[]},"plan_quota":{"quota_summary":{"used_value":125,"limit_value":300,"remaining_value":175,"usage_percentage":42,"unit":"credit"}},"resource_package_quota":{"quota_summary":{"used_value":0,"limit_value":200,"remaining_value":200,"usage_percentage":0,"unit":"credit"}}}"#;
        let snapshot = parse(snake).unwrap();
        assert_eq!(snapshot.personal.used, 125.0);
        assert_eq!(snapshot.personal.limit, 500.0);
        assert_eq!(snapshot.resets_at, Some(at(1_725_148_800)));
    }

    /// A spent personal allowance beside a team pool with room in it. Summed,
    /// they read as 68% — "plenty left" about the pool that is actually
    /// stopping the reader.
    #[test]
    fn a_team_pool_is_a_second_ring_never_a_sum() {
        let team = r#"{"userId":"redacted","quotaKey":"big_model_credits","status":"active","totalQuota":{"quotaSummary":{"usedValue":1500,"limitValue":1500,"remainingValue":0,"usagePercentage":100,"unit":"credit"}},"sharedQuota":{"quotaSummary":{"usedValue":200,"limitValue":1000,"remainingValue":800,"usagePercentage":20,"unit":"credit"}}}"#;
        let windows = windows(&parse(team).unwrap(), before());

        let labels: Vec<&str> = windows.iter().map(|w| w.label.as_str()).collect();
        assert_eq!(labels, vec!["Credits", "Shared Credits"]);
        assert_eq!(windows[0].percent_used, Some(100.0));
        assert_eq!(windows[1].percent_used, Some(20.0));
        // Qoder states the account's reset, not the team's.
        assert_eq!(windows[1].resets_at, None);
    }

    /// A mainland trial reply: 586 of 600 credits left and a `nextResetAt` a
    /// month in the past. The credits are drawn; the stale date is not.
    #[test]
    fn a_reset_date_already_past_is_dropped_not_the_credits() {
        let trial = r#"{"user_id":"redacted","quota_key":"big_model_credits","status":"active","plan_quota":{"quota_summary":{"used_value":0,"limit_value":0,"remaining_value":0,"usage_percentage":0,"unit":"credits"}},"total_quota":{"quota_summary":{"used_value":14,"limit_value":600,"remaining_value":586,"usage_percentage":3,"unit":"credits"}},"nextResetAt":1787304668180}"#;
        let snapshot = parse(trial).unwrap();
        assert_eq!(snapshot.personal.used, 14.0);
        assert_eq!(snapshot.personal.limit, 600.0);
        assert_eq!(snapshot.resets_at, Some(at(1_787_304_668)));

        let windows = windows(&snapshot, at(1_790_179_680));
        assert_eq!(windows.len(), 1);
        assert_eq!(windows[0].resets_at, None);
        assert_eq!(
            windows[0].percent_used,
            Some(percent_from_fraction(14.0 / 600.0))
        );
    }

    /// A limit of zero is an account with nothing granted. Not a ring at 100%,
    /// which would say something was spent.
    #[test]
    fn no_credits_at_all_draws_nothing() {
        let none = r#"{"userId":"redacted","quotaKey":"big_model_credits","totalQuota":{"quotaSummary":{"usedValue":0,"limitValue":0,"remainingValue":0,"unit":"credit"},"quotaDetail":[]},"sharedQuota":{"quotaSummary":{"usedValue":0,"limitValue":0,"remainingValue":0,"unit":"credit"}}}"#;
        let snapshot = parse(none).unwrap();
        assert!(snapshot.shared.is_none());
        assert!(windows(&snapshot, before()).is_empty());
    }

    #[test]
    fn a_reply_without_the_accounts_summary_is_unreadable_not_empty() {
        for body in [
            r#"{"quotaKey":"big_model_credits"}"#,
            "<html>sign in</html>",
            r#"{"totalQuota":{"quotaSummary":{"usedValue":-1,"limitValue":5}}}"#,
            r#"{"totalQuota":{"quotaSummary":{"usedValue":1}}}"#,
            r#"{"totalQuota":[]}"#,
            // An unreadable team pool is not proof that no allowance remains.
            r#"{"totalQuota":{"quotaSummary":{"usedValue":1,"limitValue":5}},"sharedQuota":[]}"#,
        ] {
            assert!(parse(body).is_err(), "read {body}");
        }
    }
}
