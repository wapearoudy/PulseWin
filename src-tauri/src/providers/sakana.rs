//! Sakana AI's subscription: a five-hour and a weekly limit, each stated as a
//! percentage on the console's billing page, and the pay-as-you-go credit
//! balance on the same page's other tab.
//!
//! **A page, not an API.** Sakana publishes no usage route; the console renders
//! the figures on the server, so the page is read as HTML with the pasted
//! session:
//! `GET https://console.sakana.ai/billing`, and `?tab=payAsYouGo` for the
//! balance, which that tab alone renders.
//!
//! **The credential is a pasted cookie.** The original imports the session out
//! of the browser the reader signed in with (`Auth/BrowserCookies.swift`),
//! which on macOS reads a Chromium cookie store through the login keychain;
//! PulseWin cannot, so the `Cookie` header copied out of a signed-in request is
//! the credential here instead. The console signs in with Auth.js, whose
//! cookies its sign-in page sets under the `authjs` prefix, so only
//! `__Secure-authjs.session-token` is kept.
//!
//! **Redirects are not followed.** A signed-out console redirects to its
//! sign-in page; following it would carry the session there and read the
//! sign-in page as a billing page. A redirect is an expired session.
//!
//! The balance is best effort: a tab that fails, or does not state its figure
//! in dollars, leaves the balance off and the limits standing.
//!
//! The markup is second-hand — taken from the original and its fixtures, not
//! from a captured page. The original reads it with four regular expressions;
//! this is the same reader without a regular-expression engine, over the same
//! shapes.

use std::sync::Arc;

use super::{describe_reqwest_error, Ctx, FetchFuture, Provider};
use crate::model::{ProviderUsage, UsageWindow};

const ID: &str = "sakana";
const NAME: &str = "Sakana AI";

const BILLING: &str = "https://console.sakana.ai/billing";
const PAY_AS_YOU_GO: &str = "https://console.sakana.ai/billing?tab=payAsYouGo";

/// The one cookie the console signs in with, and the one that has to be there.
const COOKIES: [&str; 1] = ["__Secure-authjs.session-token"];

/// How far past the "Credit balance" heading the figure may sit, in bytes — the
/// original's `[\s\S]{0,900}?`.
const BALANCE_WINDOW: usize = 900;

pub struct Sakana;

impl Provider for Sakana {
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
        return ProviderUsage::failed(ID, NAME, super::pasted::needed(ID, "Sakana AI session cookie"));
    };

    let page = match get(&ctx, BILLING, &cookie).await {
        Ok(page) => page,
        Err(reason) => return ProviderUsage::failed(ID, NAME, reason),
    };

    // Best effort, and asked only once the page that must answer has: a tab
    // that fails leaves the balance off.
    let balance = get(&ctx, PAY_AS_YOU_GO, &cookie).await.ok();

    reading(&page, balance.as_deref())
}

/// One page, with the pasted header, and what its status means.
async fn get(ctx: &Ctx, url: &str, cookie: &str) -> Result<String, String> {
    let response = ctx
        .gateway_client
        .get(url)
        .header("Cookie", cookie)
        .header("Accept", "text/html,application/xhtml+xml")
        // The labels read below are the English ones.
        .header("Accept-Language", "en-US,en;q=0.9")
        .send()
        .await
        .map_err(|e| format!("request failed: {}", describe_reqwest_error(&e)))?;

    let status = response.status();
    let body = response
        .text()
        .await
        .map_err(|e| format!("cannot read body: {e}"))?;

    if status.is_redirection() {
        return Err("HTTP 3xx — the session has expired; copy a fresh cookie from console.sakana.ai"
            .to_string());
    }
    if !status.is_success() {
        let hints: &[(u16, &str)] = &[
            (401, " — the session has expired; copy a fresh cookie from console.sakana.ai"),
            (403, " — the session has expired; copy a fresh cookie from console.sakana.ai"),
            (429, " — rate limited, try again shortly"),
        ];
        return Err(super::http_failure(status, hints));
    }

    Ok(body)
}

// ---------------------------------------------------------------------------
// Reading the page
// ---------------------------------------------------------------------------

/// The two limits the page names, with the length each one is.
const LIMITS: [(&str, &str, i64); 2] = [
    ("5-hour", "5h", 5 * 3_600),
    ("Weekly", "7d", 7 * 86_400),
];

