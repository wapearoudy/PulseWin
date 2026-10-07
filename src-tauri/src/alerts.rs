//! Evidence-based, persistent alert memory. Cached display figures never prove a reset.
use crate::{model::{ProviderUsage, UsageWindow, WindowKind}, usage_cache::{timestamp,valid_age,window_usable,moved_on}};
use serde::{Deserialize,Serialize};
use std::collections::HashMap;

#[derive(Debug,Clone,Default,PartialEq,Serialize,Deserialize)]
#[serde(rename_all="camelCase",default)]
pub struct AlertPreferences {
    pub threshold: Option<u32>, pub on_reset: bool, pub on_failure: bool,
    pub low_balance: HashMap<String,f64>,
}
impl AlertPreferences {
    pub fn normalize(&mut self,known:&[String]) {
        self.threshold=self.threshold.filter(|v| [60,70,75,80,85,90,95].contains(v));
        self.low_balance.retain(|id,v| known.contains(id)&&v.is_finite()&&*v>0.0);
    }
}
#[derive(Debug,Clone,PartialEq,Serialize,Deserialize)]
#[serde(rename_all="camelCase")]
pub struct UsageAlert { pub account:String, pub title:String, pub body:String, pub kind:String }
#[derive(Default,Clone,Serialize,Deserialize)]
#[serde(rename_all="camelCase",default)]
pub struct AccountMemory {
    pub windows:HashMap<String,WindowMemory>, pub failures:u32,
    pub reported_failure:bool, pub low_balance_warned_for:Option<f64>,
}
#[derive(Clone,Serialize,Deserialize)]
#[serde(rename_all="camelCase")]
pub struct WindowMemory {
    pub announced:u32, pub reading:UsageWindow,
    // A legacy percentage-only alert must not suppress the first paced alert.
    #[serde(default)] pub pace_announced:u32,
}
#[derive(Default,Clone,Serialize,Deserialize)]
#[serde(rename_all="camelCase",default)]
pub struct AlertMemory { pub accounts:HashMap<String,AccountMemory> }

