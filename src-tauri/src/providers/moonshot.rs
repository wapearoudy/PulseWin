//! Moonshot, the Kimi Open Platform API: the money left on the account. Not
//! Kimi Code, which is a subscription with windows and its own provider.
//!
//! Read with an API key the user enters, from the platform's own balance
//! endpoint, `GET /v1/users/me/balance`. There are two platforms with two
//! hosts, and a key is issued by one of them:
//!
//! - international, `api.moonshot.ai`, priced in US dollars;
//! - China mainland, `api.moonshot.cn`, priced in yuan.
//!
//! **Which one is found by asking, not chosen in Settings** — the original has
//! no region picker for a profiled provider. The international host is asked
//! first; a key it refuses is asked of the China host, which is Moonshot's too.
//! Whichever accepted it is remembered for the rest of the launch, so a China
//! key is not offered to the other host every refresh. The currency follows the
//! host that answered: the reply itself names none.
//!
//! The platform reports a balance and no allowance, so there is no ring.

use std::collections::HashMap;
use std::sync::Arc;

use serde_json::Value;
use tokio::sync::Mutex;

use super::{describe_reqwest_error, Ctx, FetchFuture, Provider};
use crate::credentials;
use crate::model::ProviderUsage;

/// The host each key was last accepted by, for this launch only.
static ACCEPTED: Mutex<Option<HashMap<String, Region>>> = Mutex::const_new(None);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Region {
    International,
    China,
}

impl Region {
    const ALL: [Region; 2] = [Region::International, Region::China];

    fn endpoint(self) -> &'static str {
        match self {
            Region::International => "https://api.moonshot.ai/v1/users/me/balance",
            Region::China => "https://api.moonshot.cn/v1/users/me/balance",
        }
    }

    /// What the platform prices in. Stated by the region, not the reply.
    fn currency(self) -> &'static str {
        match self {
            Region::International => "USD",
            Region::China => "CNY",
        }
    }
}

pub struct Moonshot;

impl Provider for Moonshot {
    fn id(&self) -> &'static str {
        "moonshot"
    }

    fn name(&self) -> &'static str {
        "Moonshot"
    }

    /// The key the fetch reads, and nothing else. Which of the two platforms it
    /// belongs to is answered by the fetch, not here.
    fn is_configured(&self) -> bool {
        super::provider_key(self.id()).is_some()
    }

    fn fetch(&self, ctx: Arc<Ctx>) -> FetchFuture {
        Box::pin(async move { fetch_inner(ctx).await })
    }
}

async fn fetch_inner(ctx: Arc<Ctx>) -> ProviderUsage {
    const ID: &str = "moonshot";
    const NAME: &str = "Moonshot";

    let key = match super::provider_key(ID) {
        Some(key) => key,
        None => return ProviderUsage::failed(ID, NAME, super::missing_key(ID, "")),
    };

    // The host that took this key last time first, then the other.
    let known = {
        let cache = ACCEPTED.lock().await;
        cache.as_ref().and_then(|map| map.get(&key).copied())
    };

    let order: Vec<Region> = match known {
        Some(first) => std::iter::once(first)
            .chain(Region::ALL.into_iter().filter(|r| *r != first))
            .collect(),
        None => Region::ALL.to_vec(),
    };

    for region in order {
        let response = ctx
            .client
            .get(region.endpoint())
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

        // A redirect is never followed and is a sign-in page for every service
        // here, so it reads as the credential being turned away — the same
        // classification `ProfileHTTP` makes.
        if !status.is_success() {
            match status.as_u16() {
                300..=399 | 401 | 403 => {
                    // Refused by one platform: it may be the other's key.
                    continue;
                }
                429 => return ProviderUsage::failed(ID, NAME, "rate limited, try again shortly"),
                _ => {
                    return ProviderUsage::failed(ID, NAME, super::http_failure(status, &[]));
                }
            }
        }

        let json: Value = match serde_json::from_str(&body) {
            Ok(v) => v,
            Err(e) => return ProviderUsage::failed(ID, NAME, format!("bad JSON: {e}")),
        };

        super::claude_code::debug_dump(ID, &json);

        return match reading(&json, region) {
            Ok(usage) => {
                let mut cache = ACCEPTED.lock().await;
                cache.get_or_insert_with(HashMap::new).insert(key.clone(), region);
                usage.with_account(Some(format!("Moonshot {}", region.currency())))
            }
            Err(e) => ProviderUsage::failed(ID, NAME, e),
        };
    }

    ProviderUsage::failed(
        ID,
        NAME,
        "the API key was refused by both the international and the mainland platform",
    )
}

