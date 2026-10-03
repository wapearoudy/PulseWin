//! User choices, distinct from credential discovery and transient usage.
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct Preferences {
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
    pub reset_celebration: bool,
    pub enabled_providers: Vec<String>,
    pub provider_order: Vec<String>,
    pub panel_size: f64,
    pub rail_spacing: f64,
    pub round_ends: bool,
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
    pub account_appearance: std::collections::HashMap<String, AccountAppearance>,
    pub pinned_windows: std::collections::HashMap<String, String>,
}

impl Default for Preferences {
    fn default() -> Self {
        Self { panel_visible:true, tray_shows_usage:false, tray_style:"figure".into(), tray_account:None, accounts: vec![], account_labels: Default::default(), open_settings_shortcut: None, toggle_panel_shortcut: None, alerts: Default::default(), token_spend_enabled: false, token_spend_span: "week".into(), reset_celebration: true, enabled_providers: vec![], provider_order: vec![], panel_size: 1.0,
            rail_spacing: 1.0, round_ends: false, shows_remaining: false,
            show_percentages: true, label_above: false, auto_collapse: true,
            warning_threshold: 0.75, refresh_seconds: 120, refresh_automatic: true, show_reset_clock: false,
            clock_remaining: false, show_second_ring: false, top_show_percentages: false, animate_activity: true, dock_alert_colour: true, show_forecast: false, hide_in_fullscreen: true, follow_active_display: false, account_appearance: Default::default(), pinned_windows: Default::default() }
    }
}

impl Preferences {
    pub fn normalize(&mut self, known: &[String]) {
        let mut seen_accounts=std::collections::HashSet::new();
        self.accounts.retain(|a| crate::accounts::valid(a) && seen_accounts.insert(a.id.clone()));
        self.accounts.truncate(64);
        let known=known.iter().cloned().chain(self.accounts.iter().map(|a|a.id.clone())).collect::<Vec<_>>();
        let known=known.as_slice();
        self.account_labels.retain(|id,value|known.contains(id) && crate::accounts::label(value).is_ok());
        for value in self.account_labels.values_mut(){*value=value.trim().to_owned();}
        for value in [&mut self.open_settings_shortcut,&mut self.toggle_panel_shortcut] {
            if value.as_deref().is_some_and(|v|crate::application::validate(v).is_err()){*value=None;}
        }
        self.alerts.normalize(known);
        if !["today","week","month"].contains(&self.token_spend_span.as_str()){self.token_spend_span="week".into();}
        self.account_appearance.retain(|id, _| known.contains(id));
        for value in self.account_appearance.values_mut() { value.normalize(); }
        self.pinned_windows.retain(|id, label| known.contains(id) && !label.is_empty());
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
    }
}

fn finite_clamp(value: f64, min: f64, max: f64, fallback: f64) -> f64 {
    if value.is_finite() { value.clamp(min, max) } else { fallback }
}

pub fn load() -> Preferences {
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
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct AccountAppearance {
    pub detailed_card: bool,
    pub animated_mark: bool,
    pub persona: String,
    pub body: String,
    pub mark_colour: Option<String>,
    pub ring_colour: Option<String>,
}
impl Default for AccountAppearance {
    fn default() -> Self { Self { detailed_card: false, animated_mark: false, persona: "automatic".into(), body: "blob".into(), mark_colour: None, ring_colour: None } }
}
impl AccountAppearance {
    fn normalize(&mut self) {
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
}
