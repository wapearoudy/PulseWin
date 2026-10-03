use tauri::{AppHandle, Manager, WebviewUrl, WebviewWindowBuilder};
use tauri_plugin_opener::OpenerExt;

pub const PAGES:&[(&str,&str)]=&[
    ("claude-code","https://claude.ai/settings/usage"),("codex","https://chatgpt.com/codex/settings/usage"),
    ("cursor","https://cursor.com/dashboard?tab=usage"),("copilot","https://github.com/settings/copilot"),
    ("deepseek","https://platform.deepseek.com/usage"),("openai-api","https://platform.openai.com/usage"),
    ("kimi-code","https://www.kimi.com/code/console"),("ollama-cloud","https://ollama.com/settings"),
    ("xiaomi-mimo","https://platform.xiaomimimo.com/#/console/balance"),("replicate","https://replicate.com/account/billing"),
    ("qwen-cloud","https://home.qwencloud.com/billing/subscription/token-plan-individual"),
    ("perplexity","https://www.perplexity.ai/account/usage"),("gitkraken","https://gitkraken.dev/account#ai-usage"),
    ("neuralwatt","https://portal.neuralwatt.com/dashboard"),("amp","https://ampcode.com/settings"),
    ("typesafe","https://console.typesafe.ai/settings/billing"),("mistral","https://admin.mistral.ai/organization/usage"),("xai-api","https://console.x.ai"),
];
#[tauri::command]
pub fn get_usage_pages()->std::collections::HashMap<String,String>{PAGES.iter().map(|(id,url)|(id.to_string(),url.to_string())).collect()}
#[tauri::command]
pub fn open_usage_page(app:AppHandle,account:String)->Result<(),String>{
    let prefs=app.state::<crate::AppState>().preferences.lock().unwrap().clone();
    let base=prefs.accounts.iter().find(|a|a.id==account).map(|a|a.provider.as_str()).unwrap_or(&account);
    let url=PAGES.iter().find(|(id,_)|*id==base).map(|(_,url)|*url).ok_or("这个服务没有已知的官方用量页面")?;
    app.opener().open_url(url,None::<String>).map_err(|e|e.to_string())?;
    hide(&app);Ok(())
}
#[tauri::command]
pub fn hide_usage_dashboard(app:AppHandle){hide(&app)}
pub fn hide(app:&AppHandle){if let Some(w)=app.get_webview_window("usage"){let _=w.hide();}}
#[tauri::command]
pub async fn show_usage_dashboard(app:AppHandle)->Result<(),String>{open(&app,None)}
#[tauri::command]
pub fn resize_usage_dashboard(app:AppHandle,height:f64)->Result<(),String>{
    if !height.is_finite(){return Err("无效的面板高度".into())}
    let Some(window)=app.get_webview_window("usage") else{return Ok(())};
    let anchor=*app.state::<crate::AppState>().usage_anchor.lock().unwrap();
    place(&window,anchor,height.clamp(220.0,560.0))
}
#[tauri::command]
pub fn quit_application(app:AppHandle){app.exit(0)}

pub fn request_open(app:&AppHandle,anchor:Option<(i32,i32)>){
    let app=app.clone();tauri::async_runtime::spawn_blocking(move||{
        if let Err(error)=open(&app,anchor){eprintln!("PulseWin: {error}");}
    });
}
pub fn open(app:&AppHandle,anchor:Option<(i32,i32)>)->Result<(),String>{
    let state=app.state::<crate::AppState>();
    let _creation=state.usage_creation.lock().unwrap();
    let window=match app.get_webview_window("usage") {Some(w)=>w,None=>WebviewWindowBuilder::new(app,"usage",WebviewUrl::App("index.html?view=usage".into()))
        .title("PulseWin · 用量概览").inner_size(320.0,560.0).visible(false).decorations(false).shadow(true)
        .resizable(false).skip_taskbar(true).always_on_top(true).build().map_err(|e|e.to_string())?};
    *app.state::<crate::AppState>().usage_anchor.lock().unwrap()=anchor;
    let height=window.inner_size().map(|s|s.height as f64/window.scale_factor().unwrap_or(1.0)).unwrap_or(320.0);
    place(&window,anchor,height)?;
    window.show().map_err(|e|e.to_string())?;window.set_focus().map_err(|e|e.to_string())?;Ok(())
}
fn place(window:&tauri::WebviewWindow,anchor:Option<(i32,i32)>,height:f64)->Result<(),String>{
    let monitor=match anchor {Some((x,y))=>window.monitor_from_point(x as f64,y as f64),None=>window.current_monitor()}
        .map_err(|e|e.to_string())?.or(window.primary_monitor().map_err(|e|e.to_string())?).ok_or("没有可用显示器")?;
    let area=monitor.work_area();let scale=monitor.scale_factor();
    let logical_height=height.min(area.size.height as f64/scale);
    let logical_width=320.0f64.min(area.size.width as f64/scale);
    window.set_size(tauri::LogicalSize::new(logical_width,logical_height)).map_err(|e|e.to_string())?;
    let work=crate::desktop::Rect{left:area.position.x,top:area.position.y,right:area.position.x+area.size.width as i32,bottom:area.position.y+area.size.height as i32};
    let at=anchor.unwrap_or((work.right-24,work.bottom));
    let (x,y)=crate::tray_usage::popup_position(at,((logical_width*scale).round() as i32,(logical_height*scale).round() as i32),work);
    window.set_position(tauri::PhysicalPosition::new(x,y)).map_err(|e|e.to_string())?;
    Ok(())
}

#[cfg(test)]mod tests{
    use super::*;
    #[test]fn usage_links_are_original_known_pages_and_registered(){let registry=crate::providers::registry();for (id,url) in PAGES{assert!(registry.iter().any(|p|p.id()==*id),"{id}");assert!(url.starts_with("https://"));}}
}
