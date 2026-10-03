//! Mistral: the subscription's included allowances — API and Vibe — each as the
//! percentage Mistral reports, and the account's available credit.
//!
//! Read from what Mistral's own admin pages load:
//! `GET https://admin.mistral.ai/api/billing/credits`,
//! `https://admin.mistral.ai/subscription` (whose server-rendered data carries
//! the allowances), and — only when that page has no Vibe allowance —
//! `console.mistral.ai`'s `billing.vibeUsage`.
//!
//! **The credential is a pasted cookie.** The original imports the session out
//! of a Chromium browser's cookie store through the macOS login keychain, which
//! PulseWin cannot, so the `Cookie` header copied out of a signed-in request is
//! the credential here. Mistral signs in with Ory, whose session cookie is
//! `ory_session_` followed by the deployment's own suffix, so the name is a
//! **prefix**: only the `ory_session_…` cookies and `csrftoken` are ever sent.
//!
//! **What is not read.** The original's neighbour leads with the month's spend,
//! which it works out from token counts and a price table of its own. That is
//! an estimate, and this port shows only what Mistral itself reports.
//!
//! **Redirects are not followed.** A missing or expired session is answered
//! with a redirect to Mistral's sign-in page; followed, it would read as a page
//! that "worked" and would carry the cookies along with it.
//!
//! The shapes are second-hand — taken from the original and its fixtures, not
//! from captured replies.

use std::sync::Arc;

use serde_json::Value;

use super::{describe_reqwest_error, percent_from_scale, Ctx, FetchFuture, Provider};
use crate::model::{parse_reset, ProviderUsage, UsageWindow};

const ID: &str = "mistral";
const NAME: &str = "Mistral";

const CREDITS_ENDPOINT: &str = "https://admin.mistral.ai/api/billing/credits";
const SUBSCRIPTION_PAGE: &str = "https://admin.mistral.ai/subscription";
const VIBE_ENDPOINT: &str = "https://console.mistral.ai/api-ui/trpc/billing.vibeUsage";
/// The console's own tRPC envelope for that call, as its page sends it.
const VIBE_INPUT: &str = r#"{"0":{"json":null,"meta":{"values":["undefined"],"v":1}}}"#;

/// The Ory session — under whatever suffix this deployment gave it — and the
/// CSRF token. The session is the one that has to be there.
const COOKIES: [&str; 2] = ["ory_session_*", "csrftoken"];

pub struct Mistral;

