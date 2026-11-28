//! User choices, distinct from credential discovery and transient usage.
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct Preferences {
    pub network_proxy: crate::proxy::NetworkProxySettings,
    pub panel_visible: bool,
    pub tray_shows_usage: bool,
    pub tray_style: String,
    pub tray_account: Option<String>,
    pub accounts: Vec<crate::accounts::Account>,
    pub account_labels: std::collections::HashMap<String, String>,
    pub open_settings_shortcut: Option<String>,
    pub toggle_panel_shortcut: Option<String>,
    pub alerts: crate::alerts::AlertPreferences,
    pub token_spend_enabled: bool,
    pub token_spend_span: String,
    /// The reader's own span, as two `YYYY-MM-DD` days in order. Empty
    /// until they pick one: the pane then counts the last seven days
    /// itself rather than this file inventing two dates on a day nobody
    /// asked about.
    pub spend_custom_days: Vec<String>,
    pub spend_activity_view: String,
    pub recap_monthly_price:Option<f64>,
    pub recap_hides_projects:bool,
    pub reset_celebration: bool,
    pub enabled_providers: Vec<String>,
    pub provider_order: Vec<String>,
    pub panel_colour: String,
    pub panel_size: f64,
    pub rail_spacing: f64,
    pub round_ends: bool,
    /// How limit bars are drawn on the card and in the menu (`UsageBarStyle`):
    /// `continuous` or `segmented`. Segmented bars are taller so their ticks
    /// read as ticks, which changes the card's frame budget.
    pub usage_bar_style: String,
    /// Experimental: while a docked rail is pulled away, its movement lags the
    /// pointer and the rail thins like a liquid neck until it breaks free.
    pub liquid_docking: bool,
    pub shows_remaining: bool,
    pub show_percentages: bool,
    pub label_above: bool,
    pub auto_collapse: bool,
    pub warning_threshold: f64,
    pub refresh_seconds: u64,
    pub refresh_automatic: bool,
    pub show_reset_clock: bool,
    pub clock_remaining: bool,
    pub show_second_ring: bool,
    pub top_show_percentages: bool,
    pub animate_activity: bool,
    pub dock_alert_colour: bool,
    pub show_forecast: bool,
    pub hide_in_fullscreen: bool,
    pub follow_active_display: bool,
    /// Keep the panel out of screenshots and recordings.
    ///
    /// A request, not a guarantee: Windows honours `WDA_EXCLUDEFROMCAPTURE`
    /// for the capture APIs that ask, and a capture tool that grabs the screen
    /// buffer directly can still include the panel. Off by default, because it
    /// hides the panel from the reader's own pictures of it too.
    pub hide_from_capture: bool,
    pub account_appearance: std::collections::HashMap<String, AccountAppearance>,
    pub pinned_windows: std::collections::HashMap<String, String>,
    /// The window starter: which providers it acts for, when, and what it last
    /// did. **Off or empty by default** — this is the one setting that sends
    /// something on the reader's behalf, so nothing is switched on by a default
    /// and nothing is inferred from the absence of a value.
    pub primed_providers: Vec<String>,
    pub primer_hours: crate::window_starter::PrimerHours,
    /// Epoch seconds of the last attempt per provider, and its outcome as the
    /// starter's own string, so a value written by one build is still read by the
    /// next and the pane can show why a window did not start.
    pub primer_run_times: std::collections::HashMap<String, i64>,
    pub primer_run_outcomes: std::collections::HashMap<String, String>,
    /// The currency estimates are shown in (`DisplayCurrency`): one of the
    /// seven `exchange_rates::CURRENCIES`. **Only estimates are converted** —
    /// the money worked out from US-dollar API prices: the value estimate,
    /// Token spend, the cards' history, the recap. What a service reports
    /// itself — a balance, credits, what a console charged — is that
    /// service's figure, shown in the currency it was given in, or it would
    /// disagree with the service's own page.
    pub display_currency: String,
    /// Rates the reader typed, per currency code, which win over the fetched
    /// ones. Empty by default: an unasked-for rate is somebody else's guess.
    pub manual_exchange_rates: std::collections::HashMap<String, f64>,
    /// Which route Antigravity's figures are read by: `"auto"`, `"cli"` or
    /// `"desktop"`. A route, not a credential: both routes are the same login,
    /// and the choice is which one to ask.
    pub antigravity_usage_source: String,
    /// Accounts drawn as one ring per model group. Off by default, and only
    /// meaningful for a provider that reports more than one group — today only
    /// Antigravity. An account not named here is drawn whole.
    pub split_accounts: Vec<String>,
    /// The groups that keep a ring, per split account. **A missing entry is
    /// every group.** A choice rather than something read off a reading, so the
    /// rail's budget moves when a setting does and not when a reply arrives.
    pub shown_model_groups: std::collections::HashMap<String, Vec<String>>,
    /// Which MiniMax storefront the account buys from: `"international"` or
    /// `"china"`. The two are one service with two sites and two keys, so a site
    /// is a setting on one provider rather than a provider of its own — which is
    /// what `minimax-cn` used to be, and what [`Preferences::migrate_minimax_cn`]
    /// folds away.
    pub minimax_site: String,
    /// The one request for a GitHub star. It is due when the first day PulseWin
    /// was counted is at least [`STAR_MINIMUM_AGE_MS`] behind and the app has
    /// run on at least [`STAR_MINIMUM_DAYS`] separate days since; it is spent
    /// when the banner is shown, not when it is answered. Kept here rather than
    /// in its own file because these are settings, and because a copy installed
    /// and forgotten must never be asked.
    pub star_request_since: String,
    pub star_request_days: i64,
    pub star_request_last_day: String,
    pub star_request_shown: bool,
}

