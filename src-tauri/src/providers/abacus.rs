//! Abacus AI's ChatLLM / RouteLLM plan: one allowance of compute credits,
//! with the used amount and the total both reported, and the next billing
//! date as its reset.
//!
//! Read from the two endpoints Abacus's own web app calls:
//! `GET https://apps.abacus.ai/api/_getOrganizationComputePoints` for the
//! credits, and `POST /api/_getBillingInfo` for the next billing date and the
//! plan's name. The second is optional: without it the credits still show, with
//! no reset.
//!
//! **The credential is a pasted cookie.** The original imports the session from
//! the browser the reader signed in with (`Auth/BrowserCookies.swift`), which
//! on macOS reads a Chromium cookie store through the login keychain; PulseWin
//! cannot, so the `Cookie` header copied out of a signed-in request is the
//! credential here instead.
//!
//! **No length is claimed.** Nothing in either reply says how long a cycle is;
//! the billing date says only when this one ends.

use std::sync::Arc;

use serde_json::Value;

use super::{describe_reqwest_error, percent_from_fraction, Ctx, FetchFuture, Provider};
use crate::model::{parse_reset, ProviderUsage, UsageWindow};

const ID: &str = "abacus";
const NAME: &str = "Abacus AI";

const POINTS_URL: &str = "https://apps.abacus.ai/api/_getOrganizationComputePoints";
const BILLING_URL: &str = "https://apps.abacus.ai/api/_getBillingInfo";

/// Abacus's session sits under any one of these names; the reference
/// implementation accepts each of them and names none as the one. Any one is
/// enough, and every one present is kept.
const COOKIES: [&str; 1] = ["sessionid|session_id|session_token|auth_token|access_token"];

pub struct Abacus;

impl Provider for Abacus {
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
    let Some(cookie) = super::pasted::cookie(ID) else {
        return ProviderUsage::failed(ID, NAME, super::pasted::needed(ID, "Abacus session cookie"));
    };
    let Some(cookie) = super::pasted::keep(&cookie, &COOKIES) else {
        return ProviderUsage::failed(ID, NAME, super::pasted::needed(ID, "Abacus session cookie"));
    };

    let points = match send(&ctx, POINTS_URL, reqwest::Method::GET, &cookie).await {
        Ok(body) => body,
        Err(error) => return ProviderUsage::failed(ID, NAME, error),
    };

    // The billing date is extra: a failure there costs only the reset.
    let billing = send(&ctx, BILLING_URL, reqwest::Method::POST, &cookie)
        .await
        .ok();

    reading(&points, billing.as_deref())
}

/// One request with the pasted header, and what its status means.
///
/// Made on the client that refuses redirects: a `Cookie` header set by hand
/// rides a redirect to whatever host it names, so a signed-out session has to
/// arrive as the 3xx to the sign-in page rather than be followed with the
/// session attached.
async fn send(ctx: &Ctx, url: &str, method: reqwest::Method, cookie: &str) -> Result<String, String> {
    let post = method == reqwest::Method::POST;
    let mut request = ctx
        .gateway_client
        .request(method, url)
        .header("Cookie", cookie)
        .header("Accept", "application/json");

    if post {
        request = request
            .header("Content-Type", "application/json")
            .body("{}");
    }

    let response = match request.send().await {
        Ok(response) => response,
        Err(e) => return Err(format!("request failed: {}", describe_reqwest_error(&e))),
    };

    let status = response.status();
    let body = match response.text().await {
        Ok(body) => body,
        Err(e) => return Err(format!("cannot read body: {e}")),
    };

    if status.is_redirection() {
        return Err("HTTP 3xx — the session has expired; copy a fresh cookie".to_string());
    }
    if !status.is_success() {
        let hints: &[(u16, &str)] = &[
            (401, " — the session has expired; copy a fresh cookie from abacus.ai"),
            (403, " — the session has expired; copy a fresh cookie from abacus.ai"),
            (429, " — rate limited, try again shortly"),
        ];
        return Err(super::http_failure(status, hints));
    }

    Ok(body)
}

