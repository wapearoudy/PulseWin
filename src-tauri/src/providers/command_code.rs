//! Command Code's plan limits, credit pool and org spend limits.
//!
//! Command Code is a terminal coding agent published by CommandCodeAI and
//! installed from npm as `command-code` (its binaries are `cmd`, `cmdc`,
//! `command-code` and `commandcode`). It bills a **credit balance in US
//! dollars** rather than a token allowance, and layers rolling usage windows
//! and per-organisation spend limits on top of it.
//!
//! The credential comes from one of two places, in this order:
//!
//! 1. **A key the reader supplied** — `PULSEWIN_COMMAND_CODE_KEY` (or the
//!    generic `PULSEWIN_KEY`), or `%APPDATA%\PulseWin\command-code.json`. It
//!    wins, for the same reason it does for OpenCode Go: somebody who typed a
//!    key meant that one, and a stale login left behind by the CLI should not
//!    quietly override a deliberate choice.
//! 2. **What `cmd login` saved**, in `~/.commandcode/auth.json` — a plain JSON
//!    object carrying `apiKey` alongside `userId`, `userName`, `keyName` and
//!    `authenticatedAt`, written owner-only.
//!
//! `is_configured` is true when either is there, and makes no network call.
//!
//! The CLI also honours a `COMMAND_CODE_API_KEY` environment variable. That is
//! deliberately **not** read here, as it is not in the original: a launched app
//! does not inherit the user's shell environment, so looking would find nothing
//! on the machines where it is set and would only add a way to be confusing
//! about it.
//!
//! ## The route
//!
//! Four undocumented account routes on `https://api.commandcode.ai`, each
//! carrying `Authorization: Bearer <apiKey>` — the same four the CLI's own
//! `/usage` overlay reads, in the same order:
//!
//! - `GET /alpha/whoami?limits=1` — the organisation id the other three are
//!   scoped by, and the org's spend limits.
//! - `GET /alpha/billing/credits` — the remaining credit, split into monthly,
//!   purchased and free, plus the rolling window limits.
//! - `GET /alpha/billing/subscriptions` — the plan and the billing period.
//! - `GET /alpha/usage/summary?since=<period start>` — what has been spent
//!   inside that period.
//!
//! None of this is documented by the vendor; it can change without notice,
//! exactly like the undocumented routes the other agents here are read from.
//!
//! ## The one inferred figure, and how it is labelled
//!
//! **What is not done:** the CLI carries a hard-coded table of monthly credit
//! allowances per plan id and prefers it as the denominator when a subscription
//! is active. The original does not use *that* table; it keeps its own, in
//! `PLAN_GRANTS` below, because the CLI's matches on a plan-id *prefix* and
//! would size a hypothetical `individual-pro-v2` as the $30 `individual-pro`.
//! The table is still a table in a client — a compromise made with open eyes,
//! and the only place here where a denominator was not reported. It is bounded:
//! **a plan this table cannot size draws no ring at all** (not zero, not a
//! guess), and the row says so.
//!
//! What *is* reported is the remainder, so the subtraction is the account's own
//! number and only the denominator is inferred. The original marks that row
//! `estimated`, which the card and `--json` read; **this port's windows have no
//! such field**, so the qualifier rides in the heading — `Monthly · estimated` —
//! and never as a bare "Monthly". When prices change, this file is the thing
//! that goes stale, which is why the table is kept apart from the fetching.
//! Last checked against `command-code@1.51.3`, 2026-09-09.
//!
//! ## The balance
//!
//! The original carries what is left as a credit balance beside the windows;
//! this port's result has only windows, so it rides as a window with no
//! fraction and the amount on its detail line. It is still not a *reading*: an
//! account with a balance and no limit at all reports no limits, exactly as the
//! original does.
//!
//! **The organisation's own "exceeded" word is carried on the detail line** for
//! the same reason: it has a flag beside the window that this port's windows
//! have nowhere to put, and a limit the account calls exceeded should not read
//! as an ordinary one.

use std::path::PathBuf;
use std::sync::Arc;

use chrono::{DateTime, Utc};
use serde_json::Value;

use super::{by_window_length, Ctx, FetchFuture, Provider};
use crate::credentials;
use crate::model::{ProviderUsage, UsageWindow};

const HOST: &str = "https://api.commandcode.ai";
const WHOAMI_PATH: &str = "/alpha/whoami";
const CREDITS_PATH: &str = "/alpha/billing/credits";
const SUBSCRIPTIONS_PATH: &str = "/alpha/billing/subscriptions";
const SUMMARY_PATH: &str = "/alpha/usage/summary";

/// What each Command Code subscription grants per month, in US dollars.
///
/// Plan id, exactly as `subscriptions.data.planId` and `credits.planId` report
/// it, to the dollars it grants each month.
const PLAN_GRANTS: [(&str, f64); 8] = [
    ("individual-go", 10.0),
    ("individual-provider", 15.0),
    ("individual-pro", 30.0),
    // Not a typo and not a duplicate: the same displayed name, "Pro", at two
    // very different allowances. The clearest evidence there is that a table
    // like this cannot be reasoned about from the plan's *name*.
    ("individual-pro-v1", 80.0),
    ("teams-pro", 40.0),
    ("individual-goat", 70.0),
    ("individual-max", 150.0),
    ("individual-ultra", 300.0),
];