impl Default for Preferences {
    fn default() -> Self {
        Self { network_proxy:Default::default(), panel_visible:true, tray_shows_usage:false, tray_style:"figure".into(), tray_account:None, accounts: vec![], account_labels: Default::default(), open_settings_shortcut: None, toggle_panel_shortcut: None, alerts: Default::default(), token_spend_enabled: false, token_spend_span: "week".into(), spend_custom_days: Vec::new(), spend_activity_view:"daily".into(),recap_monthly_price:None,recap_hides_projects:false, reset_celebration: true, enabled_providers: vec![], provider_order: vec![], panel_colour: "black".into(), panel_size: 1.0,
            rail_spacing: 1.0, round_ends: false, usage_bar_style: "continuous".into(), liquid_docking: false, shows_remaining: false,
            show_percentages: true, label_above: false, auto_collapse: true,
            warning_threshold: 0.75, refresh_seconds: 120, refresh_automatic: true, show_reset_clock: false,
            clock_remaining: false, show_second_ring: false, top_show_percentages: false, animate_activity: true, dock_alert_colour: true, show_forecast: false, hide_in_fullscreen: true, follow_active_display: false, hide_from_capture: false, account_appearance: Default::default(), pinned_windows: Default::default(),
            primed_providers: Vec::new(), primer_hours: Default::default(),
            primer_run_times: Default::default(), primer_run_outcomes: Default::default(),
            display_currency: "usd".into(), manual_exchange_rates: Default::default(),
            antigravity_usage_source: "auto".into(), split_accounts: Vec::new(), shown_model_groups: Default::default(),
            minimax_site: "international".into(),
            star_request_since: String::new(), star_request_days: 0, star_request_last_day: String::new(), star_request_shown: false }
    }
}

impl Preferences {
    /// Fold the retired `minimax-cn` provider into `minimax`'s site setting.
    ///
    /// The mainland and international storefronts used to be two providers. They
    /// are one service with two keys, so they are now one provider and a site —
    /// but an account that only ever had the mainland one must keep its key, its
    /// label, its place in the rail and its card settings. `cnWins` is the only
    /// record of where that key came from once the ids are one, so the site is
    /// set from it and the rest is folded over.
    ///
    /// Pure and idempotent. The credential migration is handled separately,
    /// using the original DPAPI identity of each file.
    pub fn migrate_minimax_cn(&mut self) {
        let retired = crate::providers::minimax::RETIRED_ID;
        let live = crate::providers::minimax::ID;
        let present = |ids: &[String]| ids.iter().any(|id| id == retired);
        let cn_wins = present(&self.enabled_providers)
            && !self.enabled_providers.iter().any(|id| id == live);
        // Already migrated lists stay as they are; a stale entry is only ever
        // folded, never removed, so nothing the user set is lost.
        if !cn_wins && !present(&self.enabled_providers) && !present(&self.provider_order) {
            return;
        }
        if cn_wins {
            self.minimax_site = "china".into();
        }
        fold_ids(&mut self.enabled_providers, retired, live);
        fold_ids(&mut self.provider_order, retired, live);
        fold_ids(&mut self.split_accounts, retired, live);
        fold_map(&mut self.account_labels, retired, live);
        fold_map(&mut self.account_appearance, retired, live);
        fold_map(&mut self.pinned_windows, retired, live);
        fold_map(&mut self.shown_model_groups, retired, live);
        fold_map(&mut self.alerts.low_balance, retired, live);
        if self.tray_account.as_deref() == Some(retired) {
            self.tray_account = Some(live.to_owned());
        }
    }

