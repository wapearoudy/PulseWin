//! Ollama Cloud: the session usage and weekly usage allowances, read off the
//! signed-in settings page.
//!
//! **A page, not an API.** Ollama publishes no quota route at all: the figures
//! are rendered on `https://ollama.com/settings`, so the page is read as HTML.
//!
//! **The credential is a pasted cookie.** This is the provider the original's
//! browser-cookie reader was written for — `Auth/BrowserCookies.swift` says so
//! in its own first paragraph — and on macOS that reader needs the login
//! keychain for a Chromium key or Full Disk Access for Safari. PulseWin reads
//! neither, so the `Cookie` header copied out of a signed-in request is the
//! credential here instead. Only the four session names are ever sent; the
//! analytics and preference cookies the host also sets stay where they are.
//!
//! The page is read in one pass: the two labels have to appear exactly once
//! each, each label's own section has to state one `N% used` and at most one
//! `data-time`, and **both windows have to be there**. A page that has changed
//! shape is said to be unreadable rather than read as zero usage.

use std::sync::Arc;

use super::{describe_reqwest_error, percent_from_fraction, Ctx, FetchFuture, Provider};
use crate::model::{parse_reset, ProviderUsage, UsageWindow};

const ID: &str = "ollama-cloud";
const NAME: &str = "Ollama Cloud";

const SETTINGS_URL: &str = "https://ollama.com/settings";

/// The session names, each of which may also arrive with a `.<digits>` suffix
/// — Auth.js numbers its session cookies when a site issues more than one.
const COOKIES: [&str; 4] = [
    "wos-session",
    "__Secure-session",
    "__Secure-next-auth.session-token",
    "next-auth.session-token",
];

/// The original's own two ceilings: a page larger than this is not read, and
/// neither is a section longer than this.
const MAXIMUM_BYTES: usize = 2 * 1024 * 1024;
const MAXIMUM_SECTION: usize = 20_000;

pub struct OllamaCloud;

