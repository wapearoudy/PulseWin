//! Read-only Windows power and clock signals. No power-setting writes, timer
//! resolution changes, display control, credential discovery or network I/O.
use std::sync::{Arc, atomic::{AtomicBool, Ordering}};
use tokio::sync::Notify;

/// Exact SYSTEM_POWER_STATUS layout (winbase.h), including unknown=255 flags.
#[derive(Debug, Clone, Copy, Default)]
#[repr(C)]
pub struct PowerStatus {
    pub ac_line_status: u8,
    pub battery_flag: u8,
    pub battery_life_percent: u8,
    pub system_status_flag: u8,
    pub battery_life_time: u32,
    pub battery_full_life_time: u32,
}
impl PowerStatus {
    pub fn constrained(self) -> bool {
        let battery_known=self.battery_flag!=255 && self.battery_flag&128==0;
        // Use Windows' low/critical verdict, not an invented percentage or
        // the mere presence of a battery. An AC-connected laptop is not low
        // power unless Windows actually reports the saver switch as on.
        self.system_status_flag==1 || (self.ac_line_status==0 && battery_known && self.battery_flag&6!=0)
    }
}

#[cfg(windows)]
pub fn current_power() -> Option<PowerStatus> {
    #[link(name="kernel32")] extern "system" {fn GetSystemPowerStatus(status:*mut PowerStatus)->i32;}
    let mut status=PowerStatus::default();
    if unsafe{GetSystemPowerStatus(&mut status)}!=0 {Some(status)}else{None}
}
#[cfg(not(windows))]pub fn current_power()->Option<PowerStatus>{None}

pub const SUSPEND:u32=4;
pub const RESUME_CRITICAL:u32=6;
pub const RESUME_USER:u32=7;
pub const RESUME_AUTOMATIC:u32=0x12;

