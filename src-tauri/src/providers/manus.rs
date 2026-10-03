//! Manus, the agent: the plan's monthly credits, the credits that refresh on a
//! shorter clock, and the account's total credit balance. Each allowance is a
//! size and a remainder the service states.
//!
//! **The credential is a pasted cookie, sent as a bearer token.** The original
//! imports the `session_id` out of the browser the reader signed in with
//! (`Auth/BrowserCookies.swift`), which on macOS reads a Chromium cookie store
//! through the login keychain; PulseWin cannot, so the `Cookie` header copied
//! out of a signed-in request is the credential here — and, as in the original,
//! only the `session_id` value out of it goes on the wire, as
//! `Authorization: Bearer …`.
//!
//! Read from `POST https://api.manus.im/user.v1.UserService/GetAvailableCredits`
//! — a Connect call with an empty body, from the site's own origin, in a
//! browser's name. Nothing else about the session goes with it.
//!
//! A figure the reply leaves out is left out here, never read as zero: an
//! allowance with no remainder has no share used. The refresh's reset is read
//! only as an ISO 8601 date; its length only when the reply names the interval
//! as daily.
//!
//! The shape is second-hand — taken from the original and its fixture, not from
//! a captured reply.

use std::sync::Arc;

use serde_json::Value;

use super::{describe_reqwest_error, percent_from_fraction, Ctx, FetchFuture, Provider};
use crate::model::{ProviderUsage, UsageWindow};

const ID: &str = "manus";
const NAME: &str = "Manus";

const ENDPOINT: &str = "https://api.manus.im/user.v1.UserService/GetAvailableCredits";

/// The one cookie the site signs in with, and the one that has to be there.
const COOKIES: [&str; 1] = ["session_id"];

/// The headers the original sends: the site's own origin, and a browser's name.
const BROWSER_USER_AGENT: &str = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 \
                                  (KHTML, like Gecko) Chrome/135.0.0.0 Safari/537.36";

/// The credit figures the reply may carry, in the order a reading takes them.
const FIGURES: [&str; 5] = [
    "totalCredits",
    "periodicCredits",
    "proMonthlyCredits",
    "refreshCredits",
    "maxRefreshCredits",
];

/// The objects Manus has wrapped its credits in, in the order they are looked
/// inside.
const ENVELOPES: [&str; 4] = ["data", "result", "response", "availableCredits"];

pub struct Manus;

impl Provider for Manus {
    fn id(&self) -> &'static str {
        ID
    }

    fn name(&self) -> &'static str {
        NAME
    }

    /// Whether a pasted header naming a session is on the machine. No network
    /// call.
    fn is_configured(&self) -> bool {
        super::pasted::cookie(ID)
            .and_then(|header| session_token(&header))
            .is_some()
    }

    fn fetch(&self, ctx: Arc<Ctx>) -> FetchFuture {
        Box::pin(async move { fetch_inner(ctx).await })
    }
}

/// The `session_id` value out of the kept `name=value` header.
fn session_token(header: &str) -> Option<String> {
    super::pasted::keep(header, &COOKIES)
        .and_then(|kept| kept.split_once('=').map(|(_, value)| value.to_string()))
        .filter(|token| !token.is_empty())
}

async fn fetch_inner(ctx: Arc<Ctx>) -> ProviderUsage {
    let Some(token) = super::pasted::cookie(ID).and_then(|header| session_token(&header)) else {
        return ProviderUsage::failed(ID, NAME, super::pasted::needed(ID, "Manus session cookie"));
    };

    // The client that refuses redirects, so the token is never carried to
    // another host.
    let response = ctx
        .gateway_client
        .post(ENDPOINT)
        .header("Authorization", format!("Bearer {token}"))
        .header("Content-Type", "application/json")
        .header("Accept", "application/json")
        .header("Connect-Protocol-Version", "1")
        .header("Origin", "https://manus.im")
        .header("Referer", "https://manus.im/")
        .header("User-Agent", BROWSER_USER_AGENT)
        .body("{}")
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
        return ProviderUsage::failed(
            ID,
            NAME,
            "HTTP 3xx — the session has expired; copy a fresh cookie from manus.im",
        );
    }
    if !status.is_success() {
        let hints: &[(u16, &str)] = &[
            (401, " — the session has expired; copy a fresh cookie from manus.im"),
            (403, " — the session has expired; copy a fresh cookie from manus.im"),
            (429, " — rate limited, try again shortly"),
        ];
        return ProviderUsage::failed(ID, NAME, super::http_failure(status, hints));
    }

    reading(&body)
}