impl Provider for OllamaCloud {
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

#[derive(Debug)]
struct Session {
    /// The page's windows: percent used, and the reset the page states.
    session: Window,
    weekly: Window,
}

#[derive(Debug)]
struct Window {
    used: f64,
    resets_at: Option<String>,
}

async fn fetch_inner(ctx: Arc<Ctx>) -> ProviderUsage {
    let Some(pasted) = super::pasted::cookie(ID) else {
        return ProviderUsage::failed(ID, NAME, super::pasted::needed(ID, "Ollama session cookie"));
    };

    let cookie = match session_cookie(&pasted) {
        Ok(cookie) => cookie,
        Err(reason) => {
            return ProviderUsage::failed(
                ID,
                NAME,
                match reason {
                    Reason::Missing => super::pasted::needed(ID, "Ollama session cookie"),
                    Reason::Invalid => {
                        "the pasted cookie is not a usable Cookie header — paste the session \
                         cookie itself, without quotes or spaces in its value"
                            .to_string()
                    }
                },
            )
        }
    };

    let response = ctx
        .gateway_client
        .get(SETTINGS_URL)
        .header("Cookie", &cookie)
        .header("Accept", "text/html")
        .header("Accept-Language", "en-US,en;q=0.9")
        .send()
        .await;

    let response = match response {
        Ok(response) => response,
        Err(e) => {
            return ProviderUsage::failed(
                ID,
                NAME,
                format!("request failed: {}", describe_reqwest_error(&e)),
            )
        }
    };

    let status = response.status();
    let content_type = response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or("")
        .to_lowercase()
        .to_string();
    let body = match response.text().await {
        Ok(body) => body,
        Err(e) => return ProviderUsage::failed(ID, NAME, format!("cannot read body: {e}")),
    };

    // A signed-out request is answered with a redirect to the sign-in page;
    // the client refuses redirects, so it arrives as the 3xx itself. A status
    // the page's own reader knows is mapped the same way it maps it.
    if status.is_redirection() {
        return ProviderUsage::failed(
            ID,
            NAME,
            "HTTP 3xx — the session has expired; copy a fresh cookie from ollama.com",
        );
    }
    match status.as_u16() {
        200 => {}
        401 | 403 => {
            return ProviderUsage::failed(
                ID,
                NAME,
                "HTTP 401 — the session has expired; copy a fresh cookie from ollama.com",
            )
        }
        429 => {
            return ProviderUsage::failed(ID, NAME, "HTTP 429 — rate limited, try again shortly")
        }
        _ => {
            return ProviderUsage::failed(
                ID,
                NAME,
                super::http_failure(status, &[]),
            )
        }
    }
    if body.len() > MAXIMUM_BYTES || !content_type.contains("text/html") {
        return ProviderUsage::failed(ID, NAME, unreadable_page());
    }

    match reading(&body) {
        Ok(session) => {
            let windows = vec![
                UsageWindow::new("5h", Some(percent_from_fraction(session.session.used)))
                    .with_reset(session.session.resets_at),
                UsageWindow::new("7d", Some(percent_from_fraction(session.weekly.used)))
                    .with_reset(session.weekly.resets_at),
            ];
            ProviderUsage::ok(ID, NAME, windows)
        }
        Err(reason) => ProviderUsage::failed(ID, NAME, reason),
    }
}

fn unreadable_page() -> String {
    "the Ollama settings page could not be read — it may have changed".to_string()
}

// ---------------------------------------------------------------------------
// The cookie
// ---------------------------------------------------------------------------

#[derive(Debug, PartialEq, Eq)]
enum Reason {
    Missing,
    Invalid,
}

/// The session names out of a pasted `Cookie:` header, and nothing else.
///
/// The original's `OllamaSessionCookie.normalize`, whose two rules matter here:
/// a value with a quote, a backslash or a space in it is refused rather than
/// sent, and **a repeated name is kept once** — a browser store routinely holds
/// a host-only and a domain row for one session, and the original's comment
/// records that throwing there discarded the whole browser instead.
fn session_cookie(input: &str) -> Result<String, Reason> {
    if input.chars().any(|c| c.is_control() || !c.is_ascii()) {
        return Err(Reason::Invalid);
    }
    let trimmed = input.trim();
    let header = match trimmed.get(.."cookie:".len()) {
        Some(head) if head.eq_ignore_ascii_case("cookie:") => trimmed["cookie:".len()..].trim(),
        _ => trimmed,
    };
    if header.is_empty() {
        return Err(Reason::Missing);
    }
    if header.len() > 32_768 {
        return Err(Reason::Invalid);
    }

    let mut kept: Vec<String> = Vec::new();
    let mut seen: Vec<&str> = Vec::new();
    for pair in header.split(';') {
        let pair = pair.trim();
        if pair.is_empty() {
            continue;
        }
        let Some((name, value)) = pair.split_once('=') else {
            return Err(Reason::Invalid);
        };
        if !recognised(name) {
            continue;
        }
        if value.is_empty() || value.contains('"') || value.contains('\\') || value.contains(' ') {
            return Err(Reason::Invalid);
        }
        if seen.contains(&name) {
            continue;
        }
        seen.push(name);
        kept.push(format!("{name}={value}"));
    }

    if kept.is_empty() {
        return Err(Reason::Invalid);
    }
    Ok(kept.join("; "))
}

fn recognised(name: &str) -> bool {
    COOKIES.iter().any(|base| {
        name == *base
            || name
                .strip_prefix(base)
                .and_then(|rest| rest.strip_prefix('.'))
                .is_some_and(|suffix| !suffix.is_empty() && suffix.chars().all(|c| c.is_ascii_digit()))
    })
}

// ---------------------------------------------------------------------------
// The page
// ---------------------------------------------------------------------------

/// Both windows the card shows, or why the page could not be read.
fn reading(html: &str) -> Result<Session, String> {
    if html.len() > MAXIMUM_BYTES || super::find_ignoring_case(html, "<!ENTITY").is_some() {
        return Err(unreadable_page());
    }

    let visible = without_scripts(html);

    // The page's own sign-in form is the page saying so.
    if form_asks_for_sign_in(&visible) {
        return Err("the session has expired; copy a fresh cookie from ollama.com".to_string());
    }

    let session = section(&visible, "Session usage", "Weekly usage").ok_or_else(unreadable_page)?;
    let weekly = section(&visible, "Weekly usage", "Session usage").ok_or_else(unreadable_page)?;
    Ok(Session { session, weekly })
}

/// What follows one label, up to the other one.
///
/// The original walks up from the label's own element until it finds one that
/// states a percentage, stopping before it crosses into the other window. This
/// takes the whole run of text between the two labels instead, which is the
/// region that walk ends at — and holds it to the same two rules: exactly one
/// `N% used` and at most one `data-time`.
fn section(visible: &str, label: &str, other: &str) -> Option<Window> {
    let at = occurrences(visible, label);
    if at.len() != 1 {
        return None;
    }
    let start = at[0] + label.len();
    let rest = &visible[start..];
    let end = occurrences(rest, other)
        .first()
        .copied()
        .unwrap_or(rest.len())
        .min(MAXIMUM_SECTION);
    let region = &rest[..end];

    let percentages = percentages(region);
    if percentages.len() != 1 {
        return None;
    }
    let percent = percentages[0];
    if !(0.0..=100.0).contains(&percent) {
        return None;
    }

    let times = times(region);
    if times.len() > 1 {
        return None;
    }
    let resets_at = match times.first() {
        Some(text) => Some(parse_reset(&serde_json::Value::String(text.clone()))?),
        None => None,
    };

    Some(Window {
        used: percent / 100.0,
        resets_at,
    })
}

/// Every offset `needle` appears at, without regard to case.
fn occurrences(text: &str, needle: &str) -> Vec<usize> {
    let mut found = Vec::new();
    let mut from = 0;
    while from < text.len() {
        let Some(at) = super::find_ignoring_case(&text[from..], needle) else {
            break;
        };
        found.push(from + at);
        from += at + needle.len().max(1);
    }
    found
}

/// A copy of the page with its scripts, styles and templates taken out.
///
/// The original detaches those nodes before reading anything, so an embedded
/// script cannot masquerade as usage. This does the same to the text.
fn without_scripts(html: &str) -> String {
    const SKIPPED: [&str; 3] = ["<script", "<style", "<template"];

    let mut out = String::with_capacity(html.len());
    let mut index = 0;
    while index < html.len() {
        let rest = &html[index..];
        let next = SKIPPED
            .iter()
            .filter_map(|tag| super::find_ignoring_case(rest, tag).map(|at| (at, *tag)))
            .min_by_key(|(at, _)| *at);

        let Some((at, tag)) = next else {
            out.push_str(rest);
            break;
        };
        out.push_str(&rest[..at]);
        let close = format!("</{}>", &tag[1..]);
        match super::find_ignoring_case(&rest[at..], &close) {
            Some(end) => index += at + end + close.len(),
            // Unterminated: nothing after it can be trusted.
            None => break,
        }
    }
    out
}

/// Whether the page carries a form that posts to somewhere named sign-in.
fn form_asks_for_sign_in(html: &str) -> bool {
    let mut from = 0;
    while let Some(at) = super::find_ignoring_case(&html[from..], "action") {
        let cursor = from + at + "action".len();
        let rest = &html[cursor..];
        let rest = rest.trim_start();
        let rest = rest.strip_prefix('=').map(str::trim_start).unwrap_or(rest);
        let Some(quote) = rest.chars().next().filter(|c| *c == '"' || *c == '\'') else {
            from = cursor;
            continue;
        };
        let value = &rest[quote.len_utf8()..];
        let end = value.find(quote).unwrap_or(value.len());
        let action = value[..end].to_lowercase();
        if action.contains("signin") || action.contains("login") {
            return true;
        }
        from = cursor;
    }
    false
}

/// Every `N% used` figure in a region, as the page writes it.
fn percentages(region: &str) -> Vec<f64> {
    let bytes = region.as_bytes();
    let mut found = Vec::new();
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] != b'%' {
            index += 1;
            continue;
        }