    pub fn normalize(&mut self, known: &[String]) {
        self.migrate_minimax_cn();
        self.network_proxy.normalize();
        if !["black","light"].contains(&self.panel_colour.as_str()) {self.panel_colour="black".into();}
        if !["continuous","segmented"].contains(&self.usage_bar_style.as_str()) {self.usage_bar_style="continuous".into();}
        let mut seen_accounts=std::collections::HashSet::new();
        // An extension is an account of its own but has no credential and no
        // provider in `accounts::SUPPORTED`, which is the list the add-account
        // pane offers. Its id is the one proof it belongs here.
        self.accounts.retain(|a| {
            let usable = crate::accounts::valid(a)
                || (a.provider == crate::extension::ACCOUNT_PROVIDER
                    && crate::extension::id_from_account_id(&a.id).is_some());
            usable && seen_accounts.insert(a.id.clone())
        });
        self.accounts.truncate(64);
        let known=known.iter().cloned().chain(self.accounts.iter().map(|a|a.id.clone())).collect::<Vec<_>>();
        let known=known.as_slice();
        self.account_labels.retain(|id,value|known.contains(id) && crate::accounts::label(value).is_ok());
        for value in self.account_labels.values_mut(){*value=value.trim().to_owned();}
        for value in [&mut self.open_settings_shortcut,&mut self.toggle_panel_shortcut] {
            if value.as_deref().is_some_and(|v|crate::application::validate(v).is_err()){*value=None;}
        }
        self.alerts.normalize(known);
        if !["today","week","month","quarter","thisMonth","lastMonth","thisYear","all","custom"]
            .contains(&self.token_spend_span.as_str())
        {self.token_spend_span="week".into();}
        // Two days, in order, and never a day that does not exist: anything
        // else is dropped whole, so a half-written range cannot narrow the
        // pane to a span nobody asked for.
        self.spend_custom_days = match self.spend_custom_days.as_slice() {
            [first, last] if day(first).zip(day(last)).is_some_and(|(a, b)| a <= b) => {
                vec![first.clone(), last.clone()]
            }
            _ => Vec::new(),
        };
        if !["daily","weekly","cumulative"].contains(&self.spend_activity_view.as_str()){self.spend_activity_view="daily".into();}
        self.recap_monthly_price=self.recap_monthly_price.filter(|p|p.is_finite()&&*p>0.0&&*p<=10000.0);
        self.account_appearance.retain(|id, _| known.contains(id));
        for value in self.account_appearance.values_mut() { value.normalize(); }
        self.pinned_windows.retain(|id, label| known.contains(id) && !label.is_empty());
        // A route from a build that knew a longer list is `auto`, which is the
        // route that asks the app and falls back to `agy` rather than refusing
        // to read at all.
        if !["auto","cli","desktop"].contains(&self.antigravity_usage_source.as_str()) {self.antigravity_usage_source="auto".into();}
        if !["international","china"].contains(&self.minimax_site.as_str()) {self.minimax_site = "international".into();}
        let mut split = std::collections::HashSet::new();
        self.split_accounts.retain(|id| known.contains(id) && split.insert(id.clone()));
        // "No groups chosen" is a choice to keep the account whole, so an empty
        // list is dropped rather than stored as a choice that matches nothing.
        self.shown_model_groups.retain(|id, groups| {
            groups.retain(|group| !group.trim().is_empty());
            let mut chosen = std::collections::HashSet::new();
            groups.retain(|group| chosen.insert(group.clone()));
            known.contains(id) && !groups.is_empty()
        });
        self.enabled_providers.retain(|id| known.contains(id));
        if !["figure","ring","split"].contains(&self.tray_style.as_str()){self.tray_style="figure".into();}
        if self.tray_account.as_ref().is_some_and(|id|!self.enabled_providers.contains(id)){self.tray_account=None;}
        self.provider_order.retain(|id| known.contains(id));
        let mut seen = std::collections::HashSet::new();
        self.enabled_providers.retain(|id| seen.insert(id.clone()));
        seen.clear();
        self.provider_order.retain(|id| seen.insert(id.clone()));
        for id in known { if !self.provider_order.contains(id) { self.provider_order.push(id.clone()); } }
        self.panel_size = finite_clamp(self.panel_size, 0.75, 1.5, 1.0);
        self.rail_spacing = finite_clamp(self.rail_spacing, 0.6, 1.4, 1.0);
        self.warning_threshold = finite_clamp(self.warning_threshold, 0.6, 0.9, 0.75);
        self.refresh_seconds = self.refresh_seconds.clamp(120, 1800);
        // A currency from a build that knew a longer list is dropped rather than
        // carried into a conversion, and so is a rate that is zero, negative or
        // not a number — a figure converted with one would be a figure read as
        // free or as its own opposite.
        if !crate::exchange_rates::valid_currency(&self.display_currency) { self.display_currency = "usd".into(); }
        self.manual_exchange_rates.retain(|code, rate| {
            code.len() == 3
                && crate::exchange_rates::CURRENCIES.iter().any(|c| crate::exchange_rates::code(c) == *code)
                && crate::exchange_rates::usable_rate(*rate).is_some()
        });
        // The dollar needs no rate of its own, and keeping one would read as a
        // conversion that never happens.
        self.manual_exchange_rates.remove("USD");
        // The starter acts for a closed list of providers. An id from a build
        // that knew a longer list is dropped rather than carried into a send.
        self.primed_providers.retain(|id| crate::window_starter::PROVIDERS.contains(&id.as_str()));
        let mut primed = std::collections::HashSet::new();
        self.primed_providers.retain(|id| primed.insert(id.clone()));
        self.primer_hours.start = self.primer_hours.start.min(23);
        self.primer_hours.end = self.primer_hours.end.min(23);
        // The star request is a record of days, not a user choice: it is only
        // trimmed here, so a hand-edited file cannot make its count negative.
        self.star_request_since = self.star_request_since.trim().to_string();
        self.star_request_last_day = self.star_request_last_day.trim().to_string();
        self.star_request_days = self.star_request_days.clamp(0, 1_000_000);
    }
}

/// A stored `YYYY-MM-DD` day, if it is one. `chrono` rejects 31 February
/// rather than rolling it into March, which is what makes this a check.
fn day(text: &str) -> Option<chrono::NaiveDate> {
    if text.len() != 10 { return None }
    chrono::NaiveDate::parse_from_str(text, "%Y-%m-%d").ok().filter(|d| d.to_string() == text)
}

