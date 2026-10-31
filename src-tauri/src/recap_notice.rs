//! Monthly notifications use only counts from an already completed opt-in scan.
//! This module never starts a scan or discovers a tool's directories.
use chrono::{Datelike, Local, NaiveDate};
use std::{
    collections::HashSet,
    sync::{Mutex, OnceLock},
    time::Duration,
};
use tauri::Manager;
static KEPT: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();
static CHECK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());
pub fn keep(snapshot: &crate::token_spend::SpendSnapshot) {
    let mut months = KEPT
        .get_or_init(|| Mutex::new(HashSet::new()))
        .lock()
        .unwrap();
    *months = snapshot
        .records
        .iter()
        .filter(|r| r.tally.total() > 0 || r.unclassified_tokens > 0)
        .filter_map(|r| r.day.get(..7).map(str::to_owned))
        .collect();
}
pub fn clear() {
    if let Some(kept) = KEPT.get() {
        kept.lock().unwrap().clear();
    }
}
fn candidate(now: NaiveDate) -> Option<String> {
    if now.day() > 3 {
        return None;
    }
    let year = now.year() - i32::from(now.month() == 1);
    let month = if now.month() == 1 {
        12
    } else {
        now.month() - 1
    };
    Some(format!("{year:04}-{month:02}"))
}
fn due(now: NaiveDate, announced: Option<&str>, months: &HashSet<String>) -> Option<String> {
    let month = candidate(now)?;
    (announced != Some(month.as_str()) && months.contains(&month)).then_some(month)
}
pub fn check(app: &tauri::AppHandle) {
    if std::env::args().any(|a| a == "--native-smoke") {
        return;
    }
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        let _operation = CHECK.lock().await;
        let prefs = app
            .state::<crate::AppState>()
            .preferences
            .lock()
            .unwrap()
            .clone();
        if !prefs.token_spend_enabled || !prefs.alerts.on_recap {
            return;
        }
        let Some(path) = crate::settings::directory().map(|d| d.join("recap-notice.json")) else {
            return;
        };
        let previous = std::fs::read(&path).ok().filter(|b| b.len() < 1024);
        let announced = previous
            .as_ref()
            .and_then(|b| serde_json::from_slice::<String>(b).ok());
        let month = due(
            Local::now().date_naive(),
            announced.as_deref(),
            &KEPT
                .get_or_init(|| Mutex::new(HashSet::new()))
                .lock()
                .unwrap(),
        );
        let Some(month) = month else { return };
        // Persist before delivery: a timer and an opt-in toggle cannot announce twice.
        let Ok(bytes) = serde_json::to_vec(&month) else {
            return;
        };
        if crate::credential_store::atomic_write_bounded(&path, &bytes, previous.as_deref(), 1024)
            .is_err()
        {
            return;
        }
        let identifier = app.config().identifier.clone();
        let body = format!("你的 {month} 工作回顾已准备好，可在 Token 消耗页查看。");
        // A Windows toast click opens the recap directly; notify-rust exposes the
        // desktop activation callback that the Tauri notification plugin does not.
        std::thread::spawn(move || {
            let Ok(handle) = notify_rust::Notification::new()
                .app_id(&identifier)
                .summary("Pulse · 月度回顾")
                .body(&body)
                .show()
            else {
                return;
            };
            let _ = handle.wait_for_response(
                move |response: &notify_rust::NotificationResponse| {
                    if matches!(
                        response,
                        notify_rust::NotificationResponse::Default
                            | notify_rust::NotificationResponse::Action(_)
                    ) {
                        crate::open_recap(&app);
                    }
                },
            );
        });
    });
}
pub fn watch(app: tauri::AppHandle) {
    if std::env::args().any(|a| a == "--native-smoke") {
        return;
    }
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(Duration::from_secs(90)).await;
        loop {
            check(&app);
            tokio::time::sleep(Duration::from_secs(1800)).await
        }
    });
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn notice_only_announces_seen_month_once_in_first_three_days() {
        let months = HashSet::from(["2025-12".into()]);
        for day in 1..=3 {
            assert_eq!(
                due(
                    NaiveDate::from_ymd_opt(2026, 1, day).unwrap(),
                    None,
                    &months
                ),
                Some("2025-12".into())
            );
        }
        assert!(due(NaiveDate::from_ymd_opt(2026, 1, 4).unwrap(), None, &months).is_none());
        assert!(due(
            NaiveDate::from_ymd_opt(2026, 1, 1).unwrap(),
            Some("2025-12"),
            &months
        )
        .is_none());
        assert!(due(
            NaiveDate::from_ymd_opt(2026, 1, 1).unwrap(),
            None,
            &HashSet::new()
        )
        .is_none())
    }
}