pub struct SystemEvents {
    power_constrained:AtomicBool,
    suspended:AtomicBool,
    resume_announced:AtomicBool,
    resume_pending:AtomicBool,
    listening:AtomicBool,
    wake:Arc<Notify>,
}
impl SystemEvents {
    pub fn new(wake:Arc<Notify>)->Self {
        Self{power_constrained:AtomicBool::new(false),suspended:AtomicBool::new(false),resume_announced:AtomicBool::new(false),resume_pending:AtomicBool::new(false),listening:AtomicBool::new(false),wake}
    }
    pub fn constrained(&self)->bool {self.power_constrained.load(Ordering::Acquire)||self.suspended.load(Ordering::Acquire)}
    pub fn listening(&self)->bool {self.listening.load(Ordering::Acquire)}
    pub fn take_resume(&self)->bool {self.resume_pending.swap(false,Ordering::AcqRel)}
    pub fn poll_power(&self) {self.update_power(current_power());}
    pub fn update_power(&self,status:Option<PowerStatus>) {
        // A failed read does not manufacture a new state. Keep the last
        // successfully observed constraint until the OS answers again.
        if let Some(status)=status {
            if self.power_constrained.swap(status.constrained(),Ordering::AcqRel)!=status.constrained(){self.wake.notify_one();}
        }
    }
    pub fn recovered(&self) {
        self.suspended.store(false,Ordering::Release);
        self.resume_pending.store(true,Ordering::Release);
        self.wake.notify_one();
    }
    pub fn observe(&self,event:u32) {
        match event {
            SUSPEND=>{
                self.resume_announced.store(false,Ordering::Release);
                self.suspended.store(true,Ordering::Release);
                self.wake.notify_one();
            },
            RESUME_AUTOMATIC=>{
                // Automatic resume is the authoritative start of a new wake,
                // including when this process missed the preceding suspend.
                self.resume_announced.store(true,Ordering::Release);
                self.recovered();
            },
            RESUME_USER|RESUME_CRITICAL=>{
                self.suspended.store(false,Ordering::Release);
                // Windows sends automatic resume, then user resume for one
                // wake. Those are one refresh, not two queued passes.
                if !self.resume_announced.swap(true,Ordering::AcqRel){self.recovered();}
            },
            _=>{},
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct ClockSample {
    pub wall_millis:i64,
    /// GetTickCount64 includes time spent suspended and ignores wall changes.
    pub uptime_millis:u64,
    /// QueryUnbiasedInterruptTime excludes sleep/hibernation. None means the
    /// OS did not provide this signal, not that the display was asleep.
    pub awake_millis:Option<u64>,
}
#[derive(Debug, PartialEq)]
pub struct ClockChange {pub wall_shift_millis:i64,pub suspended_millis:u64}
#[derive(Default)]
pub struct ClockWatcher {previous:Option<ClockSample>}
pub const CLOCK_TOLERANCE_MILLIS:u64=5000;
impl ClockWatcher {
    pub fn observe(&mut self,sample:ClockSample)->Option<ClockChange> {
        let old=self.previous.replace(sample)?;
        let elapsed=sample.uptime_millis.checked_sub(old.uptime_millis)?;
        let wall_shift=sample.wall_millis.saturating_sub(old.wall_millis).saturating_sub(elapsed.min(i64::MAX as u64) as i64);
        let suspended=old.awake_millis.zip(sample.awake_millis)
            .and_then(|(old,new)|new.checked_sub(old)).and_then(|awake|elapsed.checked_sub(awake)).unwrap_or(0);
        if wall_shift.unsigned_abs()>=CLOCK_TOLERANCE_MILLIS||suspended>=CLOCK_TOLERANCE_MILLIS {
            Some(ClockChange{wall_shift_millis:wall_shift,suspended_millis:suspended})
        }else{None}
    }
}
#[cfg(windows)]
pub fn clock_sample(wall_millis:i64)->Option<ClockSample> {
    #[link(name="kernel32")] extern "system" {fn GetTickCount64()->u64;fn QueryUnbiasedInterruptTime(time:*mut u64)->i32;}
    let mut awake=0;
    let awake_millis=if unsafe{QueryUnbiasedInterruptTime(&mut awake)}!=0{Some(awake/10_000)}else{None};
    Some(ClockSample{wall_millis,uptime_millis:unsafe{GetTickCount64()},awake_millis})
}
#[cfg(not(windows))]pub fn clock_sample(_:i64)->Option<ClockSample>{None}

#[cfg(windows)]mod notifications {
    use super::*;
    use std::{ffi::c_void,sync::OnceLock};
    // One bounded, process-lifetime callback context. Windows may dispatch on
    // another thread while a subscription is being removed; the callback
    // never dereferences freed app/window/context pointers or takes app locks.
    static EVENTS:OnceLock<Arc<SystemEvents>>=OnceLock::new();
    #[repr(C)]struct Parameters {
        callback:unsafe extern "system" fn(*mut c_void,u32,*mut c_void)->u32,
        context:usize,
    }
    #[link(name="powrprof")]extern "system" {
        fn PowerRegisterSuspendResumeNotification(flags:u32,recipient:*mut c_void,registration:*mut *mut c_void)->u32;
        fn PowerUnregisterSuspendResumeNotification(registration:*mut c_void)->u32;
    }
    unsafe extern "system" fn callback(_: *mut c_void,event:u32,_:*mut c_void)->u32 {
        if let Some(events)=EVENTS.get(){events.observe(event);}
        0
    }
    pub struct Subscription {handle:usize,_parameters:Box<Parameters>}
    impl Drop for Subscription {
        fn drop(&mut self) {
            unsafe{PowerUnregisterSuspendResumeNotification(self.handle as *mut c_void);}
            if let Some(events)=EVENTS.get(){events.listening.store(false,Ordering::Release);}
        }
    }
    pub fn listen(events:Arc<SystemEvents>)->Option<Subscription> {
        if EVENTS.set(events.clone()).is_err(){return None;}
        let mut parameters=Box::new(Parameters{callback,context:0});let mut handle=std::ptr::null_mut();
        // DEVICE_NOTIFY_CALLBACK=2, ERROR_SUCCESS=0. This subscribes to
        // suspend/resume notifications; it does not request or prevent sleep.
        if unsafe{PowerRegisterSuspendResumeNotification(2,(&mut *parameters as *mut Parameters).cast(),&mut handle)}!=0||handle.is_null(){return None;}
        events.listening.store(true,Ordering::Release);
        Some(Subscription{handle:handle as usize,_parameters:parameters})
    }
}
#[cfg(windows)]pub use notifications::listen;
#[cfg(not(windows))]pub fn listen(_:Arc<SystemEvents>)->Option<()>{None}