/// The mapping, kept apart from the request so a fixture can drive it.
fn reading(page: &str, pay_as_you_go: Option<&str>) -> ProviderUsage {
    if page.is_empty() {
        return ProviderUsage::failed(ID, NAME, "the reply could not be read");
    }

    let mut rows: Vec<(i64, UsageWindow)> = Vec::new();
    for (label, name, seconds) in LIMITS {
        // Both the figure and its own paragraph: a limit the page renders
        // without one is left off, and the other still stands.
        let Some(section) = section(page, label) else {
            continue;
        };
        let Some(percent) = used_percent(section) else {
            continue;
        };
        rows.push((
            seconds,
            UsageWindow::new(name, Some(super::percent_from_scale(percent)))
                .with_reset(reset_stamp(section)),
        ));
    }

    let mut windows = super::by_window_length(rows);

    // Money on the other tab, when it states it in dollars. A purse with no
    // stated size has no fraction to draw, which is what `balance_window` is.
    let prepaid = pay_as_you_go.and_then(credit_balance);
    if let Some(amount) = prepaid {
        windows.push(super::balance_window("Balance", format!("{amount:.2} USD")));
    }

    if windows.is_empty() {
        return ProviderUsage::failed(ID, NAME, "the reply could not be read");
    }

    let mut usage = ProviderUsage::ok(ID, NAME, windows).with_plan(plan_name(page));
    if let Some(amount) = prepaid { usage = usage.with_credit_remaining(amount, "USD"); }
    usage
}

/// What follows a limit's label, up to the next limit or the next card.
fn section<'a>(html: &'a str, label: &str) -> Option<&'a str> {
    let (start, _) = paragraph(html, 0, |text| text == label)?;
    let rest = &html[start..];
    let end = boundary(rest).unwrap_or(rest.len());
    Some(&rest[..end])
}

/// Where the next limit's own paragraph begins, or where the next card does.
///
/// The original's boundary is `<p…>(?:5-hour|Weekly)</p>` or the opening of a
/// card, taken case-insensitively; the earliest of them ends the section.
fn boundary(html: &str) -> Option<usize> {
    let mut found: Vec<usize> = Vec::new();

    for label in ["5-hour", "Weekly"] {
        let mut at = 0;
        while let Some((open, body)) = open_tag(html, at, "p") {
            match html.get(body..).and_then(|rest| rest.find("</p>")) {
                Some(end) if html[body..body + end].trim().eq_ignore_ascii_case(label) => {
                    found.push(open);
                    break;
                }
                Some(_) => at = open + 1,
                None => break,
            }
        }
    }

    let mut at = 0;
    while let Some((open, body)) = open_tag(html, at, "div") {
        let slot = attribute(&html[open..body], "data-slot");
        if slot.is_some_and(|slot| {
            slot.eq_ignore_ascii_case("card") || slot.eq_ignore_ascii_case("card-title")
        }) {
            found.push(open);
            break;
        }
        at = open + 1;
    }

    found.into_iter().min()
}

/// `<p[^>]*>\s*([0-9]+(?:\.[0-9]+)?)% used\s*</p>` — the share a limit states.
fn used_percent(html: &str) -> Option<f64> {
    let mut at = 0;
    while let Some((open, body)) = open_tag(html, at, "p") {
        let end = html.get(body..).and_then(|rest| rest.find("</p>"))?;
        if let Some(percent) = percent_stated(html[body..body + end].trim()) {
            return Some(percent);
        }
        at = open + 1;
    }
    None
}

/// A paragraph's text as the share it states: a figure and then `% used`, in
/// whichever case the page wrote it.
fn percent_stated(text: &str) -> Option<f64> {
    let cut = text.len().checked_sub("% used".len())?;
    let head = text.get(..cut)?;
    if !text.get(cut..)?.eq_ignore_ascii_case("% used") {
        return None;
    }
    figure(head.trim())
}

/// `<p[^>]*>\s*Resets on ([^<]+?)\s*</p>` — when a limit turns over, or nothing
/// when the page states something that is not a date.
fn reset_stamp(html: &str) -> Option<String> {
    let mut at = 0;
    while let Some((open, body)) = open_tag(html, at, "p") {
        let end = html.get(body..).and_then(|rest| rest.find("</p>"))?;
        if let Some(text) = strip_prefix_ignoring_case(html[body..body + end].trim(), "Resets on ") {
            return reset_date(text);
        }
        at = open + 1;
    }
    None
}

/// The page renders "Resets on June 23, 2026 at 2:53 PM" on the server, in
/// UTC; only the browser's script moves it to local time afterwards.
fn reset_date(text: &str) -> Option<String> {
    let naive = chrono::NaiveDateTime::parse_from_str(text.trim(), "%B %d, %Y at %I:%M %p").ok()?;
    Some(naive.and_utc().to_rfc3339_opts(chrono::SecondsFormat::Secs, true))
}