fn finite_clamp(value: f64, min: f64, max: f64, fallback: f64) -> f64 {
    if value.is_finite() { value.clamp(min, max) } else { fallback }
}

/// Rename a retired provider id inside a list, keeping the first occurrence so
/// the rail's order survives the fold.
fn fold_ids(values: &mut Vec<String>, retired: &str, live: &str) {
    if !values.iter().any(|id| id == retired) {
        return;
    }
    for value in values.iter_mut() {
        if value == retired {
            *value = live.to_owned();
        }
    }
    let mut seen = std::collections::HashSet::new();
    values.retain(|id| seen.insert(id.clone()));
}

/// Move a retired provider's per-account settings onto the live id. An entry the
/// live id already has is the newer one and wins.
fn fold_map<V>(map: &mut std::collections::HashMap<String, V>, retired: &str, live: &str) {
    if let Some(value) = map.remove(retired) {
        map.entry(live.to_owned()).or_insert(value);
    }
}

/// A fortnight of use and a week of separate days before the one request for a
/// star, and never for a copy that was installed and forgotten.
pub const STAR_MINIMUM_AGE_MS: i64 = 14 * 24 * 60 * 60 * 1000;
pub const STAR_MINIMUM_DAYS: i64 = 7;

/// The record after PulseWin is seen running at `now_ms`: the first sighting
/// starts the count, and each new calendar day adds one.
pub fn note_star_activity(now_ms: i64) -> Result<(), String> {
    let mut prefs = load();
    if prefs.star_request_since.is_empty() {
        prefs.star_request_since = stamp_of(now_ms);
    }
    let day = day_of(now_ms);
    if !day.is_empty() && prefs.star_request_last_day != day {
        prefs.star_request_last_day = day;
        prefs.star_request_days += 1;
    }
    save(&prefs)
}

/// Whether the banner is due as the settings window opens, spending the request
/// if so. It is spent when shown, not when answered: PulseWin cannot tell
/// whether the star was given and does not try; the About pane keeps a link.
pub fn claim_star_request(now_ms: i64) -> Result<bool, String> {
    let mut prefs = load();
    let due = star_due(
        (!prefs.star_request_since.is_empty()).then(|| epoch_ms(&prefs.star_request_since)).flatten(),
        prefs.star_request_days,
        prefs.star_request_shown,
        now_ms,
    );
    if due {
        prefs.star_request_shown = true;
        save(&prefs)?;
    }
    Ok(due)
}

fn star_due(since_ms: Option<i64>, days: i64, shown: bool, now_ms: i64) -> bool {
    let Some(since) = since_ms else { return false };
    !shown && now_ms.saturating_sub(since) >= STAR_MINIMUM_AGE_MS && days >= STAR_MINIMUM_DAYS
}

fn stamp_of(now_ms: i64) -> String {
    chrono::DateTime::from_timestamp_millis(now_ms).map(|d| d.to_rfc3339()).unwrap_or_default()
}

fn epoch_ms(text: &str) -> Option<i64> {
    chrono::DateTime::parse_from_rfc3339(text).ok().map(|d| d.timestamp_millis())
}

fn day_of(now_ms: i64) -> String {
    chrono::DateTime::from_timestamp_millis(now_ms).map(|d| d.with_timezone(&chrono::Local).format("%Y-%m-%d").to_string()).unwrap_or_default()
}

pub fn load() -> Preferences {
    let mut prefs = load_before_provider_migrations();
    prefs.migrate_minimax_cn();
    prefs
}

pub fn load_before_provider_migrations() -> Preferences {
    crate::settings::directory().and_then(|d| std::fs::read_to_string(d.join("preferences.json")).ok())
        .and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default()
}