/// The envelope says whether the call worked; the money is a number.
///
/// Kept as reported, below zero included: an account in deficit is one somebody
/// should see, not one to round up to nothing.
fn reading(json: &Value, region: Region) -> Result<ProviderUsage, String> {
    const ID: &str = "moonshot";
    const NAME: &str = "Moonshot";

    let code = json.get("code").and_then(|v| v.as_i64());
    let status = json.get("status").and_then(|v| v.as_bool());
    let (Some(code), Some(status)) = (code, status) else {
        return Err("bad reply: no status envelope".to_string());
    };

    if code != 0 || !status {
        return Err(format!("the service refused the request (code {code})"));
    }

    let available = credentials::dig_f64(json, "data.available_balance")
        .filter(|v| v.is_finite())
        .ok_or("no balance in response")?;

    let money = format!("{available:.2} {}", region.currency());
    Ok(ProviderUsage::ok(ID, NAME, vec![super::balance_window("Balance", money)]).with_credit_remaining(available, region.currency()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn reads_the_available_balance() {
        let payload = json!({
            "code": 0, "status": true,
            "data": { "available_balance": 95.5, "voucher_balance": 0, "cash_balance": 95.5 }
        });
        let usage = reading(&payload, Region::International).unwrap();
        assert_eq!(usage.credit_remaining.as_ref().unwrap().amount, 95.5);
        assert_eq!(usage.credit_remaining.as_ref().unwrap().currency, "USD");
        assert_eq!(reading(&payload, Region::China).unwrap().credit_remaining.unwrap().currency, "CNY");
        assert_eq!(usage.windows.len(), 1);
        assert_eq!(usage.windows[0].label, "Balance");
        // No allowance and no period, so no fraction — the money is the reading.
        assert_eq!(usage.windows[0].percent_used, None);
        assert_eq!(usage.windows[0].detail.as_deref(), Some("95.50 USD"));
    }

    #[test]
    fn the_currency_follows_the_host_that_answered() {
        let payload = json!({ "code": 0, "status": true, "data": { "available_balance": 30.0 } });
        let usage = reading(&payload, Region::China).unwrap();
        assert_eq!(usage.windows[0].detail.as_deref(), Some("30.00 CNY"));
    }

    #[test]
    fn an_overdrawn_account_is_a_reading_not_a_zero() {
        let payload = json!({ "code": 0, "status": true, "data": { "available_balance": -4.25 } });
        let usage = reading(&payload, Region::International).unwrap();
        assert_eq!(usage.windows[0].detail.as_deref(), Some("-4.25 USD"));
    }

    #[test]
    fn a_non_zero_envelope_code_is_the_services_refusal() {
        let payload = json!({ "code": 401, "status": false, "data": {} });
        assert!(reading(&payload, Region::International).is_err());
    }

    #[test]
    fn a_reply_with_no_balance_is_unreadable() {
        let payload = json!({ "code": 0, "status": true, "data": {} });
        assert!(reading(&payload, Region::International).is_err());
        assert!(reading(&json!({}), Region::International).is_err());
    }

    #[test]
    fn the_two_hosts_are_the_two_platforms() {
        assert_eq!(Region::International.endpoint(), "https://api.moonshot.ai/v1/users/me/balance");
        assert_eq!(Region::China.endpoint(), "https://api.moonshot.cn/v1/users/me/balance");
    }
}