/// Seconds and milliseconds are told apart by magnitude at 1e11: 1e11 seconds
/// is the year 5138 and 1e11 milliseconds is 1973, so nothing real is anywhere
/// near it.
const MILLISECONDS_ABOVE: f64 = 100_000_000_000.0;

pub struct CommandCode;

impl Provider for CommandCode {
    fn id(&self) -> &'static str {
        "command-code"
    }

    fn name(&self) -> &'static str {
        "Command Code"
    }

    /// A key the reader supplied, or the key `cmd login` wrote. The file only
    /// has to carry one to count; whether the service still takes it is what
    /// the fetch finds out.
    fn is_configured(&self) -> bool {
        super::provider_key(self.id()).is_some() || stored_key().is_some()
    }

    fn fetch(&self, ctx: Arc<Ctx>) -> FetchFuture {
        Box::pin(async move { fetch_inner(ctx).await })
    }
}

/// The key `cmd login` wrote.
///
/// Only the production file. The CLI writes `auth.staging.json` and
/// `auth.local.json` when it is pointed at the vendor's own staging or a
/// developer's laptop, and neither is a credential for the service this reports
/// on.
fn stored_key() -> Option<String> {
    let path: PathBuf = credentials::home_relative(&[".commandcode", "auth.json"])
        .into_iter()
        .next()?;
    let root = credentials::read_json(&path)?;
    credentials::dig_str(&root, "apiKey")
}

async fn fetch_inner(ctx: Arc<Ctx>) -> ProviderUsage {
    const ID: &str = "command-code";
    const NAME: &str = "Command Code";

    let Some(key) = super::provider_key(ID).or_else(stored_key) else {
        return ProviderUsage::failed(
            ID,
            NAME,
            super::missing_key(
                ID,
                ", or sign in with `cmd login` so ~/.commandcode/auth.json exists",
            ),
        );
    };

    // `whoami` first, and alone: it is the cheapest call, it is what says
    // whether the key is any good, and the organisation id it returns is what
    // scopes the other three. Failing here ends the fetch rather than firing
    // three more requests with a credential already known to be bad.
    let whoami = match get(&ctx, WHOAMI_PATH, &[("limits", "1".to_string())], &key).await {
        Ok(reply) => reply,
        Err(problem) => return ProviderUsage::failed(ID, NAME, problem),
    };

    super::claude_code::debug_dump(ID, &whoami);

    let org = credentials::dig_str(&whoami, "org.id").filter(|id| !id.is_empty());
    let org_query: Vec<(&str, String)> = org
        .as_deref()
        .map(|id| vec![("orgId", id.to_string())])
        .unwrap_or_default();

    // Independent of each other, so they run side by side.
    let (credits, subscription) = futures::future::join(
        get(&ctx, CREDITS_PATH, &org_query, &key),
        get(&ctx, SUBSCRIPTIONS_PATH, &org_query, &key),
    )
    .await;

    // The credit pool is the reading. Losing it is losing the answer, so its
    // failure is reported rather than papered over with the windows that happen
    // to have arrived.
    let credits = match credits {
        Ok(reply) => reply,
        Err(problem) => return ProviderUsage::failed(ID, NAME, problem),
    };

    // A subscription this account has not got is not a failure — a
    // pay-as-you-go balance is a complete answer — so this one is allowed to
    // come back empty and the billing period simply goes unstated.
    let subscription = subscription.ok();

    // The period start is passed through exactly as it arrived: it is a query
    // parameter to the service that produced it, not a date this side has any
    // business reformatting.
    let mut summary_query = org_query;
    if let Some(since) = subscription
        .as_ref()
        .and_then(|reply| object(Some(reply), "data"))
        .and_then(|data| stamp(data.get("currentPeriodStart")))
    {
        summary_query.push(("since", since.query));
    }

    let summary = get(&ctx, SUMMARY_PATH, &summary_query, &key).await.ok();

    let reading = Reading {
        whoami: &whoami,
        credits: &credits,
        subscription: subscription.as_ref(),
        summary: summary.as_ref(),
    };

    usage_reading(&reading)
}

fn usage_reading(reading: &Reading<'_>) -> ProviderUsage {
    const ID: &str = "command-code";
    const NAME: &str = "Command Code";
    let mut windows = windows(reading);
    let prepaid = remaining(reading.credits);
    if let Some(remaining) = prepaid {
        windows.push(super::balance_window(
            "Balance",
            format!("{remaining:.2} USD"),
        ));
    }

    if windows.is_empty() { return ProviderUsage::failed(ID, NAME, "no limits reported"); }
    let plan = plan_name(plan_id(reading).as_deref());

    let mut usage = ProviderUsage::ok(ID, NAME, windows).with_plan(plan);
    if let Some(remaining) = prepaid { usage = usage.with_credit_remaining(remaining, "USD"); }
    usage
}

// ---------------------------------------------------------------------------
// Requests
// ---------------------------------------------------------------------------

