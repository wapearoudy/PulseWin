//! Pulse's automatic 2–30 minute ladder and independent primary pacing.
//! Clock values are Unix milliseconds; waits are seconds. This module does no
//! discovery, file scanning or network access.
use std::collections::HashMap;
use crate::model::ProviderUsage;

pub const FLOOR: u64 = 120;
pub const CEILING: u64 = 1800;
pub const UNWATCHED_CEILING: u64 = 300;
pub const TIMER_FLOOR: u64 = 15;
pub const LOOK_COOLDOWN_MS: i64 = 30_000;

#[derive(Debug, Clone, Copy, Default)]
pub struct Signals {
    pub last_agent_activity: Option<i64>,
    pub last_looked: Option<i64>,
    pub panel_visible: bool,
    pub constrained: bool,
}

pub fn interval(signals: Signals, last_changed: Option<i64>, watched: bool, now: i64) -> u64 {
    if signals.constrained || !signals.panel_visible { return CEILING; }
    let cap = if watched { CEILING } else { UNWATCHED_CEILING };
    let quiet = [signals.last_agent_activity, last_changed, signals.last_looked].into_iter().flatten()
        .filter_map(|at| now.checked_sub(at).filter(|age| *age >= 0)).min();
    let wait = match quiet {
        Some(age) if age < 300_000 => FLOOR,
        Some(age) if age < 3_600_000 => 300,
        Some(age) if age < 14_400_000 => 900,
        _ => CEILING,
    };
    wait.min(cap)
}

/// Ported from Provider.spendingIsWatchedLocally and each ProviderProfile.
/// V2EX is deliberately included despite reporting tokens instead of money.
const UNWATCHED_IDS: &[&str] = &[
    "deepseek","command-code","sub2api","new-api","v2ex","abacus","aixy","amp",
    "atlas-cloud","bifrost","chutes","clawrouter","deepinfra","dev-pass","elevenlabs",
    "gitkraken","hugging-face","hyper","kilo-code","litellm","llm-proxy","manus","moonshot",
    "mistral","neuralwatt","notion-ai","nous-portal","openai-api","perplexity","poe",
    "raycast-ai","replicate","sakana","synthetic","t3-chat","typesafe","v0","venice",
    "vercel-ai-gateway","xkiro","xai-api","zenmux","zoom-mate",
];
pub fn spending_is_watched(id: &str) -> bool { !UNWATCHED_IDS.contains(&id) }

#[derive(Debug, Clone, Copy, Default)]
pub struct Primary { pub asked_at: Option<i64>, pub last_changed: Option<i64> }

#[derive(Debug, Default)]
pub struct Schedule {
    pub primary: HashMap<String, Primary>,
    pub last_looked: Option<i64>,
    last_look_refresh: Option<i64>,
}

#[derive(Debug, PartialEq)]
pub enum Plan { Ask(Vec<String>), Wait(Option<std::time::Duration>) }

impl Schedule {
    /// A signal cancels an existing one-shot timer. Check due work before
    /// applying the timer's 15-second floor again, otherwise a transcript
    /// write every two seconds can indefinitely postpone an already-due ask.
    pub fn plan(&self, enabled: &[String], automatic: bool, fixed: u64, signals: Signals, now: i64) -> Plan {
        let due=self.due(enabled,true,automatic,fixed,signals,now);
        if due.is_empty(){Plan::Wait(self.next_wait(enabled,automatic,fixed,signals,now))}else{Plan::Ask(due)}
    }
    pub fn cadence(&self, id: &str, automatic: bool, fixed: u64, mut signals: Signals, now: i64) -> u64 {
        if !automatic { return fixed.clamp(FLOOR, CEILING); }
        signals.last_looked = self.last_looked;
        interval(signals, self.primary.get(id).and_then(|entry|entry.last_changed), spending_is_watched(id), now)
    }

    pub fn due(&self, enabled: &[String], due_only: bool, automatic: bool, fixed: u64, signals: Signals, now: i64) -> Vec<String> {
        enabled.iter().filter(|id| {
            if !due_only { return true; }
            self.primary.get(id.as_str()).and_then(|entry|entry.asked_at).map_or(true, |asked|
                // A wall-clock rollback makes an old ask appear in the
                // future. Re-establish it once instead of waiting hours for
                // the new clock to catch up; the next ask uses the real now.
                asked>now || now.saturating_sub(asked) >= self.cadence(id,automatic,fixed,signals,now).saturating_sub(1) as i64 * 1000)
        }).cloned().collect()
    }