impl Provider for Mistral {
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

/// A credit balance: what is available to spend, and in what.
struct Credit {
    amount: f64,
    currency: String,
}

/// One allowance as Mistral reports it: a percentage, and when it resets.
struct Allowance {
    percent_used: f64,
    resets_at: Option<String>,
}

#[derive(Default)]
struct Allowances {
    api: Option<Allowance>,
    vibe: Option<Allowance>,
}

async fn fetch_inner(ctx: Arc<Ctx>) -> ProviderUsage {
    let Some(cookie) = super::pasted::cookie(ID).and_then(|header| super::pasted::keep(&header, &COOKIES))
    else {
        return ProviderUsage::failed(ID, NAME, super::pasted::needed(ID, "Mistral session cookie"));
    };
    let csrf = csrf(&cookie);

    // Credit first: it is the one JSON route, so it is where a session that no
    // longer works is found out.
    let credit = match get(
        &ctx,
        CREDITS_ENDPOINT,
        &cookie,
        csrf,
        "application/json",
        true,
    )
    .await
    {
        Ok(body) => credit(&body),
        Err(reason) => return ProviderUsage::failed(ID, NAME, reason),
    };

    // The allowances are best effort, as they are in Mistral's own pages: an
    // account without a subscription has none.
    let mut allowances = Allowances::default();
    if let Ok(page) = get(&ctx, SUBSCRIPTION_PAGE, &cookie, csrf, "text/html", true).await {
        allowances = from_page(&page);
    }

    // The console is a different origin, so it gets only what it needs: the
    // session, the CSRF cookie, and the same token as a header.
    if allowances.vibe.is_none() && csrf.is_some() {
        let url = format!("{VIBE_ENDPOINT}?batch=1&input={VIBE_INPUT}");
        if let Ok(body) = get(&ctx, &url, &cookie, csrf, "*/*", false).await {
            allowances.vibe = vibe(&body);
        }
    }

    reading(&allowances, credit.as_ref())
}

/// The CSRF token out of the kept header, for the `X-CSRFTOKEN` the admin
/// routes want as well as the cookie.
fn csrf(cookie: &str) -> Option<&str> {
    cookie.split(';').find_map(|part| {
        let (name, value) = part.trim().split_once('=')?;
        (name == "csrftoken" && !value.is_empty()).then_some(value)
    })
}

/// One request, on the client that refuses redirects, and what its status
/// means.
async fn get(
    ctx: &Ctx,
    url: &str,
    cookie: &str,
    csrf: Option<&str>,
    accept: &str,
    admin: bool,
) -> Result<String, String> {
    let mut request = ctx
        .gateway_client
        .get(url)
        .header("Cookie", cookie)
        .header("Accept", accept);

    if admin {
        request = request
            .header("Origin", "https://admin.mistral.ai")
            .header("Referer", "https://admin.mistral.ai/organization/billing");
    }
    // Two spellings, because two routes want them: the admin API sends
    // `X-CSRFTOKEN`, the console's tRPC call `X-CSRFToken`.
    if let Some(csrf) = csrf {
        request = request
            .header("X-CSRFTOKEN", csrf)
            .header("X-CSRFToken", csrf);
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

    // A redirect is a session that no longer works; everything else means what
    // it means everywhere.
    if status.is_redirection() {
        return Err(
            "HTTP 3xx — the session has expired; copy a fresh cookie from mistral.ai".to_string(),
        );
    }
    if !status.is_success() {
        let hints: &[(u16, &str)] = &[
            (401, " — the session has expired; copy a fresh cookie from mistral.ai"),
            (403, " — the session has expired; copy a fresh cookie from mistral.ai"),
            (429, " — rate limited, try again shortly"),
        ];
        return Err(super::http_failure(status, hints));
    }

    Ok(body)
}

// ---------------------------------------------------------------------------
// Reading the replies
// ---------------------------------------------------------------------------

/// What is available to spend: the wallet and any credit notes, less the usage
/// already run up against them and not yet settled — all three as Mistral
/// reports them. `None` if the wallet or the currency is missing, or if the
/// result is below zero, which is a debt and not a balance.
fn credit(body: &str) -> Option<Credit> {
    let reply: Value = serde_json::from_str(body).ok()?;
    if !reply.is_object() {
        return None;
    }

    let wallet = super::dig_number(reply.get("wallet_amount")).filter(|wallet| wallet.is_finite())?;

    let currency = reply.get("currency").and_then(Value::as_str)?.trim().to_uppercase();
    if currency.chars().count() != 3 || !currency.chars().all(|c| c.is_ascii_alphabetic()) {
        return None;
    }

    let notes = super::dig_number(reply.get("credit_notes_amount")).unwrap_or(0.0);
    let ongoing = super::dig_number(reply.get("ongoing_usage_balance")).unwrap_or(0.0);
    let available = wallet + notes - ongoing;
    if !available.is_finite() || available < 0.0 {
        return None;
    }

    Some(Credit {
        amount: available,
        currency,
    })
}

/// The subscription page is a Next.js page: its data arrives as strings pushed
/// onto `self.__next_f`, which together make one stream. The allowances are
/// `api_budget` and `vibe_budget` objects inside it.
fn from_page(html: &str) -> Allowances {
    let stream = flight_stream(html);
    Allowances {
        api: budget(&stream, "api_budget"),
        vibe: budget(&stream, "vibe_budget"),
    }
}

/// Every `[1, "…"]` the page pushed, joined into the one stream they make.
fn flight_stream(html: &str) -> String {
    const MARKER: &str = "self.__next_f.push(";

    let mut stream = String::new();
    let mut cursor = 0;

    while let Some(at) = html.get(cursor..).and_then(|rest| rest.find(MARKER)) {
        let after = cursor + at + MARKER.len();
        cursor = after;

        let Some(rest) = html.get(after..) else {
            break;
        };
        let start = after + (rest.len() - rest.trim_start().len());
        if !html.get(start..).is_some_and(|rest| rest.starts_with('[')) {
            continue;
        }
        let Some(end) = container_end(html, start) else {
            continue;
        };

        if let Ok(Value::Array(array)) = serde_json::from_str::<Value>(&html[start..end]) {
            // A push is `[1, "…"]`; the `[0]` a page opens with is not one.
            if array.len() >= 2 && array[0].as_i64() == Some(1) {
                if let Some(chunk) = array[1].as_str() {
                    stream.push_str(chunk);
                }
            }
        }

        cursor = end;
    }

    stream
}

/// Where the JSON array or object opening at `start` closes, skipping whatever
/// is inside strings. `None` if it never does, or closes wrongly.
fn container_end(text: &str, start: usize) -> Option<usize> {
    let mut closers: Vec<char> = Vec::new();
    let mut in_string = false;
    let mut escaped = false;

    for (offset, character) in text.get(start..)?.char_indices() {
        if in_string {
            if escaped {
                escaped = false;
            } else if character == '\\' {
                escaped = true;
            } else if character == '"' {
                in_string = false;
            }
            continue;
        }

        match character {
            '"' => in_string = true,
            '[' => closers.push(']'),
            '{' => closers.push('}'),
            ']' | '}' => {
                if closers.last() != Some(&character) {
                    return None;
                }
                closers.pop();
                if closers.is_empty() {
                    return Some(start + offset + character.len_utf8());
                }
            }
            _ => {}
        }
    }

    None
}

/// The allowance under one name, where the stream carries **one** value for it.
/// Where it carries two different ones, neither is taken — there is no telling
/// which is this account's.
fn budget(stream: &str, name: &str) -> Option<Allowance> {
    let key = format!("\"{name}\":");

    let mut seen: Vec<String> = Vec::new();
    let mut budgets: Vec<Allowance> = Vec::new();
    let mut cursor = 0;

    while let Some(at) = stream.get(cursor..).and_then(|rest| rest.find(&key)) {
        let after = cursor + at + key.len();
        cursor = after;

        let Some(rest) = stream.get(after..) else {
            break;
        };
        let start = after + (rest.len() - rest.trim_start().len());
        if !stream.get(start..).is_some_and(|rest| rest.starts_with('{')) {
            continue;
        }
        let Some(end) = container_end(stream, start) else {
            continue;
        };
        let object = stream[start..end].to_string();
        cursor = end;

        if seen.contains(&object) {
            continue;
        }
        seen.push(object.clone());
        if let Some(found) = allowance(&object) {
            budgets.push(found);
        }
    }

    match budgets.len() {
        1 => budgets.into_iter().next(),
        _ => None,
    }
}

/// One allowance, by its reported percentage. A server-rendered date may arrive
/// as React's `$D…` form, which is the same stamp behind a tag.
fn allowance(object: &str) -> Option<Allowance> {
    let reply: Value = serde_json::from_str(object).ok()?;

    let percent_used = super::dig_number(reply.get("usage_percentage"))
        .filter(|percent| percent.is_finite() && *percent >= 0.0)?;

    let resets_at = reply
        .get("reset_at")
        .and_then(Value::as_str)
        .map(|stamp| stamp.strip_prefix("$D").unwrap_or(stamp))
        .and_then(|stamp| parse_reset(&Value::String(stamp.to_string())));

    Some(Allowance {
        percent_used,
        resets_at,
    })
}

/// The console's Vibe figure, used only when the subscription page has none:
/// `[{ result: { data: { json: { usage_percentage, reset_at } } } }]`.
fn vibe(body: &str) -> Option<Allowance> {
    let replies: Value = serde_json::from_str(body).ok()?;
    let json = replies
        .as_array()?
        .first()?
        .get("result")?
        .get("data")?
        .get("json")?;
    allowance(&json.to_string())
}

/// Mistral bills a subscription by the month, and the allowances reset with it;
/// the month's length is not a stated one, so nothing claims it.
fn reading(allowances: &Allowances, credit: Option<&Credit>) -> ProviderUsage {
    let mut windows: Vec<UsageWindow> = Vec::new();

    for (scope, allowance) in [("API", &allowances.api), ("Vibe", &allowances.vibe)] {
        let Some(allowance) = allowance else {
            continue;
        };
        windows.push(
            UsageWindow::new(
                format!("Monthly · {scope}"),
                Some(percent_from_scale(allowance.percent_used)),
            )
            .with_reset(allowance.resets_at.clone()),
        );
    }

    // Money and no fraction: a purse with no stated size has no ring.
    if let Some(credit) = credit {
        windows.push(super::balance_window(
            "Balance",
            format!("{:.2} {}", credit.amount, credit.currency),
        ));
    }

    if windows.is_empty() {
        return ProviderUsage::failed(ID, NAME, "no limits reported in the reply");
    }

    let mut usage = ProviderUsage::ok(ID, NAME, windows);
    if let Some(credit) = credit { usage = usage.with_credit_remaining(credit.amount, &credit.currency); }
    usage
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// Second-hand, from the original's fixtures: the page data and the replies
    /// CodexBar's Mistral provider describes.
    const BUDGETS: &str = concat!(
        r#"5:["$","div",null,{"budget":{"api_budget":{"usage_percentage":42.5,"initial_budget":10,"currency":"EUR","reset_at":"$D2026-10-01T00:00:00.000Z"},"vibe_budget":{"usage_percentage":12,"initial_budget":20,"currency":"EUR","reset_at":"2026-10-01T00:00:00Z"}}}]"#,
        "\n"
    );

    /// A Next.js page whose server-rendered data is `stream`, pushed in pieces
    /// the way the page pushes it.
    fn page(stream: &str, pieces: usize) -> String {
        let size = (stream.len() / pieces).max(1);
        let mut scripts = String::from("<script>self.__next_f.push([0])</script>");

        let bytes = stream.as_bytes();
        let mut at = 0;
        while at < bytes.len() {
            // Split on a character boundary, as the page's own writer does.
            let mut end = (at + size).min(bytes.len());
            while end < bytes.len() && !stream.is_char_boundary(end) {
                end += 1;
            }
            let pushed = json!([1, &stream[at..end]]).to_string();
            scripts.push_str(&format!(
                "<script>self.__next_f.push({pushed})</script>"
            ));
            at = end;
        }

        format!("<html><body>{scripts}</body></html>")
    }

    fn credits_fixture() -> String {
        r#"{"wallet_amount":12.5,"credit_notes_amount":2.25,"ongoing_usage_balance":1.5,"currency":"USD","minimum_credits_purchase":10,"maximum_credits_purchase":1000}"#
            .to_string()
    }

    fn vibe_fixture() -> String {
        r#"[{"result":{"data":{"json":{"usage_percentage":37,"quota_changed_this_month":false,"payg_enabled":false,"reset_at":"2026-07-01T00:00:00Z"}}}}]"#
            .to_string()
    }

    #[test]
    fn both_allowances_are_read_from_the_page_as_the_percentages_mistral_reports() {
        let found = from_page(&page(BUDGETS, 3));

        let api = found.api.as_ref().unwrap();
        assert_eq!(api.percent_used, 42.5);
        assert_eq!(api.resets_at.as_deref(), Some("2026-10-01T00:00:00Z"));

        let vibe = found.vibe.as_ref().unwrap();
        assert_eq!(vibe.percent_used, 12.0);
        assert_eq!(vibe.resets_at.as_deref(), Some("2026-10-01T00:00:00Z"));
    }

    #[test]
    fn a_page_with_no_allowance_gives_none() {
        let none = from_page(&page(r#"1:["$","p",null,{"children":"Free"}]"#, 2));
        assert!(none.api.is_none() && none.vibe.is_none());
        let signed_out = from_page("<html>signed out</html>");
        assert!(signed_out.api.is_none() && signed_out.vibe.is_none());
    }

    #[test]
    fn two_allowances_under_one_name_are_both_refused_and_one_repeated_is_still_one() {
        let twice = concat!(
            r#"1:{"api_budget":{"usage_percentage":10}}"#,
            "\n",
            r#"2:{"api_budget":{"usage_percentage":90}}"#
        );
        assert!(from_page(&page(twice, 2)).api.is_none());

        let same = concat!(
            r#"1:{"api_budget":{"usage_percentage":10}}"#,
            "\n",
            r#"2:{"api_budget":{"usage_percentage":10}}"#
        );
        assert_eq!(from_page(&page(same, 2)).api.unwrap().percent_used, 10.0);

        // A figure that is not one is left off.
        let negative = page(r#"1:{"vibe_budget":{"usage_percentage":-4}}"#, 1);
        assert!(from_page(&negative).vibe.is_none());
    }

    #[test]
    fn the_consoles_vibe_figure_when_the_page_has_none() {
        let found = vibe(&vibe_fixture()).unwrap();
        assert_eq!(found.percent_used, 37.0);
        assert_eq!(found.resets_at.as_deref(), Some("2026-07-01T00:00:00Z"));
        assert!(vibe("[]").is_none());
        assert!(vibe("<html>").is_none());
    }

    #[test]
    fn available_credit_is_the_wallet_and_credit_notes_less_usage_not_yet_settled() {
        let found = credit(&credits_fixture()).unwrap();
        // 12.50 + 2.25 − 1.50.
        assert_eq!(found.amount, 13.25);
        assert_eq!(found.currency, "USD");

        // Below zero is a debt, not a balance; no currency is no money.
        assert!(credit(r#"{"wallet_amount":1,"ongoing_usage_balance":3,"currency":"EUR"}"#).is_none());
        assert!(credit(r#"{"wallet_amount":5}"#).is_none());
        assert!(credit("<html>").is_none());
    }

    #[test]
    fn the_reading_is_two_monthly_allowances_scoped_by_product_and_the_balance() {
        let allowances = from_page(&page(BUDGETS, 2));
        let credit = credit(&credits_fixture());
        let usage = reading(&allowances, credit.as_ref());
        assert_eq!(usage.credit_remaining.as_ref().unwrap().amount, 13.25);
        assert_eq!(usage.credit_remaining.as_ref().unwrap().currency, "USD");
        let labels: Vec<&str> = usage.windows.iter().map(|w| w.label.as_str()).collect();
        assert_eq!(labels, vec!["Monthly · API", "Monthly · Vibe", "Balance"]);
        assert_eq!(usage.windows[0].percent_used, Some(42.5));
        assert_eq!(usage.windows[1].percent_used, Some(12.0));
        assert_eq!(
            usage.windows[0].resets_at.as_deref(),
            Some("2026-10-01T00:00:00Z")
        );
        // Money and no fraction: the balance draws no ring.
        assert_eq!(usage.windows[2].percent_used, None);
        assert_eq!(usage.windows[2].detail.as_deref(), Some("13.25 USD"));
    }

    #[test]
    fn nothing_reported_at_all_is_no_limits() {
        let usage = reading(&Allowances::default(), None);
        assert!(usage.error.is_some());
    }

    /// Only the Ory session and the CSRF token are kept from a pasted header —
    /// and the suffix-less `ory_session_` is not a session.
    #[test]
    fn only_the_ory_session_and_the_csrf_token_are_kept() {
        let kept = super::super::pasted::keep(
            "Cookie: _ga=GA1; ory_session_coolstack=abc; csrftoken=tok; ajs_user_id=me; ory_session_=empty",
            &COOKIES,
        );
        assert_eq!(kept.as_deref(), Some("ory_session_coolstack=abc; csrftoken=tok"));
        assert_eq!(csrf(kept.as_deref().unwrap()).as_deref(), Some("tok"));

        assert_eq!(
            super::super::pasted::keep("csrftoken=tok; _ga=GA1", &COOKIES),
            None
        );
        assert_eq!(super::super::pasted::keep("ory_session_x=", &COOKIES), None);
        // A header with no CSRF token is still a session.
        let session_only = super::super::pasted::keep("ory_session_x=abc", &COOKIES).unwrap();
        assert_eq!(csrf(&session_only), None);
    }

    /// The stream is one string however the page chopped it up, and a `]`
    /// inside it does not end the push.
    #[test]
    fn the_pushed_stream_is_read_through_its_own_strings() {
        let stream = flight_stream(&page(BUDGETS, 4));
        assert_eq!(stream, BUDGETS);

        // A closer that does not match, and a string that is never closed.
        assert_eq!(container_end("[1,2}", 0), None);
        assert_eq!(container_end(r#"[1,"unclosed"#, 0), None);
        assert_eq!(container_end(r#"[1,"a]b"]"#, 0), Some(9));
    }
}