/// One call's reply, or the line that says why there is none.
///
/// Only 200 is a reading, which is the reference's own switch. A 401 or 403 is
/// a refused key — **not** a "sign in required", which is the message that
/// names Codex and would be the wrong provider on this card.
async fn get(
    ctx: &Ctx,
    path: &str,
    query: &[(&str, String)],
    key: &str,
) -> Result<Value, String> {
    let mut url = reqwest::Url::parse(&format!("{HOST}{path}"))
        .map_err(|e| format!("bad route: {e}"))?;
    if !query.is_empty() {
        url.query_pairs_mut()
            .extend_pairs(query.iter().map(|(name, value)| (*name, value.as_str())));
    }

    let response = ctx
        .client
        .get(url)
        .header("Authorization", format!("Bearer {key}"))
        .header("Content-Type", "application/json")
        .send()
        .await
        .map_err(|e| format!("request failed: {}", super::describe_reqwest_error(&e)))?;

    let status = response.status();
    let body = response
        .text()
        .await
        .map_err(|e| format!("cannot read body: {e}"))?;

    if status != reqwest::StatusCode::OK {
        return Err(match status.as_u16() {
            401 | 403 => "the API key was refused".to_string(),
            429 => "rate limited, try again shortly".to_string(),
            _ => format!("the service returned {}", status.as_u16()),
        });
    }

    serde_json::from_str(&body).map_err(|e| format!("bad JSON: {e}"))
}

// ---------------------------------------------------------------------------
// Reading the replies
// ---------------------------------------------------------------------------

/// The four replies of one pass, so the mapping below can be driven from
/// captured JSON without a network.
struct Reading<'a> {
    whoami: &'a Value,
    credits: &'a Value,
    subscription: Option<&'a Value>,
    summary: Option<&'a Value>,
}

impl Reading<'_> {
    /// The credit pots, where the reply carries them.
    fn pots(&self) -> Option<&Value> {
        object(Some(self.credits), "credits")
    }

    /// The rolling window limits, where the reply carries them.
    fn window_limits(&self) -> Option<&Value> {
        object(Some(self.credits), "windowLimits")
    }

    /// The subscription, which is the one reply that nests: it arrives under
    /// `data`, where the other three put their fields at the top level.
    fn subscription_data(&self) -> Option<&Value> {
        object(self.subscription, "data")
    }
}

/// A nested object, where a null or a value of another type is the same as
/// absent — which is what Swift's optional decoding does and therefore what the
/// mapping above expects.
fn object<'a>(parent: Option<&'a Value>, key: &str) -> Option<&'a Value> {
    parent?.get(key).filter(|value| value.is_object())
}

/// A figure that is a finite number, or one written as a string.
fn number(value: Option<&Value>) -> Option<f64> {
    super::dig_number(value).filter(|figure| figure.is_finite())
}

/// A reset stamp as **epoch milliseconds**, which is what the rolling windows
/// carry: the CLI subtracts this from `Date.now()` to say how long is left.
fn milliseconds(value: Option<&Value>) -> Option<String> {
    let ms = number(value)?;
    Some(stamp_of(DateTime::from_timestamp((ms / 1_000.0) as i64, 0)?))
}

/// A billing-period boundary, which the reply may spell either as a date string
/// or as an epoch number.
///
/// The CLI hands both straight to JavaScript's `Date`, which takes either
/// without saying which it got, so neither form can be ruled out from the
/// client alone. Both are accepted, and the text that arrived is kept — as the
/// service will get it back.
struct Stamp {
    query: String,
    date: Option<DateTime<Utc>>,
}

fn stamp(value: Option<&Value>) -> Option<Stamp> {
    match value? {
        Value::String(text) => Some(Stamp {
            query: text.clone(),
            date: DateTime::parse_from_rfc3339(text.trim())
                .ok()
                .map(|at| at.with_timezone(&Utc)),
        }),
        Value::Number(number) => {
            let value = number.as_f64()?;
            let seconds = if value.abs() > MILLISECONDS_ABOVE {
                value / 1_000.0
            } else {
                value
            };
            // Handed back to the service as it arrived, so a whole number does
            // not acquire a decimal point on the way.
            let whole = value.fract() == 0.0 && value.abs() < 9e18;
            Some(Stamp {
                query: if whole {
                    format!("{}", value as i64)
                } else {
                    format!("{value}")
                },
                date: DateTime::from_timestamp(seconds as i64, 0),
            })
        }
        _ => None,
    }
}

fn stamp_of(at: DateTime<Utc>) -> String {
    at.to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
}

/// Shortest window first, which is the order the other providers' limits arrive
/// in and the order they matter in — the one about to bite leads.
///
/// **Ties keep the order they were built in**, and that is why this sorts on the
/// pair rather than on the length alone: this provider produces equal lengths as
/// a matter of course — a weekly rolling limit beside a weekly org limit, a
/// monthly org limit beside a billing period that happens to be thirty days —
/// and rows that swapped between refreshes would be the shuffling the rest of
/// this port takes care to avoid.
fn windows(reading: &Reading) -> Vec<UsageWindow> {
    let mut rows = rolling_windows(reading);
    rows.extend(org_windows(reading));
    rows.extend(credit_window(reading));

    by_window_length(rows)
}

/// The two rolling limits, when the account says it is subject to them.
///
/// `limited` is the account's own statement that these windows are in force. A
/// cap reported for an account that is not window-limited is not a limit
/// anybody is being held to, so it is not shown as one.
///
/// The lengths are stated by the field names — `fiveHour` and `weekly` — in the
/// same way OpenCode Go's are. Whether either window rolls rather than sitting
/// on a fixed boundary has not been established from a live account.
fn rolling_windows(reading: &Reading) -> Vec<(i64, UsageWindow)> {
    let Some(limits) = reading.window_limits() else {
        return Vec::new();
    };
    if limits.get("limited").and_then(Value::as_bool) != Some(true) {
        return Vec::new();
    }

    [
        ("fiveHour", "5h", 5 * 3_600),
        ("weekly", "7d", 7 * 86_400),
    ]
    .iter()
    .filter_map(|(key, heading, seconds)| {
        let window = limits.get(*key)?;
        let cap = number(window.get("cap")).filter(|cap| *cap > 0.0)?;
        let used = number(window.get("used"))?;

        Some((
            *seconds,
            UsageWindow::new(
                *heading,
                Some(super::percent_from_fraction(
                    (used / cap).clamp(0.0, 1.0),
                )),
            )
            .with_reset(milliseconds(window.get("resetAt"))),
        ))
    })
    .collect()
}