pub fn save(value: &Preferences) -> Result<(), String> {
    let dir = crate::settings::directory().ok_or("APPDATA is unavailable")?;
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    std::fs::write(dir.join("preferences.json"), crate::settings::to_json(value)).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn a_star_is_requested_once_after_a_fortnight_of_real_use(){
        let day=24*60*60*1000;
        let since=1_700_000_000_000i64;
        // A fortnight old but only three days of use is not due: an install
        // that was opened twice must never be asked.
        assert!(!star_due(Some(since),3,false,since+14*day));
        // Seven separate days but not yet a fortnight is not due either.
        assert!(!star_due(Some(since),7,false,since+13*day));
        assert!(star_due(Some(since),7,false,since+14*day));
        // Spent when shown, not when answered: it never comes back.
        assert!(!star_due(Some(since),7,true,since+60*day));
        // Nothing is counted before the first sighting.
        assert!(!star_due(None,0,false,since+60*day));
    }
    #[test] fn star_activity_counts_calendar_days_once(){
        let mut p=Preferences::default();
        assert!(p.star_request_since.is_empty()&&p.star_request_days==0);
        p.normalize(&[]);
        assert_eq!(p.star_request_days,0);
        // A hand-edited count below zero is not a way to skip the wait.
        p.star_request_days=-5;p.normalize(&[]);assert_eq!(p.star_request_days,0);
        assert!(epoch_ms(&stamp_of(1_700_000_000_000))==Some(1_700_000_000_000));
    }
    #[test] fn panel_colour_is_backward_compatible_and_round_trips(){
        let mut p:Preferences=serde_json::from_str("{}").unwrap();
        assert_eq!(p.panel_colour,"black");
        p.panel_colour="light".into();p.normalize(&[]);
        let saved:Preferences=serde_json::from_str(&serde_json::to_string(&p).unwrap()).unwrap();
        assert_eq!(saved.panel_colour,"light");assert!(saved.enabled_providers.is_empty());
        p.panel_colour="invalid".into();p.normalize(&[]);assert_eq!(p.panel_colour,"black");
    }
    #[test] fn usage_bar_style_defaults_and_normalizes(){
        let mut p:Preferences=serde_json::from_str("{}").unwrap();
        assert_eq!(p.usage_bar_style,"continuous");
        p.usage_bar_style="segmented".into();p.normalize(&[]);assert_eq!(p.usage_bar_style,"segmented");
        p.usage_bar_style="ticks".into();p.normalize(&[]);assert_eq!(p.usage_bar_style,"continuous");
    }
    #[test] fn tray_only_keeps_collection_and_legacy_defaults() {
        let mut p:Preferences=serde_json::from_str(r#"{"enabledProviders":["codex"],"panelVisible":false,"trayShowsUsage":true,"trayStyle":"ring","trayAccount":"codex"}"#).unwrap();
        p.normalize(&["codex".into()]);assert!(!p.panel_visible);assert_eq!(p.enabled_providers,vec!["codex"]);assert_eq!(p.tray_account.as_deref(),Some("codex"));
        let restored:Preferences=serde_json::from_str(&serde_json::to_string(&p).unwrap()).unwrap();assert_eq!(p,restored);
        let legacy:Preferences=serde_json::from_str("{}").unwrap();assert!(legacy.panel_visible);assert!(!legacy.tray_shows_usage);
        p.enabled_providers.clear();p.tray_style="broken".into();p.normalize(&["codex".into()]);assert!(p.tray_account.is_none());assert_eq!(p.tray_style,"figure");
    }
    #[test] fn first_launch_never_enables_discovered_services() {
        let mut p = Preferences::default(); p.normalize(&["codex".into()]);
        assert!(p.enabled_providers.is_empty());
    }
    #[test] fn additional_accounts_keep_independent_preferences_and_legacy_defaults() {
        let mut p:Preferences=serde_json::from_str(r#"{"enabledProviders":["codex"],"accountAppearance":{"codex":{"body":"gem"}}}"#).unwrap();
        assert!(p.accounts.is_empty());
        p.accounts.push(crate::accounts::Account{id:"codex--account-ab".into(),provider:"codex".into()});
        p.enabled_providers.push("codex--account-ab".into());p.account_labels.insert("codex--account-ab".into(),"工作".into());
        p.pinned_windows.insert("codex--account-ab".into(),"secondary_window".into());
        p.normalize(&["codex".into()]);
        assert_eq!(p.enabled_providers.len(),2);assert_eq!(p.account_appearance["codex"].body,"gem");
        assert_eq!(p.pinned_windows["codex--account-ab"],"secondary_window");
        let roundtrip:Preferences=serde_json::from_str(&serde_json::to_string(&p).unwrap()).unwrap();assert_eq!(p,roundtrip);
        p.accounts.clear();p.normalize(&["codex".into()]);
        assert_eq!(p.enabled_providers,vec!["codex"]);assert!(p.account_labels.is_empty());assert!(p.pinned_windows.is_empty());
    }
    #[test] fn restores_order_without_duplicates_or_unknown_accounts() {
        let mut p = Preferences { enabled_providers: vec!["codex".into(), "gone".into(), "codex".into()],
            provider_order: vec!["codex".into(), "codex".into(), "gone".into()], ..Default::default() };
        p.normalize(&["claude".into(), "codex".into()]);
        assert_eq!(p.enabled_providers, vec!["codex"]);
        assert_eq!(p.provider_order, vec!["codex", "claude"]);
    }
    #[test] fn the_antigravity_route_is_one_of_three_and_defaults_to_auto(){
        let mut p:Preferences=serde_json::from_str("{}").unwrap();
        assert_eq!(p.antigravity_usage_source,"auto");
        p.antigravity_usage_source="cli".into();p.normalize(&[]);assert_eq!(p.antigravity_usage_source,"cli");
        p.antigravity_usage_source="desktop".into();p.normalize(&[]);assert_eq!(p.antigravity_usage_source,"desktop");
        // A route this build does not know reads through the app and falls back
        // to the CLI, rather than refusing to read anything at all.
        p.antigravity_usage_source="endpoint".into();p.normalize(&[]);assert_eq!(p.antigravity_usage_source,"auto");
        let roundtrip:Preferences=serde_json::from_str(&serde_json::to_string(&p).unwrap()).unwrap();
        assert_eq!(roundtrip.antigravity_usage_source,"auto");
    }
    #[test] fn group_ring_choices_keep_known_accounts_and_drop_an_empty_choice(){
        let mut p:Preferences=serde_json::from_str(r#"{"splitAccounts":["antigravity","gone","antigravity"],"shownModelGroups":{"antigravity":["Gemini","","Gemini","Gemini 4 Argon"],"gone":["Gemini"],"antigravity--account-ab":["","  "]}}"#).unwrap();
        p.normalize(&["antigravity".into()]);
        assert_eq!(p.split_accounts,vec!["antigravity"]);
        assert_eq!(p.shown_model_groups["antigravity"],vec!["Gemini","Gemini 4 Argon"]);
        assert!(!p.shown_model_groups.contains_key("gone"));
        // A list with nothing in it is a choice to keep the account whole, so it
        // is stored as no choice at all — the missing entry that means every
        // group — rather than as a choice matching nothing.
        assert!(!p.shown_model_groups.contains_key("antigravity--account-ab"));
        let roundtrip:Preferences=serde_json::from_str(&serde_json::to_string(&p).unwrap()).unwrap();
        assert_eq!(roundtrip.shown_model_groups,p.shown_model_groups);
    }
    #[test] fn malformed_settings_do_not_enable_everything() {
        let p: Preferences = serde_json::from_str("{}").unwrap();
        assert!(p.enabled_providers.is_empty());
    }
    #[test] fn automatic_refresh_is_default_and_fixed_choice_keeps_its_interval() {
        let default:Preferences=serde_json::from_str("{}").unwrap();
        assert!(default.refresh_automatic);
        let mut fixed:Preferences=serde_json::from_str(r#"{"refreshAutomatic":false,"refreshSeconds":600}"#).unwrap();
        fixed.normalize(&[]);assert!(!fixed.refresh_automatic);assert_eq!(fixed.refresh_seconds,600);
        let saved:Preferences=serde_json::from_str(&serde_json::to_string(&fixed).unwrap()).unwrap();
        assert!(!saved.refresh_automatic);assert_eq!(saved.refresh_seconds,600);
    }
    #[test] fn clamps_external_preferences() {
        let mut p = Preferences { panel_size: f64::NAN, warning_threshold: 5.0, refresh_seconds: 0, ..Default::default() };
        p.normalize(&[]); assert_eq!(p.panel_size, 1.0); assert_eq!(p.warning_threshold, 0.9); assert_eq!(p.refresh_seconds, 120);
    }
    #[test] fn the_window_starter_is_off_and_between_seven_and_twenty_three() {
        // **Off is the shipped state.** It is the one setting that sends
        // something on the reader's behalf, so an absent value must not mean
        // "on" and must not pick hours nobody chose.
        let p:Preferences=serde_json::from_str("{}").unwrap();
        assert!(p.primed_providers.is_empty());
        assert_eq!(p.primer_hours.start,7);assert_eq!(p.primer_hours.end,23);
        assert!(p.primer_run_times.is_empty());assert!(p.primer_run_outcomes.is_empty());
    }
    #[test] fn the_window_starter_only_keeps_providers_it_can_start() {
        let mut p=Preferences { primed_providers: vec!["codex".into(),"codex".into(),"cursor".into(),"claude-code".into()], ..Default::default() };
        p.primer_hours=crate::window_starter::PrimerHours { start: 31, end: 40 };
        p.normalize(&["codex".into()]);
        // A source from a build that knew a longer list is dropped rather than
        // carried into a send; the duplicate would start one window twice.
        assert_eq!(p.primed_providers, vec!["codex", "claude-code"]);
        assert_eq!(p.primer_hours.start,23);assert_eq!(p.primer_hours.end,23);
        let roundtrip:Preferences=serde_json::from_str(&serde_json::to_string(&p).unwrap()).unwrap();
        assert_eq!(p,roundtrip);
    }
    #[test] fn an_extension_account_survives_normalising_without_a_credential() {
        // An extension is an account with no credential and no provider in
        // `accounts::SUPPORTED`. Its derived id is the one proof it belongs.
        let mut p=Preferences::default();
        let account=crate::extension::account_id("acme-quota");
        p.accounts.push(crate::accounts::Account { id: account.clone(), provider: crate::extension::ACCOUNT_PROVIDER.into() });
        p.enabled_providers.push(account.clone());
        p.normalize(&[]);
        assert_eq!(p.accounts.len(),1);
        assert!(p.enabled_providers.contains(&account));
        // A forged provider or a mangled id is still refused.
        p.accounts.push(crate::accounts::Account { id: account.clone(), provider: "extension--account-zz".into() });
        p.accounts.push(crate::accounts::Account { id: "extension--account-zzzz".into(), provider: crate::extension::ACCOUNT_PROVIDER.into() });
        p.normalize(&[]);
        assert_eq!(p.accounts.len(),1);
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct AccountAppearance {
    pub detailed_card: bool,
    /// None keeps the current default; Some(empty) means the limits alone.
    pub card_parts: Option<Vec<String>>,
    pub animated_mark: bool,
    pub robot_figure: Option<String>,
    pub robot_material: String,
    pub persona: String,
    pub body: String,
    pub mark_colour: Option<String>,
    pub ring_colour: Option<String>,
}
impl Default for AccountAppearance {
    fn default() -> Self { Self { detailed_card: false, card_parts: None, animated_mark: false, robot_figure: None, robot_material: "flat".into(), persona: "automatic".into(), body: "blob".into(), mark_colour: None, ring_colour: None } }
}
impl AccountAppearance {
    fn normalize(&mut self) {
        const PARTS: [&str; 12] = ["plan", "updated", "forecast", "value", "amounts", "figures", "chart", "topModel", "cacheHitRate", "promptCache", "lastStep", "resetCredits"];
        if let Some(parts) = &mut self.card_parts {
            let selected = PARTS.iter().filter(|part| parts.iter().any(|p| p == **part));
            *parts = selected.map(|part| (*part).to_owned()).collect();
        }
        if self.robot_figure.as_deref().is_some_and(|figure| ![
            "mug", "ghost", "cat", "lightbulb", "chat", "cloud", "egg", "controller",
            "bunny", "star", "rocket", "book", "camera", "shield", "sprout", "brain",
            "bull", "palette", "magnifier", "witch-hat", "paint-brush", "barbell", "watering-can",
        ].contains(&figure)) {
            self.robot_figure = None;
        }
        if self.robot_material != "fluffy" { self.robot_material = "flat".into(); }
        if !["automatic","calm","eager","steady","curious","sleepy","playful","stoic","proud"].contains(&self.persona.as_str()) { self.persona = "automatic".into(); }
        if !["blob","pebble","bean","egg","squircle","tablet","capsule","cylinder","hex","gem","crystal","wedge","shield","dome","arch","cloud","teardrop","leaf"].contains(&self.body.as_str()) { self.body = "blob".into(); }
        for colour in [&mut self.mark_colour, &mut self.ring_colour] {
            if !colour.as_ref().is_some_and(|s| s.len()==7 && s.starts_with('#') && s[1..].chars().all(|c| c.is_ascii_hexdigit())) { *colour=None; }
        }
    }
}