#[cfg(test)]mod tests {
    use super::*;
    fn events()->SystemEvents{SystemEvents::new(Arc::new(Notify::new()))}
    fn status(ac:u8,battery:u8,saver:u8)->PowerStatus{PowerStatus{ac_line_status:ac,battery_flag:battery,system_status_flag:saver,..Default::default()}}
    #[test]fn windows_power_flags_do_not_invent_constraints_from_unknown_or_missing_batteries(){
        assert_eq!(std::mem::size_of::<PowerStatus>(),12);
        for (ac,battery,saver,constrained) in [(0,2,0,true),(0,4,0,true),(0,10,0,true),(1,2,0,false),(255,2,0,false),(0,255,0,false),(0,128,0,false),(1,128,1,true),(0,1,0,false),(0,0,0,false),(0,255,255,false)] {
            assert_eq!(status(ac,battery,saver).constrained(),constrained,"AC {ac}, battery {battery}, saver {saver}");
        }
        let events=events();events.update_power(Some(status(0,2,0)));assert!(events.constrained());
        events.update_power(None);assert!(events.constrained());
        events.update_power(Some(status(1,2,0)));assert!(!events.constrained());
    }
    #[test]fn one_windows_wake_pair_requests_one_pass_and_the_next_suspend_rearms_it(){
        let events=events();events.observe(SUSPEND);assert!(events.constrained());assert!(!events.take_resume());
        events.observe(RESUME_AUTOMATIC);assert!(!events.constrained());assert!(events.take_resume());
        events.observe(RESUME_USER);assert!(!events.take_resume());
        events.observe(RESUME_AUTOMATIC);assert!(events.take_resume());
        events.observe(RESUME_USER);assert!(!events.take_resume());
        events.observe(SUSPEND);events.observe(RESUME_USER);assert!(events.take_resume());
        events.observe(1234);assert!(!events.take_resume());
    }
    #[test]fn suspend_clock_fixture_is_distinct_from_a_busy_process_and_a_wall_clock_jump(){
        let initial=ClockSample{wall_millis:1_000_000,uptime_millis:100_000,awake_millis:Some(90_000)};
        let mut watcher=ClockWatcher::default();assert!(watcher.observe(initial).is_none());
        let busy=ClockSample{wall_millis:1_060_000,uptime_millis:160_000,awake_millis:Some(150_000)};
        assert!(watcher.observe(busy).is_none());
        let recovered=ClockSample{wall_millis:1_662_000,uptime_millis:762_000,awake_millis:Some(152_000)};
        assert_eq!(watcher.observe(recovered),Some(ClockChange{wall_shift_millis:0,suspended_millis:600_000}));
        let forward=ClockSample{wall_millis:1_964_000,uptime_millis:764_000,awake_millis:Some(154_000)};
        assert_eq!(watcher.observe(forward),Some(ClockChange{wall_shift_millis:300_000,suspended_millis:0}));
        let backward=ClockSample{wall_millis:1_666_000,uptime_millis:766_000,awake_millis:Some(156_000)};
        assert_eq!(watcher.observe(backward),Some(ClockChange{wall_shift_millis:-300_000,suspended_millis:0}));
    }
    #[test]fn clock_probe_ignores_timer_resolution_jitter_and_unknown_sleep_data(){
        let mut watcher=ClockWatcher::default();let initial=ClockSample{wall_millis:0,uptime_millis:0,awake_millis:None};
        assert!(watcher.observe(initial).is_none());
        assert!(watcher.observe(ClockSample{wall_millis:2000,uptime_millis:2016,awake_millis:None}).is_none());
        assert_eq!(watcher.observe(ClockSample{wall_millis:9020,uptime_millis:4020,awake_millis:None}),Some(ClockChange{wall_shift_millis:5016,suspended_millis:0}));
    }
    #[test]fn clock_probe_five_second_boundary_requires_an_os_sleep_measurement(){
        let mut known=ClockWatcher::default();
        known.observe(ClockSample{wall_millis:0,uptime_millis:0,awake_millis:Some(0)});
        assert_eq!(known.observe(ClockSample{wall_millis:7000,uptime_millis:7000,awake_millis:Some(2000)}),Some(ClockChange{wall_shift_millis:0,suspended_millis:5000}));
        let mut unknown=ClockWatcher::default();
        unknown.observe(ClockSample{wall_millis:0,uptime_millis:0,awake_millis:None});
        assert!(unknown.observe(ClockSample{wall_millis:600_000,uptime_millis:600_000,awake_millis:None}).is_none());
        // A recovering system may never have delivered suspend to this new
        // process; the first real resume still refreshes once.
        let events=events();events.observe(RESUME_CRITICAL);assert!(events.take_resume());
        events.observe(RESUME_USER);assert!(!events.take_resume());
    }
    #[test]fn clock_probe_compares_elapsed_counts_even_when_absolute_clock_offsets_differ(){
        // The APIs can expose different absolute offsets and 10–16ms timer
        // resolution. Only elapsed deltas establish a suspension interval.
        let mut watcher=ClockWatcher::default();
        watcher.observe(ClockSample{wall_millis:0,uptime_millis:100_000,awake_millis:Some(1_000_000)});
        assert!(watcher.observe(ClockSample{wall_millis:2000,uptime_millis:102_000,awake_millis:Some(1_002_007)}).is_none());
        assert_eq!(watcher.observe(ClockSample{wall_millis:604_000,uptime_millis:704_000,awake_millis:Some(1_004_007)}),Some(ClockChange{wall_shift_millis:0,suspended_millis:600_000}));
    }
}