// ---------------------------------------------------------------------------
// Reading the reply
// ---------------------------------------------------------------------------

/// One credit figure: a number, or a numeric string. Anything else — a word, a
/// boolean, a null — is no figure rather than a zero.
fn figure(object: &Value, key: &str) -> Option<f64> {
    super::dig_number(object.get(key)).filter(|figure| figure.is_finite())
}

/// The credits object, bare or inside one of the envelopes Manus has used.
fn credits(root: &Value) -> Option<&Value> {
    let named = ENVELOPES
        .iter()
        .filter_map(|key| root.get(*key))
        .find(|nested| FIGURES.iter().any(|key| figure(nested, key).is_some()));

    named.or_else(|| FIGURES.iter().any(|key| figure(root, key).is_some()).then_some(root))
}

/// The mapping, kept apart from the request so a fixture can drive it.
fn reading(body: &str) -> ProviderUsage {
    let root: Value = match serde_json::from_str(body) {
        Ok(value) => value,
        Err(_) => return ProviderUsage::failed(ID, NAME, "the reply could not be read"),
    };

    let Some(credits) = credits(&root) else {
        return ProviderUsage::failed(ID, NAME, "the reply could not be read");
    };

    // "DAILY REFRESH" and "daily" are the same answer, and neither is one this
    // build knows unless it says daily.
    let daily = credits
        .get("refreshInterval")
        .and_then(Value::as_str)
        .is_some_and(|interval| interval.to_lowercase().contains("daily"));

    let mut rows: Vec<(i64, UsageWindow)> = Vec::new();

    if let Some(window) = allowance(
        daily.then_some("Daily").unwrap_or("Credits"),
        figure(credits, "maxRefreshCredits"),
        figure(credits, "refreshCredits"),
        date(credits.get("nextRefreshTime").and_then(Value::as_str)),
    ) {
        rows.push((86_400, window));
    }

    // The plan's monthly credits. No renewal date is reported, so no reset is
    // claimed and the month is a sort key.
    if let Some(window) = allowance(
        "Monthly",
        figure(credits, "proMonthlyCredits"),
        figure(credits, "periodicCredits"),
        None,
    ) {
        rows.push((30 * 86_400, window));
    }

    let mut windows = super::by_window_length(rows);

    // Manus's credits are its own unit, not money, so this is a count and draws
    // no ring: a purse with no stated size has no fraction to draw.
    let total = figure(credits, "totalCredits").filter(|total| *total >= 0.0);
    if let Some(total) = total {
        windows.push(super::balance_window(
            "Balance",
            format!("{} credits", total.round() as i64),
        ));
    }

    if windows.is_empty() {
        return ProviderUsage::failed(ID, NAME, "no limits reported in the reply");
    }

    ProviderUsage::ok(ID, NAME, windows)
}

/// Used is the size less what is left, both as reported. A size of zero is no
/// allowance, and either half missing is no share.
fn allowance(label: &str, size: Option<f64>, left: Option<f64>, reset: Option<String>) -> Option<UsageWindow> {
    let size = size.filter(|size| *size > 0.0)?;
    let left = left.filter(|left| *left >= 0.0)?;
    Some(
        UsageWindow::new(
            label,
            Some(percent_from_fraction((size - left).max(0.0) / size)),
        )
        .with_reset(reset),
    )
}

