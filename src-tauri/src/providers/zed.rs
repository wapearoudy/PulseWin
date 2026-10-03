//! Zed, the editor: its edit-prediction allowance and its token spend against
//! the spending limit the account set.
//!
//! **Read with a browser session, not the editor's login.** The editor keeps
//! its credential in the macOS Keychain, and reading that item is a Keychain
//! prompt PulseWin cannot make; `zed.dev`'s own billing page has a signed-in
//! session instead, and the `Cookie` header copied out of a request to it is
//! the credential here. Signing in only inside the editor does not create one;
//! signing in at `zed.dev` in a browser does — which is exactly why the
//! original reads the browser rather than the app.
//!
//! `GET https://cloud.zed.dev/frontend/billing/usage`, the page's own frontend
//! call, undocumented. The shape is second-hand — taken from the original and
//! its fixture, not from a captured reply.
//!
//! **A limit the account does not have is left off**: unlimited predictions, or
//! no spending limit, draw nothing rather than a ring at zero. Nothing states a
//! period either, so neither row claims a length — the two are sorted by the
//! month their plans turn over on, which is a sort key and not a reading.

use std::sync::Arc;

use serde_json::Value;

use super::{describe_reqwest_error, percent_from_fraction, Ctx, FetchFuture, Provider};
use crate::model::{ProviderUsage, UsageWindow};

const ID: &str = "zed";
const NAME: &str = "Zed";

const ENDPOINT: &str = "https://cloud.zed.dev/frontend/billing/usage";

/// The one cookie `zed.dev` signs in with, and the one that has to be there.
const COOKIES: [&str; 1] = ["zed.session"];

pub struct Zed;

impl Provider for Zed {
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
        return ProviderUsage::failed(ID, NAME, super::pasted::needed(ID, "zed.dev session cookie"));
    };

    // The client that refuses redirects, so a signed-out session arrives as the
    // 3xx to the sign-in page rather than being carried there.
    let response = ctx
        .gateway_client
        .get(ENDPOINT)
        .header("Cookie", cookie)
        .header("Accept", "application/json")
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
            "HTTP 3xx — the session has expired; copy a fresh cookie from zed.dev",
        );
    }
    if !status.is_success() {
        let hints: &[(u16, &str)] = &[
            (401, " — the session has expired; copy a fresh cookie from zed.dev"),
            (403, " — the session has expired; copy a fresh cookie from zed.dev"),
            (429, " — rate limited, try again shortly"),
        ];
        return ProviderUsage::failed(ID, NAME, super::http_failure(status, hints));
    }

    reading(&body)
}

/// The mapping, kept apart from the request so a fixture can drive it.
fn reading(body: &str) -> ProviderUsage {
    let root: Value = match serde_json::from_str(body) {
        Ok(value @ Value::Object(_)) => value,
        Ok(_) => return ProviderUsage::failed(ID, NAME, "the reply is not an object"),
        Err(_) => return ProviderUsage::failed(ID, NAME, "the reply could not be read"),
    };

    let Some(usage) = root.get("current_usage").filter(|value| value.is_object()) else {
        return ProviderUsage::failed(ID, NAME, "the reply could not be read");
    };

    let mut windows: Vec<UsageWindow> = Vec::new();

    // So many predictions in the account's allowance, and so many used. The
    // reply names no period; Zed's plans count them by the month, so the row is
    // named for what it counts and not for a length.
    if let Some(predictions) = usage.get("edit_predictions") {
        let used = figure(predictions.get("used"));
        let limit = prediction_limit(predictions.get("limit"));
        if let (Some(used), Some(limit)) = (used, limit) {
            if limit > 0.0 {
                windows.push(UsageWindow::new(
                    "Monthly · Edit Predictions",
                    Some(percent_from_fraction(used / limit)),
                ));
            }
        }
    }

    // Token spend against the spending limit, both in cents. No limit set is
    // spend with nothing to measure it against, and is left off.
    if let Some(spend) = usage.get("token_spend") {
        let spent = figure(spend.get("spend_in_cents"));
        let limit = figure(spend.get("limit_in_cents"));
        if let (Some(spent), Some(limit)) = (spent, limit) {
            if limit > 0.0 {
                windows.push(UsageWindow::new(
                    "Spend",
                    Some(percent_from_fraction(spent / limit)),
                ));
            }
        }
    }

    if windows.is_empty() {
        return ProviderUsage::failed(ID, NAME, "no limits reported in the reply");
    }

    let plan = root.get("plan").and_then(Value::as_str).and_then(plan_name);
    ProviderUsage::ok(ID, NAME, windows).with_plan(plan)
}

/// A reported figure: a finite number, not negative. Anything else — a boolean,
/// a word, a null — is left off rather than read as zero.
fn figure(value: Option<&Value>) -> Option<f64> {
    super::dig_number(value).filter(|figure| figure.is_finite() && *figure >= 0.0)
}