    pub fn next_wait(&self, enabled: &[String], automatic: bool, fixed: u64, signals: Signals, now: i64) -> Option<std::time::Duration> {
        let millis = enabled.iter().map(|id| {
            let cadence = self.cadence(id,automatic,fixed,signals,now) as i64 * 1000;
            self.primary.get(id).and_then(|entry|entry.asked_at).map_or(0,|asked| asked.saturating_add(cadence).saturating_sub(now).max(0))
        }).min()?;
        Some(std::time::Duration::from_millis((millis as u64).max(TIMER_FLOOR * 1000)))
    }

    /// Mark before awaiting a fetch. An error/refusal is still an ask, so it
    /// cannot remain perpetually due and spin the timer.
    pub fn asked(&mut self, ids: &[String], now: i64) {
        for id in ids { self.primary.entry(id.clone()).or_default().asked_at = Some(now); }
    }

    pub fn changed(&mut self, id: &str, now: i64) { self.primary.entry(id.into()).or_default().last_changed = Some(now); }

    /// A deliberate look can bring an old card current, at most once every
    /// 30 seconds. Never-read/error placeholders have no newest reading.
    pub fn looked(&mut self, automatic: bool, fixed: u64, signals: Signals, newest_reading: Option<i64>, now: i64) -> bool {
        let cadence = if automatic { interval(Signals{last_looked:self.last_looked,..signals},self.primary.values().filter_map(|entry|entry.last_changed).max(),true,now) }
            else { fixed.clamp(FLOOR,CEILING) };
        self.last_looked = Some(now);
        let overdue = newest_reading.is_some_and(|at|now.saturating_sub(at) > (cadence as i64 * 2 + 60) * 1000);
        let cooling = self.last_look_refresh.is_some_and(|at| (0..LOOK_COOLDOWN_MS).contains(&now.saturating_sub(at)));
        if !cooling && (cadence > FLOOR || overdue) { self.last_look_refresh = Some(now); true } else { false }
    }
}

/// Compare actual reconciled figures against the banked reading. Fetch times,
/// plan/account prose and error transitions alone cannot claim spending moved.
pub fn figures_changed(previous: Option<&ProviderUsage>, reading: &ProviderUsage) -> bool {
    if reading.stale || reading.error.is_some() { return false; }
    previous.map_or(!reading.windows.is_empty() || reading.credit_remaining.is_some(), |previous|
        previous.windows != reading.windows || previous.credit_remaining != reading.credit_remaining)
}