/// The mapping, kept apart from the requests so a fixture can drive it.
fn reading(points: &str, billing: Option<&str>) -> ProviderUsage {
    let points: Value = match serde_json::from_str(points) {
        Ok(value) => value,
        Err(e) => return ProviderUsage::failed(ID, NAME, format!("bad JSON: {e}")),
    };

    let (total, left) = match payload(&points) {
        Ok(result) => (figure(&result, "totalComputePoints"), figure(&result, "computePointsLeft")),
        Err(reason) => return ProviderUsage::failed(ID, NAME, reason),
    };

    // Both halves are reported; neither is ever assumed.
    let (Some(total), Some(left)) = (total, left) else {
        return ProviderUsage::failed(ID, NAME, "no limits reported in the reply");
    };
    if !total.is_finite() || !left.is_finite() || total <= 0.0 {
        return ProviderUsage::failed(ID, NAME, "no limits reported in the reply");
    }

    let plan = billing
        .and_then(|body| serde_json::from_str::<Value>(body).ok())
        .and_then(|body| payload(&body).ok())
        .and_then(|result| {
            result
                .get("currentTier")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|tier| !tier.is_empty())
                .map(str::to_string)
        });

    let reset = billing
        .and_then(|body| serde_json::from_str::<Value>(body).ok())
        .and_then(|body| payload(&body).ok())
        .and_then(|result| result.get("nextBillingDate").cloned())
        .and_then(|value| parse_reset(&value));

    let used = (total - left).max(0.0) / total;
    let window = UsageWindow::new("Credits", Some(percent_from_fraction(used))).with_reset(reset);

    ProviderUsage::ok(ID, NAME, vec![window]).with_plan(plan)
}

/// The `{ success, result, error }` envelope both endpoints answer, unwrapped.
fn payload(body: &Value) -> Result<Value, String> {
    let success = body.get("success").and_then(Value::as_bool);
    let result = body.get("result").filter(|value| !value.is_null());

    if success == Some(true) {
        if let Some(result) = result {
            return Ok(result.clone());
        }
    }

    // A refused session can come back as a 200 that says so in `error`.
    if success == Some(false) {
        let error = body
            .get("error")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_lowercase();
        let signed_out = [
            "expired",
            "session",
            "login",
            "authenticate",
            "unauthorized",
            "unauthenticated",
            "forbidden",
        ]
        .iter()
        .any(|phrase| error.contains(phrase));
        return Err(if signed_out {
            "the session has expired; copy a fresh cookie from abacus.ai".to_string()
        } else {
            "the service reported an error".to_string()
        });
    }

    Err("the reply could not be read".to_string())
}

/// A figure that may arrive as a number or as a numeric string.
fn figure(value: &Value, key: &str) -> Option<f64> {
    super::dig_number(value.get(key)).filter(|figure| figure.is_finite())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn body(result: Value) -> String {
        json!({ "success": true, "result": result }).to_string()
    }

    #[test]
    fn reads_the_credits_and_the_billing_date() {
        let points = body(json!({ "totalComputePoints": 5000, "computePointsLeft": 1250 }));
        let billing = body(json!({
            "nextBillingDate": "2026-11-01T00:00:00Z",
            "currentTier": "Pro"
        }));

        let usage = reading(&points, Some(&billing));
        assert_eq!(usage.windows[0].label, "Credits");
        // 3,750 of 5,000 spent is 75%.
        assert_eq!(usage.windows[0].percent_used, Some(75.0));
        assert_eq!(
            usage.windows[0].resets_at.as_deref(),
            Some("2026-11-01T00:00:00Z")
        );
        assert_eq!(usage.plan.as_deref(), Some("Pro"));
    }

    #[test]
    fn the_credits_stand_without_the_billing_reply() {
        let points = body(json!({ "totalComputePoints": "100", "computePointsLeft": "100" }));
        let usage = reading(&points, None);
        assert_eq!(usage.windows[0].percent_used, Some(0.0));
        assert_eq!(usage.windows[0].resets_at, None);
        assert_eq!(usage.plan, None);
    }

    #[test]
    fn a_spent_allowance_reads_full_and_more_left_than_granted_reads_empty() {
        let spent = body(json!({ "totalComputePoints": 10, "computePointsLeft": 0 }));
        assert_eq!(reading(&spent, None).windows[0].percent_used, Some(100.0));

        let over = body(json!({ "totalComputePoints": 10, "computePointsLeft": 25 }));
        assert_eq!(reading(&over, None).windows[0].percent_used, Some(0.0));
    }

    #[test]
    fn half_a_figure_or_no_total_is_no_reading() {
        let no_left = body(json!({ "totalComputePoints": 5000 }));
        assert!(reading(&no_left, None).error.is_some());

        // A total of zero is nothing to measure against, not a full ring.
        let no_total = body(json!({ "totalComputePoints": 0, "computePointsLeft": 0 }));
        assert!(reading(&no_total, None).error.is_some());
    }

    #[test]
    fn a_refused_session_in_a_good_http_status_is_said_so() {
        let refused = json!({ "success": false, "error": "Session expired, please log in" }).to_string();
        let usage = reading(&refused, None);
        assert!(usage.error.unwrap().contains("expired"));
    }

    #[test]
    fn a_reply_that_is_not_one_is_unreadable() {
        assert!(reading("not json", None).error.is_some());
        assert!(reading(&json!({ "result": {} }).to_string(), None).error.is_some());
        assert!(reading(&json!({ "success": true }).to_string(), None).error.is_some());
    }
}
