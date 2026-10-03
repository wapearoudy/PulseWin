//! Replicate: the prepaid credit left on the account, read with the signed-in
//! replicate.com session.
//!
//! Replicate's API token runs models; the billing figures are only on the
//! website, behind its session. So the credential is a pasted `Cookie` header,
//! kept to `sessionid` (the one that has to be there) and `csrftoken`. The
//! original reads that session out of a browser's cookie store through the
//! macOS login keychain, which PulseWin cannot.
//!
//! Two requests, both to replicate.com:
//!
//! 1. `GET /account/billing` as a page. Its server-rendered React props name
//!    the account the page is for — a user or an organization — and that is
//!    the only way to know whose credit to ask for.
//! 2. `GET /api/{users|organizations}/{name}/unused-credit`, the page's own
//!    call for the balance.
//!
//! **What is left out, and why.** The month's spend, from the invoices the page
//! lists, has no limit to measure it against and nowhere to go yet, so it is
//! not asked for. Replicate reports no allowance and no percentage, and none is
//! drawn: the balance is the reading, and a purse with no stated size has no
//! fraction to be one.
//!
//! The shapes are second-hand — taken from the original and its fixtures, not
//! from captured replies.

use std::sync::Arc;

use serde_json::Value;

use super::{describe_reqwest_error, find_ignoring_case, Ctx, FetchFuture, Provider};
use crate::model::ProviderUsage;

const ID: &str = "replicate";
const NAME: &str = "Replicate";

const BILLING_PAGE: &str = "https://replicate.com/account/billing";

/// The session, and the CSRF token the site keeps beside it. The session is the
/// one that has to be there; a header without it is no sign-in.
const COOKIES: [&str; 2] = ["sessionid", "csrftoken"];

/// How many nodes of a page's props are looked at before the page is given up
/// on. The page is not ours, and a payload that nests without end must not be
/// walked without an end.
const VISIT_CEILING: usize = 4_000;

pub struct Replicate;

impl Provider for Replicate {
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
        return ProviderUsage::failed(ID, NAME, super::pasted::needed(ID, "Replicate session cookie"));
    };

    let page = match get(&ctx, BILLING_PAGE, &cookie, "text/html").await {
        Ok(page) => page,
        Err(reason) => return ProviderUsage::failed(ID, NAME, reason),
    };

    let account = match account(&page) {
        PageAccount::Found(account) => account,
        PageAccount::SignedOut => {
            return ProviderUsage::failed(
                ID,
                NAME,
                "the session has expired; copy a fresh cookie from replicate.com",
            )
        }
        PageAccount::Unrecognized => {
            return ProviderUsage::failed(
                ID,
                NAME,
                "the billing page names no account this build can find",
            )
        }
    };

    let credit = match get(&ctx, &account.credit_url(), &cookie, "application/json").await {
        Ok(body) => body,
        Err(reason) => return ProviderUsage::failed(ID, NAME, reason),
    };

    reading(&credit)
}

/// One request with the pasted header, and what its status means.
async fn get(ctx: &Ctx, url: &str, cookie: &str, accept: &str) -> Result<String, String> {
    // The client that refuses redirects: a `Cookie` header set by hand rides a
    // redirect to whatever host it names.
    let response = ctx
        .gateway_client
        .get(url)
        .header("Cookie", cookie)
        .header("Accept", accept)
        .send()
        .await
        .map_err(|e| format!("request failed: {}", describe_reqwest_error(&e)))?;

    let status = response.status();
    let body = response
        .text()
        .await
        .map_err(|e| format!("cannot read body: {e}"))?;

    if status.is_redirection() {
        return Err(
            "HTTP 3xx — the session has expired; copy a fresh cookie from replicate.com".to_string(),
        );
    }
    if !status.is_success() {
        let hints: &[(u16, &str)] = &[
            (401, " — the session has expired; copy a fresh cookie from replicate.com"),
            (403, " — the session has expired; copy a fresh cookie from replicate.com"),
            (429, " — rate limited, try again shortly"),
        ];
        return Err(super::http_failure(status, hints));
    }

    Ok(body)
}

// ---------------------------------------------------------------------------
// Whose billing page this is
// ---------------------------------------------------------------------------

/// The account the billing page is for, and the route its credit is read from.
struct Account {
    is_organization: bool,
    username: String,
}