/// The card's title, as the page's own markup states it:
/// `<div data-slot="card-title"><span>Standard</span>`.
fn plan_name(html: &str) -> Option<String> {
    let mut at = 0;
    while let Some((open, body)) = open_tag(html, at, "div") {
        let slot = attribute(&html[open..body], "data-slot");
        if slot.is_some_and(|slot| slot.eq_ignore_ascii_case("card-title")) {
            let rest = &html[body..];
            let inner = rest.len() - rest.trim_start().len();
            let span = open_tag(rest, inner, "span").filter(|(open, _)| *open == inner);
            let (_, span_body) = span?;
            let end = rest.get(span_body..).and_then(|tail| tail.find("</span>"))?;
            let text = rest[span_body..span_body + end].trim();
            return if text.is_empty() {
                None
            } else {
                Some(text.to_string())
            };
        }
        at = open + 1;
    }
    None
}

/// The card's "Credit balance", when it is stated in dollars. A figure without
/// its `$` is left off rather than given a currency.
fn credit_balance(html: &str) -> Option<f64> {
    let mut at = 0;
    while let Some((open, body)) = open_tag(html, at, "h2") {
        let close = html.get(body..).and_then(|rest| rest.find("</h2>"))?;
        if !html[body..body + close]
            .trim()
            .eq_ignore_ascii_case("Credit balance")
        {
            at = open + 1;
            continue;
        }

        // The window bounds where the figure's own tag may **start**; the
        // paragraph itself is read whole.
        let after = body + close + "</h2>".len();
        let limit = char_boundary_at(html, (after + BALANCE_WINDOW).min(html.len()));
        return dollar_paragraph(html, after, limit);
    }
    None
}

/// The first `<p …>` that starts before `limit`, carries the tabular figures
/// class, and states its amount in dollars.
fn dollar_paragraph(html: &str, from: usize, limit: usize) -> Option<f64> {
    let mut at = from;
    while let Some((open, body)) = open_tag(html, at, "p") {
        if open >= limit {
            return None;
        }
        if has_tabular_nums(&html[open..body]) {
            if let Some(end) = html.get(body..).and_then(|rest| rest.find("</p>")) {
                if let Some(amount) = dollars(html[body..body + end].trim()) {
                    return Some(amount);
                }
            }
        }
        at = open + 1;
    }
    None
}

/// `tabular-nums[^"]*"` inside a tag: the class the console writes its figures
/// with, closed by the quote that ends the attribute.
fn has_tabular_nums(tag: &str) -> bool {
    super::find_ignoring_case(tag, "tabular-nums")
        .is_some_and(|at| tag[at + "tabular-nums".len()..].contains('"'))
}

/// `\$([0-9][0-9,]*(?:\.[0-9]+)?)` — an amount carrying its own currency mark,
/// with the thousands separators the console writes.
fn dollars(text: &str) -> Option<f64> {
    let rest = text.strip_prefix('$')?;
    let (whole, fraction) = match rest.split_once('.') {
        Some((whole, fraction)) => (whole, Some(fraction)),
        None => (rest, None),
    };
    if !whole.starts_with(|c: char| c.is_ascii_digit())
        || !whole.bytes().all(|b| b.is_ascii_digit() || b == b',')
    {
        return None;
    }
    if let Some(fraction) = fraction {
        if fraction.is_empty() || !fraction.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
    }

    let amount: String = rest.chars().filter(|c| *c != ',').collect();
    amount
        .parse::<f64>()
        .ok()
        .filter(|amount| amount.is_finite() && *amount >= 0.0)
}

/// A figure written the way the page writes one: digits, an optional point and
/// the digits after it, and nothing else — the shape the original's own
/// expression accepts.
fn figure(text: &str) -> Option<f64> {
    let (whole, fraction) = match text.split_once('.') {
        Some((whole, fraction)) => (whole, Some(fraction)),
        None => (text, None),
    };
    if whole.is_empty() || !whole.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    if let Some(fraction) = fraction {
        if fraction.is_empty() || !fraction.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
    }
    text.parse::<f64>().ok().filter(|value| value.is_finite())
}

// ---------------------------------------------------------------------------
// The page's own markup
// ---------------------------------------------------------------------------

