//! Venice: the balance its API spends from — US dollars, or DIEM, the token
//! Venice's stakers are allotted each epoch.
//!
//! Read with an API key the user enters, from
//! `GET https://api.venice.ai/api/v1/billing/balance`.
//!
//! **Only the balance, never a percentage.** The reply's `canConsume` is a
//! yes-or-no about whether the next call will go through; the reference
//! implementation draws it as 100% or 0%, which is a health flag wearing a
//! percentage's clothes. The port shows the balance the reply states and draws
//! no ring. Neither is the DIEM epoch allocation drawn as an allowance: what is
//! left of it reads as a balance like the dollars do.

use std::sync::Arc;

use serde_json::Value;

use super::{describe_reqwest_error, Ctx, FetchFuture, Provider};
use crate::model::ProviderUsage;

const ENDPOINT: &str = "https://api.venice.ai/api/v1/billing/balance";

pub struct Venice;

impl Provider for Venice {
    fn id(&self) -> &'static str {
        "venice"
    }

    fn name(&self) -> &'static str {
        "Venice"
    }

    /// The key the fetch reads, and nothing else.
    fn is_configured(&self) -> bool {
        super::provider_key(self.id()).is_some()
    }

    fn fetch(&self, ctx: Arc<Ctx>) -> FetchFuture {
        Box::pin(async move { fetch_inner(ctx).await })
    }
}

async fn fetch_inner(ctx: Arc<Ctx>) -> ProviderUsage {
    const ID: &str = "venice";
    const NAME: &str = "Venice";

    let key = match super::provider_key(ID) {
        Some(key) => key,
        None => return ProviderUsage::failed(ID, NAME, super::missing_key(ID, "")),
    };

    let response = ctx
        .client
        .get(ENDPOINT)
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
            (401, " — the API key was refused"),
            (403, " — the API key was refused"),
            (429, " — rate limited, try again shortly"),
        ];
        return ProviderUsage::failed(ID, NAME, super::http_failure(status, hints));
    }

    let json: Value = match serde_json::from_str(&body) {
        Ok(v) => v,
        Err(e) => return ProviderUsage::failed(ID, NAME, format!("bad JSON: {e}")),
    };

    super::claude_code::debug_dump(ID, &json);

    reading(&json)
}

/// A figure the reply may write three ways, and the difference matters: a
/// missing one is a balance this account does not have, while a malformed one
/// is a reply that cannot be read at all.
#[derive(Debug, PartialEq)]
enum Figure {
    Missing,
    Value(f64),
    Malformed,
}

/// A number, or one written as a string; null or empty is missing.
fn figure(value: Option<&Value>) -> Figure {
    match value {
        None | Some(Value::Null) => Figure::Missing,
        Some(Value::Number(n)) => match n.as_f64() {
            Some(v) if v.is_finite() => Figure::Value(v),
            _ => Figure::Malformed,
        },
        Some(Value::String(text)) => {
            let trimmed = text.trim();
            if trimmed.is_empty() {
                return Figure::Missing;
            }
            match trimmed.parse::<f64>() {
                Ok(v) if v.is_finite() => Figure::Value(v),
                _ => Figure::Malformed,
            }
        }
        Some(_) => Figure::Malformed,
    }
}

/// The mapping, kept apart from the request so a fixture can drive it.
fn reading(json: &Value) -> ProviderUsage {
    const ID: &str = "venice";
    const NAME: &str = "Venice";

    // `canConsume` has to be a real boolean: it is the reply's own statement
    // that it is a balance reply at all, and it is never drawn.
    let can_consume = json.get("canConsume");
    if !matches!(can_consume, Some(Value::Bool(_))) {
        return ProviderUsage::failed(ID, NAME, "bad reply: no canConsume flag");
    }

    let Some(balances) = json.get("balances").and_then(|v| v.as_object()) else {
        return ProviderUsage::failed(ID, NAME, "bad reply: no balances");
    };

    match json.get("consumptionCurrency") {
        None | Some(Value::Null) | Some(Value::String(_)) => {}
        Some(_) => return ProviderUsage::failed(ID, NAME, "bad reply: unreadable currency"),
    }

    let usd = figure(balances.get("usd"));
    let diem = figure(balances.get("diem"));

    if usd == Figure::Malformed || diem == Figure::Malformed {
        return ProviderUsage::failed(ID, NAME, "bad reply: unreadable balance");
    }

    let usd = match usd {
        Figure::Value(value) => Some(value),
        _ => None,
    };
    let diem = match diem {
        Figure::Value(value) => Some(value),
        _ => None,
    };

    // The balance the account spends from, as the reply names it; failing a
    // name, dollars before DIEM.
    let spends_diem = json
        .get("consumptionCurrency")
        .and_then(|v| v.as_str())
        .map(|currency| currency.to_uppercase() == "DIEM")
        .unwrap_or(false);

    if spends_diem {
        if let Some(diem) = diem {
            return balance(tokens(diem));
        }
    }
    if let Some(usd) = usd {
        return balance(format!("{usd:.2} USD")).with_credit_remaining(usd, "USD");
    }
    if let Some(diem) = diem {
        return balance(tokens(diem));
    }

    ProviderUsage::failed(ID, NAME, "no balance in the reply")
}

