//! The day's rates from US dollars, so estimates can be shown in another
//! currency (`Money`, `DisplayCurrency` in the original).
//!
//! **Source:** ExchangeRate-API's open access endpoint — no key, updated once a
//! day, one small JSON file with every currency per US dollar and the time of
//! the next update. Its terms ask for two things, and the original does both:
//! a "Rates By Exchange Rate API" link wherever the rates are used
//! ([`ATTRIBUTION`]), and no more than a request a day or so — a caller that
//! asks more often than hourly is answered 429 for twenty minutes. The data may
//! be kept, and is: `exchange-rates.json`, so a launch offline still converts at
//! last day's rate rather than not at all.
//!
//! **Asked only while a currency other than the dollar is chosen** — see
//! [`watch`] — and only once the source's own next-update time has passed. A
//! rate the reader types wins over the fetched one
//! (`Preferences::manual_exchange_rates`).

use std::collections::HashMap;
use std::time::Duration;

use serde::{Deserialize, Serialize};

/// The endpoint: units of each currency to one US dollar.
pub const SOURCE: &str = "https://open.er-api.com/v6/latest/USD";
/// The page the terms ask the rates to link to.
pub const ATTRIBUTION: &str = "https://www.exchangerate-api.com";

/// The least time between two requests, whatever the source says: the rate the
/// terms promise is never limited.
pub const MINIMUM_INTERVAL: i64 = 60 * 60;

/// How often the keeper looks. In practice it asks once a day; this is only how
/// soon it may notice that a day has passed.
const LOOK_INTERVAL: Duration = Duration::from_secs(20 * 60);

/// The currencies a reader may pick, in the order they are offered. One per
/// language the original speaks; the dollar is the one estimates are worked out
/// in, and the other six are conversions of it.
pub const CURRENCIES: [&str; 7] = ["usd", "cny", "hkd", "twd", "jpy", "krw", "rub"];

/// The ISO code for a chosen currency — what `rates` and `manual` are keyed by,
/// and what the frontend formats with.
pub fn code(currency: &str) -> String {
    currency.to_ascii_uppercase()
}

/// Whether `currency` is one this build can show. Called on every load, so a
/// value written by a build that knew a longer list is dropped rather than
/// carried into a conversion.
pub fn valid_currency(currency: &str) -> bool {
    CURRENCIES.contains(&currency)
}

/// A rate as this build will use it: a positive, finite number of units to a
/// dollar. `None` for anything else — including zero, which would make every
/// estimate read as free.
pub fn usable_rate(rate: f64) -> Option<f64> {
    (rate.is_finite() && rate > 0.0).then_some(rate)
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Stored {
    /// When the source last moved its rates, in epoch seconds.
    pub updated_at: i64,
    /// When the source says it will move them next; nothing is asked before
    /// then.
    pub next_update_at: i64,
    /// When Pulse last asked, in epoch seconds.
    pub fetched_at: i64,
    /// Units of each currency to one US dollar, by ISO code. Only the
    /// currencies above are kept.
    pub rates: HashMap<String, f64>,
}

fn file() -> Option<std::path::PathBuf> {
    crate::settings::directory().map(|dir| dir.join("exchange-rates.json"))
}

/// The rates kept by the last run, if any. A file that will not parse is no
/// rates, not a failure — the dollar then simply stays the dollar.
pub fn load() -> Option<Stored> {
    let text = std::fs::read_to_string(file()?).ok()?;
    let stored: Stored = serde_json::from_str(&text).ok()?;
    (!stored.rates.is_empty()).then_some(stored)
}

pub fn save(stored: &Stored) -> Result<(), String> {
    let path = file().ok_or("APPDATA is unavailable")?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let bytes = serde_json::to_vec(stored).map_err(|e| e.to_string())?;
    std::fs::write(&path, bytes).map_err(|e| e.to_string())
}

/// Whether `kept` is old enough to ask again: none kept, or the source's own
/// next update has passed and the last request is an hour gone.
pub fn is_due(kept: Option<&Stored>, now: i64) -> bool {
    match kept {
        None => true,
        Some(kept) => now >= kept.next_update_at && now - kept.fetched_at >= MINIMUM_INTERVAL,
    }
}

/// `{"result": "success", "base_code": "USD", "time_last_update_unix": …,
/// "time_next_update_unix": …, "rates": {"CNY": 6.71, …}}`, the offered
/// currencies only, each a positive number. `None` for anything else — an
/// `"error"` result, another base, no rates Pulse offers.
pub fn parse(bytes: &[u8], fetched_at: i64) -> Option<Stored> {
    let value: serde_json::Value = serde_json::from_slice(bytes).ok()?;
    if value.get("result").and_then(|v| v.as_str()) != Some("success") {
        return None;
    }
    if value.get("base_code").and_then(|v| v.as_str()) != Some("USD") {
        return None;
    }
    let table = value.get("rates")?.as_object()?;
    let updated = value.get("time_last_update_unix").and_then(|v| v.as_f64())?;
    let next = value.get("time_next_update_unix").and_then(|v| v.as_f64())?;
    let mut rates = HashMap::new();
    for currency in CURRENCIES {
        let code = code(currency);
        if code == "USD" {
            continue;
        }
        if let Some(rate) = table.get(&code).and_then(|v| v.as_f64()).and_then(usable_rate) {
            rates.insert(code, rate);
        }
    }
    if rates.is_empty() {
        return None;
    }
    Some(Stored {
        updated_at: updated as i64,
        next_update_at: next as i64,
        fetched_at,
        rates,
    })
}

/// One request, through the proxy Settings chose. `None` on any failure —
/// a rate is never invented from a bad answer.
pub async fn download(client: &reqwest::Client) -> Option<Vec<u8>> {
    const MAX_BYTES: usize = 1024 * 1024;
    let response = client
        .get(SOURCE)
        .timeout(Duration::from_secs(20))
        .header("Accept", "application/json")
        .send()
        .await
        .ok()?;
    if response.status() != reqwest::StatusCode::OK {
        return None;
    }
    let bytes = response.bytes().await.ok()?;
    (bytes.len() <= MAX_BYTES).then(|| bytes.to_vec())
}

/// The rates this build holds, for the frontend to convert estimates with.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Rates {
    /// The chosen currency, as a lowercase code (`usd`, `cny`, …).
    pub currency: String,
    /// Units of each currency to one US dollar.
    pub rates: HashMap<String, f64>,
    /// Rates the reader typed, which win over the fetched ones.
    pub manual: HashMap<String, f64>,
    /// When the source last moved its rates, in epoch seconds; `None` when no
    /// rates have been fetched yet.
    pub updated_at: Option<i64>,
    /// Where the rates come from, and the page to credit.
    pub source: &'static str,
    pub attribution: &'static str,
}