fn failed(raw:&ProviderUsage)->bool {
    let Some(error)=raw.error.as_deref() else {return false};
    // Setup, missing credentials, an inactive app and unknown conditions stay neutral.
    let lower=error.to_lowercase();
    lower.starts_with("request failed") || lower.starts_with("http ") ||
        lower.starts_with("cannot read body") || lower.starts_with("bad json") ||
        lower.contains("timed out") || lower.contains("key was refused") || lower.contains("token was refused")
}
fn alert(raw:&ProviderUsage,kind:&str,body:String)->UsageAlert {
    UsageAlert {account:raw.id.clone(), title:format!("Pulse · {}",raw.name),body,kind:kind.into()}
}
impl AlertMemory {
    pub fn load()->Self {
        crate::settings::directory().and_then(|d|std::fs::read(d.join("alerts.json")).ok())
            .and_then(|b|serde_json::from_slice(&b).ok()).unwrap_or_default()
    }
    pub fn save(&self)->Result<(),String> {
        let dir=crate::settings::directory().ok_or("APPDATA is unavailable")?;
        std::fs::create_dir_all(&dir).map_err(|e|e.to_string())?;
        std::fs::write(dir.join("alerts.json"),crate::settings::to_json(self)).map_err(|e|e.to_string())
    }
    pub fn observe(&mut self,raw:&ProviderUsage,display:&ProviderUsage,prefs:&AlertPreferences,now:i64)->Vec<UsageAlert> {
        let record=self.accounts.entry(raw.id.clone()).or_default();let mut alerts=vec![];
        if failed(raw) {
            let old_enough=display.windows.is_empty() || timestamp(&display.fetched_at).is_some_and(|at|now-at>=1800);
            if old_enough { record.failures=record.failures.saturating_add(1); }
            if prefs.on_failure && record.failures>=3 && !record.reported_failure {
                record.reported_failure=true;
                alerts.push(alert(raw,"unreadable","连续三次检查未能读取用量，请检查连接。".into()));
            }
            return alerts;
        }
        if raw.error.is_some() { return alerts; }
        record.failures=0;record.reported_failure=false;
        if raw.stale || !valid_age(&raw.fetched_at,now) {return alerts;}
        if let Some(balance)=&raw.credit_remaining {
            if let Some(threshold)=prefs.low_balance.get(&raw.id) {
                if balance.amount.is_finite()&&balance.amount<*threshold&&record.low_balance_warned_for!=Some(*threshold) {
                    record.low_balance_warned_for=Some(*threshold);
                    alerts.push(alert(raw,"lowBalance",format!("余额 {} {:.2}，低于设定的 {:.2}。",balance.currency,balance.amount,threshold)));
                } else if balance.amount>=*threshold {record.low_balance_warned_for=None;}
            }
        }
        for (index,window) in raw.windows.iter().enumerate().filter(|(_,w)|window_usable(w,now)) {
            // Label is the current adapter contract; index separates repeated labels.
            let key=window.id.clone().unwrap_or_else(||format!("{}:{}",window.label,index));
            let memory=record.windows.entry(key).or_insert_with(||WindowMemory {announced:0,pace_announced:0,reading:window.clone()});
            if moved_on(&memory.reading,window) {
                if (memory.pace_announced>0 || memory.announced==100) && prefs.threshold.is_some() && prefs.on_reset {
                    alerts.push(alert(raw,"reset",format!("{} 的额度窗口已重置。",window.label)));
                }
                memory.announced=0;
                memory.pace_announced=0;
            }
            memory.reading=window.clone();
            let spent=window.is_exhausted || !matches!(window.kind,WindowKind::Balance|WindowKind::TopUp) &&
                window.percent_used.is_some_and(|v|v.is_finite()&&v.floor()>=100.0);
            if let Some(threshold)=prefs.threshold {
                let step=if spent {100} else if window.quota_warning(threshold as f64,now){threshold}else{0};
                let first=if spent {memory.announced<100} else {step>memory.pace_announced && memory.announced<100};
                if step>0 && first {
                    memory.announced=memory.announced.max(step);
                    memory.pace_announced=memory.pace_announced.max(step);
                    alerts.push(alert(raw,if spent {"spent"}else{"approaching"},if spent {
                        format!("{} 的额度已用尽。",window.label)
                    } else {format!("{} 已使用 {:.1}%，时间进度 {:.1}%，用量消耗超前。",window.label,window.percent_used.unwrap(),window.time_progress_percent(now).unwrap())}));
                }
            }
        }
        alerts
    }
}