#[cfg(test)] mod appearance_tests {
    use super::*;
    #[test] fn appearance_is_opt_in_and_malformed_values_fall_back() {
        let mut a=AccountAppearance {persona:"unknown".into(),body:"unknown".into(),mark_colour:Some("red;url(x)".into()),ring_colour:Some("#123abc".into()),..Default::default()};
        a.normalize();assert!(!a.animated_mark);assert_eq!(a.persona,"automatic");assert_eq!(a.body,"blob");assert_eq!(a.mark_colour,None);assert_eq!(a.ring_colour.as_deref(),Some("#123abc"));
    }
    #[test] fn account_appearance_round_trips_without_enabling_monitoring() {
        let mut p=Preferences::default();p.account_appearance.insert("codex".into(),AccountAppearance{animated_mark:true,persona:"curious".into(),body:"gem".into(),..Default::default()});
        let saved:Preferences=serde_json::from_str(&serde_json::to_string(&p).unwrap()).unwrap();
        assert_eq!(saved,p);assert!(saved.enabled_providers.is_empty());
    }
    #[test]
    fn card_parts_keep_default_and_explicit_empty_distinct() {
        let legacy: AccountAppearance = serde_json::from_str(r#"{"detailedCard":false}"#).unwrap();
        assert_eq!(legacy.card_parts, None);
        let mut compact: AccountAppearance = serde_json::from_str(r#"{"cardParts":[]}"#).unwrap();
        compact.normalize();
        assert_eq!(compact.card_parts, Some(vec![]));
        let mut custom: AccountAppearance = serde_json::from_str(r#"{"cardParts":["chart","plan","chart","removed"]}"#).unwrap();
        custom.normalize();
        assert_eq!(custom.card_parts, Some(vec!["plan".into(), "chart".into()]));
    }

    #[test]
    fn legacy_robots_keep_enablement_and_colour_with_automatic_flat_figure() {
        let mut legacy: AccountAppearance = serde_json::from_str(r##"{"animatedMark":true,"body":"gem","markColour":"#334455"}"##).unwrap();
        legacy.normalize();
        assert!(legacy.animated_mark);
        assert_eq!(legacy.mark_colour.as_deref(), Some("#334455"));
        assert_eq!(legacy.robot_figure, None);
        assert_eq!(legacy.robot_material, "flat");
        legacy.robot_figure = Some("cat".into());
        legacy.robot_material = "fluffy".into();
        legacy.normalize();
        assert_eq!(legacy.robot_figure.as_deref(), Some("cat"));
        assert_eq!(legacy.robot_material, "fluffy");
    }
}

#[cfg(test)] mod currency_tests {
    use super::*;
    #[test] fn the_dollar_is_the_shipped_currency_and_no_rate_is_invented() {
        // **The dollar by default**: a conversion nobody asked for would put a
        // figure on screen at a rate from some other day.
        let p: Preferences = serde_json::from_str("{}").unwrap();
        assert_eq!(p.display_currency, "usd");
        assert!(p.manual_exchange_rates.is_empty());
    }
    #[test] fn a_chosen_currency_and_typed_rate_survive_a_round_trip() {
        let mut p = Preferences { display_currency: "cny".into(), ..Default::default() };
        p.manual_exchange_rates.insert("CNY".into(), 7.12);
        p.normalize(&[]);
        assert_eq!(p.display_currency, "cny");
        assert_eq!(p.manual_exchange_rates.get("CNY"), Some(&7.12));
        let roundtrip: Preferences = serde_json::from_str(&serde_json::to_string(&p).unwrap()).unwrap();
        assert_eq!(p, roundtrip);
    }
    #[test] fn a_currency_or_rate_from_another_build_is_dropped_not_carried() {
        let mut p = Preferences { display_currency: "eur".into(), ..Default::default() };
        p.manual_exchange_rates.insert("CNY".into(), 7.12);
        p.manual_exchange_rates.insert("USD".into(), 1.0);
        p.manual_exchange_rates.insert("EUR".into(), 1.1);
        p.manual_exchange_rates.insert("XYZ".into(), 2.0);
        p.manual_exchange_rates.insert("JPY".into(), 0.0);
        p.manual_exchange_rates.insert("KRW".into(), f64::NAN);
        p.manual_exchange_rates.insert("TWD".into(), -3.0);
        p.normalize(&[]);
        // Not a currency Pulse offers is back to the dollar, not to a guess.
        assert_eq!(p.display_currency, "usd");
        // Only the one usable offered rate is kept; the dollar itself is not a
        // conversion, and zero, a negative or a NaN would print a figure that
        // is not money.
        assert_eq!(p.manual_exchange_rates.len(), 1);
        assert_eq!(p.manual_exchange_rates.get("CNY"), Some(&7.12));
    }
}