/// The organisation's spend limits, which are money rather than tokens and
/// arrive already counted as **spent** — no inversion here.
///
/// **A ceiling of zero or less is not a denominator, so there is no row.** It
/// used to be drawn as a full, exhausted ring on the reasoning that a limit with
/// no room in it is a limit already reached. That is a guess about an encoding
/// nobody has seen: `-1` is the usual way to say *unlimited*, and reading it as
/// "reached" would paint an untouched organisation solid red and announce a
/// limit as spent that the account never reported.
fn org_windows(reading: &Reading) -> Vec<(i64, UsageWindow)> {
    let limits = reading
        .whoami
        .get("orgLimits")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or_default();

    limits
        .iter()
        .filter_map(|limit| {
            let ceiling = number(limit.get("limit")).filter(|ceiling| *ceiling > 0.0)?;
            let spent = number(limit.get("spent"))?;

            // A model-scoped limit names its model; anything else is the
            // organisation as a whole and is left unscoped, because a row
            // reading "Spend · Org-wide" says nothing the heading does not
            // already say.
            let scope = if limit.get("scope").and_then(Value::as_str) == Some("model") {
                ["modelLabel", "model"]
                    .iter()
                    .find_map(|key| limit.get(*key).and_then(Value::as_str))
                    .map(str::trim)
                    .filter(|name| !name.is_empty())
            } else {
                None
            };

            let (seconds, _stated) = interval(limit.get("resetInterval").and_then(Value::as_str));

            let label = match scope {
                Some(scope) => format!("Spend {scope}"),
                None => "Spend".to_string(),
            };

            let detail = (limit.get("exceeded").and_then(Value::as_bool) == Some(true))
                .then(|| "spent".to_string());

            Some((
                seconds,
                UsageWindow::new(label, Some(super::percent_from_fraction(
                    (spent / ceiling).clamp(0.0, 1.0),
                )))
                .with_reset(
                    limit
                        .get("resetAt")
                        .and_then(Value::as_str)
                        .and_then(|text| {
                            DateTime::parse_from_rfc3339(text.trim())
                                .ok()
                                .map(|at| stamp_of(at.with_timezone(&Utc)))
                        }),
                )
                .with_detail(detail),
            ))
        })
        .collect()
}

/// A reset interval as a length, and whether that length is one the provider
/// actually **stated**.
///
/// A day and a week are exact. A month is 28 to 31 days and is stored as a flat
/// thirty so the row sorts after the others — the same stand-in Cursor's billing
/// cycle and Copilot's calendar month use, and it must not feed the window clock
/// or the forecast. `total` is a lifetime cap with no period at all, so its
/// seconds are nothing but a sort key that puts it last.
fn interval(name: Option<&str>) -> (i64, bool) {
    match name {
        Some("daily") => (86_400, true),
        Some("weekly") => (7 * 86_400, true),
        Some("monthly") => (30 * 86_400, false),
        _ => (365 * 86_400, false),
    }
}

/// The monthly row: the plan's grant while one is running, the purchased balance
/// when none is.
///
/// Two different questions, and answering the wrong one is the failure this
/// splits to avoid. A plan's allowance resets every period and purchased credit
/// does not, so adding the pots together draws a Pro subscriber who holds $200
/// of top-up and has burnt $28 of a $30 month at **12%** — silence, and then a
/// wall.
fn credit_window(reading: &Reading) -> Option<(i64, UsageWindow)> {
    reading.pots()?;

    if is_on_a_plan(reading) {
        plan_window(reading)
    } else {
        pool_window(reading)
    }
}

/// How much of this month's plan grant is gone.
///
/// The grant is not reported by anything — see `PLAN_GRANTS` for where that
/// compromise is argued and where it goes stale. What *is* reported is the
/// remainder, so the subtraction is the account's own number and only the
/// denominator is inferred.
///
/// **A plan this build cannot size draws nothing.** Falling through to the pool
/// below would answer the other question, and answer it wrongly; a zero would be
/// worse still.
fn plan_window(reading: &Reading) -> Option<(i64, UsageWindow)> {
    let grant = plan_grant(plan_id(reading).as_deref()).filter(|grant| *grant > 0.0)?;

    let pots = reading.pots()?;
    // Absent is not zero. Without the remainder there is no numerator, and a
    // plan drawn as wholly spent is the loudest way to be wrong.
    let reported = number(pots.get("monthlyCredits"))?;

    let remaining = reported.clamp(0.0, grant);
    let period = billing_period(reading);

    Some((
        period.seconds.unwrap_or(30 * 86_400),
        UsageWindow::new(
            // The heading says which half of this row the account reported and
            // which half this build supplied.
            "Monthly · estimated",
            Some(super::percent_from_fraction((grant - remaining) / grant)),
        )
        .with_reset(period.end),
    ))
}

