//! DeepSeek's prepaid balance.
//!
//! One documented route, `GET https://api.deepseek.com/user/balance`, reached
//! with a key the user pastes into Settings. Unlike most of the original's
//! providers this is not an undocumented account endpoint borrowed from a
//! product's own UI — it is in DeepSeek's published API reference, alongside
//! chat completions:
//!
//! ```json
//! { "is_available": true,
//!   "balance_infos": [ { "currency": "CNY", "total_balance": "110.00",
//!                        "granted_balance": "10.00",
//!                        "topped_up_balance": "100.00" } ] }
//! ```
//!
//! **There is no allowance, no window, no reset and no spend history** — not in
//! this reply and not anywhere else in the API. Every other provider carries at
//! least one percentage; this one reports money and stops. So the denominator
//! behind the ring has to come from somewhere, and `Basis` below is the
//! enumeration of the only three places it can: something PulseWin watched,
//! nothing at all, or a figure the user typed. Which is in force is the user's
//! choice (`PULSEWIN_DEEPSEEK_BASIS`), defaulting to the measured one.
//!
//! Every figure arrives as a **string**, including the money, and is parsed
//! here so nothing downstream has to know that.

use std::path::PathBuf;
use std::sync::Arc;

use serde_json::Value;

use super::{describe_reqwest_error, percent_from_fraction, Ctx, FetchFuture, Provider};
use crate::credentials::{self, config_relative};
use crate::model::ProviderUsage;

const BALANCE_URL: &str = "https://api.deepseek.com/user/balance";

/// Where the watched peaks live. The original keeps them beside its other
/// state (`PulseStorage`); PulseWin has no such directory yet, so they sit
/// with the rest of this port's per-provider files.
fn baseline_path() -> Option<PathBuf> {
    config_relative(&["PulseWin", "deepseek-baseline.json"])
        .into_iter()
        .next()
}

pub struct DeepSeek;

impl Provider for DeepSeek {
    fn id(&self) -> &'static str {
        "deepseek"
    }

    fn name(&self) -> &'static str {
        "DeepSeek"
    }

    /// The key the fetch reads. The basis, budget and currency overrides this
    /// provider also honours are settings on top of a key, never a way in
    /// without one.
    fn is_configured(&self) -> bool {
        super::provider_key(self.id()).is_some()
    }

    fn fetch(&self, ctx: Arc<Ctx>) -> FetchFuture {
        Box::pin(async move { fetch_inner(ctx).await })
    }
}

/// How the ring gets a denominator. The original's `BalanceBasis`, and there
/// are exactly three because there are only three places one can come from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Basis {
    /// The highest balance PulseWin has seen since it last went up. Nobody has
    /// to type anything, and the number is one this app **watched**.
    SinceTopUp,
    /// No denominator at all: the card shows the money, not a fraction.
    BalanceOnly,
    /// A figure the user considers a full tank.
    Budget,
}

impl Basis {
    fn parse(raw: &str) -> Option<Self> {
        match raw.trim().to_lowercase().replace(['_', ' '], "-").as_str() {
            "since-top-up" | "sincetopup" | "peak" => Some(Self::SinceTopUp),
            "balance-only" | "balanceonly" | "none" => Some(Self::BalanceOnly),
            "budget" | "my-budget" => Some(Self::Budget),
            _ => None,
        }
    }
}

async fn fetch_inner(ctx: Arc<Ctx>) -> ProviderUsage {
    const ID: &str = "deepseek";
    const NAME: &str = "DeepSeek";

    let key = match super::provider_key(ID) {
        Some(key) => key,
        None => return ProviderUsage::failed(ID, NAME, super::missing_key(ID, "")),
    };

    let response = ctx
        .client
        .get(BALANCE_URL)
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

    let purse = match choose_purse(&purses(&json), configured_currency().as_deref()) {
        Some(p) => p,
        None => return ProviderUsage::failed(ID, NAME, "no balance in response"),
    };

    // The mark is advanced on every reading, whichever basis is in force:
    // switching to "since top-up" later should find a peak already there rather
    // than start over from whatever the balance happens to be that afternoon.
    let peak = advance_baseline(&purse.currency, purse.total);

    let windows = windows_for(
        &purse,
        configured_basis(),
        configured_budget(),
        peak,
        is_available(&json),
    );

    ProviderUsage::ok(ID, NAME, windows)
        .with_credit_remaining(purse.total,purse.currency.clone())
        .with_account(Some(money_text(purse.total, &purse.currency)))
}