        // `% used`, with any amount of space between the two.
        let mut after = index + 1;
        while after < bytes.len() && bytes[after].is_ascii_whitespace() {
            after += 1;
        }
        let word = b"used";
        let is_used = bytes.len() >= after + word.len()
            && bytes[after..after + word.len()].eq_ignore_ascii_case(word)
            && bytes
                .get(after + word.len())
                .map_or(true, |next| !next.is_ascii_alphanumeric());

        if is_used {
            // Walk back over the space, then over the figure itself.
            let mut end = index;
            while end > 0 && bytes[end - 1].is_ascii_whitespace() {
                end -= 1;
            }
            let mut start = end;
            while start > 0 && (bytes[start - 1].is_ascii_digit() || bytes[start - 1] == b'.') {
                start -= 1;
            }
            let before_is_digit = start > 0 && bytes[start - 1].is_ascii_digit();
            if start < end && !before_is_digit {
                if let Ok(value) = region[start..end].parse::<f64>() {
                    if value.is_finite() {
                        found.push(value);
                    }
                }
            }
        }
        index += 1;
    }
    found
}

/// Every `data-time` value in a region.
fn times(region: &str) -> Vec<String> {
    let mut found = Vec::new();
    let mut from = 0;
    while let Some(at) = super::find_ignoring_case(&region[from..], "data-time") {
        let cursor = from + at + "data-time".len();
        let rest = region[cursor..].trim_start();
        let rest = rest.strip_prefix('=').map(str::trim_start).unwrap_or(rest);
        let Some(quote) = rest.chars().next().filter(|c| *c == '"' || *c == '\'') else {
            from = cursor;
            continue;
        };
        let value = &rest[quote.len_utf8()..];
        let end = value.find(quote).unwrap_or(value.len());
        let stamp = value[..end].trim();
        if !stamp.is_empty() {
            found.push(stamp.to_string());
        }
        from = cursor;
    }
    found
}

