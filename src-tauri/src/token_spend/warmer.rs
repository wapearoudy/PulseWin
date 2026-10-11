//! Pulse 1.9 SpendWarmer: opt-in only, 20s launch delay, 10min refresh.
//! Kept counter snapshots survive closing Settings; opt-out forgets them.
use super::*;
use tauri::Emitter;
static KEPT: OnceLock<Mutex<Option<SpendSnapshot>>> = OnceLock::new();
static ACTIVE: OnceLock<Mutex<Option<Arc<AtomicBool>>>> = OnceLock::new();
static WAKE: tokio::sync::Notify = tokio::sync::Notify::const_new();
pub(super) fn keep(snapshot: &SpendSnapshot) {
    let mut kept = KEPT.get_or_init(Default::default).lock().unwrap();
    if !crate::preferences::load().token_spend_enabled {
        return;
    }
    *kept = Some(snapshot.clone());
    crate::recap_notice::keep(snapshot)
}
pub(super) fn cancel() {
    if let Some(slot) = ACTIVE.get() {
        if let Some(cancel) = slot.lock().unwrap().take() {
            cancel.store(true, Ordering::Release)
        }
    }
}
pub(super) fn forget() {
    cancel();
    let mut kept = KEPT.get_or_init(Default::default).lock().unwrap();
    *kept = None;
    crate::recap_notice::clear()
}
pub(super) fn changed() {
    cancel();
    WAKE.notify_one()
}
pub(super) fn snapshot() -> Option<SpendSnapshot> {
    KEPT.get_or_init(Default::default).lock().unwrap().clone()
}
pub(super) fn watch(app: tauri::AppHandle) {
    if std::env::args().any(|a| a == "--native-smoke") {
        return;
    }
    tauri::async_runtime::spawn(async move {
        let mut delay = Duration::from_secs(20);
        loop {
            tokio::select! {_=tokio::time::sleep(delay)=>{},_=WAKE.notified()=>{}}
            delay = Duration::from_secs(600);
            if !crate::preferences::load().token_spend_enabled {
                forget();
                continue;
            }
            let manual_running = state()
                .lock()
                .ok()
                .and_then(|s| s.as_ref().map(|r| r.scan.status == "running"))
                .unwrap_or(false);
            if manual_running {
                continue;
            }
            let cancel = Arc::new(AtomicBool::new(false));
            *ACTIVE.get_or_init(Default::default).lock().unwrap() = Some(cancel.clone());
            if !crate::preferences::load().token_spend_enabled {
                forget();
                continue;
            }
            let Some(home) = crate::credentials::home_dir() else {
                continue;
            };
            let cache = crate::credentials::cache_dir()
                .unwrap_or_else(|| home.join(".cache"))
                .join("PulseWin/token-spend");
            let (prices, prices_at, pricing_status) = fetch_prices(&cache, &cancel).await;
            if check(&cancel).is_err() {
                continue;
            }
            let worker = cancel.clone();
            let result = tokio::task::spawn_blocking(move || {
                scan_sources(&home, &cache, &prices, false, &worker, |_, _| {})
            })
            .await;
            if check(&cancel).is_err() || !crate::preferences::load().token_spend_enabled {
                continue;
            }
            if let Ok(Ok(mut snapshot)) = result {
                snapshot.prices_at = prices_at;
                snapshot.pricing_status = pricing_status;
                keep(&snapshot);
                crate::recap_notice::check(&app);
                let _ = app.emit("spend-snapshot-ready", &snapshot.scanned_at);
            }
        }
    });
}