fn configured_basis() -> Basis {
    credentials::env_override("deepseek", "basis")
        .and_then(|raw| Basis::parse(&raw))
        .unwrap_or(Basis::SinceTopUp)
}

fn configured_budget() -> Option<f64> {
    credentials::env_override("deepseek", "budget").and_then(|raw| raw.parse::<f64>().ok())
}

/// Which currency the ring follows. Nil takes the first the reply lists that
/// has any money in it.
fn configured_currency() -> Option<String> {
    credentials::env_override("deepseek", "currency")
}

/// One currency's money, with the strings turned into numbers.
#[derive(Debug, Clone, PartialEq)]
struct Purse {
    currency: String,
    /// Granted plus topped up.
    total: f64,
    granted: Option<f64>,
    topped_up: Option<f64>,
}

/// DeepSeek's own word for whether this account can still make calls.
fn is_available(json: &Value) -> Option<bool> {
    json.get("is_available").and_then(|v| v.as_bool())
}

/// **The reply is an array**, and an account can hold both CNY and USD. They
/// cannot be added together and PulseWin will not pick a "main" one by
/// comparing figures across currencies — ¥100 against $10 is not a comparison.
/// So: the user's choice if they made one, else the first entry with money in
/// it, else the first entry at all. The card lists every currency regardless of
/// which one the ring follows.
fn choose_purse(purses: &[Purse], preferred: Option<&str>) -> Option<Purse> {
    if purses.is_empty() {
        return None;
    }
    if let Some(currency) = preferred {
        if let Some(chosen) = purses.iter().find(|p| p.currency == currency) {
            return Some(chosen.clone());
        }
    }
    purses
        .iter()
        .find(|p| p.total > 0.0)
        .or_else(|| purses.first())
        .cloned()
}

fn purses(json: &Value) -> Vec<Purse> {
    let Some(infos) = json.get("balance_infos").and_then(|v| v.as_array()) else {
        return Vec::new();
    };

    infos
        .iter()
        .filter_map(|info| {
            let currency = info.get("currency").and_then(|v| v.as_str())?;
            if currency.is_empty() {
                return None;
            }
            let total = money(info.get("total_balance"))?;
            Some(Purse {
                currency: currency.to_string(),
                total,
                granted: money(info.get("granted_balance")),
                topped_up: money(info.get("topped_up_balance")),
            })
        })
        .collect()
}

/// Money arrives as a string. A field that is absent or unparseable is
/// **absent**, not zero — a balance read as zero is a full red ring and a
/// notification saying the account is spent.
fn money(value: Option<&Value>) -> Option<f64> {
    let raw = match value? {
        Value::String(s) => s.trim().to_string(),
        Value::Number(n) => n.to_string(),
        _ => return None,
    };
    if raw.is_empty() {
        return None;
    }
    raw.parse::<f64>().ok().filter(|v| v.is_finite())
}