#[cfg(test)] mod tests {
    use super::*;
    use chrono::DateTime;
    fn reading(percent:f64,reset:i64)->ProviderUsage {
        let mut p=ProviderUsage::ok("codex","Codex",vec![UsageWindow::new("5h",Some(percent)).with_duration(Some(18000.)).with_reset(Some(DateTime::from_timestamp(reset,0).unwrap().to_rfc3339()))]);
        p.fetched_at=DateTime::from_timestamp(1000,0).unwrap().to_rfc3339();p
    }
    fn prefs()->AlertPreferences {AlertPreferences{threshold:Some(90),on_reset:true,on_failure:true,..Default::default()}}
    #[test] fn equal_or_slower_time_progress_stays_quiet_and_does_not_consume_a_warning() {
        let mut memory=AlertMemory::default();let mut preferences=prefs();preferences.threshold=Some(75);
        // 5h window: 4,500 seconds left means exactly 75% elapsed.
        let equal=reading(75.,5500);
        assert!(memory.observe(&equal,&equal,&preferences,1000).is_empty());
        let behind=reading(80.,2800); // 90% elapsed.
        assert!(memory.observe(&behind,&behind,&preferences,1000).is_empty());
        let mut memory=AlertMemory::default();memory.observe(&equal,&equal,&preferences,1000);
        let ahead=reading(80.,5500);
        let alerts=memory.observe(&ahead,&ahead,&preferences,1000);
        assert_eq!(alerts.len(),1);assert_eq!(alerts[0].kind,"approaching");
        assert!(alerts[0].body.contains("时间进度 75.0%"));
        assert!(memory.observe(&ahead,&ahead,&preferences,1000).is_empty());
    }
    #[test] fn early_fast_usage_still_obeys_the_minimum_threshold() {
        let mut memory=AlertMemory::default();let mut preferences=prefs();preferences.threshold=Some(75);
        let below=reading(74.9,17200); // 10% elapsed.
        assert!(memory.observe(&below,&below,&preferences,1000).is_empty());
        let reached=reading(75.,17200);
        assert_eq!(memory.observe(&reached,&reached,&preferences,1000)[0].kind,"approaching");
    }
    #[test] fn approaching_requires_valid_reported_cycle_information() {
        for mutation in 0..7 {
            let mut p=reading(95.,5500);
            match mutation {
                0=>p.windows[0].window_seconds=None,
                1=>p.windows[0].resets_at=None,
                2=>p.windows[0].resets_at=Some("invalid".into()),
                3=>p.windows[0].window_seconds=Some(f64::NAN),
                4=>p.windows[0].window_seconds=Some(0.),
                5=>p=reading(95.,20000), // Reported start is still in the future.
                _=>p=reading(95.,1000), // Already reset.
            }
            assert!(AlertMemory::default().observe(&p,&p,&prefs(),1000).is_empty(),"case {mutation}");
        }
        let mut locked=reading(20.,5500);locked.windows[0].window_seconds=None;locked.windows[0].is_exhausted=true;
        assert_eq!(AlertMemory::default().observe(&locked,&locked,&prefs(),1000)[0].kind,"spent");
    }
    #[test] fn five_hour_weekly_and_independent_accounts_have_independent_pace() {
        let mut p=reading(75.,5500);p.windows[0].id=Some("5h".into());
        p.windows.push(UsageWindow::new("7d",Some(90.)).with_id(Some("week".into())).with_duration(Some(604800.)).with_reset(Some(DateTime::from_timestamp(152200,0).unwrap().to_rfc3339())));
        let mut preferences=prefs();preferences.threshold=Some(75);let mut memory=AlertMemory::default();
        let alerts=memory.observe(&p,&p,&preferences,1000);assert_eq!(alerts.len(),1);assert!(alerts[0].body.starts_with("7d"));
        let mut other=p.clone();other.id="codex--account-work".into();
        assert_eq!(memory.observe(&other,&other,&preferences,1000)[0].account,other.id);
        assert!(memory.observe(&p,&p,&preferences,1000).is_empty());
    }
    #[test] fn old_fixed_threshold_memory_does_not_suppress_the_first_paced_alert() {
        let p=reading(93.,10000);
        let saved=serde_json::json!({"accounts":{"codex":{"windows":{"5h:0":{"announced":90,"reading":p.windows[0]}},"failures":0,"reportedFailure":false,"lowBalanceWarnedFor":null}}});
        let mut memory:AlertMemory=serde_json::from_value(saved).unwrap();
        assert_eq!(memory.observe(&p,&p,&prefs(),1000)[0].kind,"approaching");
        let mut restored:AlertMemory=serde_json::from_slice(&serde_json::to_vec(&memory).unwrap()).unwrap();
        assert!(restored.observe(&p,&p,&prefs(),1000).is_empty());
    }
    #[test] fn legacy_exhaustion_is_not_repeated_and_legacy_percentage_alone_does_not_trigger_a_reset() {
        let p=reading(100.,10000);
        let saved=serde_json::json!({"accounts":{"codex":{"windows":{"5h:0":{"announced":100,"reading":p.windows[0]}}}}});
        let mut memory:AlertMemory=serde_json::from_value(saved).unwrap();
        assert!(memory.observe(&p,&p,&prefs(),1000).is_empty());
        let next=reading(0.,20000);
        assert_eq!(memory.observe(&next,&next,&prefs(),1000)[0].kind,"reset");
        let saved=serde_json::json!({"accounts":{"codex":{"windows":{"5h:0":{"announced":90,"reading":p.windows[0]}}}}});
        let mut memory:AlertMemory=serde_json::from_value(saved).unwrap();
        assert!(memory.observe(&next,&next,&prefs(),1000).is_empty());
    }
    #[test] fn cached_failed_future_and_expired_observations_never_prove_fast_consumption() {
        let good=reading(95.,5500);
        for case in 0..4 {
            let mut p=good.clone();let now=match case {2=>999,3=>1000+86401,_=>1000};
            if case==0 {p.stale=true;}if case==1 {p.error=Some("HTTP 503".into());}
            assert!(AlertMemory::default().observe(&p,&good,&prefs(),now).is_empty());
        }
    }
    #[test] fn first_existing_limit_warns_once_and_oscillation_does_not_rearm() {
        let mut m=AlertMemory::default();let p=reading(93.,10000);
        assert_eq!(m.observe(&p,&p,&prefs(),1000).len(),1);
        for percent in [89.,93.,95.] {let p=reading(percent,10000);assert!(m.observe(&p,&p,&prefs(),1000).is_empty());}
        let p=reading(100.,10000);assert_eq!(m.observe(&p,&p,&prefs(),1000)[0].kind,"spent");
    }
    #[test] fn stale_cache_does_not_announce_reset_and_99_point_6_is_not_spent() {
        let mut m=AlertMemory::default();let p=reading(99.6,10000);
        assert_eq!(m.observe(&p,&p,&prefs(),1000)[0].kind,"approaching");
        let mut p=reading(0.,20000);p.stale=true;assert!(m.observe(&p,&p,&prefs(),1000).is_empty());
        p.stale=false;assert_eq!(m.observe(&p,&p,&prefs(),1000)[0].kind,"reset");
    }
    #[test] fn failures_need_three_and_first_30_minutes_of_cache_are_protected() {
        let mut m=AlertMemory::default();let good=reading(93.,10000);let fail=ProviderUsage::failed("codex","Codex","request failed: timeout");
        for _ in 0..4 {assert!(m.observe(&fail,&good,&prefs(),1100).is_empty());}
        for _ in 0..2 {assert!(m.observe(&fail,&good,&prefs(),3000).is_empty());}
        assert_eq!(m.observe(&fail,&good,&prefs(),3000)[0].kind,"unreadable");
        assert!(m.observe(&fail,&good,&prefs(),3000).is_empty());
    }
    #[test] fn disabled_alerts_stay_quiet_and_a_balance_is_never_spent_by_fraction() {
        let mut m=AlertMemory::default();let mut p=reading(100.,10000);p.windows[0].kind=WindowKind::Balance;
        assert!(m.observe(&p,&p,&AlertPreferences::default(),1000).is_empty());
        let mut preferences=prefs();preferences.threshold=Some(95);
        assert!(m.observe(&p,&p,&preferences,1000).is_empty());
    }
    #[test] fn low_balance_rearms_only_on_top_up_or_changed_threshold() {
        let mut m=AlertMemory::default();let mut p=reading(0.,10000);
        p.credit_remaining=Some(crate::model::CreditRemaining{amount:3.,currency:"CNY".into()});
        let mut pref=AlertPreferences::default();pref.low_balance.insert("codex".into(),5.);
        assert_eq!(m.observe(&p,&p,&pref,1000)[0].kind,"lowBalance");assert!(m.observe(&p,&p,&pref,1000).is_empty());
        pref.low_balance.insert("codex".into(),10.);assert_eq!(m.observe(&p,&p,&pref,1000)[0].kind,"lowBalance");
    }
    #[test] fn neutral_setup_does_not_increment_failure_streak() {
        let mut m=AlertMemory::default();let p=ProviderUsage::failed("codex","Codex","no credentials found");
        for _ in 0..4 {assert!(m.observe(&p,&p,&prefs(),1000).is_empty());}
        assert_eq!(m.accounts["codex"].failures,0);
    }
}