/// What the keeper has fetched this run. A `std::sync::Mutex` because the
/// command reads it synchronously and holds it for one clone.
#[derive(Default)]
pub struct RatesState {
    kept: std::sync::Mutex<Option<Stored>>,
    /// Woken when the chosen currency changes, so a reader who switches to yuan
    /// is not left in dollars for up to a look interval. The original re-arms
    /// its keeper on a currency change the same way.
    wake: tokio::sync::Notify,
}

impl RatesState {
    pub fn kept(&self) -> Option<Stored> {
        self.kept.lock().unwrap().clone()
    }
    fn put(&self, stored: Option<Stored>) {
        *self.kept.lock().unwrap() = stored;
    }
}

/// The currency changed: look at the rates now rather than at the next look.
pub fn preferences_changed(app: &tauri::AppHandle) {
    use tauri::Manager;
    if let Some(state) = app.try_state::<RatesState>() {
        state.wake.notify_one();
    }
}

/// The rates to draw with: the kept table, the reader's own rates, and the
/// chosen currency. Seeded from disk at launch, so the first figure drawn is
/// already in the chosen currency.
#[tauri::command]
pub fn get_exchange_rates(state: tauri::State<RatesState>) -> Rates {
    let prefs = crate::preferences::load();
    let currency = if valid_currency(&prefs.display_currency) { prefs.display_currency.clone() } else { "usd".to_string() };
    let kept = state.kept();
    Rates {
        currency,
        rates: kept.as_ref().map(|k| k.rates.clone()).unwrap_or_default(),
        manual: prefs.manual_exchange_rates.clone(),
        updated_at: kept.map(|k| k.updated_at),
        source: SOURCE,
        attribution: ATTRIBUTION,
    }
}