#[cfg(test)] mod network_tests {
    use super::*;
    #[test] fn legacy_preferences_follow_system_without_enabling_accounts(){
        let p:Preferences=serde_json::from_str("{}").unwrap();assert_eq!(p.network_proxy,crate::proxy::NetworkProxySettings::default());assert!(p.enabled_providers.is_empty());
    }
    #[test] fn manual_network_roundtrip_and_invalid_settings_keep_selection(){
        let mut p:Preferences=serde_json::from_str(r#"{"networkProxy":{"mode":"manual","kind":"socks5","host":"127.0.0.1","port":1080},"enabledProviders":["codex"]}"#).unwrap();
        p.normalize(&["codex".into()]);assert_eq!(p.network_proxy.endpoint().as_deref(),Some("socks5h://127.0.0.1:1080"));
        assert_eq!(serde_json::from_str::<Preferences>(&serde_json::to_string(&p).unwrap()).unwrap(),p);
        p.network_proxy.host="http://bad/path".into();p.normalize(&["codex".into()]);assert_eq!(p.network_proxy,crate::proxy::NetworkProxySettings::default());assert_eq!(p.enabled_providers,vec!["codex"]);
    }
}
#[cfg(test)] mod span_tests {
    use super::*;
    // The nine span names are the original's `SpendSpan`, spelled the same way:
    // a stored id from another build must not survive as a span nothing draws.
    #[test] fn every_span_the_original_names_is_kept_and_anything_else_is_not() {
        for span in ["today","week","month","quarter","thisMonth","lastMonth","thisYear","all","custom"] {
            let mut p = Preferences { token_spend_span: span.into(), ..Default::default() };
            p.normalize(&[]);
            assert_eq!(p.token_spend_span, span);
        }
        let mut p = Preferences { token_spend_span: "yesterday".into(), ..Default::default() };
        p.normalize(&[]);
        assert_eq!(p.token_spend_span, "week");
    }
    // Two days in order, both real. Anything else is dropped whole rather than
    // repaired: a half-read range would narrow the pane to a span nobody chose.
    #[test] fn only_two_real_days_in_order_survive() {
        let kept = |days: Vec<&str>| {
            let mut p = Preferences { spend_custom_days: days.into_iter().map(str::to_owned).collect(), ..Default::default() };
            p.normalize(&[]);
            p.spend_custom_days
        };
        assert_eq!(kept(vec!["2026-09-01", "2026-09-30"]), vec!["2026-09-01", "2026-09-30"]);
        assert_eq!(kept(vec!["2026-09-01", "2026-09-01"]), vec!["2026-09-01", "2026-09-01"]);
        assert!(kept(vec!["2026-09-30", "2026-09-01"]).is_empty());
        assert!(kept(vec!["2026-02-30", "2026-03-01"]).is_empty());
        assert!(kept(vec!["2026-09-01"]).is_empty());
        assert!(kept(vec!["2026-09-01", "2026-09-30", "2026-10-01"]).is_empty());
        assert!(kept(vec!["2026-9-1", "2026-09-30"]).is_empty());
        assert!(kept(vec!["2026-09-01T00:00:00Z", "2026-09-30"]).is_empty());
    }
}

#[cfg(test)] mod capture_tests {
    use super::*;
    // **Off by default.** The original ships it off for the same reason: it
    // hides the panel from the reader's own screenshots too, so switching it on
    // is a choice rather than something done to them.
    #[test] fn hiding_from_capture_is_off_until_asked_for() {
        let p: Preferences = serde_json::from_str("{}").unwrap();
        assert!(!p.hide_from_capture);
        let roundtrip: Preferences = serde_json::from_str(&serde_json::to_string(&p).unwrap()).unwrap();
        assert!(!roundtrip.hide_from_capture);
    }
    // The field is `hideFromCapture` over the wire; a stored `true` has to come
    // back `true` or the panel silently reappears in every capture after a
    // restart.
    #[test] fn a_chosen_value_survives_a_round_trip() {
        let p = Preferences { hide_from_capture: true, ..Default::default() };
        let wire = serde_json::to_string(&p).unwrap();
        assert!(wire.contains("\"hideFromCapture\":true"), "{wire}");
        let roundtrip: Preferences = serde_json::from_str(&wire).unwrap();
        assert!(roundtrip.hide_from_capture);
    }
}
