//! Application entry points are independent of quota collection.
use std::{collections::HashMap, sync::Mutex};
use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager};
use tauri_plugin_global_shortcut::{GlobalShortcutExt, Modifiers, Shortcut, ShortcutState};
use tauri_plugin_autostart::ManagerExt;

#[derive(Default)]
pub struct ApplicationState { shortcuts: Mutex<Registrations> }
#[derive(Default)]
struct Registrations { registered: HashMap<String,Shortcut>, unavailable: HashMap<String,String> }
#[derive(Serialize)]
#[serde(rename_all="camelCase")]
pub struct ApplicationSettings {
    open_settings_shortcut: Option<String>, toggle_panel_shortcut: Option<String>,
    unavailable: HashMap<String,String>, autostart_enabled: Option<bool>, autostart_error: Option<String>,
}
pub fn validate(value:&str)->Result<Shortcut,String> {
    let shortcut=value.parse::<Shortcut>().map_err(|_|"无法识别这个快捷键。".to_string())?;
    if !shortcut.mods.intersects(Modifiers::CONTROL|Modifiers::ALT|Modifiers::SUPER) {
        return Err("请同时按 Ctrl、Alt 或 Win；单独按键和仅 Shift 的组合不能注册。".into());
    }
    Ok(shortcut)
}
fn update(app:&AppHandle,action:&str,value:Option<&str>) {
    let state=app.state::<ApplicationState>();
    let mut registrations=state.shortcuts.lock().unwrap();
    let next=value.and_then(|v|validate(v).ok());
    if registrations.registered.get(action)==next.as_ref() && !registrations.unavailable.contains_key(action){return;}
    if let Some(old)=registrations.registered.remove(action) {
        if app.global_shortcut().unregister(old).is_err() {
            registrations.registered.insert(action.into(),old);
            registrations.unavailable.insert(action.into(),"旧快捷键暂时无法释放，请重启应用再试。".into());
            return;
        }
    }
    registrations.unavailable.remove(action);
    let Some(next)=next else {return};
    let open=action=="openSettings";
    let result=app.global_shortcut().on_shortcut(next,move |app,_,event| {
        if event.state!=ShortcutState::Pressed{return;}
        if open {crate::open_settings(app)} else if let Some(window)=app.get_webview_window("main") {
            if window.is_visible().unwrap_or(false) {
                app.state::<crate::AppState>().fullscreen_hidden.store(false,std::sync::atomic::Ordering::SeqCst);
                let _=window.hide();
            } else {crate::show_panel(app);}
        }
    });
    match result {
        Ok(())=>{registrations.registered.insert(action.into(),next);}
        Err(_)=>{registrations.unavailable.insert(action.into(),"这个组合无法注册，可能已被其他应用或另一个快捷键占用。".into());}
    }
}
pub fn apply(app:&AppHandle) {
    let prefs=app.state::<crate::AppState>().preferences.lock().unwrap().clone();
    update(app,"openSettings",prefs.open_settings_shortcut.as_deref());
    update(app,"togglePanel",prefs.toggle_panel_shortcut.as_deref());
}
#[tauri::command]
pub fn get_application_settings(app:AppHandle)->ApplicationSettings {
    let prefs=app.state::<crate::AppState>().preferences.lock().unwrap().clone();
    let unavailable=app.state::<ApplicationState>().shortcuts.lock().unwrap().unavailable.clone();
    let (autostart_enabled,autostart_error)=match app.autolaunch().is_enabled(){
        Ok(enabled)=>(Some(enabled),None),Err(_)=>(None,Some("无法读取 Windows 登录启动状态。".into())),
    };
    ApplicationSettings {open_settings_shortcut:prefs.open_settings_shortcut,toggle_panel_shortcut:prefs.toggle_panel_shortcut,unavailable,autostart_enabled,autostart_error}
}
#[tauri::command]
pub fn set_application_shortcut(app:AppHandle,action:String,value:Option<String>)->Result<ApplicationSettings,String> {
    if !["openSettings","togglePanel"].contains(&action.as_str()){return Err("未知的快捷键动作。".into());}
    let value=value.map(|v|v.trim().to_string()).filter(|v|!v.is_empty());
    if let Some(value)=&value {validate(value)?;}
    let state=app.state::<crate::AppState>();
    {
        let mut prefs=state.preferences.lock().unwrap();let mut next=prefs.clone();
        if action=="openSettings"{next.open_settings_shortcut=value.clone();}else{next.toggle_panel_shortcut=value.clone();}
        crate::preferences::save(&next)?;*prefs=next.clone();
        let _=app.emit("preferences-changed",next);
    }
    update(&app,&action,value.as_deref());
    Ok(get_application_settings(app))
}
#[tauri::command]
pub fn set_autostart(app:AppHandle,enabled:bool)->Result<ApplicationSettings,String> {
    let result=if enabled{app.autolaunch().enable()}else{app.autolaunch().disable()};
    result.map_err(|_|"无法更改登录启动设置，请检查当前用户的权限。".to_string())?;
    Ok(get_application_settings(app))
}
#[cfg(test)] mod tests {
    use super::*;
    #[test] fn bare_keys_and_shift_only_are_not_global_shortcuts() {
        for key in ["KeyP","Shift+KeyP","F5","Shift+F5",""]{assert!(validate(key).is_err());}
        for key in ["Control+KeyP","Alt+F5","Super+Shift+KeyP"]{assert!(validate(key).is_ok());}
    }
}