/// A number, or `{ "limited": n }`. `"unlimited"` and null are no limit, and
/// neither is a word this build cannot read.
fn prediction_limit(value: Option<&Value>) -> Option<f64> {
    match value? {
        Value::Object(limited) => figure(limited.get("limited")),
        other => figure(Some(other)),
    }
}

/// `zed_pro_trial` → "Zed Pro Trial". A product's own name, so it is not
/// translated.
fn plan_name(raw: &str) -> Option<String> {
    let words: Vec<String> = raw
        .split(|c| c == '_' || c == ' ')
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
        .collect();

    if words.is_empty() {
        None
    } else {
        Some(words.join(" "))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Second-hand, from the original's fixture: the reply CodexBar's Zed
    /// plugin describes, which is what the route is read against.
    fn fixture() -> String {
        r#"{"plan":"zed_pro","current_usage":{"token_spend":{"spend_in_cents":250,"limit_in_cents":1000},"edit_predictions":{"used":12,"limit":100}}}"#
            .to_string()
    }

    #[test]
    fn reads_the_predictions_and_the_spend_as_reported() {
        let usage = reading(&fixture());
        let labels: Vec<&str> = usage.windows.iter().map(|w| w.label.as_str()).collect();
        assert_eq!(labels, vec!["Monthly · Edit Predictions", "Spend"]);
        assert_eq!(usage.windows[0].percent_used, Some(12.0));
        assert_eq!(usage.windows[1].percent_used, Some(25.0));
        // No period is stated, and nothing says when either turns over.
        assert!(usage.windows.iter().all(|w| w.resets_at.is_none()));
        assert_eq!(usage.plan.as_deref(), Some("Zed Pro"));
    }

    #[test]
    fn a_limit_written_as_an_object_reads_the_same() {
        let limited = r#"{"plan":"zed_pro_trial","current_usage":{"edit_predictions":{"used":10,"limit":{"limited":20}}}}"#;
        let usage = reading(limited);
        assert_eq!(usage.windows.len(), 1);
        assert_eq!(usage.windows[0].percent_used, Some(50.0));
        assert_eq!(usage.plan.as_deref(), Some("Zed Pro Trial"));
    }

    #[test]
    fn an_overspend_reads_full_rather_than_past_the_ring() {
        let over = r#"{"plan":"zed_pro","current_usage":{"token_spend":{"spend_in_cents":1500,"limit_in_cents":1000}}}"#;
        assert_eq!(reading(over).windows[0].percent_used, Some(100.0));
    }

    #[test]
    fn unlimited_predictions_and_no_spending_limit_draw_nothing() {
        for reply in [
            r#"{"plan":"zed_pro","current_usage":{"token_spend":{"spend_in_cents":250,"limit_in_cents":null},"edit_predictions":{"used":12,"limit":"unlimited"}}}"#,
            r#"{"plan":"zed_pro","current_usage":{"token_spend":{"spend_in_cents":250},"edit_predictions":{"used":12,"limit":null}}}"#,
        ] {
            assert!(reading(reply).error.is_some(), "read {reply}");
        }
    }

    #[test]
    fn a_figure_that_is_not_one_is_left_off() {
        for reply in [
            r#"{"current_usage":{"token_spend":{"spend_in_cents":-1,"limit_in_cents":1000},"edit_predictions":{"used":true,"limit":100}}}"#,
            r#"{"current_usage":{"token_spend":{"spend_in_cents":250,"limit_in_cents":-1},"edit_predictions":{"used":12,"limit":0}}}"#,
        ] {
            assert!(reading(reply).error.is_some(), "read {reply}");
        }
    }

    #[test]
    fn a_reply_that_cannot_be_read() {
        for reply in ["{}", "[]", "<html>login</html>", r#"{"current_usage":[]}"#] {
            assert!(reading(reply).error.is_some(), "read {reply}");
        }
    }

    /// Only the session cookie is kept from the browser.
    #[test]
    fn only_the_session_cookie_is_kept() {
        let kept = super::super::pasted::keep("_ga=1; zed.session=abc; other=2", &COOKIES);
        assert_eq!(kept.as_deref(), Some("zed.session=abc"));
        assert_eq!(super::super::pasted::keep("_ga=1", &COOKIES), None);
    }

    #[test]
    fn plan_names_are_the_products_own() {
        assert_eq!(plan_name("zed_pro").as_deref(), Some("Zed Pro"));
        assert_eq!(plan_name("zed_pro_trial").as_deref(), Some("Zed Pro Trial"));
        assert_eq!(plan_name("zed__pro").as_deref(), Some("Zed Pro"));
        assert_eq!(plan_name("   "), None);
    }
}