/// At most one window, because there is at most one denominator.
///
/// `BalanceOnly` produces none at all and the card shows the money in place of
/// a percentage. The other two produce a single `Balance` row whose detail says
/// where its denominator came from, because that is the whole question a reader
/// has about it.
///
/// **No length and no reset**, ever: prepaid credit does not turn over.
///
/// The original also carries DeepSeek's own `is_available` flag as the window's
/// `isExhausted`, which this port's `UsageWindow` has no field for — so the
/// verdict is written into the detail rather than dropped. A zero balance is
/// arithmetic, not the provider's word, and never reaches it.
fn windows_for(
    purse: &Purse,
    basis: Basis,
    budget: Option<f64>,
    peak: f64,
    is_available: Option<bool>,
) -> Vec<crate::model::UsageWindow> {
    let fraction = match basis {
        Basis::BalanceOnly => None,
        Basis::SinceTopUp => used_fraction(purse.total, peak),
        // Finite, not merely positive: an infinite denominator makes the
        // fraction NaN, which the clamps propagate rather than catch.
        Basis::Budget => budget.filter(|b| b.is_finite() && *b > 0.0).map(|budget| {
            ((budget - purse.total) / budget).clamp(0.0, 1.0)
        }),
    };

    let mut detail = money_text(purse.total, &purse.currency);
    match basis {
        Basis::SinceTopUp => detail.push_str(" since top-up"),
        Basis::Budget => detail.push_str(" of budget"),
        Basis::BalanceOnly => {}
    }
    if is_available == Some(false) {
        detail.push_str(" · account unavailable");
    }

    // The balance-only basis has no fraction, and the card shows the money in
    // place of a percentage — which is what the detail line is for here. A
    // window with neither would leave the ring section's "no usage windows"
    // guard to drop the whole card.
    vec![
        crate::model::UsageWindow::new("Balance", fraction.map(percent_from_fraction))
            .with_kind(crate::model::WindowKind::Balance)
            .with_exhausted(is_available == Some(false))
            .with_detail(Some(detail)),
    ]
}

/// How much of the watched peak is gone, or none where there is no denominator.
///
/// A peak of zero is an account that has never had any credit to spend, which
/// is not the same as one that has spent all of it.
fn used_fraction(balance: f64, peak: f64) -> Option<f64> {
    if peak > 0.0 {
        Some(((peak - balance) / peak).clamp(0.0, 1.0))
    } else {
        None
    }
}

fn money_text(amount: f64, currency: &str) -> String {
    format!("{amount:.2} {currency}")
}

// ---------------------------------------------------------------------------
// The watched peak
// ---------------------------------------------------------------------------