fn balance(text: String) -> ProviderUsage {
    ProviderUsage::ok(
        "venice",
        "Venice",
        vec![super::balance_window("Balance", text)],
    )
}

/// DIEM is a token's symbol, the same in every language, so it follows the
/// figure untranslated.
fn tokens(amount: f64) -> String {
    format!("{amount:.2} DIEM")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn window_text(reply: &Value) -> Option<String> {
        reading(reply).windows[0].detail.clone()
    }

    #[test]
    fn reads_the_dollar_balance_and_draws_no_ring() {
        let usage = reading(&json!({
            "canConsume": true,
            "balances": { "usd": 12.5, "diem": 0 }
        }));
        assert_eq!(usage.windows.len(), 1);
        assert_eq!(usage.windows[0].label, "Balance");
        assert_eq!(usage.windows[0].detail.as_deref(), Some("12.50 USD"));
        assert_eq!(usage.credit_remaining.as_ref().unwrap().amount, 12.5);
        assert_eq!(usage.credit_remaining.as_ref().unwrap().currency, "USD");
        // Nobody reported a percentage, so none is invented.
        assert_eq!(usage.windows[0].percent_used, None);
    }

    /// `canConsume` is not a figure, whatever the reference implementation
    /// draws with it.
    #[test]
    fn the_consume_flag_never_becomes_a_percentage() {
        let usage = reading(&json!({ "canConsume": false, "balances": { "usd": 3.0 } }));
        assert_eq!(usage.windows[0].percent_used, None);
        assert_eq!(usage.windows[0].detail.as_deref(), Some("3.00 USD"));
    }

    #[test]
    fn the_spending_currency_decides_which_balance_is_shown() {
        let diem = json!({
            "canConsume": true,
            "consumptionCurrency": "DIEM",
            "balances": { "usd": 12.5, "diem": 400.0 }
        });
        assert_eq!(window_text(&diem).as_deref(), Some("400.00 DIEM"));
        assert!(reading(&diem).credit_remaining.is_none());

        // Without a name, dollars come first and DIEM is the fallback.
        let unnamed = json!({ "canConsume": true, "balances": { "diem": 400.0 } });
        assert_eq!(window_text(&unnamed).as_deref(), Some("400.00 DIEM"));
        assert!(reading(&unnamed).credit_remaining.is_none());
    }

    #[test]
    fn a_balance_written_as_a_string_is_a_figure() {
        let reply = json!({ "canConsume": true, "balances": { "usd": "8.25" } });
        assert_eq!(window_text(&reply).as_deref(), Some("8.25 USD"));
    }

    #[test]
    fn a_missing_balance_is_missing_and_a_malformed_one_is_unreadable() {
        // Missing is fine as long as something else is there.
        let one_missing = json!({ "canConsume": true, "balances": { "usd": null, "diem": 5.0 } });
        assert_eq!(window_text(&one_missing).as_deref(), Some("5.00 DIEM"));

        // Malformed is not.
        let malformed = json!({ "canConsume": true, "balances": { "usd": "a lot", "diem": 5.0 } });
        assert!(reading(&malformed).error.is_some());

        let boolean = json!({ "canConsume": true, "balances": { "usd": true } });
        assert!(reading(&boolean).error.is_some());
    }

    #[test]
    fn a_reply_with_no_balance_at_all_is_no_reading() {
        assert!(reading(&json!({ "canConsume": true, "balances": {} }))
            .error
            .is_some());
        assert!(reading(&json!({ "balances": { "usd": 1.0 } })).error.is_some());
        assert!(reading(&json!({ "canConsume": true })).error.is_some());
        assert!(reading(&json!({})).error.is_some());
        // A currency that is present and not a string is unreadable.
        assert!(reading(&json!({
            "canConsume": true, "consumptionCurrency": 7, "balances": { "usd": 1.0 }
        }))
        .error
        .is_some());
    }
}