impl Account {
    /// The account's own credit route. The name is a path segment, so anything
    /// in it that is not one — a space in an organization's name — is escaped.
    fn credit_url(&self) -> String {
        let kind = if self.is_organization {
            "organizations"
        } else {
            "users"
        };
        format!(
            "https://replicate.com/api/{kind}/{}/unused-credit",
            escape_segment(&self.username)
        )
    }
}

/// What the billing page turned out to be.
enum PageAccount {
    Found(Account),
    /// Replicate's public sign-in page, which a lapsed session is sent to.
    SignedOut,
    Unrecognized,
}

/// The account named in the page's `react-component-props` JSON, found wherever
/// it is nested.
fn account(html: &str) -> PageAccount {
    let mut visited = 0;

    for (attributes, body) in scripts(html) {
        if !is_props_script(attributes) {
            continue;
        }
        let Ok(payload) = serde_json::from_str::<Value>(body) else {
            continue;
        };

        // Breadth first, and bounded. Only what can hold an account is put on
        // the queue, so a page of strings is walked in one step.
        let mut queue: Vec<&Value> = vec![&payload];
        let mut index = 0;
        while index < queue.len() && visited < VISIT_CEILING {
            let item = queue[index];
            index += 1;
            visited += 1;

            if let Some(found) = account_in(item) {
                return PageAccount::Found(found);
            }

            match item {
                Value::Object(map) => queue.extend(
                    map.values()
                        .filter(|value| value.is_object() || value.is_array()),
                ),
                Value::Array(items) => queue.extend(
                    items
                        .iter()
                        .filter(|value| value.is_object() || value.is_array()),
                ),
                _ => {}
            }
        }
    }

    // Both markers of Replicate's own sign-in page, not just the word.
    if has_sign_in_title(html) && has_github_login_link(html) {
        PageAccount::SignedOut
    } else {
        PageAccount::Unrecognized
    }
}

/// The account inside one node, when it names one this build can ask for.
fn account_in(item: &Value) -> Option<Account> {
    let account = item.get("account")?;
    let kind = account.get("kind").and_then(Value::as_str)?;
    if kind != "user" && kind != "organization" {
        return None;
    }
    let username = account.get("username").and_then(Value::as_str)?.trim();
    if username.is_empty() {
        return None;
    }
    Some(Account {
        is_organization: kind == "organization",
        username: username.to_string(),
    })
}

/// `<title>Sign in | Replicate</title>`, with the spaces the original's own
/// expression lets sit anywhere — including nowhere.
fn has_sign_in_title(html: &str) -> bool {
    let mut at = 0;
    while let Some((_, body)) = open_tag(html, at, "title") {
        let Some(end) = html.get(body..).and_then(|rest| rest.find("</title")) else {
            return false;
        };
        let squeezed: String = html[body..body + end]
            .chars()
            .filter(|c| !c.is_whitespace())
            .collect();
        if squeezed.eq_ignore_ascii_case("Signin|Replicate") {
            return true;
        }
        at = body;
    }
    false
}

/// Replicate's own GitHub sign-in link, which only its sign-in page carries.
fn has_github_login_link(html: &str) -> bool {
    let mut at = 0;
    while let Some((open, body)) = open_tag(html, at, "a") {
        if attribute(&html[open..body], "href")
            .is_some_and(|href| href.starts_with("/login/github/"))
        {
            return true;
        }
        at = open + 1;
    }
    false
}

/// React's own props script: the id `react-component-props…` **and** a JSON
/// type, both, as the original requires.
fn is_props_script(attributes: &str) -> bool {
    attribute(attributes, "id")
        .is_some_and(|id| id.to_lowercase().starts_with("react-component-props"))
        && attribute(attributes, "type")
            .is_some_and(|kind| kind.eq_ignore_ascii_case("application/json"))
}

// ---------------------------------------------------------------------------
// Reading the balance
// ---------------------------------------------------------------------------

