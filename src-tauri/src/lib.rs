//! PulseWin — a Windows tray monitor for AI coding tool quota.
//!
//! Mirrors the original macOS Pulse: a tray/status item plus a glass panel that
//! opens on hover or click, showing one ring per provider.

// Public so `examples/probe.rs` (and anything else) can drive a single provider
// from the command line without starting the GUI.
pub mod accounts;
pub mod credentials;
pub mod credential_store;
pub mod model;
pub mod providers;
pub mod proxy;
pub mod settings;
pub mod preferences;
pub mod activity;
pub mod adaptive_refresh;
pub mod power_state;
pub mod desktop;
pub mod alerts;
pub mod usage_cache;
pub mod usage_report;
pub mod token_spend;
pub mod application;
pub mod updates;

use std::sync::Arc;
use std::time::Duration;

use tauri::menu::{Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Emitter, Manager, State};
use tokio::sync::Mutex;

use model::{now_rfc3339, Snapshot};
use providers::Ctx;

/// Event name the frontend listens on for fresh data.
pub const EVENT_USAGE_UPDATED: &str = "usage-updated";

/// Event name carrying the id of the region the pointer is on, or null.
pub const EVENT_HOVER_CHANGED: &str = "hover-changed";

/// Event name carrying the edge the rail has just docked to.
pub const EVENT_EDGE_CHANGED: &str = "edge-changed";

/// How often the pointer watcher samples the cursor.
const POINTER_INTERVAL: Duration = Duration::from_millis(60);

/// A tracking or painted region, in CSS pixels relative to the window.
///
/// The window has to be far wider than the rail so a card can open inside it
/// without the frame changing, and all of that extra width is empty space. A
/// transparent window still eats clicks, so the panel has to know which parts
/// of itself are real before it can hand the rest back to the desktop.
#[derive(Clone, serde::Deserialize, serde::Serialize)]
pub struct HitRegion {
    pub id: String,
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
    /// Closed contours sampled from the very SVG path that paints the surface.
    /// Multiple subpaths use the same nonzero fill rule as the SVG renderer.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub outline: Option<Vec<Vec<[f64; 2]>>>,
    /// Entry rectangles are clipped to their painted rail, without duplicating
    /// a long outline once per account.
    #[serde(default, rename = "clipTo", skip_serializing_if = "Option::is_none")]
    pub clip_to: Option<String>,
    /// Hover forgiveness never makes a transparent pixel claim a press.
    #[serde(default, rename = "hoverOnly")]
    pub hover_only: bool,
}

/// Which screen edge the rail is fused to.
///
/// The rail is drawn facing right and turned into place, so this is the only
/// thing that has to change when it moves house: the shape, the hit regions and
/// the side the tail leaves from are all derived from it.
#[derive(Clone, Copy, PartialEq, Eq, Debug, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PanelEdge {
    Left,
    Right,
    Top,
}

impl Default for PanelEdge {
    fn default() -> Self {
        PanelEdge::Right
    }
}

/// A drag in progress: where the pointer was, and where the window was, when it
/// started. Both are needed — the window follows the pointer's *delta*, so that
/// grabbing the rail at any point keeps that point under the finger.
#[derive(Clone, Copy)]
pub struct Drag {
    cursor: (i32, i32),
    grab: (f64, f64),
    moved: bool,
    awaiting_metrics: bool,
}

pub struct AppState {
    pub refresh_schedule: std::sync::Mutex<adaptive_refresh::Schedule>,
    pub refresh_wake: Arc<tokio::sync::Notify>,
    pub power_events: Arc<power_state::SystemEvents>,
    pub placement_layout_ready: std::sync::atomic::AtomicBool,
    pub usage_cache: std::sync::Mutex<usage_cache::UsageCache>,
    pub alert_memory: std::sync::Mutex<alerts::AlertMemory>,
    pub resets: std::sync::Mutex<std::collections::HashMap<String,i64>>,
    pub fullscreen_hidden: std::sync::atomic::AtomicBool,
    pub activity: std::sync::Mutex<activity::Activity>,
    pub menu_open: std::sync::atomic::AtomicBool,
    pub generation: std::sync::atomic::AtomicU64,
    pub preferences: std::sync::Mutex<preferences::Preferences>,
    pub ctx: Arc<Ctx>,
    pub snapshot: Mutex<Snapshot>,
    /// Serializes refreshes so a manual refresh cannot overlap the timer loop.
    pub refreshing: Mutex<()>,
    pub account_mutations: Mutex<()>,
    /// Regions the frontend currently claims; empty means "not laid out yet".
    pub hit_regions: Mutex<Vec<HitRegion>>,
    /// Which edge the rail is docked to. Held here rather than in the frontend
    /// because placing the window needs it, and the window is placed from Rust.
    pub edge: std::sync::Mutex<PanelEdge>,
    pub placement: std::sync::Mutex<settings::PanelSettings>,
    pub rail_metrics: std::sync::Mutex<(f64, f64, f64, f64)>,
    /// Set while the rail is being dragged off the edge. See `watch_pointer`.
    pub drag: std::sync::Mutex<Option<Drag>>,
}

impl AppState {
    fn new(ctx: Ctx) -> Self {
        // Restore placement and explicitly stale readings while fresh checks run.
        let saved = settings::load();
        let edge = saved.dock_edge.or(saved.facing_edge).unwrap_or_default();
        let refresh_wake=Arc::new(tokio::sync::Notify::new());
        let power_events=Arc::new(power_state::SystemEvents::new(Arc::clone(&refresh_wake)));
        Self {
            refresh_schedule: std::sync::Mutex::new(adaptive_refresh::Schedule::default()),
            refresh_wake,
            power_events,
            placement_layout_ready: std::sync::atomic::AtomicBool::new(false),
            usage_cache: std::sync::Mutex::new(usage_cache::UsageCache::load()),
            alert_memory: std::sync::Mutex::new(alerts::AlertMemory::load()),
            resets: Default::default(),
            fullscreen_hidden: std::sync::atomic::AtomicBool::new(false),
            activity: std::sync::Mutex::new(activity::Activity::default()),
            menu_open: std::sync::atomic::AtomicBool::new(false),
            generation: std::sync::atomic::AtomicU64::new(0),
            preferences: std::sync::Mutex::new({
                let mut prefs = preferences::load();
                prefs.normalize(&providers::registry().iter().map(|p| p.id().to_string()).collect::<Vec<_>>());
                prefs
            }),
            ctx: Arc::new(ctx),
            snapshot: Mutex::new({
                let prefs=preferences::load();let mut snapshot=usage_cache::stored_snapshot(&prefs.enabled_providers);
                for p in &mut snapshot.providers{if let Some(label)=prefs.account_labels.get(&p.id){p.name=label.clone();}}
                snapshot
            }),
            refreshing: Mutex::new(()),
            account_mutations: Mutex::new(()),
            hit_regions: Mutex::new(Vec::new()),
            edge: std::sync::Mutex::new(edge),
            placement: std::sync::Mutex::new(settings::load()),
            rail_metrics: std::sync::Mutex::new((0.0, 0.0, 64.0, 100.0)),
            drag: std::sync::Mutex::new(None),
        }
    }
}

/// Fetch every provider and publish the result to the UI and the tray tooltip.
async fn refresh(app: &AppHandle, state: &AppState) -> Snapshot {
    // If another refresh is in flight, wait for it instead of doubling the load.
    let _guard = state.refreshing.lock().await;

    refresh_locked(app, state, None).await
}

async fn refresh_locked(app: &AppHandle, state: &AppState, target: Option<&str>) -> Snapshot {
    refresh_pass(app,state,target,false).await
}

fn refresh_signals(app:&AppHandle,state:&AppState)->adaptive_refresh::Signals {
    adaptive_refresh::Signals{
        last_agent_activity:state.activity.lock().unwrap().last_write,
        last_looked:None,
        panel_visible:app.get_webview_window("main").is_some_and(|window|window.is_visible().unwrap_or(false)),
        // Real Windows battery/saver and suspend notifications. Thermal
        // pressure and independently sleeping displays remain unsupported.
        constrained:state.power_events.constrained(),
    }
}

