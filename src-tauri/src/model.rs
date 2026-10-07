use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum WindowKind { #[default] Other, Limit, Balance, TopUp, Credits, SharedCredits }

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct CreditRemaining { pub amount: f64, pub currency: String }

/// One quota window within a provider, e.g. Claude's 5-hour window or a weekly window.
///
/// `percent_used` is 0..=100. Pulse's original UI is built around "how much is LEFT",
/// so the frontend derives `percent_left` as `100 - percent_used`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct UsageWindow {
    /// Stable identity/scope only where the adapter has provider evidence.
    #[serde(default)]
    pub id: Option<String>,
    #[serde(default)]
    pub scope: Option<String>,
    #[serde(default)]
    pub kind: WindowKind,
    /// The provider's explicit restriction flag; separate from ring saturation.
    #[serde(default)]
    pub is_exhausted: bool,
    /// Only a duration stated by the provider; never inferred from the label.
    #[serde(default)]
    pub window_seconds: Option<f64>,
    /// Short label rendered inside/under the ring: "5h", "7d", "Monthly", ...
    pub label: String,
    /// 0..=100. `None` when the provider reported a usage we could not normalize.
    pub percent_used: Option<f64>,
    /// RFC3339 timestamp of the next reset, when the provider reports one.
    pub resets_at: Option<String>,
    /// Free-form secondary line, e.g. "1,234 / 5,000 requests".
    pub detail: Option<String>,
}

impl UsageWindow {
    /// Compare only a real timed allowance, never a label or a cash balance.
    pub fn time_progress_percent(&self, now: i64) -> Option<f64> {
        if matches!(self.kind, WindowKind::Balance | WindowKind::TopUp) { return None; }
        let seconds=self.window_seconds.filter(|v|v.is_finite() && *v>0.0)?;
        let reset=chrono::DateTime::parse_from_rfc3339(self.resets_at.as_deref()?).ok()?.timestamp();
        let remaining=reset.checked_sub(now)? as f64;
        if remaining<=0.0 || remaining>seconds {return None;}
        Some((1.0-remaining/seconds)*100.0)
    }
    pub fn quota_warning(&self, threshold: f64, now: i64) -> bool {
        if self.is_exhausted {return true;}
        if matches!(self.kind, WindowKind::Balance | WindowKind::TopUp) {return false;}
        let Some(used)=self.percent_used.filter(|v|v.is_finite()) else {return false;};
        used>=100.0 || used>=threshold && self.time_progress_percent(now).is_some_and(|elapsed|used>elapsed+1e-7)
    }
    pub fn new(label: impl Into<String>, percent_used: Option<f64>) -> Self {
        Self {
            id: None, scope: None,
            kind: WindowKind::Other,
            is_exhausted: false,
            window_seconds: None,
            label: label.into(),
            percent_used,
            resets_at: None,
            detail: None,
        }
    }

    pub fn with_reset(mut self, resets_at: Option<String>) -> Self {
        self.resets_at = resets_at;
        self
    }

    pub fn with_kind(mut self, kind: WindowKind) -> Self { self.kind = kind; self }

    pub fn with_id(mut self, id: Option<String>) -> Self { self.id = id; self }
    pub fn with_scope(mut self, scope: Option<String>) -> Self { self.scope = scope; self }

    pub fn with_duration(mut self, seconds: Option<f64>) -> Self {
        self.window_seconds = seconds.filter(|v| v.is_finite() && *v > 0.0);
        self
    }

    pub fn with_exhausted(mut self, exhausted: bool) -> Self {
        self.is_exhausted = exhausted;
        self
    }

    pub fn with_detail(mut self, detail: Option<String>) -> Self {
        self.detail = detail;
        self
    }
}

/// Aggregate result for a single provider, as shown on one card in the panel.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderUsage {
    #[serde(default)]
    pub stale: bool,
    #[serde(default)]
    pub source: Option<String>,
    #[serde(default)]
    pub credit_remaining: Option<CreditRemaining>,
    pub id: String,
    pub name: String,
    /// Subscription tier as reported by the provider, e.g. "Max", "Pro".
    pub plan: Option<String>,
    /// Account e-mail / login when the provider exposes it.
    pub account: Option<String>,
    pub windows: Vec<UsageWindow>,
    /// Populated instead of `windows` when the fetch failed.
    pub error: Option<String>,
    /// Whether this tool is present on the machine at all.
    ///
    /// **Not the same question as "did it answer".** A tool with a credential
    /// that failed to fetch still belongs on the rail — it is the thing the
    /// reader is looking for when a ring has gone quiet — while a tool that is
    /// not installed here must not be drawn at all, because a ring at zero is
    /// a reading and "you do not have this" is not.
    ///
    /// The original does this by not fetching disabled providers in the first
    /// place. Here the registry is fixed and every provider is asked, so the
    /// verdict has to travel with the result.
    pub configured: bool,
    /// RFC3339 timestamp of when this snapshot was taken.
    pub fetched_at: String,
}