/// The next opening tag named `name` at or after `from`, as the index of its
/// `<` and the index just past its `>`.
///
/// Stricter than the original's `<p[^>]*>` in one place, and deliberately: a
/// tag's name has to end where the name does, so `<pre>` is not read as a `<p>`.
fn open_tag(html: &str, from: usize, name: &str) -> Option<(usize, usize)> {
    let mut at = from;
    loop {
        let open = at + html.get(at..)?.find('<')?;
        let after = open + 1;
        let rest = html.get(after..)?;

        // A closing tag, a comment or a doctype is not an opening one.
        if !rest.starts_with('/') && !rest.starts_with('!') {
            if rest
                .get(..name.len())
                .is_some_and(|head| head.eq_ignore_ascii_case(name))
            {
                let ends = match rest[name.len()..].chars().next() {
                    None | Some('>') | Some('/') => true,
                    Some(c) => c.is_whitespace(),
                };
                if ends {
                    return Some((open, after + rest.find('>')? + 1));
                }
            }
        }

        at = after;
    }
}

/// The first `<p …>…</p>` at or after `from` whose text `wanted` accepts, as
/// the index the paragraph ended at and the text it held.
fn paragraph<'a, F>(html: &'a str, from: usize, wanted: F) -> Option<(usize, &'a str)>
where
    F: Fn(&str) -> bool,
{
    let mut at = from;
    while let Some((open, body)) = open_tag(html, at, "p") {
        let end = html.get(body..).and_then(|rest| rest.find("</p>"))?;
        let text = html[body..body + end].trim();
        if wanted(text) {
            return Some((body + end + "</p>".len(), text));
        }
        at = open + 1;
    }
    None
}

/// An attribute's value inside a tag, quoted with either mark.
fn attribute(tag: &str, name: &str) -> Option<String> {
    let mut from = 0;
    while let Some(at) = super::find_ignoring_case(&tag[from..], name) {
        let after = from + at + name.len();
        let rest = tag[after..].trim_start();
        if let Some(rest) = rest.strip_prefix('=') {
            let rest = rest.trim_start();
            if let Some(quote) = rest.chars().next().filter(|c| *c == '"' || *c == '\'') {
                let value = &rest[quote.len_utf8()..];
                if let Some(end) = value.find(quote) {
                    return Some(value[..end].to_string());
                }
            }
        }
        from = after;
    }
    None
}

/// The largest character boundary at or before `at`, so a window cut in bytes
/// cannot land inside a character.
fn char_boundary_at(text: &str, at: usize) -> usize {
    let mut at = at.min(text.len());
    while at > 0 && !text.is_char_boundary(at) {
        at -= 1;
    }
    at
}

/// `text` without `prefix`, where the two differ only in case.
fn strip_prefix_ignoring_case<'a>(text: &'a str, prefix: &str) -> Option<&'a str> {
    text.get(..prefix.len())
        .filter(|head| head.eq_ignore_ascii_case(prefix))?;
    text.get(prefix.len()..)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Second-hand, from the original's fixtures: the markup CodexBar's Sakana
    /// provider describes, which is what the page is read against. It pins the
    /// shape this reads; it does not prove the shape is right.
    const BILLING_PAGE: &str = r#"
    <main>
      <div data-slot="card-title"><span>Standard</span><span>$20/mo</span></div>
      <div data-slot="card-title">Usage limit</div>
      <p class="font-medium text-sm">5-hour</p>
      <p class="text-muted-foreground text-xs tabular-nums">Resets on June 23, 2026 at 2:53 PM</p>
      <button aria-label="The 5-hour window starts with your first request."></button>
      <p class="text-muted-foreground text-sm">92% used</p>
      <p class="font-medium text-sm">Weekly</p>
      <p class="text-muted-foreground text-xs tabular-nums">Resets on June 29, 2026 at 12:00 AM</p>
      <button aria-label="Weekly usage resets every Monday at 00:00 UTC."></button>
      <p class="text-muted-foreground text-sm">32% used</p>
    </main>
    "#;

    const BALANCE_TAB: &str = r#"
    <main>
      <h2 class="font-semibold text-base">Credit balance</h2>
      <button aria-label="Credit updates may be delayed."></button>
      <p class="font-semibold text-3xl tabular-nums">$1,212.34</p>
      <h2 class="font-semibold">Usage</h2>
      <span class="text-muted-foreground text-sm">Total<!-- -->: <!-- -->$5.67</span>
    </main>
    "#;

    #[test]
    fn reads_both_limits_as_stated_with_the_plan_and_the_balance() {
        let usage = reading(BILLING_PAGE, Some(BALANCE_TAB));
        assert_eq!(usage.credit_remaining.as_ref().unwrap().amount, 1212.34);
        assert_eq!(usage.credit_remaining.as_ref().unwrap().currency, "USD");
        let labels: Vec<&str> = usage.windows.iter().map(|w| w.label.as_str()).collect();
        assert_eq!(labels, vec!["5h", "7d", "Balance"]);
        assert_eq!(usage.windows[0].percent_used, Some(92.0));
        assert_eq!(usage.windows[1].percent_used, Some(32.0));
        // Rendered on the server in UTC, and read as UTC.
        assert_eq!(
            usage.windows[0].resets_at.as_deref(),
            Some("2026-06-23T14:53:00Z")
        );
        assert_eq!(
            usage.windows[1].resets_at.as_deref(),
            Some("2026-06-29T00:00:00Z")
        );
        assert_eq!(usage.plan.as_deref(), Some("Standard"));
        // Money and no fraction: the balance draws no ring.
        assert_eq!(usage.windows[2].percent_used, None);
        assert_eq!(usage.windows[2].detail.as_deref(), Some("1212.34 USD"));
    }

    #[test]
    fn a_reset_that_does_not_read_as_a_date_is_left_off_not_the_limit() {
        let page = BILLING_PAGE.replace("June 23, 2026 at 2:53 PM", "soon-ish");
        let usage = reading(&page, None);
        assert_eq!(usage.windows[0].percent_used, Some(92.0));
        assert_eq!(usage.windows[0].resets_at, None);
    }

    #[test]
    fn a_limit_with_no_figure_is_left_off_and_the_other_stands() {
        let page = BILLING_PAGE.replace("92% used", "");
        let usage = reading(&page, None);
        let labels: Vec<&str> = usage.windows.iter().map(|w| w.label.as_str()).collect();
        assert_eq!(labels, vec!["7d"]);
    }

    #[test]
    fn a_balance_not_stated_in_dollars_is_left_off() {
        let tab = BALANCE_TAB.replace("$1,212.34", "1,212.34");
        assert_eq!(credit_balance(&tab), None);
        // A card with no balance heading at all is not one either.
        assert_eq!(credit_balance(BILLING_PAGE), None);
    }

    #[test]
    fn a_page_with_neither_limit_nor_balance_cannot_be_read() {
        for page in ["<main>Billing</main>", ""] {
            assert!(reading(page, None).error.is_some(), "read {page}");
        }
    }

    /// A tab that fails leaves the limits standing; only the page that must
    /// answer decides whether there is a reading at all.
    #[test]
    fn the_limits_stand_without_the_balance_tab() {
        let usage = reading(BILLING_PAGE, None);
        assert_eq!(usage.windows.len(), 2);
        assert_eq!(usage.plan.as_deref(), Some("Standard"));
    }

    /// Only the sign-in cookie is kept from the browser, and it is required.
    #[test]
    fn only_the_sign_in_cookie_is_kept() {
        let kept = super::super::pasted::keep(
            "theme=dark; __Secure-authjs.session-token=t; other=1",
            &COOKIES,
        );
        assert_eq!(kept.as_deref(), Some("__Secure-authjs.session-token=t"));
        assert_eq!(super::super::pasted::keep("theme=dark; other=1", &COOKIES), None);
    }

    /// The page is read by hand, so the pieces it is read with are pinned.
    #[test]
    fn the_markup_is_read_as_the_original_reads_it() {
        // A tag's name ends where the name does: `<pre>` is not a paragraph.
        let pre = "<pre>5-hour</pre><p>5-hour</p><p>10% used</p>";
        let five_hour = section(pre, "5-hour").unwrap();
        assert_eq!(used_percent(five_hour), Some(10.0));

        // The section stops at the next limit's own paragraph.
        let both = "<p>5-hour</p><p>10% used</p><p>Weekly</p><p>20% used</p>";
        assert_eq!(used_percent(section(both, "5-hour").unwrap()), Some(10.0));
        assert_eq!(used_percent(section(both, "Weekly").unwrap()), Some(20.0));

        // A share the page writes with decimals, and one it writes in capitals.
        assert_eq!(percent_stated("12.5% used"), Some(12.5));
        assert_eq!(percent_stated("12% USED"), Some(12.0));
        assert_eq!(percent_stated("12%"), None);
        assert_eq!(percent_stated("% used"), None);
        assert_eq!(percent_stated("1.2.3% used"), None);

        // An amount with its own currency mark, and one without.
        assert_eq!(dollars("$1,212.34"), Some(1212.34));
        assert_eq!(dollars("$5"), Some(5.0));
        assert_eq!(dollars("1,212.34"), None);
        assert_eq!(dollars("$"), None);
        assert_eq!(dollars("$,"), None);
    }
}