/// Keep the rates current while a currency other than the dollar is chosen, and
/// do nothing at all while it is not.
///
/// The kept rates are put into effect at launch, before any request, so the
/// first panel drawn is already in the chosen currency. Then a look every
/// [`LOOK_INTERVAL`], which asks only when [`is_due`] — in practice once a day.
/// A failed request is tried again at the next look, not before, and a currency
/// switched back and forth does not become a request each time.
pub fn watch(app: tauri::AppHandle) {
    use tauri::Manager;
    let state = app.state::<RatesState>();
    state.put(load());
    drop(state);
    tauri::async_runtime::spawn(async move {
        let mut last_attempt: Option<std::time::Instant> = None;
        loop {
            let prefs = crate::preferences::load();
            // **Nothing is asked while the dollar is chosen**: the rates cannot
            // change a figure, so a request would be someone else's traffic.
            if prefs.display_currency != "usd" {
                let kept = app.state::<RatesState>().kept();
                let now = chrono::Utc::now().timestamp();
                let cooled = last_attempt.is_none_or(|at| at.elapsed() >= LOOK_INTERVAL);
                if cooled && is_due(kept.as_ref(), now) {
                    last_attempt = Some(std::time::Instant::now());
                    let client = app.state::<crate::AppState>().ctx.read().unwrap().client.clone();
                    if let Some(bytes) = download(&client).await {
                        if let Some(fresh) = parse(&bytes, now) {
                            let _ = save(&fresh);
                            app.state::<RatesState>().put(Some(fresh.clone()));
                            // Every figure already drawn is in the old rate;
                            // the frontend reads again when this arrives.
                            let _ = app.emit_rates(&fresh);
                        }
                    }
                }
            }
            // A currency change wakes the loop; otherwise it looks again after
            // one look interval, which is still far more often than the source
            // moves its rates.
            let state = app.state::<RatesState>();
            let _ = tokio::time::timeout(LOOK_INTERVAL, state.wake.notified()).await;
        }
    });
}

/// Told to the frontend so an open Token-spend pane or recap redraws at the new
/// rate instead of waiting for its own next read.
trait EmitRates {
    fn emit_rates(&self, stored: &Stored) -> tauri::Result<()>;
}
impl EmitRates for tauri::AppHandle {
    fn emit_rates(&self, stored: &Stored) -> tauri::Result<()> {
        use tauri::Emitter;
        self.emit("exchange-rates-changed", stored)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn payload(rates: &str) -> Vec<u8> {
        format!(
            r#"{{"result":"success","base_code":"USD","time_last_update_unix":1792000000,"time_next_update_unix":1792086400,"rates":{rates}}}"#
        )
        .into_bytes()
    }

    #[test]
    fn only_the_currencies_pulse_offers_are_kept() {
        let stored = parse(&payload(r#"{"CNY":7.12,"JPY":151.3,"XYZ":9.9,"USD":1}"#), 1792000100).unwrap();
        assert_eq!(stored.rates.get("CNY"), Some(&7.12));
        assert_eq!(stored.rates.get("JPY"), Some(&151.3));
        // Not offered, and the dollar itself needs no row.
        assert!(!stored.rates.contains_key("XYZ"));
        assert!(!stored.rates.contains_key("USD"));
        assert_eq!(stored.updated_at, 1792000000);
        assert_eq!(stored.next_update_at, 1792086400);
        assert_eq!(stored.fetched_at, 1792000100);
    }

    #[test]
    fn a_result_that_is_not_this_source_is_no_rates() {
        assert!(parse(br#"{"result":"error","base_code":"USD"}"#, 0).is_none());
        // Another base is a table of the wrong direction: a figure converted
        // with it would be wrong by the rate squared.
        assert!(parse(
            br#"{"result":"success","base_code":"EUR","time_last_update_unix":1,"time_next_update_unix":2,"rates":{"CNY":7.1}}"#,
            0
        )
        .is_none());
        assert!(parse(br#"{"result":"success","base_code":"USD"}"#, 0).is_none());
        // No rate Pulse offers is no table.
        assert!(parse(&payload(r#"{"XYZ":9.9}"#), 0).is_none());
        // Zero or a negative rate would make every estimate read as free.
        assert!(parse(&payload(r#"{"CNY":0,"JPY":-1}"#), 0).is_none());
        assert!(parse(b"{}", 0).is_none());
    }

    #[test]
    fn a_rate_is_only_ever_asked_for_when_it_is_due() {
        assert!(is_due(None, 1_000));
        let kept = Stored { updated_at: 1_000, next_update_at: 4_000, fetched_at: 1_000, rates: HashMap::from([("CNY".to_string(), 7.1)]) };
        // The source's own next-update time has not come.
        assert!(!is_due(Some(&kept), 3_999));
        // It has, but an hour has not gone since the last request.
        assert!(!is_due(Some(&kept), 4_500));
        // Both conditions met.
        assert!(is_due(Some(&kept), 4_600));
    }

    #[test]
    fn the_currency_list_is_the_seven_the_original_offers() {
        assert_eq!(CURRENCIES.len(), 7);
        assert_eq!(code("cny"), "CNY");
        assert!(valid_currency("twd"));
        // Upper case is the code, not the choice: nothing else is accepted.
        assert!(!valid_currency("TWD"));
        assert!(!valid_currency("eur"));
        // Zero is not a rate: it would print every estimate as free.
        assert_eq!(usable_rate(0.0), None);
        assert_eq!(usable_rate(-1.0), None);
        assert_eq!(usable_rate(f64::INFINITY), None);
        assert_eq!(usable_rate(151.3), Some(151.3));
    }
}