/// The mapping, kept apart from the request so a fixture can drive it.
fn reading(body: &str) -> ProviderUsage {
    let root: Value = match serde_json::from_str(body) {
        Ok(value @ Value::Object(_)) => value,
        Ok(_) => return ProviderUsage::failed(ID, NAME, "the reply could not be read"),
        Err(_) => return ProviderUsage::failed(ID, NAME, "the reply could not be read"),
    };

    // A balance the reply leaves out is nothing to show, not a zero.
    let credit = match root.get("unused_credit") {
        None | Some(Value::Null) => {
            return ProviderUsage::failed(ID, NAME, "no limits reported in the reply")
        }
        Some(value) => match figure(value) {
            Some(credit) => credit,
            None => return ProviderUsage::failed(ID, NAME, "the reply could not be read"),
        },
    };

    // Money and nothing else: Replicate reports no allowance and no percentage,
    // and a zero here would also be a full red ring and a notification saying
    // the account was spent.
    ProviderUsage::ok(
        ID,
        NAME,
        vec![super::balance_window("Balance", format!("{credit:.2} USD"))],
    )
    .with_credit_remaining(credit, "USD")
}

/// A balance that may arrive as a number or as a numeric string. The string has
/// to be a plain non-negative decimal — `NaN` and `-1` are words here, not
/// figures — and a boolean is neither.
fn figure(value: &Value) -> Option<f64> {
    match value {
        Value::String(text) => {
            let trimmed = text.trim();
            if !plain_decimal(trimmed) {
                return None;
            }
            trimmed.parse::<f64>().ok().filter(|figure| figure.is_finite())
        }
        _ => super::dig_number(Some(value)).filter(|figure| figure.is_finite() && *figure >= 0.0),
    }
}

/// `^\d+(\.\d+)?$` — digits, and a point with digits after it if there is one.
fn plain_decimal(text: &str) -> bool {
    let (whole, fraction) = match text.split_once('.') {
        Some((whole, fraction)) => (whole, Some(fraction)),
        None => (text, None),
    };
    if whole.is_empty() || !whole.bytes().all(|b| b.is_ascii_digit()) {
        return false;
    }
    match fraction {
        Some(fraction) => !fraction.is_empty() && fraction.bytes().all(|b| b.is_ascii_digit()),
        None => true,
    }
}

/// A path segment with everything but the characters that may sit in one
/// escaped, as the original's own encoder writes them.
fn escape_segment(text: &str) -> String {
    const KEPT: &str = "-._~!$&'()*+,;=:@";
    let mut escaped = String::new();
    for byte in text.bytes() {
        let c = byte as char;
        if c.is_ascii_alphanumeric() || KEPT.contains(c) {
            escaped.push(c);
        } else {
            escaped.push_str(&format!("%{byte:02X}"));
        }
    }
    escaped
}

// ---------------------------------------------------------------------------
// The page's own markup
// ---------------------------------------------------------------------------

/// Every `<script …>…</script>` on a page, as the opening tag and what the
/// element holds.
fn scripts(html: &str) -> Vec<(&str, &str)> {
    let mut found: Vec<(&str, &str)> = Vec::new();
    let mut at = 0;

    while let Some((open, body)) = open_tag(html, at, "script") {
        let Some(close) = html.get(body..).and_then(|rest| find_ignoring_case(rest, "</script")) else {
            break;
        };
        let end = body + close;
        found.push((&html[open..body], &html[body..end]));
        at = end;
    }

    found
}

