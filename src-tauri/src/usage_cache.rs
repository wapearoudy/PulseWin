//! Last-good readings retain their observation time and are visibly stale.
use crate::model::{ProviderUsage, Snapshot, WindowKind};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

pub const MAX_AGE_SECONDS: i64 = 24 * 60 * 60;
pub fn timestamp(value: &str) -> Option<i64> {
    DateTime::parse_from_rfc3339(value).ok().map(|d| d.timestamp())
}
pub fn valid_age(value: &str, now: i64) -> bool {
    timestamp(value).is_some_and(|at| (0..=MAX_AGE_SECONDS).contains(&(now - at)))
}
pub fn window_usable(window: &crate::model::UsageWindow, now: i64) -> bool {
    window.resets_at.as_deref().and_then(timestamp).map_or(true, |at| at > now)
}

/// Balance and purchased packs never reset. Credits require a moved reset date.
pub fn moved_on(previous: &crate::model::UsageWindow, next: &crate::model::UsageWindow) -> bool {
    if matches!(next.kind, WindowKind::Balance | WindowKind::TopUp) { return false; }
    if let (Some(old), Some(new)) = (previous.resets_at.as_deref().and_then(timestamp), next.resets_at.as_deref().and_then(timestamp)) {
        if new > old + 60 { return true; }
    }
    // Only a reported timed allowance can use the 40-point turnover rule.
    // Unknown/currency denominators are not evidence of a reset.
    !matches!(next.kind, WindowKind::Credits | WindowKind::SharedCredits) &&
        next.window_seconds.is_some_and(|seconds| seconds > 0.0) &&
        previous.percent_used.zip(next.percent_used).is_some_and(|(old,new)| old - new >= 40.0)
}

#[derive(Default, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UsageCache { pub version: u32, pub readings: HashMap<String, ProviderUsage> }

impl UsageCache {
    pub fn load() -> Self {
        let value = crate::settings::directory().and_then(|d| std::fs::read(d.join("usage-cache.json")).ok())
            .and_then(|b| serde_json::from_slice::<Self>(&b).ok());
        value.filter(|c| c.version == 1).unwrap_or_else(|| Self { version: 1, ..Self::default() })
    }
    pub fn save(&self) -> Result<(), String> {
        let dir = crate::settings::directory().ok_or("APPDATA is unavailable")?;
        std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        // Cache contains readings only, never the request headers or credentials.
        std::fs::write(dir.join("usage-cache.json"), crate::settings::to_json(self)).map_err(|e| e.to_string())
    }
    pub fn restored(&self, enabled: &[String], now: i64) -> Vec<ProviderUsage> {
        enabled.iter().filter_map(|id| self.readings.get(id)).filter(|p| valid_age(&p.fetched_at, now))
            .map(|p| { let mut p=p.clone(); p.stale=true; p.windows.retain(|w| window_usable(w,now)); p }).collect()
    }
    pub fn reconcile(&mut self, raw: ProviderUsage, now: i64) -> ProviderUsage {
        if raw.error.is_none() && !raw.stale && valid_age(&raw.fetched_at,now) {
            let banked=self.readings.get(&raw.id);
            // A dated capture older than a banked endpoint must not roll it back.
            if banked.is_some_and(|p| timestamp(&p.fetched_at) > timestamp(&raw.fetched_at)) {
                let mut display=banked.unwrap().clone(); display.stale=true;
                display.windows.retain(|w| window_usable(w,now)); return display;
            }
            self.readings.insert(raw.id.clone(),raw.clone());
            let mut display=raw;
            let count=display.windows.len();
            display.windows.retain(|w| window_usable(w,now));
            display.stale=count!=display.windows.len();
            return display;
        }
        if let Some(previous)=self.readings.get(&raw.id).filter(|p| valid_age(&p.fetched_at,now)) {
            let mut display=previous.clone(); display.stale=true; display.error=raw.error;
            display.windows.retain(|w| window_usable(w,now)); return display;
        }
        raw
    }
}

pub fn stored_snapshot(enabled: &[String]) -> Snapshot {
    let cache=UsageCache::load();
    Snapshot { providers: cache.restored(enabled,Utc::now().timestamp()), fetched_at: crate::model::now_rfc3339() }
}

#[cfg(test)] mod tests {
    use super::*;
    use crate::model::{UsageWindow,WindowKind};
    fn live(percent:f64, at:i64) -> ProviderUsage {
        let mut p=ProviderUsage::ok("x","X",vec![UsageWindow::new("week",Some(percent))]);
        p.fetched_at=DateTime::from_timestamp(at,0).unwrap().to_rfc3339();p
    }
    #[test] fn a_failed_fetch_keeps_the_last_real_reading_and_its_time() {
        let mut cache=UsageCache::default();cache.reconcile(live(72.,1000),1000);
        let p=cache.reconcile(ProviderUsage::failed("x","X","HTTP 503"),1200);
        assert!(p.stale);assert_eq!(p.windows[0].percent_used,Some(72.));assert_eq!(timestamp(&p.fetched_at),Some(1000));
    }
    #[test] fn undated_and_old_readings_do_not_become_current() {
        let mut cache=UsageCache::default();cache.reconcile(live(72.,1000),1000);
        assert!(cache.restored(&["x".into()],1000+MAX_AGE_SECONDS+1).is_empty());
        let mut p=live(90.,1100);p.fetched_at="invalid".into();cache.reconcile(p,1100);
        assert_eq!(cache.readings["x"].windows[0].percent_used,Some(72.));
    }
    #[test] fn expired_windows_are_never_restored_as_available_quota() {
        let mut cache=UsageCache::default();let mut p=live(90.,1000);
        p.windows[0].resets_at=Some(DateTime::from_timestamp(1100,0).unwrap().to_rfc3339());cache.reconcile(p,1000);
        assert!(cache.restored(&["x".into()],1200)[0].windows.is_empty());
    }
    #[test] fn a_live_reply_with_an_expired_window_does_not_display_it() {
        let mut cache=UsageCache::default();let mut p=live(90.,1000);
        p.windows[0].resets_at=Some(DateTime::from_timestamp(900,0).unwrap().to_rfc3339());
        let display=cache.reconcile(p,1000);
        assert!(display.windows.is_empty());assert!(display.stale);
        assert_eq!(cache.readings["x"].windows.len(),1);
    }
    #[test] fn a_top_up_is_not_a_reset_and_unknown_fraction_drops_are_not_evidence() {
        let old=UsageWindow::new("balance",Some(90.));let new=UsageWindow::new("balance",Some(0.));
        assert!(!moved_on(&old,&new));
        assert!(!moved_on(&old,&new.clone().with_duration(Some(3600.)).with_kind(WindowKind::TopUp)));
        assert!(moved_on(&old,&new.with_duration(Some(3600.))));
    }
}