#[cfg(test)]
mod tests {
    use super::*;

    fn page(session: &str, weekly: &str) -> String {
        format!(
            "<html><body><form action=\"/signout\"></form>\
             <div data-usage-meter><p>Session usage</p>{session}</div>\
             <div data-usage-meter><p>Weekly usage</p>{weekly}</div>\
             </body></html>"
        )
    }

    #[test]
    fn reads_both_windows_and_their_resets() {
        let html = page(
            "<p>42% used</p><p data-time=\"2026-10-02T05:00:00.000Z\">Resets soon</p>",
            "<p>8.5% used</p>",
        );
        let session = reading(&html).unwrap();
        assert_eq!(session.session.used, 0.42);
        assert_eq!(
            session.session.resets_at.as_deref(),
            Some("2026-10-02T05:00:00Z")
        );
        assert_eq!(session.weekly.used, 0.085);
        assert_eq!(session.weekly.resets_at, None);
    }

    #[test]
    fn a_script_that_mentions_the_labels_is_not_the_page() {
        let html = page(
            "<script>const labels = ['Session usage', 'Weekly usage'];</script>\
             <p>10% used</p>",
            "<p>20% used</p>",
        );
        let session = reading(&html).unwrap();
        assert_eq!(session.session.used, 0.10);
        assert_eq!(session.weekly.used, 0.20);
    }

    #[test]
    fn both_windows_have_to_be_there() {
        let half = "<html><body><p>Session usage</p><p>10% used</p></body></html>";
        assert!(reading(half).is_err());

        // One window stating nothing is a page this cannot read, not 0% used.
        let silent = page("<p>nothing here</p>", "<p>20% used</p>");
        assert!(reading(&silent).is_err());
    }

    #[test]
    fn a_sign_in_form_is_a_lapsed_session() {
        let html = format!(
            "{}<form action=\"/signin\"></form>",
            page("<p>10% used</p>", "<p>20% used</p>")
        );
        assert!(reading(&html).unwrap_err().contains("expired"));
    }

    #[test]
    fn two_figures_in_one_section_are_not_a_reading() {
        let html = page("<p>10% used</p><p>12% used</p>", "<p>20% used</p>");
        assert!(reading(&html).is_err());
    }

    #[test]
    fn a_figure_is_read_from_the_text_around_it() {
        assert_eq!(percentages("<p>42% used</p>"), vec![42.0]);
        assert_eq!(percentages("<p>8.5 % used</p>"), vec![8.5]);
        // A percentage that is not "used" is not a figure, and a number that
        // runs into another digit is not one either.
        assert!(percentages("<span>112% of quota</span>").is_empty());
        assert!(percentages("<p>110% used</p>").contains(&110.0));
    }

    #[test]
    fn the_cookie_is_reduced_to_the_session_names() {
        let header = "theme=dark; wos-session=abc; __Host-ga=1; next-auth.session-token.0=def";
        assert_eq!(
            session_cookie(header).unwrap(),
            "wos-session=abc; next-auth.session-token.0=def"
        );
        // A name that only looks like one is not kept.
        assert_eq!(
            session_cookie("wos-session-extra=abc").unwrap_err(),
            Reason::Invalid
        );
        assert_eq!(session_cookie("theme=dark").unwrap_err(), Reason::Invalid);
        assert_eq!(session_cookie("  ").unwrap_err(), Reason::Missing);
        // A value with a quote or a space in it is refused rather than sent.
        assert_eq!(
            session_cookie("wos-session=a b").unwrap_err(),
            Reason::Invalid
        );
    }

    #[test]
    fn a_repeated_session_name_is_kept_once() {
        assert_eq!(
            session_cookie("wos-session=host-only; wos-session=domain").unwrap(),
            "wos-session=host-only"
        );
    }
}