/// The next opening tag named `name` at or after `from`, as the index of its
/// `<` and the index just past its `>`.
fn open_tag(html: &str, from: usize, name: &str) -> Option<(usize, usize)> {
    let mut at = from;
    loop {
        let open = at + html.get(at..)?.find('<')?;
        let after = open + 1;
        let rest = html.get(after..)?;

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

/// An attribute's value inside a tag, quoted with either mark.
fn attribute(tag: &str, name: &str) -> Option<String> {
    let mut from = 0;
    while let Some(at) = find_ignoring_case(&tag[from..], name) {
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

#[cfg(test)]
mod tests {
    use super::*;

    /// Second-hand, from the original's fixtures: the page and the reply
    /// CodexBar's Replicate plugin describes, which is what the route is read
    /// against.
    const USER_PAGE: &str = r#"<html><head><title>Billing | Replicate</title></head><body>
    <script type="application/json" id="react-component-props-billing-page">
    {"page":{"account":{"kind":"user","username":"demo-user"}}}
    </script></body></html>"#;

    const SIGNED_OUT_PAGE: &str = r#"<html><head><title>Sign in | Replicate</title></head>
    <body><a class="btn" href="/login/github/?next=/account/billing">Sign in with GitHub</a></body></html>"#;

    fn fixture() -> String {
        r#"{"unused_credit":"80.00"}"#.to_string()
    }

    #[test]
    fn the_balance_is_read_as_reported_in_dollars_and_draws_no_ring() {
        let usage = reading(&fixture());
        assert_eq!(usage.credit_remaining.as_ref().unwrap().amount, 80.0);
        assert_eq!(usage.credit_remaining.as_ref().unwrap().currency, "USD");
        assert_eq!(usage.windows.len(), 1);
        assert_eq!(usage.windows[0].label, "Balance");
        // Money and no fraction: a purse with no stated size has no ring.
        assert_eq!(usage.windows[0].percent_used, None);
        assert_eq!(usage.windows[0].detail.as_deref(), Some("80.00 USD"));
    }

    #[test]
    fn the_billing_page_names_a_users_account() {
        match account(USER_PAGE) {
            PageAccount::Found(found) => {
                assert!(!found.is_organization);
                assert_eq!(found.username, "demo-user");
                assert_eq!(
                    found.credit_url(),
                    "https://replicate.com/api/users/demo-user/unused-credit"
                );
            }
            _ => panic!("the user's account should be found"),
        }
    }

    #[test]
    fn an_organization_nested_deeper_is_found_and_asked_for_as_one() {
        let page = r#"<script id='react-component-props-layout' type='application/json'>
        {"layout":{"nested":[{"x":1},{"account":{"kind":"organization","username":"demo org"}}]}}
        </script>"#;

        match account(page) {
            PageAccount::Found(found) => {
                assert!(found.is_organization);
                assert_eq!(
                    found.credit_url(),
                    "https://replicate.com/api/organizations/demo%20org/unused-credit"
                );
            }
            _ => panic!("the organization should be found"),
        }
    }

    #[test]
    fn props_in_any_other_script_are_not_trusted() {
        let page = r#"<script type="application/json" id="analytics">{"account":{"kind":"user","username":"someone"}}</script>
        <script id="react-component-props-x">{"account":{"kind":"user","username":"no-type"}}</script>"#;
        assert!(matches!(account(page), PageAccount::Unrecognized));
    }

    #[test]
    fn the_sign_in_page_is_a_lapsed_session_not_a_changed_page() {
        assert!(matches!(account(SIGNED_OUT_PAGE), PageAccount::SignedOut));
        // The title alone is not enough: the link has to be there too.
        assert!(matches!(
            account("<title>Sign in | Replicate</title>"),
            PageAccount::Unrecognized
        ));
    }

    #[test]
    fn a_balance_that_is_not_one_cannot_be_read() {
        for reply in [
            r#"{"unused_credit":"NaN"}"#,
            r#"{"unused_credit":"-1"}"#,
            r#"{"unused_credit":""}"#,
            r#"{"unused_credit":true}"#,
            r#"{"unused_credit":-3}"#,
            "<html>unavailable</html>",
            "[]",
        ] {
            assert!(reading(reply).error.is_some(), "read {reply}");
        }
    }

    #[test]
    fn no_balance_reported_is_nothing_to_show_not_a_zero() {
        assert!(reading("{}").error.is_some());
        assert!(reading(r#"{"unused_credit":null}"#).error.is_some());
        // A number, or a string that is one, is the balance either way.
        assert_eq!(
            reading(r#"{"unused_credit":12.5}"#).windows[0]
                .detail
                .as_deref(),
            Some("12.50 USD")
        );
        assert_eq!(
            reading(r#"{"unused_credit":0}"#).windows[0].detail.as_deref(),
            Some("0.00 USD")
        );
    }

    /// Only the session cookies are kept from the browser, and the session is
    /// required.
    #[test]
    fn only_the_session_cookies_are_kept() {
        let kept =
            super::super::pasted::keep("_ga=1; sessionid=abc; csrftoken=t", &COOKIES);
        assert_eq!(kept.as_deref(), Some("sessionid=abc; csrftoken=t"));
        assert_eq!(
            super::super::pasted::keep("csrftoken=t", &COOKIES),
            None
        );
    }

    #[test]
    fn a_name_that_is_not_a_path_segment_is_escaped() {
        assert_eq!(escape_segment("demo-user"), "demo-user");
        assert_eq!(escape_segment("demo org"), "demo%20org");
        assert_eq!(escape_segment("a/b"), "a%2Fb");
    }
}