impl ProviderUsage {
    pub fn ok(id: &str, name: &str, windows: Vec<UsageWindow>) -> Self {
        Self {
            stale: false, source: None, credit_remaining: None,
            id: id.to_string(),
            name: name.to_string(),
            plan: None,
            account: None,
            windows,
            error: None,
            configured: true,
            fetched_at: now_rfc3339(),
        }
    }

    pub fn failed(id: &str, name: &str, error: impl Into<String>) -> Self {
        Self {
            stale: false, source: None, credit_remaining: None,
            id: id.to_string(),
            name: name.to_string(),
            plan: None,
            account: None,
            windows: Vec::new(),
            error: Some(error.into()),
            configured: true,
            fetched_at: now_rfc3339(),
        }
    }

    /// The tool is not installed, or has no credential: nothing to draw.
    ///
    /// Returned *without* a fetch — "disabled providers are not fetched" — so
    /// this is also what keeps a rail of twenty-six tools from firing
    /// twenty-six requests at services the reader does not use.
    pub fn unconfigured(id: &str, name: &str, reason: impl Into<String>) -> Self {
        Self {
            stale: false, source: None, credit_remaining: None,
            id: id.to_string(),
            name: name.to_string(),
            plan: None,
            account: None,
            windows: Vec::new(),
            error: Some(reason.into()),
            configured: false,
            fetched_at: now_rfc3339(),
        }
    }

    pub fn with_plan(mut self, plan: Option<String>) -> Self {
        self.plan = plan;
        self
    }

    pub fn with_account(mut self, account: Option<String>) -> Self {
        self.account = account;
        self
    }
    pub fn with_credit_remaining(mut self, amount:f64, currency:impl Into<String>)->Self {
        if amount.is_finite(){self.credit_remaining=Some(CreditRemaining{amount,currency:currency.into()});}self
    }

    /// Worst-case (highest) utilization across all windows; drives the tray label
    /// and the summary ring.
    pub fn peak_percent_used(&self) -> Option<f64> {
        self.windows
            .iter()
            .filter_map(|w| w.percent_used)
            .fold(None, |acc: Option<f64>, v| Some(acc.map_or(v, |a| a.max(v))))
    }

    /// True when this tool is present on the machine.
    ///
    /// Answering "did it answer" instead would make a ring vanish exactly when
    /// it has something to say.
    pub fn is_configured(&self) -> bool {
        self.configured
    }
}

/// Full payload handed to the frontend on every refresh.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Snapshot {
    pub providers: Vec<ProviderUsage>,
    pub fetched_at: String,
}

pub fn now_rfc3339() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
}

/// Parse the common reset shapes providers return: RFC3339, unix seconds,
/// unix milliseconds, or an ISO string without a timezone (assumed UTC).
pub fn parse_reset(value: &serde_json::Value) -> Option<String> {
    match value {
        serde_json::Value::String(s) => {
            if let Ok(dt) = chrono::DateTime::parse_from_rfc3339(s) {
                return Some(dt.with_timezone(&chrono::Utc).to_rfc3339_opts(
                    chrono::SecondsFormat::Secs,
                    true,
                ));
            }
            if let Ok(dt) = chrono::NaiveDateTime::parse_from_str(s, "%Y-%m-%dT%H:%M:%S%.f") {
                return Some(
                    dt.and_utc()
                        .to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
                );
            }
            // Bare date, e.g. "2026-10-01".
            if let Ok(d) = chrono::NaiveDate::parse_from_str(s, "%Y-%m-%d") {
                return Some(
                    d.and_hms_opt(0, 0, 0)?
                        .and_utc()
                        .to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
                );
            }
            None
        }
        serde_json::Value::Number(n) => {
            let raw = n.as_f64()?;
            // Heuristic: anything past year 2286 in seconds is really milliseconds.
            let secs = if raw > 1e11 { raw / 1000.0 } else { raw };
            let dt = chrono::DateTime::from_timestamp(secs as i64, 0)?;
            Some(dt.to_rfc3339_opts(chrono::SecondsFormat::Secs, true))
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn peak_uses_highest_window() {
        let usage = ProviderUsage::ok(
            "x",
            "X",
            vec![
                UsageWindow::new("5h", Some(12.0)),
                UsageWindow::new("7d", Some(64.5)),
                UsageWindow::new("month", None),
            ],
        );
        assert_eq!(usage.peak_percent_used(), Some(64.5));
    }

    #[test]
    fn peak_is_none_without_numbers() {
        let usage = ProviderUsage::ok("x", "X", vec![UsageWindow::new("5h", None)]);
        assert_eq!(usage.peak_percent_used(), None);
    }

    #[test]
    fn parses_seconds_and_millis() {
        assert_eq!(parse_reset(&json!(0)).unwrap(), "1970-01-01T00:00:00Z");
        assert_eq!(parse_reset(&json!(0)).unwrap(), parse_reset(&json!(0.0)).unwrap());
    }

    #[test]
    fn parses_rfc3339() {
        let out = parse_reset(&json!("2026-10-01T12:00:00Z")).unwrap();
        assert_eq!(out, "2026-10-01T12:00:00Z");
    }

    #[test]
    fn rejects_garbage() {
        assert!(parse_reset(&json!("not a date")).is_none());
        assert!(parse_reset(&json!(true)).is_none());
    }
}
