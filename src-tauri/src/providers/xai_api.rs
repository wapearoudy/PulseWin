//! The xAI developer platform: a team's prepaid credit, from xAI's documented
//! Management API.
//!
//! Not Grok — that is the consumer subscription, read by its own provider — and
//! nothing is shared between the two.
//!
//! `GET https://management-api.x.ai/v1/billing/teams/{team}/prepaid/balance`,
//! with a **management** key; an inference key is refused. The ledger is
//! inverted and in cents as a string: a $10 top-up reads `"-1000"`, so what is
//! left is the negated figure. It is the **posted** ledger, which xAI updates
//! when a billing cycle closes, so mid-cycle it can read higher than the
//! console's live remainder.
//!
//! **Two values, one field.** The balance is per team and the key does not name
//! one, so the credential takes `TeamID:ManagementKey`, the way Volcengine's
//! takes its key pair. Left out: the thirty-day spend history, which this app
//! has no place for.

use std::sync::Arc;

use serde_json::Value;

use super::{describe_reqwest_error, Ctx, FetchFuture, Provider};
use crate::model::ProviderUsage;

pub struct XaiApi;

impl Provider for XaiApi {
    fn id(&self) -> &'static str {
        "xai-api"
    }

    fn name(&self) -> &'static str {
        "xAI API"
    }

    /// The Management key **and** the team id it is paired with. The credential
    /// is `TeamID:ManagementKey`, and half of it builds a path that answers
    /// nothing — so a key with no team beside it is not a way in.
    fn is_configured(&self) -> bool {
        super::provider_key(self.id())
            .and_then(|text| credential(&text))
            .is_some()
    }

    fn fetch(&self, ctx: Arc<Ctx>) -> FetchFuture {
        Box::pin(async move { fetch_inner(ctx).await })
    }
}

/// The team and the key out of `TeamID:ManagementKey`. A team id that could
/// step out of the path is not one.
fn credential(text: &str) -> Option<(String, String)> {
    let (team, key) = text.split_once(':')?;
    let team = team.trim().to_string();
    let key = key.trim().to_string();

    if team.is_empty() || key.is_empty() || team.contains('/') || team == "." || team == ".." {
        return None;
    }
    Some((team, key))
}

fn endpoint(team: &str) -> String {
    format!("https://management-api.x.ai/v1/billing/teams/{team}/prepaid/balance")
}

async fn fetch_inner(ctx: Arc<Ctx>) -> ProviderUsage {
    const ID: &str = "xai-api";
    const NAME: &str = "xAI API";

    let credential_text = match super::provider_key(ID) {
        Some(key) => key,
        None => {
            return ProviderUsage::failed(
                ID,
                NAME,
                super::missing_key(ID, " and put TeamID:ManagementKey in it"),
            )
        }
    };

    // Not in the documented shape, so nothing is sent: it is not a key xAI
    // turned away, but it is the key field that needs fixing.
    let Some((team, key)) = credential(&credential_text) else {
        return ProviderUsage::failed(
            ID,
            NAME,
            "the credential must read TeamID:ManagementKey (a management key, not an inference key)",
        );
    };

    let response = ctx
        .client
        .get(endpoint(&team))
        .header("Authorization", format!("Bearer {key}"))
        .header("Accept", "application/json")
        .send()
        .await;

    let response = match response {
        Ok(r) => r,
        Err(e) => {
            return ProviderUsage::failed(
                ID,
                NAME,
                format!("request failed: {}", describe_reqwest_error(&e)),
            )
        }
    };

    let status = response.status();
    let body = match response.text().await {
        Ok(t) => t,
        Err(e) => return ProviderUsage::failed(ID, NAME, format!("cannot read body: {e}")),
    };

    if !status.is_success() {
        let hints: &[(u16, &str)] = &[
            (401, " — a management key is needed; an inference key is refused"),
            (403, " — a management key is needed; an inference key is refused"),
            (404, " — no such team, or the key cannot read its billing"),
            (429, " — rate limited, try again shortly"),
        ];
        return ProviderUsage::failed(ID, NAME, super::http_failure(status, hints));
    }

    let json: Value = match serde_json::from_str(&body) {
        Ok(v) => v,
        Err(e) => return ProviderUsage::failed(ID, NAME, format!("bad JSON: {e}")),
    };

    super::claude_code::debug_dump(ID, &json);
    reading(&json, &team)
}