/// What an account with no plan has bought, as a spend limit.
///
/// **Both halves have to have been reported, and absent is not zero.** What is
/// left arrives directly, what is gone arrives from the summary, and their sum
/// is the money this period started with — so a missing half leaves no
/// denominator the provider gave, and there is no window.
///
/// - Every credit pot absent — the shape a renamed field produces, and this
///   route is undocumented — would total nothing left, put the whole pool in the
///   numerator, and draw a **full red ring**, telling the reader a limit is spent
///   that the provider never said a word about.
/// - The summary call failing — it is allowed to, quietly — would put zero in
///   the numerator and draw an **untouched ring** for an account that may be at
///   the wall.
fn pool_window(reading: &Reading) -> Option<(i64, UsageWindow)> {
    let pots = reading.pots()?;

    let amounts: Vec<f64> = ["monthlyCredits", "purchasedCredits", "freeCredits"]
        .iter()
        .filter_map(|key| number(pots.get(*key)))
        .collect();
    if amounts.is_empty() {
        return None;
    }

    let reported_spend = number(reading.summary?.get("totalCost"))?;

    let remaining: f64 = amounts.iter().map(|pot| pot.max(0.0)).sum();
    let spent = reported_spend.max(0.0);
    let pool = remaining + spent;
    // Nothing left and nothing spent is an account that has said nothing about
    // a pool at all, which is not the same as one that is empty.
    if pool <= 0.0 {
        return None;
    }

    let period = billing_period(reading);

    Some((
        period.seconds.unwrap_or(30 * 86_400),
        UsageWindow::new(
            "Spend",
            Some(super::percent_from_fraction((spent / pool).clamp(0.0, 1.0))),
        )
        .with_reset(period.end),
    ))
}

/// The billing period, and a length only where the reply gave both ends of it.
/// Otherwise the month is a sort key and nothing divides by it.
struct Period {
    /// When it ends, which is stated on its own where it is stated at all.
    end: Option<String>,
    /// Its length, which needs both ends.
    seconds: Option<i64>,
}

fn billing_period(reading: &Reading) -> Period {
    let data = reading.subscription_data();
    let start = data.and_then(|data| stamp(data.get("currentPeriodStart"))).and_then(|s| s.date);
    let end = data.and_then(|data| stamp(data.get("currentPeriodEnd"))).and_then(|s| s.date);

    let seconds = start
        .zip(end)
        .map(|(start, end)| (end - start).num_seconds())
        .filter(|seconds| *seconds > 0);

    Period {
        end: end.map(stamp_of),
        seconds,
    }
}

/// The plan this account is on, from whichever reply named it.
///
/// `credits` carries a `planId` of its own, and it is the one that survives the
/// subscription lookup failing — which is a real state, because that call is
/// allowed to come back empty rather than sink the whole reading.
fn plan_id(reading: &Reading) -> Option<String> {
    reading
        .subscription_data()
        .and_then(|data| credentials::dig_str(data, "planId"))
        .or_else(|| reading.pots().and_then(|pots| credentials::dig_str(pots, "planId")))
        .filter(|id| !id.is_empty())
}

/// Whether a plan is paying for this account right now.
///
/// `"active"` is the CLI's own test, and anything else — cancelled, past due,
/// trialing, a plan that lapsed — is an account back on what it has bought,
/// which is a pool this *can* measure from reported numbers alone.
fn is_on_a_plan(reading: &Reading) -> bool {
    // **No answer is not the same as an answer of "none".** The lookup is
    // allowed to fail without sinking the reading, so its silence is not
    // evidence of no plan, and reading it as such would answer with the pooled
    // balance — the wrong question for a subscriber, and the wrong number.
    let Some(subscription) = reading.subscription else {
        return plan_id(reading).is_some();
    };

    // It did answer. Take it at its word in both directions: an account it says
    // has no subscription is on what it bought, however much a stale `planId` in
    // the credits reply still names.
    let Some(data) = object(Some(subscription), "data") else {
        return false;
    };

    // A plan with no status is a reply whose shape has moved. Treat it as
    // running: the cost of being wrong that way is a row this build cannot size,
    // which draws nothing, and the cost of the other way is the pooled balance
    // passed off as a plan.
    let Some(status) = data.get("status").and_then(Value::as_str) else {
        return true;
    };
    status.to_lowercase() == "active"
}

/// The monthly grant for a plan id, or `None` for one this build has never heard
/// of — which is a plan added or renamed since it shipped.
///
/// Matched on the whole id, lowercased. The CLI matches on a *prefix*, which
/// would size a hypothetical `individual-pro-v2` as the $30 `individual-pro`;
/// being wrong by $50 is worse here than saying nothing.
fn plan_grant(id: Option<&str>) -> Option<f64> {
    let normalized = id?.to_lowercase().replace('_', "-");
    PLAN_GRANTS
        .iter()
        .find(|(plan, _)| *plan == normalized)
        .map(|(_, grant)| *grant)
}

/// `individual-pro` → "Individual Pro".
///
/// The plan id is passed through tidied rather than mapped: the CLI's own table
/// of plan names is a table in a client, so a tier added after this build would
/// be blanked by it, and an unfamiliar name still beats none.
fn plan_name(id: Option<&str>) -> Option<String> {
    let id = id.filter(|id| !id.is_empty())?;

    Some(
        id.split(['-', '_'])
            .filter(|word| !word.is_empty())
            .map(|word| {
                let mut chars = word.chars();
                match chars.next() {
                    Some(first) => {
                        first.to_uppercase().collect::<String>() + &chars.as_str().to_lowercase()
                    }
                    None => String::new(),
                }
            })
            .collect::<Vec<_>>()
            .join(" "),
    )
}