/// Only the one-shot timer calls with due_only=true. Every user/settings path
/// asks immediately; the filter and commit set stay identical.
async fn refresh_pass(app:&AppHandle,state:&AppState,target:Option<&str>,due_only:bool)->Snapshot {
    let generation = state.generation.load(std::sync::atomic::Ordering::SeqCst);
    let prefs = state.preferences.lock().unwrap().clone();
    let selected = prefs.enabled_providers.iter().filter(|id| target.map_or(true, |t| t == id.as_str())).cloned().collect::<Vec<_>>();
    let now_millis=chrono::Utc::now().timestamp_millis();
    let signals=refresh_signals(app,state);
    let enabled={
        let mut schedule=state.refresh_schedule.lock().unwrap();
        let due=schedule.due(&selected,due_only,prefs.refresh_automatic,prefs.refresh_seconds,signals,now_millis);
        schedule.asked(&due,now_millis);due
    };
    if enabled.is_empty(){return state.snapshot.lock().await.clone();}
    let started=std::time::Instant::now();
    if !enabled.is_empty() { let _=app.emit("refresh-changed",serde_json::json!({"ids":enabled,"refreshing":true})); }
    let fetched = providers::collect_accounts(Arc::clone(&state.ctx), &enabled, &prefs).await;
    if !enabled.is_empty() {
        if let Some(remaining)=Duration::from_millis(650).checked_sub(started.elapsed()) { tokio::time::sleep(remaining).await; }
        let _=app.emit("refresh-changed",serde_json::json!({"ids":enabled,"refreshing":false}));
    }
    if generation != state.generation.load(std::sync::atomic::Ordering::SeqCst) {
        return state.snapshot.lock().await.clone();
    }
    let prefs = state.preferences.lock().unwrap().clone();
    let mut providers = if target.is_some()||due_only { state.snapshot.lock().await.providers.clone() } else { vec![] };
    providers.retain(|p| prefs.enabled_providers.contains(&p.id) && !enabled.contains(&p.id));
    let now=chrono::Utc::now().timestamp();
    {
        let mut cache=state.usage_cache.lock().unwrap();
        let mut memory=state.alert_memory.lock().unwrap();
        for mut raw in fetched {
            if let Some(label)=prefs.account_labels.get(&raw.id){raw.name=label.clone();}
            let previous=cache.readings.get(&raw.id).cloned();
            if raw.error.is_none() && !raw.stale && usage_cache::valid_age(&raw.fetched_at,now) {
                if cache.readings.get(&raw.id).is_some_and(|old| usage_cache::valid_age(&old.fetched_at,now) &&
                    usage_cache::timestamp(&raw.fetched_at)>=usage_cache::timestamp(&old.fetched_at) && raw.windows.iter().filter(|w|usage_cache::window_usable(w,now)).any(|next|
                    old.windows.iter().find(|previous|previous.label==next.label)
                        .is_some_and(|previous|usage_cache::moved_on(previous,next)))) {
                    state.resets.lock().unwrap().insert(raw.id.clone(),chrono::Utc::now().timestamp_millis());
                    let _=app.emit("resets-changed",state.resets.lock().unwrap().clone());
                }
            }
            let mut display=cache.reconcile(raw.clone(),now);
            display.name=raw.name.clone();
            if adaptive_refresh::figures_changed(previous.as_ref(),&display) {
                state.refresh_schedule.lock().unwrap().changed(&raw.id,chrono::Utc::now().timestamp_millis());
            }
            if !raw.stale { post_alerts(app,&mut memory,&raw,&display,&prefs.alerts,now); }
            providers.push(display);
        }
        let _=cache.save();let _=memory.save();
    }
    providers.sort_by_key(|p| prefs.provider_order.iter().position(|id| id == &p.id).unwrap_or(usize::MAX));
    let snapshot = Snapshot {
        providers,
        fetched_at: now_rfc3339(),
    };

    {
        let mut current = state.snapshot.lock().await;
        if generation != state.generation.load(std::sync::atomic::Ordering::SeqCst) {
            return current.clone();
        }
        *current = snapshot.clone();
    }

    update_tray(app, &snapshot);
    let _ = app.emit(EVENT_USAGE_UPDATED, &snapshot);
    state.refresh_wake.notify_one();
    snapshot
}

fn watch_refresh(app:AppHandle) {
    tauri::async_runtime::spawn(async move {
        let power_events=Arc::clone(&app.state::<AppState>().power_events);
        let _power_subscription=power_state::listen(Arc::clone(&power_events));
        power_events.poll_power();
        {
            let state=app.state::<AppState>();
            refresh(&app,&state).await;
        }
        loop {
            let state=app.state::<AppState>();
            if state.power_events.take_resume(){
                // Match UsageStore's didWake route: a real wake is an event,
                // not a timer tick, so enabled accounts are asked now even
                // if their cadence would otherwise keep the old reading.
                let _guard=state.refreshing.lock().await;
                refresh_pass(&app,&state,None,false).await;
                continue;
            }
            let prefs=state.preferences.lock().unwrap().clone();
            let signals=refresh_signals(&app,&state);
            let plan=state.refresh_schedule.lock().unwrap().plan(&prefs.enabled_providers,prefs.refresh_automatic,prefs.refresh_seconds,signals,chrono::Utc::now().timestamp_millis());
            let wait=match plan{
                adaptive_refresh::Plan::Ask(_)=>{
                    let _guard=state.refreshing.lock().await;
                    refresh_pass(&app,&state,None,true).await;
                    continue;
                },
                adaptive_refresh::Plan::Wait(wait)=>wait,
            };
            let Some(wait)=wait else{state.refresh_wake.notified().await;continue};
            tokio::select! {
                _=state.refresh_wake.notified()=>continue,
                _=tokio::time::sleep(wait)=>{
                    let _guard=state.refreshing.lock().await;
                    refresh_pass(&app,&state,None,true).await;
                }
            }
        }
    });
}

/// Tray tooltip + icon tooltip text summarising the tightest quota.
fn update_tray(app: &AppHandle, snapshot: &Snapshot) {
    let Some(tray) = app.tray_by_id("pulse-tray") else {
        return;
    };

    let configured: Vec<&model::ProviderUsage> = snapshot.providers.iter().collect();

    let tooltip = if configured.is_empty() {
        "PulseWin — choose services in Settings".to_string()
    } else {
        let worst = configured
            .iter()
            .filter_map(|p| p.peak_percent_used().map(|pct| (p.name.as_str(), pct)))
            .max_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal));

        match worst {
            Some((name, used)) => format!(
                "PulseWin — {name} at {:.0}% used\n({} tool{} tracked)",
                used,
                configured.len(),
                if configured.len() == 1 { "" } else { "s" }
            ),
            None => format!("PulseWin — {} tools tracked", configured.len()),
        }
    };

    let _ = tray.set_tooltip(Some(&tooltip));
}

fn post_alerts(app:&AppHandle,memory:&mut alerts::AlertMemory,raw:&model::ProviderUsage,display:&model::ProviderUsage,prefs:&alerts::AlertPreferences,now:i64) {
    use tauri_plugin_notification::NotificationExt;
    let mut candidate=memory.clone();
    let notices=candidate.observe(raw,display,prefs,now);
    let mut delivered=true;
    for notice in notices {
        if app.notification().builder().title(notice.title).body(notice.body).show().is_err(){delivered=false;}
    }
    if delivered {*memory=candidate;}
}

// ---------------------------------------------------------------------------
// IPC commands
// ---------------------------------------------------------------------------

#[tauri::command]
async fn get_snapshot(state: State<'_, AppState>) -> Result<Snapshot, String> {
    Ok(state.snapshot.lock().await.clone())
}