/// A reset read only as the ISO 8601 date-time the original's own formatter
/// accepts: an offset is part of the shape, so a bare date is not one.
fn date(text: Option<&str>) -> Option<String> {
    let text = text?.trim();
    let at = chrono::DateTime::parse_from_rfc3339(text).ok()?;
    Some(at.with_timezone(&chrono::Utc).to_rfc3339_opts(chrono::SecondsFormat::Secs, true))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Second-hand, from the original's fixture: the reply CodexBar's Manus
    /// provider describes, which is what the route is read against.
    fn fixture() -> String {
        r#"{"totalCredits":"1200","freeCredits":0,"periodicCredits":"300","addonCredits":0,"proMonthlyCredits":1000,"refreshCredits":75,"maxRefreshCredits":300,"eventCredits":0,"nextRefreshTime":"2026-04-13T00:00:00Z","refreshInterval":"daily"}"#
            .to_string()
    }

    #[test]
    fn reads_the_daily_refresh_and_the_monthly_credits() {
        let usage = reading(&fixture());

        let labels: Vec<&str> = usage.windows.iter().map(|w| w.label.as_str()).collect();
        assert_eq!(labels, vec!["Daily", "Monthly", "Balance"]);
        // 225 of 300 spent on the refresh; 700 of 1,000 on the month.
        assert_eq!(usage.windows[0].percent_used, Some(75.0));
        assert_eq!(usage.windows[1].percent_used, Some(70.0));
        assert_eq!(
            usage.windows[0].resets_at.as_deref(),
            Some("2026-04-13T00:00:00Z")
        );
        // No renewal date is reported for the month, so none is claimed.
        assert_eq!(usage.windows[1].resets_at, None);
        // Credits are a count, not money: no ring, and the unit is its own.
        assert_eq!(usage.windows[2].percent_used, None);
        assert_eq!(usage.windows[2].detail.as_deref(), Some("1200 credits"));
    }

    #[test]
    fn an_envelope_is_looked_inside() {
        let usage = reading(r#"{"data":{"proMonthlyCredits":100,"periodicCredits":40}}"#);
        assert_eq!(usage.windows.len(), 1);
        assert_eq!(usage.windows[0].label, "Monthly");
        assert_eq!(usage.windows[0].percent_used, Some(60.0));
    }

    #[test]
    fn missing_figures_are_left_off_never_read_as_zero() {
        let sparse = r#"{"totalCredits":"1200","periodicCredits":"300","proMonthlyCredits":1000,
                         "refreshCredits":"bad","maxRefreshCredits":100,"nextRefreshTime":0,"refreshInterval":"DAILY REFRESH"}"#;
        let usage = reading(sparse);
        // The refresh has no remainder, so it has no share; the numeric reset
        // is not read either.
        let labels: Vec<&str> = usage.windows.iter().map(|w| w.label.as_str()).collect();
        assert_eq!(labels, vec!["Monthly", "Balance"]);
        assert_eq!(usage.windows[0].percent_used, Some(70.0));
    }

    #[test]
    fn a_refresh_interval_not_named_daily_claims_no_length() {
        let usage = reading(r#"{"refreshCredits":1,"maxRefreshCredits":4,"refreshInterval":"weekly"}"#);
        assert_eq!(usage.windows[0].label, "Credits");
        assert_eq!(usage.windows[0].percent_used, Some(75.0));
    }

    #[test]
    fn only_a_total_is_a_balance_with_no_ring() {
        let usage = reading(r#"{"totalCredits":5}"#);
        let labels: Vec<&str> = usage.windows.iter().map(|w| w.label.as_str()).collect();
        assert_eq!(labels, vec!["Balance"]);
        assert_eq!(usage.windows[0].detail.as_deref(), Some("5 credits"));
    }

    #[test]
    fn a_reply_with_no_credit_figure_at_all_cannot_be_read() {
        for reply in [
            r#"{"nextRefreshTime":"2026-04-13T00:00:00Z","refreshInterval":"daily"}"#,
            r#"{"code":"unauthenticated","message":"expired"}"#,
            "[]",
            "not json",
        ] {
            assert!(reading(reply).error.is_some(), "read {reply}");
        }
    }

    #[test]
    fn only_the_session_id_value_goes_as_the_token() {
        assert_eq!(
            session_token("session_id=abc; other=x").as_deref(),
            Some("abc")
        );
        // Everything after the first `=` is the value, as a cookie's own rules
        // say.
        assert_eq!(
            session_token("other=x; session_id=a=b").as_deref(),
            Some("a=b")
        );
        assert_eq!(session_token("other=x"), None);
        assert_eq!(session_token("session_id="), None);
    }

    /// An offset is part of the shape the original's formatter accepts, so a
    /// bare date and a number are not resets.
    #[test]
    fn a_reset_is_read_only_as_a_date_time() {
        assert_eq!(
            date(Some("2026-04-13T00:00:00Z")).as_deref(),
            Some("2026-04-13T00:00:00Z")
        );
        assert_eq!(date(Some("2026-04-13")), None);
        assert_eq!(date(Some("0")), None);
        assert_eq!(date(None), None);
    }
}
