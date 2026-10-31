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
//! without one.
//!
//! **The packs' end dates are carried in the detail line.** The original keeps
//! them as an expiry beside the balance; this port's window has no expiry
//! field, so the soonest one becomes `expires YYYY-MM-DD` in `detail`. When the
//! optional parts are drawn, each part carries its own expiry and the total
//! drops its line: the total's soonest lapse is always one of the parts', so
//! repeating it read as the same row twice (the original's issue #76).
//!
//! **Usage history is a second, read-only route.** `credit_days` reads the
//! account's own 31-day history, in the reader's local calendar, with the same
//! pasted session. A day is `used` before discounts and free allowances
//! (`original_credits`) and `charged` after them (`credits`); quiet days are
//! zero rather than absent.
//!
//! Reply shape, from the account page's own request. Both spellings are
//! accepted per field: the mainland site's reply mixes them (`nextResetAt`
//! beside `total_quota`), and other readers recorded all-camelCase. The shapes
//! are second-hand — taken from the original and its fixtures, not from a
//! capture made here.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use chrono::{DateTime, Local, NaiveDate, TimeZone, Utc};
use serde::Serialize;
use serde_json::Value;

use super::{describe_reqwest_error, percent_from_fraction, Ctx, FetchFuture, Provider};
use crate::credentials;
use crate::model::{ProviderUsage, UsageWindow, WindowKind};

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

/// The history route's own page size and page limit, as the original reads it.
const HISTORY_PAGE_SIZE: usize = 500;
const HISTORY_PAGE_LIMIT: usize = 6;