#[tauri::command]
fn get_activity(state: State<'_, AppState>) -> activity::Activity { state.activity.lock().unwrap().clone() }

#[tauri::command]
fn get_resets(state:State<'_,AppState>)->std::collections::HashMap<String,i64>{state.resets.lock().unwrap().clone()}

fn watch_activity(app: AppHandle) {
    tauri::async_runtime::spawn(async move {
        let mut observed_generation=u64::MAX;
        loop {
            let state=app.state::<AppState>();
            let generation=state.generation.load(std::sync::atomic::Ordering::SeqCst);
            let visible=app.get_webview_window("main").is_some_and(|w|w.is_visible().unwrap_or(false));
            let enabled=state.preferences.lock().unwrap().enabled_providers.iter().filter(|id|activity::supported(id)).cloned().collect::<Vec<_>>();
            if generation!=observed_generation || !visible || enabled.is_empty() {
                let mut current=state.activity.lock().unwrap();
                if *current!=activity::Activity::default(){*current=activity::Activity::default();let _=app.emit("activity-changed",&*current);state.refresh_wake.notify_one();}
                observed_generation=generation;
            }
            if visible && !enabled.is_empty() {
                let scan_app=app.clone();
                let result=tauri::async_runtime::spawn_blocking(move || {
                    credentials::home_dir().map(|home|activity::scan(&home,&enabled,chrono::Utc::now().timestamp_millis(),||{
                        scan_app.state::<AppState>().generation.load(std::sync::atomic::Ordering::SeqCst)!=generation ||
                        !scan_app.get_webview_window("main").is_some_and(|w|w.is_visible().unwrap_or(false))
                    }))
                }).await;
                if state.generation.load(std::sync::atomic::Ordering::SeqCst)==generation && app.get_webview_window("main").is_some_and(|w|w.is_visible().unwrap_or(false)) {
                    if let Ok(Some(readings))=result {
                        let mut current=state.activity.lock().unwrap();let previous=current.clone();
                        current.record(readings,chrono::Utc::now().timestamp_millis());
                        if *current!=previous {let _=app.emit("activity-changed",&*current);state.refresh_wake.notify_one();}
                    }
                }
            }
            tokio::time::sleep(Duration::from_secs(2)).await;
        }
    });
}

#[tauri::command]
async fn refresh_now(app: AppHandle, state: State<'_, AppState>) -> Result<Snapshot, String> {
    Ok(refresh(&app, &state).await)
}

#[tauri::command]
fn get_preferences(state: State<'_, AppState>) -> preferences::Preferences {
    state.preferences.lock().unwrap().clone()
}

#[tauri::command]
async fn save_preferences(app: AppHandle, state: State<'_, AppState>, mut value: preferences::Preferences) -> Result<(), String> {
    let _guard=state.account_mutations.lock().await;
    // Metadata is changed only by account commands; old windows cannot erase it.
    value.accounts=state.preferences.lock().unwrap().accounts.clone();
    apply_preferences(app,&state,value).await
}
async fn apply_preferences(app: AppHandle, state: &AppState, mut value: preferences::Preferences) -> Result<(), String> {
    value.normalize(&providers::registry().iter().map(|p| p.id().to_string()).collect::<Vec<_>>());
    let previous = state.preferences.lock().unwrap().clone();
    preferences::save(&value)?;
    *state.preferences.lock().unwrap() = value.clone();
    state.refresh_schedule.lock().unwrap().primary.retain(|id,_|value.enabled_providers.contains(id));
    state.refresh_wake.notify_one();
    if value.enabled_providers != previous.enabled_providers || value.accounts != previous.accounts {
        state.generation.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        token_spend::cancel_card_reads();
    }
    let _ = app.emit("preferences-changed", &value);
    if previous.token_spend_enabled && !value.token_spend_enabled { token_spend::spend_clear_snapshot(); token_spend::cancel_card_reads(); }
    if value.alerts!=previous.alerts {
        let current=state.snapshot.lock().await.clone();
        let mut memory=state.alert_memory.lock().unwrap();
        for raw in &current.providers {if !raw.stale {post_alerts(&app,&mut memory,raw,raw,&value.alerts,chrono::Utc::now().timestamp());}}
        let _=memory.save();
    }
    if value.open_settings_shortcut!=previous.open_settings_shortcut || value.toggle_panel_shortcut!=previous.toggle_panel_shortcut {application::apply(&app);}
    if value.enabled_providers.is_empty() {
        state.fullscreen_hidden.store(false,std::sync::atomic::Ordering::SeqCst);
        if let Some(window) = app.get_webview_window("main") { let _ = window.hide(); }
    } else if previous.enabled_providers.is_empty() { show_panel(&app); }
    if value.enabled_providers != previous.enabled_providers || value.provider_order != previous.provider_order || value.account_labels != previous.account_labels {
        let mut snapshot = state.snapshot.lock().await;
        snapshot.providers.retain(|p| value.enabled_providers.contains(&p.id));
        for reading in &mut snapshot.providers {
            let base=value.accounts.iter().find(|a|a.id==reading.id).map(|a|a.provider.as_str()).unwrap_or(&reading.id);
            if let Some(name)=value.account_labels.get(&reading.id) {reading.name=name.clone();}
            else if let Some(provider)=providers::registry().iter().find(|p|p.id()==base){reading.name=provider.name().into();}
        }
        snapshot.providers.sort_by_key(|p| value.provider_order.iter().position(|id| id == &p.id).unwrap_or(usize::MAX));
        let _ = app.emit(EVENT_USAGE_UPDATED, &*snapshot);
        update_tray(&app, &snapshot);
    }
    if value.enabled_providers != previous.enabled_providers && !value.enabled_providers.is_empty() {
        tauri::async_runtime::spawn(async move {
            let state = app.state::<AppState>();
            refresh(&app, &state).await;
        });
    }
    Ok(())
}

#[tauri::command]
async fn refresh_provider(app: AppHandle, state: State<'_, AppState>, provider: String) -> Result<Snapshot, String> {
    let _guard = state.refreshing.lock().await;
    if !state.preferences.lock().unwrap().enabled_providers.contains(&provider) {
        return Err("This account is not shown in the panel. Enable it before checking usage.".into());
    }
    Ok(refresh_locked(&app, &state, Some(&provider)).await)
}

/// Append a frontend error to `%TEMP%\pulsewin-errors.log`.
///
/// A Windows webview has no console, so a JavaScript exception would otherwise
/// be completely invisible: the panel renders nothing, and there is nothing to
/// read anywhere. This is the only window into the frontend during development.
#[tauri::command]
fn report_error(message: String) {
    use std::io::Write;

    let path = std::env::temp_dir().join("pulsewin-errors.log");
    if let Ok(mut file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
    {
        let _ = writeln!(file, "{}\t{message}", now_rfc3339());
    }
}

/// The places a provider looks for a credential, in the order it looks.
///
/// The provider modules own their own lookups, so this can only name the ones
/// `credentials` knows about plus the file the settings surface writes — which
/// is the point of that file: a reader who cannot find where a tool keeps its
/// token can paste one instead.
fn hint_paths(provider: &str) -> Vec<String> {
    let mut paths = match provider {
        "claude-code" => credentials::claude_credential_paths(),
        "codex" => credentials::codex_credential_paths(),
        "gemini-cli" => credentials::gemini_credential_paths(),
        "copilot" => credentials::copilot_credential_paths(),
        "cursor" => credentials::cursor_state_db_paths(),
        "opencode-go" => providers::open_code_go::auth_paths(),
        _ => Vec::new(),
    };
    if let Some(file) = settings::credential_file(provider) {
        paths.push(file);
    }
    paths
        .into_iter()
        .map(|p| p.display().to_string())
        .collect()
}

/// Resolve the path a provider would read, so the UI can show the user exactly
/// where PulseWin is looking when a card says "not detected".
#[tauri::command]
fn credential_hint(provider: String) -> Vec<String> {
    hint_paths(&provider)
}

/// One row of the settings list.
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct ProviderSetting {
    credential_error: Option<String>,
    id: String,
    provider_id: String,
    additional: bool,
    name: String,
    /// The provider's own verdict: this tool is on the machine.
    configured: bool,
    /// A credential is stored in PulseWin's own file for it.
    stored: bool,
    /// Where the reader can paste one.
    credential_path: Option<String>,
    /// Everything the provider reads, in order.
    hints: Vec<String>,
}

/// Every provider PulseWin ships, with what it knows about each.
///
/// Offline: `is_configured` makes no network call, so opening settings cannot
/// fire a request at every provider it ships.
#[tauri::command]
fn provider_settings(state: State<'_,AppState>) -> Vec<ProviderSetting> {
    let prefs=state.preferences.lock().unwrap().clone();
    let mut entries=providers::registry().into_iter().map(|provider| {
        let id=provider.id().to_owned();let stored=settings::read_credential_checked(&id);
        let error=stored.as_ref().err().map(|_|"保存的凭据无法解密，请使用原 Windows 账户。".to_string());
        let stored=stored.unwrap_or_default();
        ProviderSetting {credential_error:error,provider_id:id.clone(),additional:false,
            name:prefs.account_labels.get(&id).cloned().unwrap_or_else(||provider.name().into()),
            configured:stored.api_key.is_some()||provider.is_configured(),stored:stored.api_key.is_some()||stored.base_url.is_some(),
            credential_path:settings::credential_file(&id).map(|p|p.display().to_string()),hints:hint_paths(&id),id}
    }).collect::<Vec<_>>();
    for account in &prefs.accounts {
        let result=settings::read_credential_document(&account.id);
        let error=result.as_ref().err().map(|_|"保存的凭据无法解密，请使用原 Windows 账户。".to_string());
        let stored=result.ok().flatten().is_some_and(|d|accounts::token(&d).is_some());
        let name=providers::registry().into_iter().find(|p|p.id()==account.provider).map(|p|p.name()).unwrap_or("Account");
        let path=settings::credential_file(&account.id).map(|p|p.display().to_string());
        entries.push(ProviderSetting {id:account.id.clone(),provider_id:account.provider.clone(),additional:true,
            name:prefs.account_labels.get(&account.id).cloned().unwrap_or_else(||name.into()),
            configured:stored,stored,credential_error:error,credential_path:path.clone(),hints:path.into_iter().collect()});
    }
    entries
}

#[tauri::command]
async fn add_account(app:AppHandle,provider:String,label:String,credential:Option<String>,import_local:bool,enabled:Option<bool>)->Result<String,String> {
    let state=app.state::<AppState>();let _guard=state.account_mutations.lock().await;
    if !accounts::SUPPORTED.contains(&provider.as_str()){return Err("此服务暂不支持附加账号。".into());}
    let mut prefs=state.preferences.lock().unwrap().clone();
    if prefs.accounts.len()>=64{return Err("最多可添加 64 个附加账号。".into());}
    let label=accounts::label(&label)?;
    let document=if import_local {accounts::local_document(&provider)?} else {accounts::document(&provider,credential.as_deref().unwrap_or(""))?};
    let id=accounts::new_id(&provider);let path=settings::credential_file(&id).ok_or("无法获取凭据目录。")?;
    credential_store::write_document(&path,&id,&document).map_err(|_|"无法加密保存账号凭据。".to_string())?;
    prefs.accounts.push(accounts::Account{id:id.clone(),provider});prefs.account_labels.insert(id.clone(),label);
    if enabled.unwrap_or(true){prefs.enabled_providers.push(id.clone());}prefs.provider_order.push(id.clone());
    if let Err(error)=apply_preferences(app.clone(),&state,prefs).await {let _=std::fs::remove_file(path);return Err(error);}
    Ok(id)
}
#[tauri::command]
async fn remove_account(app:AppHandle,account:String)->Result<(),String> {
    let state=app.state::<AppState>();let _guard=state.account_mutations.lock().await;
    let mut prefs=state.preferences.lock().unwrap().clone();
    if !prefs.accounts.iter().any(|a|a.id==account){return Err("只能移除附加账号；本机账号可以关闭显示。".into());}
    prefs.accounts.retain(|a|a.id!=account);
    apply_preferences(app.clone(),&state,prefs).await?;
    let mut cache=state.usage_cache.lock().unwrap();cache.readings.remove(&account);cache.save().map_err(|e|e.to_string())?;
    state.resets.lock().unwrap().remove(&account);
    {let mut alerts=state.alert_memory.lock().unwrap();alerts.accounts.remove(&account);let _=alerts.save();}
    if let Some(path)=settings::credential_file(&account) {
        match std::fs::remove_file(path) {Ok(())=>{},Err(e) if e.kind()==std::io::ErrorKind::NotFound=>{},Err(_)=>return Err("账号已移除，但无法删除其加密凭据文件。".into())}
    }
    Ok(())
}

/// Store a pasted credential and re-read the tools that can now answer.
///
/// The refresh is the point of saving: a key that changes nothing on screen
/// looks like a key that did not work.
#[tauri::command]
async fn save_provider_credential(
    app: AppHandle,
    provider: String,
    api_key: Option<String>,
    base_url: Option<String>,
    import_local: Option<bool>,
) -> Result<(), String> {
    let state=app.state::<AppState>();let _account_guard=state.account_mutations.lock().await;
    let additional=state.preferences.lock().unwrap().accounts.iter().find(|a|a.id==provider).cloned();
    if additional.is_none() && !providers::registry().iter().any(|p| p.id() == provider) { return Err("Unknown provider".into()); }
    if let Some(account)=additional {
        let document=if import_local.unwrap_or(false){accounts::local_document(&account.provider)?} else {
            let input=api_key.filter(|v|!v.trim().is_empty()).ok_or("请填写此账号的新登录信息。")?;
            accounts::document(&account.provider,&input)?
        };
        let path=settings::credential_file(&provider).ok_or("无法获取凭据目录。")?;
        credential_store::write_document(&path,&provider,&document).map_err(|_|"无法保存账号登录信息。".to_string())?;
        state.generation.fetch_add(1,std::sync::atomic::Ordering::SeqCst);
        let _guard=state.refreshing.lock().await;
        {let mut cache=state.usage_cache.lock().unwrap();cache.readings.remove(&provider);let _=cache.save();}
        {let mut alerts=state.alert_memory.lock().unwrap();alerts.accounts.remove(&provider);let _=alerts.save();}
        state.resets.lock().unwrap().remove(&provider);
        state.snapshot.lock().await.providers.retain(|p|p.id!=provider);
        if state.preferences.lock().unwrap().enabled_providers.contains(&provider){refresh_locked(&app,&state,Some(&provider)).await;}
        return Ok(());
    }
    let stored = settings::read_credential_checked(&provider).map_err(|_|"保存的凭据无法解密或迁移，请使用原 Windows 账户并检查文件权限。".to_string())?;
    settings::write_credential(
        &provider,
        &settings::Credential {
            api_key: api_key.or(stored.api_key),
            base_url: base_url.or(stored.base_url),
        },
    )
    .map_err(|e| e.to_string())?;

    let state = app.state::<AppState>();
    let _guard = state.refreshing.lock().await;
    let enabled = state.preferences.lock().unwrap().enabled_providers.contains(&provider);
    if enabled { refresh_locked(&app, &state, Some(&provider)).await; }
    Ok(())
}

/// Open the settings window from the panel.
///
/// A card that says why a tool is not answering is only useful if there is
/// somewhere to go next, and the panel is the only thing the reader is looking
/// at when they find out.
#[tauri::command]
fn show_settings(app: AppHandle, provider: Option<String>) {
    let prefs=app.state::<AppState>().preferences.lock().unwrap().clone();
    let provider=provider.filter(|id|providers::registry().iter().any(|p|p.id()==id)||prefs.accounts.iter().any(|a|&a.id==id));
    open_account_settings(&app,provider.as_deref());
}

/// Show the settings window, or bring it forward if it is already open.
///
/// A second window rather than a view inside the panel: the panel's frame is
/// worked out from the rail's length, and nothing may resize it while a card is
/// open — a settings list inside it would be the one thing on screen able to
/// fight that rule.
fn open_settings(app: &AppHandle) {
    open_account_settings(app,None);
}

fn open_account_settings(app: &AppHandle, provider: Option<&str>) {
    if let Some(window) = app.get_webview_window("settings") {
        if let Some(provider)=provider { let _=window.emit("settings-account",provider); }
        let _ = window.show();
        let _ = window.set_focus();
        return;
    }

    let _ = tauri::WebviewWindowBuilder::new(
        app,
        "settings",
        tauri::WebviewUrl::App(provider.map(|id|format!("index.html?view=settings&account={id}")).unwrap_or_else(||"index.html?view=settings".into()).into()),
    )
    .title("PulseWin — Settings")
    .inner_size(960.0, 720.0)
    .min_inner_size(720.0, 480.0)
    .resizable(true)
    .build();
}

/// Replace the set of regions the panel claims as its own.
///
/// Called on every layout change. Empty means the frontend has not laid out
/// yet, and the panel stays fully interactive so the first frame can be clicked.
#[tauri::command]
async fn set_hit_regions(state: State<'_, AppState>, regions: Vec<HitRegion>) -> Result<(), String> {
    *state.hit_regions.lock().await = regions;
    Ok(())
}

/// Which edge the rail is currently docked to.
///
/// The frontend has to ask rather than wait to be told: a restored edge is
/// already in force by the time the webview mounts, and an event emitted before
/// the listener exists is an event that never arrived. Without this the panel
/// is placed on the restored edge while the rail is still laid out for the one
/// it shipped with — the right edge and the wrong frame size.
#[tauri::command]
async fn panel_edge(state: State<'_, AppState>) -> Result<PanelEdge, String> {
    Ok(*state.edge.lock().unwrap())
}

#[tauri::command]
async fn panel_placement(state: State<'_, AppState>) -> Result<settings::PanelSettings, String> {
    Ok(state.placement.lock().unwrap().clone())
}

#[tauri::command]
async fn set_panel_position(app: AppHandle, position: String) -> Result<settings::PanelSettings, String> {
    let state=app.state::<AppState>();
    if state.drag.lock().unwrap().is_some() { return Err("拖动结束后再更改停靠位置".into()); }
    let edge=match position.as_str() { "left"=>PanelEdge::Left,"right"=>PanelEdge::Right,"top"=>PanelEdge::Top,"free"=>PanelEdge::Right,_=>return Err("Unknown panel position".into()) };
    let window=app.get_webview_window("main").ok_or("no main window")?;
    let monitor=window.current_monitor().ok().flatten().or(window.primary_monitor().ok().flatten()).ok_or("No display available")?;
    let mut placement=state.placement.lock().unwrap().clone();
    let was_docked=placement.dock_edge.is_some();
    let display=monitor_display(&monitor);
    let (_,_,rw,rh)=*state.rail_metrics.lock().unwrap();
    let old_edge=*state.edge.lock().unwrap();
    let mut memory=desktop::resolve(&placement,&[display.clone()],Some(display.id.as_str()),(rw,rh)).map(|(_,memory)|memory).unwrap_or_default();
    memory.dock_edge=if position=="free" {None}else{Some(edge)};
    memory.facing_edge=Some(if position=="free"&&memory.horizontal_ratio<0.5{PanelEdge::Left}else{edge});
    let prefs=state.preferences.lock().unwrap().clone();let flare=if prefs.round_ends{32.}else{24.}*prefs.panel_size;
    let rail=desktop::rail_for_memory((rw,rh),old_edge,was_docked,&memory,flare);
    placement.dock_edge=memory.dock_edge;placement.facing_edge=Some(desktop::memory_edge(&memory));
    placement.rail_position=Some(desktop::restored_origin(&memory,&display,rail));
    placement.rail_space=Some(display.clone());
    placement.display=Some(display.id.clone());placement.per_display.insert(display.id.clone(),memory);
    let edge=placement.facing_edge.unwrap_or(edge);
    let chrome_changed=old_edge!=edge||was_docked!=placement.dock_edge.is_some();
    *state.edge.lock().unwrap()=edge;*state.placement.lock().unwrap()=placement.clone();settings::save(&placement);
    let _=app.emit(EVENT_EDGE_CHANGED,edge);let _=app.emit("placement-changed",&placement);
    if !chrome_changed{place_panel(&app);}
    Ok(placement)
}

#[tauri::command]
async fn show_panel_menu(app: AppHandle) -> Result<(), String> {
    let window = app.get_webview_window("main").ok_or("no main window")?;
    let settings = MenuItem::with_id(&app, "panel-settings", "设置…", true, None::<&str>).map_err(|e| e.to_string())?;
    let quit = MenuItem::with_id(&app, "panel-quit", "退出 Pulse", true, None::<&str>).map_err(|e| e.to_string())?;
    let update = MenuItem::with_id(&app, "panel-update", "检查更新…", true, None::<&str>).map_err(|e| e.to_string())?;
    let menu = Menu::with_items(&app, &[&settings, &update, &quit]).map_err(|e| e.to_string())?;
    let state = app.state::<AppState>();
    state.menu_open.store(true, std::sync::atomic::Ordering::SeqCst);
    let _ = app.emit("menu-changed", true);
    let result = window.popup_menu(&menu).map_err(|e| e.to_string());
    state.menu_open.store(false, std::sync::atomic::Ordering::SeqCst);
    let _ = app.emit("menu-changed", false);
    result
}

/// Resize the panel to the rail's measured size and re-place it.
/// The original stores the *rail's* position rather than the window's, because
/// the window is much wider than the rail. Here the same split holds: the
/// frontend measures the rail and asks for a frame that can hold it plus a
/// card, and the frame is then flushed against the screen edge.
#[tauri::command]
async fn set_panel_metrics(app: AppHandle, width: f64, height: f64, rail_x: f64, rail_y: f64, rail_width: f64, rail_height: f64) -> Result<(), String> {
    let window = app
        .get_webview_window("main")
        .ok_or_else(|| "no main window".to_string())?;

    *app.state::<AppState>().rail_metrics.lock().unwrap() = (rail_x, rail_y, rail_width, rail_height);
    app.state::<AppState>().placement_layout_ready.store(true,std::sync::atomic::Ordering::Release);
    let wanted = tauri::LogicalSize::new(width.max(1.0), height.max(1.0));
    let outer = window
        .outer_size()
        .map_err(|e| e.to_string())?
        .to_logical::<f64>(window.scale_factor().unwrap_or(1.0));
    // Only touch the window when the frame really has to change: a redundant
    // resize re-runs the webview's layout and is visible as a flicker.
    if (outer.width - wanted.width).abs() > 0.5 || (outer.height - wanted.height).abs() > 0.5 {
        let _ = window.set_size(wanted);
    }
    let state=app.state::<AppState>();
    let dragging=state.drag.lock().unwrap().is_some_and(|d|d.moved);
    if dragging {
        if let Some(drag)=state.drag.lock().unwrap().as_mut(){drag.awaiting_metrics=false;}
        if let Some((x,y))=state.placement.lock().unwrap().rail_position {
            let scale=window.scale_factor().unwrap_or(1.0);
            let _=window.set_position(tauri::PhysicalPosition::new(x-(rail_x*scale) as i32,y-(rail_y*scale) as i32));
        }
    } else {place_panel(&app);}
    Ok(())
}

/// Start dragging the rail away from its edge.
///
/// Tauri's own `start_dragging` hands the move to Windows' caption drag loop,
/// which never reports where the window was let go — and where it was let go is
/// the whole question, because that decides the edge. So the drag is driven
/// from here instead: this records the grab point, `watch_pointer` follows the
/// cursor, and the edge is chosen on release.
#[tauri::command]
async fn begin_drag(app: AppHandle, state: State<'_, AppState>) -> Result<(), String> {
    let window = app
        .get_webview_window("main")
        .ok_or_else(|| "no main window".to_string())?;

    let cursor = window.cursor_position().map_err(|e| e.to_string())?;
    let origin = window.outer_position().map_err(|e| e.to_string())?;
    let scale=window.scale_factor().unwrap_or(1.0);
    let (rx,ry,_,_)=*state.rail_metrics.lock().unwrap();

    // While the rail is off its edge nothing may be click-through, or the
    // button-up would land on whatever is underneath.
    let _ = window.set_ignore_cursor_events(false);

    *state.drag.lock().unwrap() = Some(Drag {
        cursor: (cursor.x as i32, cursor.y as i32),
        grab: ((cursor.x-origin.x as f64)/scale-rx,(cursor.y-origin.y as f64)/scale-ry),
        moved: false,
        awaiting_metrics: false,
    });

    Ok(())
}

// ---------------------------------------------------------------------------
// Window / tray wiring
// ---------------------------------------------------------------------------

/// Put the window against the docked edge of the monitor, centred along it.
///
/// The original's rule is that the panel **stores the rail's position, never
/// the window's** — "the window is much wider than the rail" — and that a frame
/// is not granted until the window is on screen. So `show_panel` places first,
/// orders the window in, and then places again: the first call puts it roughly
/// right so it does not appear at the origin and slide into place, and the
/// second measures against what it actually got. Skipping the second call left
/// the rail drawn tens of points too low until anything else re-placed it.
fn place_panel(app: &AppHandle) {
    let Some(window) = app.get_webview_window("main") else {
        return;
    };

    let state = app.state::<AppState>();
    if state.drag.lock().unwrap().is_some() { return; }
    let mut placement = state.placement.lock().unwrap().clone();
    let Ok(monitors)=window.available_monitors() else{return};
    let displays=monitors.iter().map(monitor_display).collect::<Vec<_>>();
    let primary=window.primary_monitor().ok().flatten().map(|monitor|monitor_display(&monitor).id);
    let (rx,ry,rw,rh)=*state.rail_metrics.lock().unwrap();
    let Some((display,memory))=desktop::resolve(&placement,&displays,primary.as_deref(),(rw,rh)) else{return};
    let previous=placement.clone();
    let old_edge=*state.edge.lock().unwrap();let edge=desktop::memory_edge(&memory);
    let prefs=state.preferences.lock().unwrap().clone();
    let flare=if prefs.round_ends{32.}else{24.}*prefs.panel_size;
    let rail=desktop::rail_for_memory((rw,rh),old_edge,placement.dock_edge.is_some(),&memory,flare);
    let origin=desktop::layout_origin(&placement,&memory,&display,rail);
    if !state.placement_layout_ready.load(std::sync::atomic::Ordering::Acquire) {
        // The startup tuple is only a placeholder. Place roughly before
        // showing, but migrate old absolute pixels only after the frontend
        // supplies the actual rail size and its offset inside the frame.
        let temporary=desktop::absolute_anchor(&placement,&display).unwrap_or(origin);
        if old_edge!=edge{*state.edge.lock().unwrap()=edge;let _=app.emit(EVENT_EDGE_CHANGED,edge);}
        let _=window.set_position(tauri::PhysicalPosition::new(temporary.0-(rx*display.scale).round() as i32,temporary.1-(ry*display.scale).round() as i32));
        return;
    }
    placement.dock_edge=memory.dock_edge;placement.facing_edge=Some(edge);
    desktop::record_layout(&mut placement,&display,&memory,origin,rail);
    let chrome_changed=old_edge!=edge||previous.dock_edge.is_some()!=placement.dock_edge.is_some();
    *state.edge.lock().unwrap()=edge;
    *state.placement.lock().unwrap()=placement.clone();
    if placement!=previous{settings::save(&placement);let _=app.emit("placement-changed",&placement);}
    if chrome_changed {
        let _=app.emit(EVENT_EDGE_CHANGED,edge);
        // The frontend measures the new orientation and calls this helper
        // again. Avoid moving a rail with the old orientation's card offset.
        return;
    }
    let _=window.set_position(tauri::PhysicalPosition::new(origin.0-(rx*display.scale).round() as i32,origin.1-(ry*display.scale).round() as i32));
}

/// Which edge the window is nearest, once the drag has been let go.
///
/// Only left, right and top are dockable — there is no bottom rail in the
/// original, and a rail hanging off the bottom edge would have to draw its card
/// upwards for no reason. A drop nearest the bottom therefore keeps the edge it
/// came from.
/// Show the panel, placing it before and after it is ordered in.
fn show_panel(app: &AppHandle) {
    app.state::<AppState>().fullscreen_hidden.store(false,std::sync::atomic::Ordering::SeqCst);
    let no_selection = app.state::<AppState>().preferences.lock().unwrap().enabled_providers.is_empty();
    if no_selection { open_settings(app); return; }
    let Some(window) = app.get_webview_window("main") else {
        return;
    };
    place_panel(app);
    let _ = window.show();
    place_panel(app);

    // `outer_size` is not final until the window has actually been shown, so a
    // frame measured before `show()` can be a frame out of date. Re-place once
    // the compositor has settled, and only while the panel is still on screen.
    let handle = app.clone();
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(Duration::from_millis(400)).await;
        if let Some(window) = handle.get_webview_window("main") {
            if window.is_visible().unwrap_or(false) {
                place_panel(&handle);
            }
        }
    });
}

fn toggle_panel(app: &AppHandle) {
    app.state::<AppState>().fullscreen_hidden.store(false,std::sync::atomic::Ordering::SeqCst);
    let Some(window) = app.get_webview_window("main") else {
        return;
    };
    match window.is_visible() {
        Ok(true) => {
            let _ = window.hide();
        }
        _ => show_panel(app),
    }
}

/// Whether the primary mouse button is currently held.
///
/// The webview cannot answer this: once the window is dragged, the pointer
/// leaves the rail and the rail leaves the pointer, so no `pointerup` ever
/// arrives. The drag has to be ended by asking the OS.
#[cfg(windows)]
fn left_button_down() -> bool {
    #[link(name = "user32")]
    extern "system" {
        fn GetAsyncKeyState(key: i32) -> i16;
    }
    // VK_LBUTTON. The high bit means "down now".
    unsafe { GetAsyncKeyState(0x01) < 0 }
}

#[cfg(not(windows))]
fn left_button_down() -> bool {
    false
}

/// Carry a drag one step, and dock the panel when the button comes up.
///
/// Returns true while a drag is in progress, in which case the caller must not
/// also run the hover sampling — a drag is not a hover, and the window is
/// following the pointer rather than being pointed at.
fn advance_drag(app: &AppHandle, window: &tauri::WebviewWindow) -> bool {
    let state = app.state::<AppState>();
    let drag = { *state.drag.lock().unwrap() };
    let Some(drag) = drag else {
        return false;
    };

    if !left_button_down() {
        *state.drag.lock().unwrap()=None;
        if drag.moved {
            settings::save(&state.placement.lock().unwrap());
            let _=app.emit("drag-changed",false);
            place_panel(app);
        }
        return false;
    }
    if drag.awaiting_metrics{return true}
    if let Ok(cursor)=window.cursor_position() {
        let dx=cursor.x-drag.cursor.0 as f64;let dy=cursor.y-drag.cursor.1 as f64;
        if !drag.moved&&dx*dx+dy*dy<25.{return true}
        let Ok(monitors)=window.available_monitors() else{return true};
        let Some(monitor)=monitors.iter().find(|m|{let r=monitor_rect(m);cursor.x>=r.left as f64&&cursor.x<r.right as f64&&cursor.y>=r.top as f64&&cursor.y<r.bottom as f64}) else{return true};
        let area=monitor.work_area();
        let screen=desktop::Rect{left:area.position.x,top:area.position.y,right:area.position.x+area.size.width as i32,bottom:area.position.y+area.size.height as i32};
        let (rx,ry,rw,rh)=*state.rail_metrics.lock().unwrap();
        let old=state.placement.lock().unwrap().clone();let edge=*state.edge.lock().unwrap();
        let prefs=state.preferences.lock().unwrap().clone();
        let flare=if prefs.round_ends{32.}else{24.}*prefs.panel_size;
        let landing=desktop::drag_landing((cursor.x,cursor.y),drag.grab,(rw,rh),edge,old.dock_edge.is_some(),screen,monitor.scale_factor(),flare);
        let changed=landing.edge!=edge||landing.docked!=old.dock_edge.is_some();
        if let Some(active)=state.drag.lock().unwrap().as_mut(){active.moved=true;active.grab=landing.grab;active.awaiting_metrics=changed;}
        if !drag.moved{let _=app.emit("drag-changed",true);}
        let mut placement=old.clone();placement.dock_edge=landing.docked.then_some(landing.edge);placement.rail_position=Some(landing.origin);placement.facing_edge=Some(landing.edge);
        desktop::remember(&mut placement,&monitor_display(monitor),landing.origin,landing.rail);
        *state.placement.lock().unwrap()=placement.clone();*state.edge.lock().unwrap()=landing.edge;
        if changed {
            let _=app.emit(EVENT_EDGE_CHANGED,landing.edge);let _=app.emit("placement-changed",placement);
        } else {
            let scale=monitor.scale_factor();
            let _=window.set_position(tauri::PhysicalPosition::new(landing.origin.0-(rx*scale) as i32,landing.origin.1-(ry*scale) as i32));
        }
    }

    true
}

/// Watch the cursor and hand the empty parts of the panel back to the desktop.
///
/// On macOS the panel samples the pointer because `.onHover` never fires on a
/// non-key panel. Windows has the same problem from the other side: the window
/// is several times wider than the rail, so between the rail and the edge of
/// the window sits a band of nothing that would swallow every click meant for
/// whatever is underneath. Sampling the cursor is what lets the panel answer
/// "is the pointer on me?" while it is still click-through — a webview that is
/// not receiving input cannot report its own hover.
///
/// The hovered region's id is pushed to the frontend rather than raw
/// coordinates: the raw stream would be an event per frame, and the frontend
/// only ever cares about which ring is lit.
fn monitor_rect(monitor:&tauri::Monitor)->desktop::Rect {
    let p=monitor.position();let s=monitor.size();
    desktop::Rect{left:p.x,top:p.y,right:p.x+s.width as i32,bottom:p.y+s.height as i32}
}

fn monitor_display(monitor:&tauri::Monitor)->desktop::Display {
    let bounds=monitor_rect(monitor);let area=monitor.work_area();
    desktop::Display{
        id:desktop::display_identity(monitor.name().map(|name|name.as_str()),bounds),bounds,
        work_area:desktop::Rect{left:area.position.x,top:area.position.y,right:area.position.x+area.size.width as i32,bottom:area.position.y+area.size.height as i32},
        scale:monitor.scale_factor(),
    }
}

fn watch_desktop(app:AppHandle) {
    tauri::async_runtime::spawn(async move {
        let mut displays=String::new();
        let mut last_pointer_display:Option<String>=None;
        let mut last_visible:Option<bool>=None;
        let mut next_power_check=std::time::Instant::now();
        let mut clock_watcher=power_state::ClockWatcher::default();
        loop {
            tokio::time::sleep(Duration::from_millis(250)).await;
            let state=app.state::<AppState>();
            if std::time::Instant::now()>=next_power_check {
                next_power_check=std::time::Instant::now()+Duration::from_secs(2);
                state.power_events.poll_power();
                if let Some(sample)=power_state::clock_sample(chrono::Utc::now().timestamp_millis()) {
                    if let Some(change)=clock_watcher.observe(sample){
                        // Actual uptime minus working-state uptime detects
                        // suspend, not a busy app or an assumed dark screen.
                        // The native callback owns wake events when present.
                        if change.suspended_millis>=power_state::CLOCK_TOLERANCE_MILLIS&&!state.power_events.listening(){state.power_events.recovered();}
                        state.refresh_wake.notify_one();
                    }
                }
            }
            let Some(window)=app.get_webview_window("main") else{continue};
            let visible=window.is_visible().unwrap_or(false);
            if last_visible!=Some(visible){
                if last_visible==Some(false)&&visible&&!state.refresh_schedule.lock().unwrap().primary.is_empty(){
                    let visible_app=app.clone();
                    tauri::async_runtime::spawn(async move{
                        let state=visible_app.state::<AppState>();
                        if let Ok(_guard)=state.refreshing.try_lock(){refresh_locked(&visible_app,&state,None).await;};
                    });
                }
                last_visible=Some(visible);state.refresh_wake.notify_one();
            }
            if state.drag.lock().unwrap().is_some()||state.menu_open.load(std::sync::atomic::Ordering::SeqCst){continue}
            let suppressed=state.fullscreen_hidden.load(std::sync::atomic::Ordering::SeqCst);
            if !visible&&!suppressed{continue}
            let Ok(monitors)=window.available_monitors() else{continue};
            let geometry=monitors.iter().map(monitor_display).collect::<Vec<_>>();
            let signature=format!("{geometry:?}");
            if signature!=displays {place_panel(&app);displays=signature;last_pointer_display=None;}
            let Ok(origin)=window.outer_position() else{continue};
            let (rx,ry,rw,rh)=*state.rail_metrics.lock().unwrap();let scale=window.scale_factor().unwrap_or(1.0);
            let rail=(origin.x+(rx*scale) as i32,origin.y+(ry*scale) as i32);
            let current=geometry.iter().find(|display|display.contains((rail.0 as f64+rw*scale/2.,rail.1 as f64+rh*scale/2.)));
            let prefs=state.preferences.lock().unwrap().clone();
            if prefs.enabled_providers.is_empty(){state.fullscreen_hidden.store(false,std::sync::atomic::Ordering::SeqCst);continue}
            // Follow only while visible. A fullscreen-hidden rail should not
            // unexpectedly move or reveal itself on another display.
            if visible&&prefs.follow_active_display&&monitors.len()>1 {
                if let (Some(old),Ok(pointer))=(current,window.cursor_position()) {
                    if let Some(next)=geometry.iter().find(|display|display.contains((pointer.x,pointer.y))) {
                        if last_pointer_display.as_deref()!=Some(next.id.as_str()) {
                            if next.id!=old.id {
                                let mut placement=state.placement.lock().unwrap();
                                desktop::switch_display(&mut placement,old,next,rail,(rw,rh));
                                settings::save(&placement);drop(placement);
                                place_panel(&app);
                                last_pointer_display=Some(next.id.clone());
                                continue;
                            }
                            last_pointer_display=Some(next.id.clone());
                        }
                    }
                }
            } else {last_pointer_display=None;}
            let fullscreen=prefs.hide_in_fullscreen&&current.is_some_and(|display|desktop::fullscreen_on(display.bounds));
            if fullscreen&&visible {
                if window.hide().is_ok(){state.fullscreen_hidden.store(true,std::sync::atomic::Ordering::SeqCst);let _=app.emit(EVENT_HOVER_CHANGED,Option::<String>::None);}
            } else if !fullscreen&&suppressed {
                state.fullscreen_hidden.store(false,std::sync::atomic::Ordering::SeqCst);show_panel(&app);
            }
        }
    });
}

async fn note_looked(app:&AppHandle) {
    let state=app.state::<AppState>();
    let prefs=state.preferences.lock().unwrap().clone();
    if prefs.enabled_providers.is_empty(){return}
    let signals=refresh_signals(app,&state);
    let newest={
        let snapshot=state.snapshot.lock().await;
        snapshot.providers.iter().filter(|reading|!reading.windows.is_empty()||reading.credit_remaining.is_some())
            .filter_map(|reading|chrono::DateTime::parse_from_rfc3339(&reading.fetched_at).ok().map(|at|at.timestamp_millis())).max()
    };
    let refresh=state.refresh_schedule.lock().unwrap().looked(prefs.refresh_automatic,prefs.refresh_seconds,signals,newest,chrono::Utc::now().timestamp_millis());
    state.refresh_wake.notify_one();
    if refresh {
        let app=app.clone();
        tauri::async_runtime::spawn(async move {
            let state=app.state::<AppState>();
            // An in-flight pass will already provide fresh figures. Sweeping
            // the rail must not queue another request behind every ring.
            if let Ok(_guard)=state.refreshing.try_lock(){refresh_locked(&app,&state,None).await;};
        });
    }
}

fn watch_pointer(app: AppHandle) {
    tauri::async_runtime::spawn(async move {
        let mut last: Option<String> = None;

        loop {
            tokio::time::sleep(POINTER_INTERVAL).await;

            let Some(window) = app.get_webview_window("main") else {
                continue;
            };
            if !window.is_visible().unwrap_or(false) {
                continue;
            }

            if let (Ok(cursor), Ok(origin)) = (window.cursor_position(), window.outer_position()) {
                let scale = window.scale_factor().unwrap_or(1.0);
                let _ = window.emit("pointer-position", serde_json::json!({"x": (cursor.x-origin.x as f64)/scale, "y": (cursor.y-origin.y as f64)/scale}));
            }

            if app.state::<AppState>().menu_open.load(std::sync::atomic::Ordering::SeqCst) { continue; }

            // A drag owns the pointer: the rail is being moved, not pointed at.
            if advance_drag(&app, &window) {
                continue;
            }

            let regions = app.state::<AppState>().hit_regions.lock().await.clone();
            // Nothing claimed yet: leave the panel interactive.
            if regions.is_empty() {
                if last.is_some() {
                    last = None;
                }
                continue;
            }

            let hit = pointer_region(&window, &regions);
            let hovered = hit.hovered;

            let _ = window.set_ignore_cursor_events(!hit.claims_click);
            if hovered != last {
                if hovered.is_some(){note_looked(&app).await;}
                let _ = app.emit(EVENT_HOVER_CHANGED, hovered.clone());
                last = hovered;
            }
        }
    });
}

/// Hover retention and actual click ownership are distinct in the original:
/// the card's full-width band retains hover, but only contentShape claims it.
#[derive(Default, Debug, PartialEq)]
struct PointerHit {
    hovered: Option<String>,
    claims_click: bool,
}

fn pointer_region(window: &tauri::WebviewWindow, regions: &[HitRegion]) -> PointerHit {
    let (Ok(cursor), Ok(origin)) = (window.cursor_position(), window.outer_position()) else {return PointerHit::default();};
    let scale = window.scale_factor().unwrap_or(1.0);
    if !scale.is_finite() || scale <= 0.0 {return PointerHit::default();}

    // Regions are declared in CSS pixels; the cursor arrives in physical ones.
    let x = (cursor.x - origin.x as f64) / scale;
    let y = (cursor.y - origin.y as f64) / scale;

    pointer_hits(regions,x,y)
}

fn pointer_hits(regions:&[HitRegion],x:f64,y:f64)->PointerHit {
    PointerHit {
        hovered:regions.iter().rev().find(|r|region_contains(r,regions,x,y)).map(|r|r.id.clone()),
        claims_click:regions.iter().any(|r|!r.hover_only&&region_contains(r,regions,x,y)),
    }
}

fn region_contains(region:&HitRegion,regions:&[HitRegion],x:f64,y:f64)->bool {
    if !surface_contains(region,x,y) {return false;}
    let Some(id)=region.clip_to.as_deref() else {return true;};
    // A rectangular hover target can carry the same id as its painted shape.
    // Only the painted shape is eligible to clip a slot.
    regions.iter().find(|r|r.id==id&&!r.hover_only).is_some_and(|clip|surface_contains(clip,x,y))
}

fn surface_contains(region:&HitRegion,x:f64,y:f64)->bool {
    [x,y,region.x,region.y,region.w,region.h].iter().all(|v|v.is_finite())
        &&region.w>0.0&&region.h>0.0
        &&x>=region.x&&x<region.x+region.w&&y>=region.y&&y<region.y+region.h
        &&region.outline.as_ref().map_or(true,|outline|outline_contains(outline,x,y))
}

fn outline_contains(outline:&[Vec<[f64;2]>],x:f64,y:f64)->bool {
    if !x.is_finite()||!y.is_finite()||outline.iter().flatten().flatten().any(|n|!n.is_finite()) {return false;}
    let mut winding=0i32;
    for contour in outline {
        if contour.len()<3 {continue;}
        for index in 0..contour.len() {
            let a=contour[index];let b=contour[(index+1)%contour.len()];
            let cross=(b[0]-a[0])*(y-a[1])-(x-a[0])*(b[1]-a[1]);
            if cross.abs()<=1e-8&&x>=a[0].min(b[0])-1e-8&&x<=a[0].max(b[0])+1e-8
                &&y>=a[1].min(b[1])-1e-8&&y<=a[1].max(b[1])+1e-8 {return true;}
            if a[1]<=y&&b[1]>y&&cross>0.0 {winding+=1;}
            if a[1]>y&&b[1]<=y&&cross<0.0 {winding-=1;}
        }
    }
    winding!=0
}

#[cfg(test)]
mod pointer_hit_tests {
    use super::*;
    fn rectangle(id:&str,x:f64,y:f64,w:f64,h:f64)->HitRegion {
        HitRegion {id:id.into(),x,y,w,h,outline:None,clip_to:None,hover_only:false}
    }
    #[test]
    fn transparent_corners_retain_hover_without_claiming_a_click() {
        let mut tracking=rectangle("rail",10.0,20.0,20.0,40.0);tracking.hover_only=true;
        let mut painted=tracking.clone();painted.hover_only=false;
        painted.outline=Some(vec![vec![[20.0,20.0],[30.0,30.0],[30.0,50.0],[20.0,60.0],[10.0,50.0],[10.0,30.0]]]);
        let regions=vec![tracking,painted];
        assert_eq!(pointer_hits(&regions,11.0,21.0),PointerHit{hovered:Some("rail".into()),claims_click:false});
        assert_eq!(pointer_hits(&regions,20.0,21.0),PointerHit{hovered:Some("rail".into()),claims_click:true});
        assert_eq!(pointer_hits(&regions,5.0,21.0),PointerHit::default());
    }
    #[test]
    fn slots_clip_to_the_painted_surface_not_its_tracking_rectangle() {
        let mut tracking=rectangle("rail",0.0,0.0,20.0,40.0);tracking.hover_only=true;
        let mut painted=tracking.clone();painted.hover_only=false;
        painted.outline=Some(vec![vec![[10.0,0.0],[20.0,10.0],[20.0,30.0],[10.0,40.0],[0.0,30.0],[0.0,10.0]]]);
        let mut entry=rectangle("entry:codex",0.0,0.0,20.0,20.0);entry.clip_to=Some("rail".into());
        let regions=vec![tracking,painted,entry];
        assert_eq!(pointer_hits(&regions,1.0,1.0).hovered.as_deref(),Some("rail"));
        assert!(!pointer_hits(&regions,1.0,1.0).claims_click);
        assert_eq!(pointer_hits(&regions,10.0,1.0).hovered.as_deref(),Some("entry:codex"));
        assert!(pointer_hits(&regions,10.0,1.0).claims_click);
    }
    #[test]
    fn card_overlap_uses_nonzero_winding_and_is_not_an_even_odd_hole() {
        let outline=vec![vec![[0.0,0.0],[10.0,0.0],[10.0,20.0],[0.0,20.0]],vec![[9.0,8.0],[15.0,10.0],[9.0,12.0]]];
        assert!(outline_contains(&outline,9.5,10.0));
        assert!(outline_contains(&outline,14.0,10.0));
        assert!(!outline_contains(&outline,14.0,5.0));
        assert!(outline_contains(&outline,10.0,15.0));
    }
    #[test]
    fn missing_clip_and_invalid_outlines_never_claim_a_press() {
        let mut entry=rectangle("entry:codex",0.0,0.0,20.0,20.0);entry.clip_to=Some("missing".into());
        assert_eq!(pointer_hits(&[entry],5.0,5.0),PointerHit::default());
        let mut shape=rectangle("card",0.0,0.0,20.0,20.0);shape.outline=Some(vec![]);
        assert!(!surface_contains(&shape,5.0,5.0));
        shape.outline=Some(vec![vec![[0.0,0.0],[20.0,f64::NAN],[20.0,20.0]]]);
        assert!(!surface_contains(&shape,5.0,5.0));
        assert!(!surface_contains(&rectangle("old",0.0,0.0,20.0,20.0),f64::NAN,5.0));
    }
    #[test]
    fn ipc_shape_fields_are_optional_and_use_frontend_names() {
        let legacy:HitRegion=serde_json::from_value(serde_json::json!({"id":"rail","x":0,"y":0,"w":64,"h":100})).unwrap();
        assert!(surface_contains(&legacy,1.0,1.0));assert!(!legacy.hover_only);assert!(legacy.outline.is_none());
        let shaped:HitRegion=serde_json::from_value(serde_json::json!({"id":"slot","x":0,"y":0,"w":64,"h":100,"clipTo":"rail","hoverOnly":true,"outline":[[[0,0],[64,0],[64,100],[0,100]]]})).unwrap();
        assert!(shaped.hover_only);assert_eq!(shaped.clip_to.as_deref(),Some("rail"));
        let encoded=serde_json::to_value(shaped).unwrap();assert_eq!(encoded["clipTo"],"rail");assert_eq!(encoded["hoverOnly"],true);
    }
}

fn build_tray(app: &tauri::App) -> tauri::Result<()> {
    // Names are suffixed to avoid shadowing the `refresh` function below.
    let show_item = MenuItem::with_id(app, "show", "Show PulseWin", true, None::<&str>)?;
    let settings_item = MenuItem::with_id(app, "settings", "Settings…", true, None::<&str>)?;
    let refresh_item = MenuItem::with_id(app, "refresh", "Refresh now", true, None::<&str>)?;
    let separator = PredefinedMenuItem::separator(app)?;
    let quit_item = MenuItem::with_id(app, "quit", "Quit", true, None::<&str>)?;
    let update_item = MenuItem::with_id(app, "update", "检查更新…", true, None::<&str>)?;
    let menu = Menu::with_items(
        app,
        &[&show_item, &settings_item, &refresh_item, &update_item, &separator, &quit_item],
    )?;

    let mut builder = TrayIconBuilder::with_id("pulse-tray")
        .tooltip("PulseWin")
        .menu(&menu)
        // Left click opens the panel; the menu stays on right click, which is
        // the behaviour Windows users expect from a tray utility.
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| match event.id().as_ref() {
            "show" => show_panel(app),
            "settings" => open_settings(app),
            "update" => updates::open(app),
            "quit" => app.exit(0),
            "refresh" => {
                let app = app.clone();
                tauri::async_runtime::spawn(async move {
                    let state = app.state::<AppState>();
                    refresh(&app, &state).await;
                });
            }
            _ => {}
        })
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                toggle_panel(tray.app_handle());
            }
        });

    if let Some(icon) = app.default_window_icon().cloned() {
        builder = builder.icon(icon);
    }

    builder.build(app)?;
    Ok(())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let mut context=tauri::generate_context!();
    // Native acceptance runs beside the user's installed app. Only this
    // explicit mode and a canonical workspace test profile get a separate
    // instance identity; an invalid profile must never reopen the real app.
    if std::env::args().any(|arg|arg=="--native-smoke") {
        match native_smoke_identity() {
            Ok(identity)=>context.config_mut().identifier=identity,
            Err(error)=>{eprintln!("PulseWin: {error}");std::process::exit(1);}
        }
    }
    let ctx = match Ctx::new() {
        Ok(ctx) => ctx,
        Err(e) => {
            eprintln!("PulseWin: could not build HTTP client: {e}");
            std::process::exit(1);
        }
    };

    tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app, argv, _cwd| {
            // Reopening the app remains a way back to Settings even if the
            // floating panel has been hidden.
            let _=argv;open_settings(app);
        }))
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_global_shortcut::Builder::new().build())
        .plugin(tauri_plugin_autostart::Builder::new().app_name("PulseWin").build())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .on_menu_event(|app, event| match event.id().as_ref() {
            "panel-settings" => open_settings(app),
            "panel-update" => updates::open(app),
            "panel-quit" => app.exit(0),
            _ => {}
        })
        .manage(AppState::new(ctx))
        .manage(application::ApplicationState::default())
        .manage(updates::UpdateState::default())
        .invoke_handler(tauri::generate_handler![
            get_snapshot,
            get_activity,
            get_resets,
            get_preferences,
            save_preferences,
            refresh_provider,
            refresh_now,
            credential_hint,
            report_error,
            provider_settings,
            add_account,
            remove_account,
            save_provider_credential,
            show_settings,
            set_hit_regions,
            set_panel_metrics,
            panel_edge,
            panel_placement,
            begin_drag,
            show_panel_menu,
            set_panel_position,
            token_spend::spend_begin_scan,
            token_spend::spend_get_scan,
            token_spend::spend_cancel_scan,
            token_spend::spend_clear_snapshot
            ,token_spend::spend_card_history
            ,application::get_application_settings
            ,application::set_application_shortcut
            ,application::set_autostart
            ,updates::get_update_status
            ,updates::save_update_settings
            ,updates::check_app_update
            ,updates::install_app_update
            ,updates::verify_app_update_smoke
        ])
        .setup(|app| {
            application::apply(app.handle());
            updates::watch(app.handle().clone());
            // A missing icon set makes tray creation fail; that must not take the
            // whole app down, since the panel still opens on launch.
            if let Err(e) = build_tray(app) {
                eprintln!("PulseWin: tray icon unavailable ({e}).");
                eprintln!("PulseWin: run `npx tauri icon assets/pulse-mark.svg`, then rebuild.");
            }

            // Do not block startup on the network: show the panel, then fetch.
            watch_refresh(app.handle().clone());

            // The panel starts hidden; reveal it once so the app is discoverable.
            let no_selection = app.state::<AppState>().preferences.lock().unwrap().enabled_providers.is_empty();
            if no_selection {
                if let Some(window) = app.get_webview_window("main") { let _ = window.hide(); }
                open_settings(app.handle());
            } else { show_panel(app.handle()); }

            // `--settings` opens the settings window straight away, which is
            // how it is reachable without hunting for the tray icon — and how
            // it can be opened by anything that is not a mouse.
            if std::env::args().any(|arg| arg == "--settings") {
                open_settings(app.handle());
            }

            // The panel is mostly empty space, so it has to keep asking where the
            // pointer is in order to stay out of the desktop's way.
            watch_pointer(app.handle().clone());
            watch_activity(app.handle().clone());
            watch_desktop(app.handle().clone());

            Ok(())
        })
        .on_window_event(|window, event| {
            // Closing the panel hides it; the tray keeps the app alive.
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                api.prevent_close();
                if window.label()=="settings" {token_spend::spend_clear_snapshot();}
                if window.label()=="main" {window.state::<AppState>().fullscreen_hidden.store(false,std::sync::atomic::Ordering::SeqCst);}
                let _ = window.hide();
            }
        })
        .run(context)
        .expect("error while running PulseWin");
}

fn native_smoke_identity()->Result<String,String> {
    use std::hash::{Hash,Hasher};
    let root=std::env::current_dir().map_err(|_|"test workspace is unavailable")?.join("test-results")
        .canonicalize().map_err(|_|"test-results must exist")?;
    let profile=std::env::var_os("APPDATA").map(std::path::PathBuf::from).ok_or("test APPDATA is missing")?
        .canonicalize().map_err(|_|"test APPDATA must exist")?;
    if profile.parent()!=Some(root.as_path()) || !profile.file_name().is_some_and(|n|n.to_string_lossy().starts_with("native-profile-")) {
        return Err("--native-smoke requires an isolated native-profile-* directory directly under workspace test-results".into());
    }
    let mut hash=std::collections::hash_map::DefaultHasher::new();profile.hash(&mut hash);
    Ok(format!("com.pulsewin.native-smoke.{:x}",hash.finish()))
}