#[cfg(test)] mod tests {
    use super::*;
    fn visible() -> Signals { Signals{panel_visible:true,..Default::default()} }
    #[test] fn ladder_uses_recent_evidence_and_ignores_future_timestamps() {
        for (age,wait) in [(0,120),(299_999,120),(300_000,300),(3_599_999,300),(3_600_000,900),(14_399_999,900),(14_400_000,1800)] {
            assert_eq!(interval(visible(),Some(1_000_000_000-age),true,1_000_000_000),wait);
        }
        assert_eq!(interval(visible(),None,true,1_000_000_000),1800);
        assert_eq!(interval(visible(),Some(1_000_000_001),true,1_000_000_000),1800);
        assert_eq!(interval(Signals{last_agent_activity:Some(900_000),..visible()},Some(0),true,1_000_000),120);
    }
    #[test] fn hidden_and_constrained_override_the_unwatched_cap() {
        assert_eq!(interval(visible(),None,false,10_000),300);
        assert_eq!(interval(Signals{panel_visible:false,..visible()},Some(9000),false,10_000),1800);
        assert_eq!(interval(Signals{constrained:true,..visible()},Some(9000),false,10_000),1800);
        assert!(!spending_is_watched("v2ex"));assert!(!spending_is_watched("deepseek"));assert!(spending_is_watched("codex"));
    }
    #[test] fn every_unwatched_id_names_a_registered_primary() {
        // Registry construction reads no settings, credentials or CLI files.
        let registered=crate::providers::registry();
        let mut seen=std::collections::HashSet::new();
        for id in UNWATCHED_IDS {
            assert!(seen.insert(*id),"duplicate pacing ID: {id}");
            assert!(registered.iter().any(|provider|provider.id()==*id),"unknown pacing ID: {id}");
            assert_eq!(interval(visible(),None,spending_is_watched(id),0),UNWATCHED_CEILING);
        }
    }
    #[test] fn primary_providers_have_independent_due_times_and_manual_bypasses_gating() {
        let enabled=vec!["codex".into(),"deepseek".into()];let mut schedule=Schedule::default();schedule.asked(&enabled,0);
        assert_eq!(schedule.due(&enabled,true,true,120,visible(),300_000),vec!["deepseek"]);
        assert_eq!(schedule.due(&enabled,false,true,120,visible(),300_000),enabled);
        assert!(schedule.due(&[],false,true,120,visible(),300_000).is_empty());
        assert_eq!(schedule.next_wait(&enabled,true,120,visible(),100_000),Some(std::time::Duration::from_secs(200)));
    }
    #[test] fn refusal_asked_at_prevents_a_due_loop_and_one_second_slack_is_honoured() {
        let enabled=vec!["deepseek".into()];let mut schedule=Schedule::default();
        assert_eq!(schedule.due(&enabled,true,true,120,visible(),0),enabled);
        schedule.asked(&enabled,0);
        assert!(schedule.due(&enabled,true,true,120,visible(),298_999).is_empty());
        assert_eq!(schedule.due(&enabled,true,true,120,visible(),299_000),enabled);
        schedule.asked(&enabled,299_000);
        assert_eq!(schedule.next_wait(&enabled,true,120,visible(),299_000),Some(std::time::Duration::from_secs(300)));
        assert_eq!(schedule.next_wait(&[],true,120,visible(),0),None);
    }
    #[test] fn fixed_cadence_applies_equally_and_settings_change_recomputes_wait() {
        let ids=vec!["codex".into(),"deepseek".into()];let mut schedule=Schedule::default();schedule.asked(&ids,0);
        assert_eq!(schedule.due(&ids,true,false,300,visible(),300_000),ids);
        assert_eq!(schedule.next_wait(&ids,false,120,visible(),100_000),Some(std::time::Duration::from_secs(20)));
        assert_eq!(schedule.next_wait(&ids,false,1800,visible(),100_000),Some(std::time::Duration::from_secs(1700)));
        assert_eq!(schedule.next_wait(&ids,false,120,visible(),500_000),Some(std::time::Duration::from_secs(15)));
    }
    #[test] fn unchanged_timestamps_errors_and_cached_fallback_do_not_reset_change_age() {
        let old=ProviderUsage::ok("codex","Codex",vec![crate::model::UsageWindow::new("week",Some(20.))]);
        let mut next=old.clone();next.fetched_at="2030-01-01T00:00:00Z".into();
        assert!(!figures_changed(Some(&old),&next));
        next.windows[0].percent_used=Some(30.);assert!(figures_changed(Some(&old),&next));
        next.stale=true;assert!(!figures_changed(Some(&old),&next));
        next.stale=false;next.error=Some("HTTP 429".into());assert!(!figures_changed(Some(&old),&next));
    }
    #[test] fn changing_one_primary_does_not_accelerate_another() {
        let ids=vec!["codex".into(),"claude-code".into()];let mut schedule=Schedule::default();schedule.asked(&ids,0);schedule.changed("codex",100_000);
        assert_eq!(schedule.due(&ids,true,true,120,visible(),120_000),vec!["codex"]);
    }
    #[test] fn looked_cooldown_and_never_read_overdue_are_distinct() {
        let mut schedule=Schedule::default();schedule.changed("codex",0);
        assert!(!schedule.looked(true,120,visible(),None,120_000));
        assert!(schedule.looked(false,120,visible(),Some(0),400_000));
        assert!(!schedule.looked(false,120,visible(),Some(0),410_000));
        assert!(schedule.looked(false,120,visible(),Some(0),430_000));
    }
    #[test] fn frequent_activity_notifications_cannot_postpone_an_already_due_ask() {
        let enabled=vec!["codex".into()];let mut schedule=Schedule::default();schedule.asked(&enabled,0);
        let mut asked=vec![];
        // This is the same plan operation the runtime calls after every Notify.
        // A two-second transcript update must still produce asks at 120/240s.
        for now in (0..=240_000).step_by(2000) {
            let signals=Signals{last_agent_activity:Some(now),..visible()};
            match schedule.plan(&enabled,true,120,signals,now) {
                Plan::Ask(ids)=>{assert_eq!(ids,enabled);asked.push(now);schedule.asked(&ids,now);},
                Plan::Wait(Some(_))=>{},
                Plan::Wait(None)=>panic!("an enabled primary lost its timer"),
            }
        }
        assert_eq!(asked,vec![120_000,240_000]);
    }
    #[test] fn real_power_flags_recalculate_the_due_boundary_and_fixed_cadence_stays_explicit() {
        let enabled=vec!["codex".into()];let mut schedule=Schedule::default();schedule.asked(&enabled,0);schedule.changed("codex",0);
        let low=crate::power_state::PowerStatus{ac_line_status:0,battery_flag:2,..Default::default()};
        let constrained=Signals{constrained:low.constrained(),..visible()};
        assert_eq!(schedule.plan(&enabled,true,120,constrained,120_000),Plan::Wait(Some(std::time::Duration::from_secs(1680))));
        assert_eq!(schedule.plan(&enabled,true,120,constrained,1_798_999),Plan::Wait(Some(std::time::Duration::from_secs(15))));
        assert_eq!(schedule.plan(&enabled,true,120,constrained,1_799_000),Plan::Ask(enabled.clone()));
        let plugged=crate::power_state::PowerStatus{ac_line_status:1,..low};
        let unconstrained=Signals{constrained:plugged.constrained(),..visible()};
        assert_eq!(schedule.plan(&enabled,true,120,unconstrained,120_000),Plan::Ask(enabled.clone()));
        assert_eq!(schedule.plan(&enabled,false,300,constrained,299_000),Plan::Ask(enabled));
    }
    #[test] fn resumed_event_refresh_bypasses_timer_pacing_but_never_adds_disabled_accounts() {
        let enabled=vec!["deepseek".into()];let mut schedule=Schedule::default();schedule.asked(&enabled,0);
        let events=crate::power_state::SystemEvents::new(std::sync::Arc::new(tokio::sync::Notify::new()));
        events.update_power(Some(crate::power_state::PowerStatus{ac_line_status:0,battery_flag:4,..Default::default()}));
        events.observe(crate::power_state::SUSPEND);events.observe(crate::power_state::RESUME_AUTOMATIC);
        assert!(events.take_resume());assert!(events.constrained());
        let signals=Signals{constrained:events.constrained(),..visible()};
        assert!(schedule.due(&enabled,true,true,120,signals,60_000).is_empty());
        assert_eq!(schedule.due(&enabled,false,true,120,signals,60_000),enabled);
        assert!(schedule.due(&[],false,true,120,signals,60_000).is_empty());
    }
    #[test] fn forward_and_backward_wall_changes_recheck_due_and_reestablish_asked_at_once() {
        let enabled=vec!["deepseek".into()];let mut schedule=Schedule::default();schedule.asked(&enabled,1_000_000);
        // The real clock watcher cancels the existing wait. An old future
        // timestamp cannot leave the enabled primary waiting for catch-up.
        assert_eq!(schedule.plan(&enabled,true,120,visible(),800_000),Plan::Ask(enabled.clone()));
        schedule.asked(&enabled,800_000);
        assert_eq!(schedule.plan(&enabled,true,120,visible(),800_000),Plan::Wait(Some(std::time::Duration::from_secs(300))));
        assert_eq!(schedule.plan(&enabled,true,120,visible(),1_100_000),Plan::Ask(enabled.clone()));
        schedule.asked(&enabled,1_100_000);
        assert_eq!(schedule.plan(&enabled,true,120,visible(),1_100_000),Plan::Wait(Some(std::time::Duration::from_secs(300))));
    }
}