/// The same total as a number. `None` where no pot was reported at all — absent
/// is not a balance of zero.
fn remaining(credits: &Value) -> Option<f64> {
    let pots = object(Some(credits), "credits")?;

    let amounts: Vec<f64> = ["monthlyCredits", "purchasedCredits", "freeCredits"]
        .iter()
        .filter_map(|key| number(pots.get(*key)))
        .collect();
    if amounts.is_empty() {
        return None;
    }

    Some(amounts.iter().map(|pot| pot.max(0.0)).sum())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn at(text: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(text).unwrap().with_timezone(&Utc)
    }

    /// A percentage to three decimal places, for the rows whose arithmetic is
    /// not a round number.
    fn rounded(value: Option<f64>) -> Option<f64> {
        value.map(|value| (value * 1_000.0).round() / 1_000.0)
    }

    /// The four replies of one pass, as the service documents them.
    fn fixture() -> (Value, Value, Value, Value) {
        let whoami = json!({
            "org": { "id": "org-1", "login": "someone" },
            "orgLimits": [
                { "scope": "org", "spent": 12.5, "limit": 50.0,
                  "resetInterval": "monthly", "resetAt": "2026-11-01T00:00:00Z" },
                { "scope": "model", "model": "gpt-5", "modelLabel": "GPT-5",
                  "spent": 9.0, "limit": 10.0, "exceeded": true,
                  "resetInterval": "daily", "resetAt": "2026-10-02T00:00:00Z" }
            ]
        });
        let credits = json!({
            "credits": {
                "planId": "individual-pro",
                "monthlyCredits": 22.0,
                "purchasedCredits": 5.0,
                "freeCredits": 1.0
            },
            "windowLimits": {
                "limited": true,
                "fiveHour": { "used": 2.0, "cap": 8.0, "resetAt": 1_759_500_000_000i64 },
                "weekly": { "used": 8.0, "cap": 8.0 }
            }
        });
        let subscription = json!({
            "data": {
                "planId": "individual-pro",
                "status": "active",
                "currentPeriodStart": "2026-10-01T00:00:00Z",
                "currentPeriodEnd": "2026-10-31T00:00:00Z"
            }
        });
        let summary = json!({ "totalCost": 8.0, "totalCount": 42.0 });

        (whoami, credits, subscription, summary)
    }

    #[test]
    fn reads_the_pool_the_rolling_windows_and_the_org_limits() {
        let (whoami, credits, subscription, summary) = fixture();
        let reading = Reading {
            whoami: &whoami,
            credits: &credits,
            subscription: Some(&subscription),
            summary: Some(&summary),
        };

        let usage = windows(&reading);
        let labels: Vec<&str> = usage.iter().map(|w| w.label.as_str()).collect();

        // Shortest first: the five-hour window leads, then the org's daily
        // limit, then the rolling week, then the two month-long rows in the
        // order they were built.
        assert_eq!(
            labels,
            vec!["5h", "Spend GPT-5", "7d", "Spend", "Monthly · estimated"]
        );

        // 2 of 8, and 8 of 8.
        assert_eq!(usage[0].percent_used, Some(25.0));
        assert_eq!(usage[2].percent_used, Some(100.0));
        // The daily model limit, and the organisation's month.
        assert_eq!(usage[1].percent_used, Some(90.0));
        assert_eq!(usage[3].percent_used, Some(25.0));
        // A $30 grant with $22 left.
        assert_eq!(rounded(usage[4].percent_used), Some(26.667));
    }

    #[test]
    fn spendable_credit_is_reported_as_numbers_even_without_allowance_windows() {
        let whoami = json!({}); let credits = json!({"credits":{"purchasedCredits":12.34}});
        let usage = usage_reading(&Reading { whoami:&whoami, credits:&credits, subscription:None, summary:None });
        assert!(usage.error.is_none()); assert_eq!(usage.windows.len(),1);
        assert_eq!(usage.credit_remaining.as_ref().unwrap().amount,12.34);
        assert_eq!(usage.credit_remaining.as_ref().unwrap().currency,"USD");
    }

    #[test]
    fn a_rolling_reset_is_epoch_milliseconds() {
        let (whoami, credits, subscription, summary) = fixture();
        let reading = Reading {
            whoami: &whoami,
            credits: &credits,
            subscription: Some(&subscription),
            summary: Some(&summary),
        };

        let usage = windows(&reading);
        assert_eq!(
            usage[0].resets_at.as_deref(),
            Some(stamp_of(at("2025-10-03T14:00:00Z"))).as_deref()
        );
        // A window with no reset stated claims none.
        assert!(usage[2].resets_at.is_none());
    }

    #[test]
    fn a_reset_interval_is_a_length_only_where_it_was_stated() {
        assert_eq!(interval(Some("daily")), (86_400, true));
        assert_eq!(interval(Some("weekly")), (7 * 86_400, true));
        // A month is 28 to 31 days: a sort key, and nothing divides by it.
        assert_eq!(interval(Some("monthly")), (30 * 86_400, false));
        // A lifetime cap sorts last.
        assert_eq!(interval(Some("total")), (365 * 86_400, false));
        assert_eq!(interval(None), (365 * 86_400, false));
    }

    #[test]
    fn the_accounts_own_word_for_a_limit_it_has_reached_is_kept() {
        let (whoami, credits, subscription, summary) = fixture();
        let reading = Reading {
            whoami: &whoami,
            credits: &credits,
            subscription: Some(&subscription),
            summary: Some(&summary),
        };

        let usage = windows(&reading);
        assert_eq!(usage[1].detail.as_deref(), Some("spent"));
        assert_eq!(usage[3].detail, None);
    }

    #[test]
    fn a_ceiling_of_zero_or_less_is_not_a_denominator() {
        let whoami = json!({ "orgLimits": [
            { "scope": "org", "spent": 1.0, "limit": 0.0 },
            { "scope": "org", "spent": 1.0, "limit": -1.0, "exceeded": true },
            { "scope": "org", "spent": 1.0, "limit": 10.0 }
        ] });
        let credits = json!({ "credits": { "monthlyCredits": 1.0 } });
        let reading = Reading {
            whoami: &whoami,
            credits: &credits,
            subscription: None,
            summary: None,
        };

        let limits = org_windows(&reading);
        assert_eq!(limits.len(), 1);
        assert_eq!(limits[0].1.percent_used, Some(10.0));
    }

    #[test]
    fn a_model_scope_needs_a_model_and_an_org_scope_takes_none() {
        let whoami = json!({ "orgLimits": [
            { "scope": "model", "model": "  ", "spent": 1.0, "limit": 10.0 },
            { "scope": "model", "model": "m-1", "spent": 1.0, "limit": 10.0 },
            { "scope": "org", "modelLabel": "ignored", "spent": 1.0, "limit": 10.0 }
        ] });
        let credits = json!({ "credits": { "monthlyCredits": 1.0 } });
        let reading = Reading {
            whoami: &whoami,
            credits: &credits,
            subscription: None,
            summary: None,
        };

        let labels: Vec<String> = org_windows(&reading)
            .into_iter()
            .map(|(_, window)| window.label)
            .collect();
        assert_eq!(labels, vec!["Spend", "Spend m-1", "Spend"]);
    }

    #[test]
    fn the_rolling_windows_are_drawn_only_while_the_account_says_they_bind() {
        let with = |limited: Value| {
            let whoami = json!({});
            let credits = json!({
                "credits": { "monthlyCredits": 1.0 },
                "windowLimits": { "limited": limited, "fiveHour": { "used": 1.0, "cap": 2.0 } }
            });
            let reading = Reading {
                whoami: &whoami,
                credits: &credits,
                subscription: None,
                summary: None,
            };
            rolling_windows(&reading).len()
        };

        assert_eq!(with(json!(true)), 1);
        assert_eq!(with(json!(false)), 0);
        assert_eq!(with(Value::Null), 0);
    }

    #[test]
    fn a_plan_is_the_grant_the_table_knows_and_nothing_else() {
        assert_eq!(plan_grant(Some("individual-pro")), Some(30.0));
        assert_eq!(plan_grant(Some("individual-pro-v1")), Some(80.0));
        assert_eq!(plan_grant(Some("individual_pro")), Some(30.0));
        // A plan added or renamed since this build draws nothing at all.
        assert_eq!(plan_grant(Some("individual-pro-v2")), None);
        assert_eq!(plan_grant(Some("teams-pro-v2")), None);
        assert_eq!(plan_grant(None), None);
    }

    #[test]
    fn a_plan_the_table_cannot_size_draws_no_ring_rather_than_the_pool() {
        let whoami = json!({});
        let credits = json!({
            "credits": { "planId": "individual-mystery", "monthlyCredits": 50.0 }
        });
        let subscription = json!({ "data": { "planId": "individual-mystery", "status": "active" } });
        let summary = json!({ "totalCost": 10.0 });
        let reading = Reading {
            whoami: &whoami,
            credits: &credits,
            subscription: Some(&subscription),
            summary: Some(&summary),
        };

        // Not the pooled balance, and not a zero: nothing.
        assert!(credit_window(&reading).is_none());
    }

    #[test]
    fn a_remainder_over_the_grant_is_clamped_and_a_missing_one_draws_nothing() {
        let with = |reported: Value| {
            let whoami = json!({});
            let credits = json!({ "credits": { "planId": "individual-pro", "monthlyCredits": reported } });
            let subscription = json!({ "data": { "planId": "individual-pro", "status": "active" } });
            let reading = Reading {
                whoami: &whoami,
                credits: &credits,
                subscription: Some(&subscription),
                summary: None,
            };
            credit_window(&reading).map(|(_, window)| window)
        };

        assert_eq!(rounded(with(json!(22.0)).unwrap().percent_used), Some(26.667));
        // Over the grant is a grant untouched, not a negative spend.
        assert_eq!(with(json!(45.0)).unwrap().percent_used, Some(0.0));
        // Nothing left is wholly spent.
        assert_eq!(with(json!(0.0)).unwrap().percent_used, Some(100.0));
        // Absent is not zero.
        assert!(with(Value::Null).is_none());
    }

    #[test]
    fn a_pool_needs_both_halves_the_provider_gave() {
        let build = |pots: Value, summary: Value| {
            let whoami = json!({});
            let credits = json!({ "credits": pots });
            let summary = (!summary.is_null()).then_some(summary);
            let reading = Reading {
                whoami: &whoami,
                credits: &credits,
                subscription: None,
                summary: summary.as_ref(),
            };
            credit_window(&reading).map(|(_, window)| window)
        };

        // 6 left, 8 spent: 8 of 14.
        let window = build(
            json!({ "monthlyCredits": 5.0, "purchasedCredits": 1.0 }),
            json!({ "totalCost": 8.0 }),
        )
        .unwrap();
        assert_eq!(rounded(window.percent_used), Some(57.143));
        assert_eq!(window.label, "Spend");

        // The summary failing leaves no denominator the provider gave.
        assert!(build(json!({ "monthlyCredits": 5.0 }), Value::Null).is_none());
        // Every pot absent is not "nothing left".
        assert!(build(json!({}), json!({ "totalCost": 8.0 })).is_none());
        // Nothing left and nothing spent says nothing about a pool at all.
        assert!(build(
            json!({ "monthlyCredits": 0.0 }),
            json!({ "totalCost": 0.0 })
        )
        .is_none());
    }

    #[test]
    fn a_plan_is_running_only_when_the_service_says_so() {
        let named = |subscription: Value, plan_in_credits: Value| {
            let whoami = json!({});
            let credits = json!({ "credits": { "planId": plan_in_credits } });
            let subscription = (!subscription.is_null()).then_some(subscription);
            let reading = Reading {
                whoami: &whoami,
                credits: &credits,
                subscription: subscription.as_ref(),
                summary: None,
            };
            is_on_a_plan(&reading)
        };

        assert!(named(json!({ "data": { "status": "active" } }), json!("p")));
        assert!(named(json!({ "data": { "status": "ACTIVE" } }), json!("p")));
        for status in ["canceled", "past_due", "trialing", ""] {
            assert!(!named(json!({ "data": { "status": status } }), json!("p")));
        }
        // A reply whose shape has moved is treated as running: the row it draws
        // is one this build cannot size, which draws nothing at all.
        assert!(named(json!({ "data": { "planId": "p" } }), json!("p")));
        // It answered, and said there is no subscription.
        assert!(!named(json!({}), json!("p")));

        // It did not answer at all: the credits reply is what names the plan,
        // and its silence is not evidence of no plan.
        assert!(named(Value::Null, json!("p")));
        assert!(!named(Value::Null, Value::Null));
    }

    #[test]
    fn the_billing_period_needs_both_ends_for_a_length_and_keeps_its_end() {
        let build = |start: Value, end: Value| {
            let whoami = json!({});
            let credits = json!({ "credits": { "monthlyCredits": 1.0 } });
            let subscription = json!({ "data": {
                "currentPeriodStart": start, "currentPeriodEnd": end
            } });
            let reading = Reading {
                whoami: &whoami,
                credits: &credits,
                subscription: Some(&subscription),
                summary: None,
            };
            billing_period(&reading)
        };

        let period = build(json!("2026-10-01T00:00:00Z"), json!("2026-10-31T00:00:00Z"));
        assert_eq!(period.seconds, Some(30 * 86_400));
        assert_eq!(period.end.as_deref(), Some("2026-10-31T00:00:00Z"));

        // One end is not a length.
        let period = build(Value::Null, json!("2026-10-31T00:00:00Z"));
        assert_eq!(period.seconds, None);
        assert_eq!(period.end.as_deref(), Some("2026-10-31T00:00:00Z"));

        // Either end may arrive as an epoch number, and the two spellings of
        // the same instant are not a length.
        let period = build(json!(1_759_276_800.0), json!(1_759_276_800_000i64));
        assert_eq!(period.seconds, None);
        assert_eq!(period.end.as_deref(), Some("2025-10-01T00:00:00Z"));
    }

    #[test]
    fn a_stamp_keeps_the_text_the_service_sent() {
        assert_eq!(stamp(Some(&json!("2026-10-01T00:00:00Z"))).unwrap().query, "2026-10-01T00:00:00Z");
        // A whole number goes back without a decimal point.
        assert_eq!(stamp(Some(&json!(1_759_276_800i64))).unwrap().query, "1759276800");
        assert_eq!(stamp(Some(&json!(1.5))).unwrap().query, "1.5");
        assert!(stamp(Some(&json!(true))).is_none());
        assert!(stamp(None).is_none());
    }

    #[test]
    fn the_plan_name_is_the_id_tidied_rather_than_mapped() {
        assert_eq!(plan_name(Some("individual-pro")).as_deref(), Some("Individual Pro"));
        assert_eq!(plan_name(Some("teams_pro")).as_deref(), Some("Teams Pro"));
        assert_eq!(plan_name(Some("individual-max-v2")).as_deref(), Some("Individual Max V2"));
        assert!(plan_name(Some("")).is_none());
        assert!(plan_name(None).is_none());
    }

    #[test]
    fn the_balance_is_the_pots_added_up_and_absent_is_not_zero() {
        assert_eq!(
            remaining(&json!({ "credits": {
                "monthlyCredits": 22.0, "purchasedCredits": 5.5, "freeCredits": 1.0
            } })),
            Some(28.5)
        );
        // A negative pot is money owed, not a discount on the others.
        assert_eq!(
            remaining(&json!({ "credits": { "monthlyCredits": 10.0, "freeCredits": -3.0 } })),
            Some(10.0)
        );
        assert!(remaining(&json!({ "credits": {} })).is_none());
        assert!(remaining(&json!({})).is_none());
    }
}