fn reading(json: &Value, team: &str) -> ProviderUsage {
    const ID: &str = "xai-api";
    const NAME: &str = "xAI API";
    let Some(cents) = ledger_cents(json) else {
        return ProviderUsage::failed(ID, NAME, "no balance in response");
    };

    let dollars = -cents / 100.0;
    ProviderUsage::ok(
        ID,
        NAME,
        vec![super::balance_window("Balance", money(dollars))],
    )
    .with_account(Some(format!("team {team}")))
    .with_credit_remaining(dollars, "USD")
}

fn ledger_cents(json: &Value) -> Option<f64> {
    let raw = json.get("total")?.get("val")?.as_str()?.trim().to_string();
    // The original pins this to `^-?\d+(\.\d+)?$`: a plain decimal in cents,
    // and nothing that parses as a float but is not money (an exponent, `inf`,
    // a bare `+`).
    plain_decimal(&raw)
}

/// A plain decimal: optional sign, digits, optional single fractional part.
fn plain_decimal(text: &str) -> Option<f64> {
    let digits = text.strip_prefix('-').unwrap_or(text);
    if digits.is_empty() {
        return None;
    }

    let mut seen_dot = false;
    let mut seen_digit = false;
    for ch in digits.chars() {
        match ch {
            '0'..='9' => seen_digit = true,
            '.' if !seen_dot => seen_dot = true,
            _ => return None,
        }
    }
    if !seen_digit {
        return None;
    }

    text.parse::<f64>().ok().filter(|v| v.is_finite())
}

fn money(amount: f64) -> String {
    format!("{amount:.2} USD")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn inverted_ledger_cents_feed_numeric_credit_without_display_rounding() {
        let usage = reading(&json!({"total":{"val":"-1234.5"}}),"fixture-team");
        assert_eq!(usage.credit_remaining.as_ref().unwrap().amount,12.345);
        assert_eq!(usage.credit_remaining.as_ref().unwrap().currency,"USD");
        assert_eq!(reading(&json!({"total":{"val":"750"}}),"fixture-team").credit_remaining.unwrap().amount,-7.5);
    }

    #[test]
    fn splits_the_team_and_the_key() {
        let (team, key) = credential("team_01: xai-manager-key ").unwrap();
        assert_eq!(team, "team_01");
        assert_eq!(key, "xai-manager-key");
    }

    #[test]
    fn a_team_that_could_step_out_of_the_path_is_not_one() {
        assert!(credential("..:key").is_none());
        assert!(credential("a/b:key").is_none());
        assert!(credential(":key").is_none());
        assert!(credential("team:").is_none());
        assert!(credential("no-colon").is_none());
    }

    #[test]
    fn the_ledger_is_inverted_and_in_cents() {
        // A $10 top-up reads "-1000", so what is left is the negated figure.
        let reply = json!({ "total": { "val": "-1000" } });
        assert_eq!(ledger_cents(&reply), Some(-1000.0));
        assert_eq!(money(-(-1000.0) / 100.0), "10.00 USD");
    }

    #[test]
    fn a_spent_down_ledger_reads_positive() {
        let reply = json!({ "total": { "val": "250" } });
        let dollars = -ledger_cents(&reply).unwrap() / 100.0;
        assert_eq!(dollars, -2.5);
        assert_eq!(money(dollars), "-2.50 USD");
    }

    #[test]
    fn only_a_plain_decimal_is_money() {
        assert_eq!(plain_decimal("12.5"), Some(12.5));
        assert_eq!(plain_decimal("-12.5"), Some(-12.5));
        assert_eq!(plain_decimal("1e5"), None);
        assert_eq!(plain_decimal("inf"), None);
        assert_eq!(plain_decimal(""), None);
        assert_eq!(plain_decimal("."), None);
    }

    #[test]
    fn a_reply_that_is_not_money_is_left_unread() {
        assert!(ledger_cents(&json!({ "total": { "val": "not a number" } })).is_none());
        assert!(ledger_cents(&json!({ "total": {} })).is_none());
        assert!(ledger_cents(&json!({})).is_none());
    }

    #[test]
    fn the_endpoint_names_the_team() {
        assert_eq!(
            endpoint("team_01"),
            "https://management-api.x.ai/v1/billing/teams/team_01/prepaid/balance"
        );
    }
}