/// Read the peak for `currency`, advance it with what was just seen, and write
/// it back when it moved.
///
/// The whole honesty of `SinceTopUp` rests on one distinction: it is
/// **measured, not inferred**. PulseWin reads the balance every refresh and
/// remembers the peak. A balance that goes *up* can only be a top-up, so that
/// resets the mark and the ring starts again from full. What it costs is the
/// first run: with no mark the first reading becomes one and the ring reads 0%
/// until money is actually spent — a true statement about what this app has
/// seen.
fn advance_baseline(currency: &str, balance: f64) -> f64 {
    let Some(path) = baseline_path() else {
        return balance.max(0.0);
    };

    let mut marks = credentials::read_json(&path)
        .and_then(|v| v.as_object().cloned())
        .unwrap_or_default();

    let previous = marks
        .get(currency)
        .and_then(|mark| mark.get("peak"))
        .and_then(|v| v.as_f64());

    let peak = match previous {
        Some(peak) if balance <= peak => return peak,
        _ => balance.max(0.0),
    };

    marks.insert(
        currency.to_string(),
        serde_json::json!({ "peak": peak, "set_at": crate::model::now_rfc3339() }),
    );

    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Ok(text) = serde_json::to_string(&Value::Object(marks)) {
        let _ = std::fs::write(&path, text);
    }

    peak
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// A captured reply's shape, both currencies, money as strings.
    fn fixture() -> Value {
        json!({
            "is_available": true,
            "balance_infos": [
                { "currency": "CNY", "total_balance": "110.00",
                  "granted_balance": "10.00", "topped_up_balance": "100.00" },
                { "currency": "USD", "total_balance": "0.00",
                  "granted_balance": "0.00", "topped_up_balance": "0.00" }
            ]
        })
    }

    #[test]
    fn reads_money_out_of_strings() {
        let purses = purses(&fixture());
        assert_eq!(purses.len(), 2);
        assert_eq!(purses[0].currency, "CNY");
        assert_eq!(purses[0].total, 110.0);
        assert_eq!(purses[0].granted, Some(10.0));
        assert_eq!(purses[0].topped_up, Some(100.0));
    }

    #[test]
    fn an_absent_figure_is_absent_not_zero() {
        let reply = json!({
            "balance_infos": [ { "currency": "USD", "total_balance": "" } ]
        });
        assert!(purses(&reply).is_empty());
    }

    #[test]
    fn picks_the_first_currency_with_money_in_it() {
        let purses = purses(&json!({
            "balance_infos": [
                { "currency": "USD", "total_balance": "0.00" },
                { "currency": "CNY", "total_balance": "110.00" }
            ]
        }));
        assert_eq!(choose_purse(&purses, None).unwrap().currency, "CNY");
    }

    #[test]
    fn a_typed_currency_wins_even_at_zero() {
        let purses = purses(&fixture());
        assert_eq!(choose_purse(&purses, Some("USD")).unwrap().currency, "USD");
    }

    #[test]
    fn since_top_up_measures_against_the_peak() {
        let purse = Purse {
            currency: "USD".into(),
            total: 75.0,
            granted: None,
            topped_up: None,
        };
        let windows = windows_for(&purse, Basis::SinceTopUp, None, 100.0, Some(true));
        assert_eq!(windows.len(), 1);
        assert_eq!(windows[0].label, "Balance");
        assert_eq!(windows[0].percent_used, Some(25.0));
        assert!(windows[0].resets_at.is_none(), "prepaid credit does not turn over");
        assert_eq!(windows[0].detail.as_deref(), Some("75.00 USD since top-up"));
    }

    #[test]
    fn a_peak_of_zero_has_no_denominator() {
        let purse = Purse {
            currency: "USD".into(),
            total: 0.0,
            granted: None,
            topped_up: None,
        };
        let windows = windows_for(&purse, Basis::SinceTopUp, None, 0.0, Some(true));
        assert_eq!(windows[0].percent_used, None);
    }

    #[test]
    fn balance_only_draws_no_fraction() {
        let purse = Purse {
            currency: "CNY".into(),
            total: 110.0,
            granted: None,
            topped_up: None,
        };
        let windows = windows_for(&purse, Basis::BalanceOnly, None, 500.0, Some(true));
        assert_eq!(windows[0].percent_used, None);
        assert_eq!(windows[0].detail.as_deref(), Some("110.00 CNY"));
    }

    #[test]
    fn a_budget_is_the_denominator_in_that_mode() {
        let purse = Purse {
            currency: "USD".into(),
            total: 30.0,
            granted: None,
            topped_up: None,
        };
        let windows = windows_for(&purse, Basis::Budget, Some(100.0), 100.0, Some(true));
        assert_eq!(windows[0].percent_used, Some(70.0));
        // A budget of zero or less leaves that mode with no denominator.
        let none = windows_for(&purse, Basis::Budget, Some(0.0), 100.0, Some(true));
        assert_eq!(none[0].percent_used, None);
    }

    #[test]
    fn deepseeks_own_verdict_is_carried_not_invented() {
        let purse = Purse {
            currency: "USD".into(),
            total: 4.0,
            granted: None,
            topped_up: None,
        };
        let windows = windows_for(&purse, Basis::SinceTopUp, None, 100.0, Some(false));
        assert_eq!(windows[0].percent_used, Some(96.0));
        assert!(windows[0]
            .detail
            .as_deref()
            .unwrap()
            .ends_with("account unavailable"));
    }

    #[test]
    fn the_watched_peak_only_moves_up_then_resets_on_a_top_up() {
        // A first sight sets the mark.
        assert_eq!(used_fraction(100.0, 100.0), Some(0.0));
        // Spending does not move it.
        assert_eq!(used_fraction(40.0, 100.0), Some(0.6));
        // A balance above the peak is a top-up: the caller re-marks and the
        // fraction starts again from full.
        assert_eq!(used_fraction(150.0, 150.0), Some(0.0));
    }

    #[test]
    fn parses_the_basis_names() {
        assert_eq!(Basis::parse("since-top-up"), Some(Basis::SinceTopUp));
        assert_eq!(Basis::parse("balanceOnly"), Some(Basis::BalanceOnly));
        assert_eq!(Basis::parse("My Budget"), Some(Basis::Budget));
        assert_eq!(Basis::parse("nonsense"), None);
    }
}