/// The detail card's span: today plus the thirty days before it.
const HISTORY_DAYS: i64 = 31;

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

    fn credit_history_url(&self) -> String {
        format!(
            "{}/api/v1/me/usages/big_model_credits/histories",
            self.origin()
        )
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

/// Whether the optional part rows are drawn. Off by default: the parts are the
/// total counted again, and only useful when two of them have an allowance.
fn shows_parts() -> bool {
    let json = super::settings_path(ID).and_then(|path| credentials::read_json(&path));
    shows_parts_from(
        credentials::env_override(ID, "shows_parts").as_deref(),
        json.as_ref(),
    )
}

fn shows_parts_from(override_value: Option<&str>, settings: Option<&Value>) -> bool {
    if let Some(value) = override_value {
        let value = value.trim().to_ascii_lowercase();
        return value == "1" || value == "true";
    }
    settings
        .and_then(|json| json.get("showsParts"))
        .and_then(Value::as_bool)
        .unwrap_or(false)
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

/// The headers the account page's own requests carry. `Bx-V` belongs to the
/// bot screening in front of the site: a request that does not look like the
/// page it stands in for is the one that screening turns away.
fn request(ctx: &Ctx, url: String, header: &str, site: Site) -> reqwest::RequestBuilder {
    ctx.gateway_client
        .get(url)
        .header("Cookie", header)
        .header("Accept", "application/json, text/plain, */*")
        .header("Origin", site.origin())
        .header("Referer", site.account_page())
        .header("X-Requested-With", "XMLHttpRequest")
        .header("Bx-V", "2.5.35")
        .header(
            "User-Agent",
            "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 \
             (KHTML, like Gecko) Chrome/143.0.0.0 Safari/537.36",
        )
}

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
    let response = request(&ctx, site.usage_url(), &header, site).send().await;

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

    let windows = windows(&snapshot, Utc::now(), shows_parts());
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

/// One allowance: what has been used, what the limit is, and Qoder's own
/// remainder when it states one. The remainder is not recomputed: absent says
/// nothing, and zero read from silence would mark a pool spent.
struct Pool {
    used: f64,
    limit: f64,
    remaining: Option<f64>,
}

/// A dated piece of a pool's detail list: credits still left and the day they
/// stop existing.
struct Pack {
    remaining: f64,
    expires_at: DateTime<Utc>,
}

/// One of the three things `totalQuota` adds up — the plan's own credits, a
/// resource pack bought on top, and the credits tied to one model family —
/// with the dated entries of its own detail list.
struct Part {
    pool: Pool,
    packs: Vec<Pack>,
}

/// What one read of the account page's route returns.
struct Snapshot {
    /// The account's own credits: plan plus resource packs.
    personal: Pool,
    /// A team's shared pool. None off a team plan, and none when Qoder reports
    /// an empty placeholder — a pool of zero is not one anybody can spend.
    shared: Option<Pool>,
    resets_at: Option<DateTime<Utc>>,
    /// The total's dated entries, used for the total's own expiry line.
    packs: Vec<Pack>,
    /// Parts are read on request and never cost the total: missing or
    /// unreadable means `None`, not an unreadable reply.
    plan: Option<Part>,
    resource_pack: Option<Part>,
    dedicated: Option<Part>,
}

fn parse(body: &str) -> Result<Snapshot, String> {
    let root: Value = serde_json::from_str(body).map_err(|_| unreadable())?;
    if !root.is_object() {
        return Err(unreadable());
    }

    let total = root
        .get("totalQuota")
        .or_else(|| root.get("total_quota"));
    let personal = total
        .and_then(summary)
        .and_then(pool)
        .ok_or_else(unreadable)?;

    let shared = match root.get("sharedQuota").or_else(|| root.get("shared_quota")) {
        None => None,
        Some(Value::Null) => None,
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
        packs: packs(total),
        plan: part(root.get("planQuota").or_else(|| root.get("plan_quota"))),
        resource_pack: part(
            root.get("resourcePackageQuota")
                .or_else(|| root.get("resource_package_quota")),
        ),
        dedicated: part(
            root.get("dedicatedResourcePackageQuota")
                .or_else(|| root.get("dedicated_resource_package_quota")),
        ),
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
    let remaining = figure(["remainingValue", "remaining_value"]).filter(|left| *left >= 0.0);

    Some(Pool {
        used,
        limit,
        remaining,
    })
}

/// The entries of a detail list with credits left and an end date stated. A
/// list this cannot read at all is an empty one; a single entry it cannot read
/// is left out rather than costing the summary beside it.
fn packs(container: Option<&Value>) -> Vec<Pack> {
    let Some(container) = container else {
        return Vec::new();
    };
    let Some(details) = container
        .get("quotaDetail")
        .or_else(|| container.get("quota_detail"))
        .and_then(Value::as_array)
    else {
        return Vec::new();
    };
    details.iter().filter_map(pack).collect()
}

fn pack(detail: &Value) -> Option<Pack> {
    let detail = detail.as_object()?;

    let active = detail
        .get("isActive")
        .or_else(|| detail.get("is_active"));
    if active.and_then(Value::as_bool) == Some(false) {
        return None;
    }

    let remaining = super::dig_number(
        detail
            .get("remainingValue")
            .or_else(|| detail.get("remaining_value")),
    )
    .filter(|remaining| remaining.is_finite() && *remaining > 0.0)?;

    let expires_at = date(detail.get("expiresAt").or_else(|| detail.get("expires_at")))?;
    Some(Pack {
        remaining,
        expires_at,
    })
}

/// One optional part, or nothing when the reply leaves it out or states it in
/// a shape this cannot read.
fn part(container: Option<&Value>) -> Option<Part> {
    let container = container?;
    let pool = summary(container).and_then(pool)?;
    Some(Part {
        pool,
        packs: packs(Some(container)),
    })
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

/// The account's credits, the requested parts, and the team's pool beside them.
///
/// The fraction is Qoder's `usedValue` over its `limitValue` rather than its
/// `usagePercentage`, which is the same figure rounded to a whole number — the
/// panel rounds for itself and would otherwise round a rounding. A pool with a
/// limit of zero is **not drawn**: there is no allowance to divide by, and a
/// ring at 100% would say something was spent that was never granted.
///
/// The parts are the same credits counted again, so they are drawn only where
/// they say something the total does not: two or more with an allowance. One
/// alone is the total under another name. Each part takes the end dates of its
/// own detail list; only the plan is a reset (`nextResetAt`), since the plan is
/// the part that refreshes and a pack only lapses.
fn windows(snapshot: &Snapshot, now: DateTime<Utc>, shows_parts: bool) -> Vec<UsageWindow> {
    let mut windows: Vec<UsageWindow> = Vec::new();

    let reset = snapshot.resets_at.filter(|at| *at > now);
    if let Some(window) = pool_window(
        &snapshot.personal,
        "qoder.credits",
        "Credits",
        WindowKind::Credits,
        reset,
    ) {
        windows.push(
            window.with_detail(Some(detail(&snapshot.personal, &snapshot.packs, now))),
        );
    }

    if shows_parts && !windows.is_empty() {
        let parts: [(&Option<Part>, &str, &str, WindowKind, Option<DateTime<Utc>>); 3] = [
            (
                &snapshot.plan,
                "qoder.plan",
                "Plan Credits",
                WindowKind::Credits,
                reset,
            ),
            (
                &snapshot.resource_pack,
                "qoder.pack",
                "Top-up Pack",
                WindowKind::TopUp,
                None,
            ),
            (
                &snapshot.dedicated,
                "qoder.dedicated",
                "Dedicated Credits",
                WindowKind::Credits,
                None,
            ),
        ];
        let mut drawn: Vec<UsageWindow> = Vec::new();
        for (part, id, label, kind, resets_at) in parts {
            let Some(part) = part else {
                continue;
            };
            if let Some(window) = pool_window(&part.pool, id, label, kind, resets_at) {
                drawn.push(
                    window.with_detail(Some(detail(&part.pool, &part.packs, now))),
                );
            }
        }
        // Each part says its own expiry; the total no longer does. The total's
        // soonest lapse is always one of the parts', so keeping both repeated
        // a part's line word for word and read as the same row listed twice.
        if drawn.len() > 1 {
            if let Some(total) = windows.first_mut() {
                total.detail = Some(amount_text(&snapshot.personal));
            }
            windows.extend(drawn);
        }
    }

    // The reset Qoder states is the account's. Whether a team's pool turns over
    // on the same day is not something the reply says, so it is not claimed.
    if let Some(shared) = &snapshot.shared {
        if let Some(window) = pool_window(
            shared,
            "qoder.shared",
            "Shared Credits",
            WindowKind::SharedCredits,
            None,
        ) {
            windows.push(window.with_detail(Some(amount_text(shared))));
        }
    }

    windows
}

fn pool_window(
    pool: &Pool,
    id: &str,
    label: &str,
    kind: WindowKind,
    resets_at: Option<DateTime<Utc>>,
) -> Option<UsageWindow> {
    if pool.limit <= 0.0 {
        return None;
    }
    Some(
        UsageWindow::new(label, Some(percent_from_fraction(pool.used / pool.limit)))
            .with_id(Some(id.to_string()))
            .with_kind(kind)
            .with_reset(
                resets_at.map(|at| at.to_rfc3339_opts(chrono::SecondsFormat::Secs, true)),
            )
            // `remaining` is Qoder's own figure; when it is absent, a used
            // figure at the limit is the only evidence of exhaustion there is.
            .with_exhausted(pool.remaining.map(|left| left <= 0.0).unwrap_or(pool.used >= pool.limit)),
    )
}

/// What is left of the allowance, or what is gone when Qoder stated no
/// remainder, out of all of it.
fn amount_text(pool: &Pool) -> String {
    let figure = pool.remaining.unwrap_or(pool.used);
    format!(
        "{} / {} credits",
        credit_amount(figure),
        credit_amount(pool.limit)
    )
}

/// Whole credits in full up to a million, a tenth where there is one — the
/// original's `UsageDetailCard.creditAmount`.
fn credit_amount(amount: f64) -> String {
    let amount = (amount.max(0.0) * 10.0).round() / 10.0;
    if amount >= 1_000_000.0 {
        format!("{:.1}M", amount / 1_000_000.0)
    } else if (amount - amount.round()).abs() < f64::EPSILON {
        format!("{amount:.0}")
    } else {
        format!("{amount:.1}")
    }
}

fn detail(pool: &Pool, packs: &[Pack], now: DateTime<Utc>) -> String {
    let mut text = amount_text(pool);
    if let Some(expiry) = expiry_text(packs, now) {
        text.push_str(" · ");
        text.push_str(&expiry);
    }
    text
}

/// The soonest packs to lapse, as a date. Only those still ahead and with
/// credits on them: an expiry already past is not a coming one.
fn expiry_text(packs: &[Pack], now: DateTime<Utc>) -> Option<String> {
    let ahead: Vec<&Pack> = packs
        .iter()
        .filter(|pack| pack.expires_at > now && pack.remaining > 0.0)
        .collect();
    let first = ahead.iter().min_by_key(|pack| pack.expires_at)?;
    let first_day = first.expires_at.with_timezone(&Local).date_naive();

    let days: BTreeSet<NaiveDate> = ahead
        .iter()
        .map(|pack| pack.expires_at.with_timezone(&Local).date_naive())
        .collect();
    if days.len() > 1 {
        Some(format!(
            "expires from {} in {} lots",
            first_day.format("%Y-%m-%d"),
            days.len()
        ))
    } else {
        Some(format!("expires {}", first_day.format("%Y-%m-%d")))
    }
}

// ---------------------------------------------------------------------------
// Usage history
// ---------------------------------------------------------------------------

/// One local calendar day of the account's credit history.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CreditDay {
    /// `YYYY-MM-DD`, local midnight.
    pub date: String,
    /// Σ `original_credits` of the records that began that day.
    pub used: f64,
    /// Σ `credits`.
    pub charged: f64,
}

#[derive(Debug, Clone)]
struct HistoryRecord {
    begin_at: DateTime<Utc>,
    credits: f64,
    original_credits: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PageStep {
    Last,
    Next(i64),
    Unreadable,
}

#[derive(Debug, Clone, Copy)]
struct PageResult {
    next_page: Option<i64>,
    last_page: Option<i64>,
}

impl PageResult {
    /// The original's `following(after:)`: a missing or non-positive next page
    /// closes the span; a next page that does not move forward, or jumps past
    /// the stated last page, is a reply that cannot be trusted.
    fn following(&self, page: i64) -> PageStep {
        let Some(next) = self.next_page else {
            return PageStep::Last;
        };
        if next <= 0 {
            return PageStep::Last;
        }
        if let Some(last) = self.last_page {
            if page >= last {
                return PageStep::Last;
            }
        }
        if next <= page {
            return PageStep::Unreadable;
        }
        if let Some(last) = self.last_page {
            if next > last {
                return PageStep::Unreadable;
            }
        }
        PageStep::Next(next)
    }
}

struct HistoryPage {
    records: Vec<HistoryRecord>,
    page_result: PageResult,
}

fn parse_history(body: &str) -> Result<HistoryPage, String> {
    let root: Value = serde_json::from_str(body).map_err(|_| history_unreadable())?;
    if !root.is_object() {
        return Err(history_unreadable());
    }

    // The paging is what says the window closed. The route sends `page_result`
    // even with nothing in the window, so a reply without it, or without
    // `data`, is not an empty week.
    let data = root
        .get("data")
        .and_then(Value::as_array)
        .ok_or_else(history_unreadable)?;
    let page_result = root
        .get("pageResult")
        .or_else(|| root.get("page_result"))
        .ok_or_else(history_unreadable)?;
    let page_result = parse_page_result(page_result)?;

    let mut records = Vec::with_capacity(data.len());
    for item in data {
        records.push(parse_history_record(item)?);
    }

    Ok(HistoryPage {
        records,
        page_result,
    })
}

fn parse_page_result(value: &Value) -> Result<PageResult, String> {
    let object = value.as_object().ok_or_else(history_unreadable)?;
    Ok(PageResult {
        next_page: optional_i64(object.get("nextPage").or_else(|| object.get("next_page")))?,
        last_page: optional_i64(object.get("lastPage").or_else(|| object.get("last_page")))?,
    })
}

fn optional_i64(value: Option<&Value>) -> Result<Option<i64>, String> {
    match value {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Number(number)) => number
            .as_i64()
            .map(Some)
            .ok_or_else(history_unreadable),
        Some(_) => Err(history_unreadable()),
    }
}

fn parse_history_record(value: &Value) -> Result<HistoryRecord, String> {
    let object = value.as_object().ok_or_else(history_unreadable)?;

    let begin_at = date(object.get("beginAt"))
        .or_else(|| date(object.get("begin_at")))
        .ok_or_else(history_unreadable)?;
    let credits = super::dig_number(object.get("credits"))
        .filter(|credits| credits.is_finite() && *credits >= 0.0)
        .ok_or_else(history_unreadable)?;
    let original_credits = super::dig_number(
        object
            .get("originalCredits")
            .or_else(|| object.get("original_credits")),
    )
    .filter(|credits| credits.is_finite() && *credits >= 0.0)
    .ok_or_else(history_unreadable)?;

    Ok(HistoryRecord {
        begin_at,
        credits,
        original_credits,
    })
}

fn history_unreadable() -> String {
    "the Qoder usage history could not be read".to_string()
}

/// Bucket the records by local calendar day and fill every day from `from`
/// through `today`, oldest first. A record before `from` — one the route
/// returned for overlapping the window — is not counted.
fn credit_days_from_records(
    records: &[HistoryRecord],
    from: NaiveDate,
    today: NaiveDate,
    local_day: impl Fn(&DateTime<Utc>) -> NaiveDate,
) -> Vec<CreditDay> {
    let mut totals: BTreeMap<NaiveDate, (f64, f64)> = BTreeMap::new();
    for record in records {
        let day = local_day(&record.begin_at);
        if day < from {
            continue;
        }
        let entry = totals.entry(day).or_insert((0.0, 0.0));
        entry.0 += record.original_credits;
        entry.1 += record.credits;
    }

    let mut days = Vec::new();
    let mut cursor = from;
    while cursor <= today {
        let (used, charged) = totals.get(&cursor).copied().unwrap_or((0.0, 0.0));
        days.push(CreditDay {
            date: cursor.format("%Y-%m-%d").to_string(),
            used,
            charged,
        });
        let Some(next) = cursor.succ_opt() else {
            break;
        };
        cursor = next;
    }
    days
}

fn start_of_day(date: NaiveDate) -> DateTime<Local> {
    let naive = date
        .and_hms_opt(0, 0, 0)
        .expect("every calendar date has a midnight");
    Local
        .from_local_datetime(&naive)
        .earliest()
        .unwrap_or_else(|| Local.from_utc_datetime(&naive))
}

/// The last 31 local days of the account's credit history.
///
/// This is the exact function `lib.rs` registers; the caller supplies the
/// shared `Ctx` with the app's HTTP client and proxy settings.
pub async fn credit_days(ctx: &Ctx) -> Result<Vec<CreditDay>, String> {
    credit_days_at(ctx, Local::now()).await
}

async fn credit_days_at(ctx: &Ctx, now: DateTime<Local>) -> Result<Vec<CreditDay>, String> {
    let Some(pasted) = super::pasted::cookie(ID) else {
        return Err(super::pasted::needed(ID, "Qoder session cookie"));
    };
    let header = normalize(&pasted).map_err(cookie_problem)?;
    let site = site();

    let today = now.date_naive();
    let from_date = today - chrono::Duration::days(HISTORY_DAYS - 1);
    let from = start_of_day(from_date);

    let mut records: Vec<HistoryRecord> = Vec::new();
    let mut page: i64 = 1;
    for _ in 0..HISTORY_PAGE_LIMIT {
        let reply = history_page(ctx, &header, site, page, from, now).await?;
        records.extend(reply.records);

        match reply.page_result.following(page) {
            PageStep::Last => {
                return Ok(credit_days_from_records(&records, from_date, today, |at| {
                    at.with_timezone(&Local).date_naive()
                }))
            }
            PageStep::Next(next) => page = next,
            PageStep::Unreadable => return Err(history_unreadable()),
        }
    }

    Err("the Qoder usage history did not finish paging".to_string())
}

async fn history_page(
    ctx: &Ctx,
    header: &str,
    site: Site,
    page: i64,
    from: DateTime<Local>,
    now: DateTime<Local>,
) -> Result<HistoryPage, String> {
    let response = request(ctx, site.credit_history_url(), header, site)
        .query(&[
            ("page", page.to_string()),
            ("page_size", HISTORY_PAGE_SIZE.to_string()),
            ("start_time", from.timestamp_millis().to_string()),
            ("end_time", now.timestamp_millis().to_string()),
            ("order_by", "begin_at".to_string()),
            ("order", "-1".to_string()),
        ])
        .send()
        .await
        .map_err(|e| format!("request failed: {}", describe_reqwest_error(&e)))?;

    let status = response.status();
    let body = response
        .text()
        .await
        .map_err(|e| format!("cannot read body: {e}"))?;

    if status.is_redirection() || status.as_u16() == 401 || status.as_u16() == 403 {
        return Err(session_expired());
    }
    if status.as_u16() == 429 {
        return Err(super::http_failure(
            status,
            &[(429, " — rate limited, try again shortly")],
        ));
    }
    if !status.is_success() {
        return Err(history_unreadable());
    }

    parse_history(&body)
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

    /// A reply with all three optional parts and a detail list on the total and
    /// two of them: two dated packs, one spent, one incurably undated, and one
    /// switched off.
    fn parts_reply() -> String {
        r#"{
            "userId":"redacted",
            "quotaKey":"big_model_credits",
            "nextResetAt":"2024-09-01T00:00:00Z",
            "status":"active",
            "totalQuota":{
                "quotaSummary":{"usedValue":125,"limitValue":500,"remainingValue":375,"usagePercentage":25,"unit":"credit"},
                "quotaDetail":[
                    {"remainingValue":50,"expiresAt":"2024-09-15T00:00:00Z","isActive":true},
                    {"remainingValue":20,"expiresAt":"2024-09-10T00:00:00Z"},
                    {"remainingValue":0,"expiresAt":"2024-09-12T00:00:00Z"},
                    {"remainingValue":10,"expiresAt":0},
                    {"remainingValue":5,"expiresAt":"2024-09-20T00:00:00Z","isActive":false}
                ]
            },
            "planQuota":{
                "quotaSummary":{"usedValue":100,"limitValue":300,"remainingValue":200},
                "quotaDetail":[]
            },
            "resourcePackageQuota":{
                "quotaSummary":{"usedValue":25,"limitValue":200,"remainingValue":175},
                "quotaDetail":[{"remainingValue":50,"expiresAt":"2024-09-15T00:00:00Z"}]
            },
            "dedicatedResourcePackageQuota":{
                "quotaSummary":{"usedValue":0,"limitValue":100,"remainingValue":100},
                "quotaDetail":[{"remainingValue":100,"expiresAt":"2024-10-01T00:00:00Z"}]
            }
        }"#
        .to_string()
    }

    fn at(seconds: i64) -> DateTime<Utc> {
        DateTime::from_timestamp(seconds, 0).unwrap()
    }

    /// A day before the fixtures' reset, so their date is still ahead.
    fn before() -> DateTime<Utc> {
        at(1_725_148_800 - 86_400)
    }

    fn history_record(begin_at: &str, credits: f64, original_credits: f64) -> HistoryRecord {
        HistoryRecord {
            begin_at: DateTime::parse_from_rfc3339(begin_at)
                .unwrap()
                .with_timezone(&Utc),
            credits,
            original_credits,
        }
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
        assert_eq!(snapshot.personal.remaining, Some(375.0));
        assert!(snapshot.shared.is_none());
        assert_eq!(snapshot.resets_at, Some(at(1_725_148_800)));

        let windows = windows(&snapshot, before(), false);
        assert_eq!(windows.len(), 1);
        assert_eq!(windows[0].id.as_deref(), Some("qoder.credits"));
        assert_eq!(windows[0].label, "Credits");
        assert_eq!(windows[0].percent_used, Some(25.0));
        assert_eq!(windows[0].resets_at.as_deref(), Some("2024-09-01T00:00:00Z"));
        assert_eq!(windows[0].detail.as_deref(), Some("375 / 500 credits"));
    }

    /// A null `sharedQuota` is an absent team pool, not a malformed one.
    #[test]
    fn a_null_shared_pool_is_absent() {
        let reply = r#"{"totalQuota":{"quotaSummary":{"usedValue":125,"limitValue":500}},"sharedQuota":null}"#;
        let snapshot = parse(reply).unwrap();
        assert!(snapshot.shared.is_none());
    }

    /// The earlier build's field names, with a reset stated in milliseconds.
    #[test]
    fn the_snake_case_reply_reads_the_same() {
        let snake = r#"{"user_id":"redacted","quota_key":"big_model_credits","next_reset_at":1725148800000,"status":"active","total_quota":{"quota_summary":{"used_value":125,"limit_value":500,"remaining_value":375,"usage_percentage":25,"unit":"credit"},"quota_detail":[]},"plan_quota":{"quota_summary":{"used_value":125,"limit_value":300,"remaining_value":175,"usage_percentage":42,"unit":"credit"}},"resource_package_quota":{"quota_summary":{"used_value":0,"limit_value":200,"remaining_value":200,"usage_percentage":0,"unit":"credit"}}}"#;
        let snapshot = parse(snake).unwrap();
        assert_eq!(snapshot.personal.used, 125.0);
        assert_eq!(snapshot.personal.limit, 500.0);
        assert_eq!(snapshot.personal.remaining, Some(375.0));
        assert_eq!(snapshot.resets_at, Some(at(1_725_148_800)));
        assert!(snapshot.plan.is_some());
        assert!(snapshot.resource_pack.is_some());
        assert!(snapshot.dedicated.is_none());
    }

    #[test]
    fn the_reply_reads_the_optional_parts_and_their_packs() {
        let snapshot = parse(&parts_reply()).unwrap();

        // The zero, the undated and the switched-off entry are not packs.
        assert_eq!(snapshot.packs.len(), 2);
        assert_eq!(snapshot.packs[0].remaining, 50.0);
        assert_eq!(snapshot.packs[1].remaining, 20.0);

        let plan = snapshot.plan.as_ref().expect("plan part");
        assert_eq!(plan.pool.used, 100.0);
        assert_eq!(plan.pool.limit, 300.0);
        assert!(plan.packs.is_empty());

        let pack = snapshot.resource_pack.as_ref().expect("resource pack");
        assert_eq!(pack.packs.len(), 1);

        let dedicated = snapshot.dedicated.as_ref().expect("dedicated part");
        assert_eq!(dedicated.packs.len(), 1);
    }

    #[test]
    fn parts_are_drawn_only_when_asked_and_only_when_more_than_one() {
        let snapshot = parse(&parts_reply()).unwrap();

        let total = windows(&snapshot, before(), false);
        assert_eq!(total.len(), 1);
        assert!(total[0]
            .detail
            .as_deref()
            .unwrap()
            .contains("2024-09-10"));

        let asked = windows(&snapshot, before(), true);
        let ids: Vec<&str> = asked.iter().map(|w| w.id.as_deref().unwrap()).collect();
        assert_eq!(
            ids,
            vec!["qoder.credits", "qoder.plan", "qoder.pack", "qoder.dedicated"]
        );
        let labels: Vec<&str> = asked.iter().map(|w| w.label.as_str()).collect();
        assert_eq!(
            labels,
            vec![
                "Credits",
                "Plan Credits",
                "Top-up Pack",
                "Dedicated Credits"
            ]
        );

        // The total keeps its amounts but loses the expiry the parts repeat.
        assert_eq!(asked[0].detail.as_deref(), Some("375 / 500 credits"));
        assert_eq!(asked[1].detail.as_deref(), Some("200 / 300 credits"));
        assert!(asked[2]
            .detail
            .as_deref()
            .unwrap()
            .contains(" · expires "));
        assert!(asked[3]
            .detail
            .as_deref()
            .unwrap()
            .contains(" · expires "));
    }

    /// One part alone is the total under another name; it is not drawn, and
    /// the total keeps its own expiry line.
    #[test]
    fn one_part_alone_does_not_replace_the_total() {
        let body = r#"{
            "totalQuota":{
                "quotaSummary":{"usedValue":10,"limitValue":100,"remainingValue":90},
                "quotaDetail":[{"remainingValue":10,"expiresAt":"2024-09-15T00:00:00Z"}]
            },
            "planQuota":{
                "quotaSummary":{"usedValue":5,"limitValue":50,"remainingValue":45}
            }
        }"#;
        let snapshot = parse(body).unwrap();
        let windows = windows(&snapshot, before(), true);
        assert_eq!(windows.len(), 1);
        assert!(windows[0]
            .detail
            .as_deref()
            .unwrap()
            .contains("expires 2024-09-15"));
    }

    #[test]
    fn the_parts_toggle_defaults_off_and_reads_both_switches() {
        assert!(!shows_parts_from(None, None));
        assert!(!shows_parts_from(
            Some("0"),
            Some(&serde_json::json!({"showsParts": true}))
        ));
        assert!(shows_parts_from(Some("true"), None));
        assert!(shows_parts_from(Some("1"), None));
        assert!(shows_parts_from(
            None,
            Some(&serde_json::json!({"showsParts": true}))
        ));
        assert!(!shows_parts_from(
            None,
            Some(&serde_json::json!({"showsParts": "true"}))
        ));
    }

    /// A spent personal allowance beside a team pool with room in it. Summed,
    /// they read as 68% — "plenty left" about the pool that is actually
    /// stopping the reader.
    #[test]
    fn a_team_pool_is_a_second_ring_never_a_sum() {
        let team = r#"{"userId":"redacted","quotaKey":"big_model_credits","status":"active","totalQuota":{"quotaSummary":{"usedValue":1500,"limitValue":1500,"remainingValue":0,"usagePercentage":100,"unit":"credit"}},"sharedQuota":{"quotaSummary":{"usedValue":200,"limitValue":1000,"remainingValue":800,"usagePercentage":20,"unit":"credit"}}}"#;
        let windows = windows(&parse(team).unwrap(), before(), false);

        let labels: Vec<&str> = windows.iter().map(|w| w.label.as_str()).collect();
        assert_eq!(labels, vec!["Credits", "Shared Credits"]);
        assert_eq!(windows[0].percent_used, Some(100.0));
        assert!(windows[0].is_exhausted);
        assert_eq!(windows[1].percent_used, Some(20.0));
        assert_eq!(windows[1].id.as_deref(), Some("qoder.shared"));
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

        let windows = windows(&snapshot, at(1_790_179_680), false);
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
        assert!(windows(&snapshot, before(), false).is_empty());
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

    #[test]
    fn history_pages_are_read_and_paged_like_the_original() {
        let page = parse_history(
            r#"{"data":[{"begin_at":"2026-09-01T00:00:00Z","credits":3.5,"original_credits":10}],"pageResult":{"next_page":2,"last_page":4}}"#,
        )
        .unwrap();
        assert_eq!(page.records.len(), 1);
        assert_eq!(page.records[0].credits, 3.5);
        assert_eq!(page.records[0].original_credits, 10.0);
        assert_eq!(page.page_result.following(1), PageStep::Next(2));
        assert_eq!(page.page_result.following(4), PageStep::Last);

        let result = parse_page_result(&serde_json::json!({"nextPage": 2, "lastPage": 3})).unwrap();
        assert_eq!(result.following(1), PageStep::Next(2));
        assert_eq!(result.following(2), PageStep::Unreadable);
        assert_eq!(result.following(3), PageStep::Last);

        let no_next = parse_page_result(&serde_json::json!({"nextPage": 0})).unwrap();
        assert_eq!(no_next.following(1), PageStep::Last);

        let past_last = parse_page_result(&serde_json::json!({"nextPage": 5, "lastPage": 4})).unwrap();
        assert_eq!(past_last.following(1), PageStep::Unreadable);
    }

    #[test]
    fn a_history_reply_missing_its_window_or_a_record_is_unreadable() {
        for body in [
            r#"{"pageResult":{"nextPage":0}}"#,
            r#"{"data":{}}"#,
            r#"{"data":[],"pageResult":null}"#,
            r#"{"data":[],"pageResult":[]}"#,
            r#"{"data":[{"credits":1,"original_credits":1}],"pageResult":{"nextPage":0}}"#,
            r#"{"data":[{"begin_at":"2026-09-01T00:00:00Z","original_credits":1}],"pageResult":{"nextPage":0}}"#,
            r#"{"data":[{"begin_at":"2026-09-01T00:00:00Z","credits":-1,"original_credits":1}],"pageResult":{"nextPage":0}}"#,
            r#"{"data":[{"begin_at":"2026-09-01T00:00:00Z","credits":1}],"pageResult":{"nextPage":0}}"#,
            r#"{"data":[{"begin_at":"2026-09-01T00:00:00Z","credits":1,"original_credits":1}],"pageResult":{"nextPage":"2"}}"#,
        ] {
            assert!(parse_history(body).is_err(), "read {body}");
        }
    }

    #[test]
    fn credit_history_fills_every_day_oldest_first_and_skips_before_from() {
        let records = vec![
            history_record("2026-08-31T23:00:00Z", 99.0, 99.0),
            history_record("2026-09-01T01:00:00Z", 2.0, 10.0),
            history_record("2026-09-01T23:00:00Z", 3.0, 5.0),
            history_record("2026-09-30T12:00:00Z", 4.0, 7.0),
        ];
        let from = NaiveDate::from_ymd_opt(2026, 9, 1).unwrap();
        let today = NaiveDate::from_ymd_opt(2026, 10, 1).unwrap();
        let days = credit_days_from_records(&records, from, today, |at| at.date_naive());

        assert_eq!(days.len(), 31);
        assert_eq!(days[0].date, "2026-09-01");
        assert_eq!(days[0].used, 15.0);
        assert_eq!(days[0].charged, 5.0);
        assert_eq!(days[1].date, "2026-09-02");
        assert_eq!(days[1].used, 0.0);
        assert_eq!(days[1].charged, 0.0);
        assert_eq!(days[29].date, "2026-09-30");
        assert_eq!(days[29].used, 7.0);
        assert_eq!(days[29].charged, 4.0);
        assert_eq!(days[30].date, "2026-10-01");
        assert_eq!(days[30].used, 0.0);
    }

    /// The bucket is the local calendar day, not the UTC one.
    #[test]
    fn credit_history_buckets_by_the_local_calendar_day() {
        let offset = chrono::FixedOffset::east_opt(8 * 3_600).unwrap();
        let records = vec![history_record("2026-09-01T20:00:00Z", 1.0, 2.0)];
        let from = NaiveDate::from_ymd_opt(2026, 9, 1).unwrap();
        let today = NaiveDate::from_ymd_opt(2026, 9, 2).unwrap();
        let days = credit_days_from_records(&records, from, today, |at| {
            at.with_timezone(&offset).date_naive()
        });
        assert_eq!(days[0].used, 0.0);
        assert_eq!(days[1].date, "2026-09-02");
        assert_eq!(days[1].used, 2.0);
        assert_eq!(days[1].charged, 1.0);
    }

    #[test]
    fn credit_days_serialize_to_the_ui_shape() {
        let days = vec![CreditDay {
            date: "2026-09-01".to_string(),
            used: 12.5,
            charged: 3.0,
        }];
        let json = serde_json::to_value(&days).unwrap();
        assert_eq!(json[0]["date"], "2026-09-01");
        assert_eq!(json[0]["used"], 12.5);
        assert_eq!(json[0]["charged"], 3.0);
    }
}
