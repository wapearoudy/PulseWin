//! Signed in-app updates. Local builds and HTTPS feeds use the same Tauri
//! signature verifier and Windows installer, preserving the installation path.
use std::{path::{Path, PathBuf}, sync::Mutex, time::Duration};
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager, State};
use tauri_plugin_updater::UpdaterExt;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

const MAX_MANIFEST: u64 = 256 * 1024;
const MAX_PACKAGE: u64 = 256 * 1024 * 1024;
const EVENT: &str = "update-status";

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all="camelCase",default)]
pub struct UpdateSettings { pub source:String, pub automatic:bool, pub backup_automatic:bool, pub notes_language:String }
impl Default for UpdateSettings {
    fn default() -> Self {
        Self { source:include_str!("../update-channel.txt").trim().into(),automatic:true,backup_automatic:true,notes_language:"auto".into() }
    }
}
#[derive(Clone, Serialize)]
#[serde(rename_all="camelCase")]
pub struct Status {
    pub current_version: String, pub settings: UpdateSettings,
    pub phase: String, pub version: Option<String>, pub notes: Option<String>,
    pub notes_by_language:std::collections::HashMap<String,String>,pub checked_source:Option<String>,
    pub downloaded: u64, pub total: Option<u64>, pub checked_at: Option<String>, pub error: Option<String>,
}
impl Default for Status {
    fn default() -> Self {
        Self { current_version: env!("CARGO_PKG_VERSION").into(), settings: load(),
            phase: "idle".into(), version: None, notes: None, downloaded: 0,
            total: None, checked_at: None, error: None,notes_by_language:Default::default(),checked_source:None }
    }
}
#[derive(Default)]
pub struct UpdateState { status: Mutex<Status>, operation: tokio::sync::Mutex<()> }

fn config_path() -> Option<PathBuf> { crate::settings::directory().map(|p|p.join("updates.json")) }
fn load() -> UpdateSettings {
    config_path().and_then(|p|std::fs::read(p).ok()).and_then(|b|serde_json::from_slice::<UpdateSettings>(&b).ok())
        .filter(|s|validate_source(&s.source).is_ok()).unwrap_or_default()
}
fn publish(app:&AppHandle, mutate:impl FnOnce(&mut Status)) -> Status {
    let state=app.state::<UpdateState>();
    let mut status=state.status.lock().unwrap(); mutate(&mut status);
    let value=status.clone(); drop(status); let _=app.emit(EVENT,&value);
    if let Some(tray)=app.tray_by_id("pulse-tray") {
        let text=if value.phase=="available"{format!("PulseWin · 新版本 {} 可用",value.version.as_deref().unwrap_or(""))}else{"PulseWin".into()};
        let _=tray.set_tooltip(Some(text));
    }
    value
}
fn fail(app:&AppHandle, error:&str) -> String {
    publish(app,|s|{s.phase="error".into();s.error=Some(error.into());});error.into()
}
enum Source { Local(PathBuf), Online(reqwest::Url) }
fn validate_source(source:&str) -> Result<Source,String> {
    let source=source.trim();
    if source.is_empty() {return Err("请填写本机更新目录或 HTTPS 更新地址。".into());}
    if source.starts_with("https://") {
        let url=reqwest::Url::parse(source).map_err(|_|"更新地址无效。")?;
        if url.host_str().is_none() || !url.username().is_empty() || url.password().is_some() || url.fragment().is_some() {
            return Err("更新地址不能包含用户名、密码或片段。".into());
        }
        return Ok(Source::Online(url));
    }
    let path=PathBuf::from(source);
    if !path.is_absolute() || source.contains("://") {return Err("请使用绝对目录或 HTTPS 地址；不支持普通 HTTP。".into());}
    Ok(Source::Local(path))
}
#[derive(Deserialize)]
struct LocalManifest { version:String, #[serde(default)] notes:Option<String>,file:String,signature:String,#[serde(default,rename="notesByLanguage")] notes_by_language:std::collections::HashMap<String,String> }
fn parse_local(bytes:&[u8],root:&Path) -> Result<(LocalManifest,PathBuf),String> {
    if bytes.len() as u64>MAX_MANIFEST {return Err("更新清单超过大小限制。".into());}
    let manifest:LocalManifest=serde_json::from_slice(bytes).map_err(|_|"更新清单格式无效。")?;
    semver::Version::parse(&manifest.version).map_err(|_|"更新版本号无效。")?;
    if manifest.signature.trim().is_empty() {return Err("更新包缺少签名。".into());}
    // Exactly one filename; manifests cannot escape the chosen update folder.
    let name=&manifest.file;
    if name.is_empty() || name.contains(['/', '\\', ':']) || name=="." || name==".." || !name.ends_with(".exe") {
        return Err("更新包文件名无效。".into());
    }
    let package=root.join(name); Ok((manifest,package))
}
fn newer(version:&str,current:&str) -> Result<bool,String> {
    Ok(semver::Version::parse(version).map_err(|_|"更新版本号无效。")?>semver::Version::parse(current).map_err(|_|"当前版本号无效。")?)
}
async fn bounded_file(path:&Path,limit:u64) -> Result<Vec<u8>,String> {
    let file=tokio::fs::File::open(path).await.map_err(|_|"无法读取更新文件，请检查更新目录。")?;
    if file.metadata().await.map_err(|_|"无法读取更新文件信息。")?.len()>limit {return Err("更新文件超过大小限制。".into());}
    let mut bytes=Vec::new(); file.take(limit+1).read_to_end(&mut bytes).await.map_err(|_|"更新文件读取失败。")?;
    if bytes.len() as u64>limit {return Err("更新文件超过大小限制。".into());} Ok(bytes)
}

#[tauri::command]
pub fn get_update_status(state:State<'_,UpdateState>) -> Status { state.status.lock().unwrap().clone() }
#[tauri::command]
pub async fn save_update_settings(app:AppHandle, settings:UpdateSettings) -> Result<Status,String> {
    validate_source(&settings.source)?;
    if !["auto","zh-CN","zh-TW","en","ja","ko"].contains(&settings.notes_language.as_str()){return Err("请选择有效的发布说明语言。".into())}
    let state=app.state::<UpdateState>();
    let _operation=state.operation.try_lock().map_err(|_|"正在检查或更新，请稍后修改来源。")?;
    let settings=UpdateSettings{source:settings.source.trim().into(),..settings};
    let path=config_path().ok_or("无法定位更新设置目录。")?;
    tokio::fs::create_dir_all(path.parent().unwrap()).await.map_err(|_|"无法创建更新设置目录。")?;
    // Atomic replacement uses the existing credential-store file helper.
    let previous=std::fs::read(&path).ok();
    crate::credential_store::atomic_write(&path,&serde_json::to_vec_pretty(&settings).map_err(|_|"无法保存更新设置。")?,previous.as_deref())
        .map_err(|_|"无法保存更新设置，请重试。")?;
    Ok(publish(&app,|s|{if s.settings.source!=settings.source{s.phase="idle".into();s.version=None;s.notes=None;s.notes_by_language.clear();s.checked_source=None;s.error=None;}s.settings=settings;}))
}

fn online_builder(app:&AppHandle,url:reqwest::Url) -> Result<tauri_plugin_updater::Updater,String> {
    let mut builder=app.updater_builder().endpoints(vec![url]).map_err(|_|"更新地址无效。")?
        .timeout(Duration::from_secs(30));
    let network=app.state::<crate::AppState>().preferences.lock().unwrap().network_proxy.clone();
    let policy=crate::proxy::Policy::capture(&network)?;
    // Validate before building the infallible updater client callback. This
    // gives the updater the same manual/system/environment route as quota.
    let _ = policy.configure_updater(updater_http::Client::builder())?;
    builder=builder.configure_client(move|client|policy.configure_updater(client).expect("validated updater proxy policy"));
    builder.build().map_err(|_|"无法初始化更新程序。".into())
}

fn github_repository(url:&reqwest::Url)->Option<(String,String)>{
    if url.host_str()!=Some("github.com"){return None}let p=url.path_segments()?.collect::<Vec<_>>();
    if p.len()!=6 || p[2..]!=["releases","latest","download","latest.json"]{return None}
    let valid=|s:&str|!s.is_empty()&&s!="."&&s!=".."&&s.len()<=120&&s.bytes().all(|b|b.is_ascii_alphanumeric()||b"_-.".contains(&b));
    (valid(p[0])&&valid(p[1])).then(||(p[0].into(),p[1].into()))
}
fn github_asset(v:&serde_json::Value,owner:&str,repo:&str)->Result<reqwest::Url,String>{
    let a=v["assets"].as_array().and_then(|a|a.iter().find(|a|a["name"]=="latest.json"&&a["state"]=="uploaded")).ok_or("GitHub 备用通道没有已发布的更新清单。")?;
    if !a["size"].as_u64().is_some_and(|n|n>0&&n<=MAX_MANIFEST){return Err("备用更新清单大小无效。".into())}
    let url=a["url"].as_str().ok_or("备用更新地址无效。")?.parse::<reqwest::Url>().map_err(|_|"备用更新地址无效。")?;
    let prefix=format!("/repos/{owner}/{repo}/releases/assets/");
    if url.scheme()!="https"||url.host_str()!=Some("api.github.com")||!url.username().is_empty()||url.password().is_some()||url.query().is_some()||url.fragment().is_some()||!url.path().strip_prefix(&prefix).is_some_and(|id|!id.is_empty()&&id.bytes().all(|b|b.is_ascii_digit())){return Err("GitHub 备用地址与发布仓库不一致。".into())}
    Ok(url)
}
async fn github_backup(app:&AppHandle,url:&reqwest::Url)->Result<tauri_plugin_updater::Updater,String>{
    let (owner,repo)=github_repository(url).ok_or("此更新来源没有 GitHub 备用通道。")?;
    let state=app.state::<crate::AppState>();let prefs=state.preferences.lock().unwrap().clone();
    let ctx=crate::providers::Ctx::current(&state.ctx,&prefs.network_proxy).map_err(|_|"无法应用更新代理设置。")?;
    let mut response=ctx.client.get(format!("https://api.github.com/repos/{owner}/{repo}/releases/latest")).header("Accept","application/vnd.github+json").timeout(Duration::from_secs(20)).send().await.map_err(|_|"无法连接 GitHub 备用通道。")?;
    if !response.status().is_success(){return Err(format!("GitHub 备用通道返回 HTTP {}。",response.status().as_u16()))}
    let mut bytes=vec![];while let Some(chunk)=response.chunk().await.map_err(|_|"无法读取备用更新清单。")?{if bytes.len()+chunk.len()>MAX_MANIFEST as usize{return Err("备用清单超过大小限制。".into())}bytes.extend_from_slice(&chunk)}
    let value=serde_json::from_slice(&bytes).map_err(|_|"GitHub 备用清单无效。")?;
    let endpoint=github_asset(&value,&owner,&repo)?;
    let network=state.preferences.lock().unwrap().network_proxy.clone();let policy=crate::proxy::Policy::capture(&network)?;let _ = policy.configure_updater(updater_http::Client::builder())?;
    app.updater_builder().endpoints(vec![endpoint]).map_err(|_|"备用更新地址无效。")?.header("Accept","application/octet-stream").map_err(|_|"备用请求头无效。")?.timeout(Duration::from_secs(30)).configure_client(move|client|policy.configure_updater(client).expect("validated updater proxy policy")).build().map_err(|_|"无法初始化备用更新。".into())
}
async fn check_online(app:&AppHandle,url:reqwest::Url,backup:bool)->Result<(Option<tauri_plugin_updater::Update>,&'static str),String>{
    match online_builder(app,url.clone())?.check().await{
        Ok(update)=>Ok((update,"primary")),
        Err(_) if backup&&github_repository(&url).is_some()=>github_backup(app,&url).await?.check().await.map(|u|(u,"github-api")).map_err(|_|"主更新通道和 GitHub 备用通道均未能读取更新。".into()),
        Err(_)=>Err("检查更新失败，请检查网络或发布地址后重试。".into()),
    }
}
fn notes_languages(v:&serde_json::Value)->std::collections::HashMap<String,String>{
    v.get("notesByLanguage").and_then(|v|serde_json::from_value::<std::collections::HashMap<String,String>>(v.clone()).ok()).unwrap_or_default().into_iter().filter(|(k,v)|["zh-CN","zh-TW","en","ja","ko"].contains(&k.as_str())&&v.len()<=MAX_MANIFEST as usize).collect()
}

#[tauri::command]
pub async fn check_app_update(app:AppHandle) -> Result<Status,String> {
    let state=app.state::<UpdateState>();
    let _operation=state.operation.try_lock().map_err(|_|"更新操作正在进行。")?;
    let settings=state.status.lock().unwrap().settings.clone();let source=settings.source.clone();
    publish(&app,|s|{s.phase="checking".into();s.error=None;});
    let mut languages=std::collections::HashMap::new();let mut checked="local";
    let result:Result<Option<(String,Option<String>)>,String>=async {
        match validate_source(&source)? {
            Source::Local(root)=>{
                let path=root.join("latest.json");
                if !tokio::fs::try_exists(&path).await.map_err(|_|"无法访问更新目录。")? {return Ok(None);}
                let (manifest,package)=parse_local(&bounded_file(&path,MAX_MANIFEST).await?,&root)?;
                if !newer(&manifest.version,env!("CARGO_PKG_VERSION"))? {return Ok(None);}
                if !tokio::fs::try_exists(package).await.map_err(|_|"无法访问更新包。")? {return Err("清单中的更新包不存在，请重新发布更新。".into());}
                languages=manifest.notes_by_language;Ok(Some((manifest.version,manifest.notes)))
            },
            Source::Online(url)=>{
                let (update,channel)=check_online(&app,url,settings.backup_automatic).await?;checked=channel;
                if let Some(update)=update {
                    validate_download(&update.download_url)?;
                    languages=notes_languages(&update.raw_json);
                    Ok(Some((update.version,update.body)))
                }else{Ok(None)}
            }
        }
    }.await;
    match result {
        Ok(update)=>Ok(publish(&app,|s|{s.checked_at=Some(crate::model::now_rfc3339());s.downloaded=0;s.total=None;s.notes_by_language=languages;s.checked_source=Some(checked.into());
            if let Some((version,notes))=update {s.phase="available".into();s.version=Some(version);s.notes=notes;}
            else{s.phase="current".into();s.version=None;s.notes=None;}})),
        Err(error)=>Err(fail(&app,&error))
    }
}
fn validate_download(url:&reqwest::Url) -> Result<(),String> {
    if url.scheme()!="https" || !url.username().is_empty() || url.password().is_some() {return Err("在线更新包必须来自 HTTPS 地址。".into());} Ok(())
}

// A short-lived loopback bridge lets local builds use Tauri's unmodified
// signature verifier / installer. Only these two in-memory files are served,
// on a random loopback port; no directory, credentials or permanent service.
async fn local_bridge(manifest:&LocalManifest,package:Vec<u8>) -> Result<(reqwest::Url,tokio::task::JoinHandle<()>),String> {
    let listener=tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST,0)).await.map_err(|_|"无法启动本机更新传输。")?;
    let origin=format!("http://127.0.0.1:{}",listener.local_addr().map_err(|_|"无法启动本机更新传输。")?.port());
    let body=serde_json::to_vec(&serde_json::json!({"version":manifest.version,"notes":manifest.notes,"url":format!("{origin}/package"),"signature":manifest.signature})).map_err(|_|"更新清单无效。")?;
    let endpoint=format!("{origin}/manifest").parse().map_err(|_|"本机更新地址无效。")?;
    let task=tokio::spawn(async move {
        for _ in 0..4 {
            let Ok(Ok((mut socket,_)))=tokio::time::timeout(Duration::from_secs(30),listener.accept()).await else{break};
            let mut request=[0u8;8192];
            let Ok(Ok(n))=tokio::time::timeout(Duration::from_secs(3),socket.read(&mut request)).await else{continue};
            let request=String::from_utf8_lossy(&request[..n]);
            let data=if request.starts_with("GET /manifest HTTP/"){&body[..]}else if request.starts_with("GET /package HTTP/"){&package[..]}else{&[]};
            let header=format!("HTTP/1.1 {}\r\nContent-Length: {}\r\nContent-Type: application/octet-stream\r\nConnection: close\r\n\r\n",if data.is_empty(){"404 Not Found"}else{"200 OK"},data.len());
            let _=tokio::time::timeout(Duration::from_secs(30),async {socket.write_all(header.as_bytes()).await?;socket.write_all(data).await?;socket.shutdown().await}).await;
        }
    });
    Ok((endpoint,task))
}

#[tauri::command]
pub async fn install_app_update(app:AppHandle,version:String) -> Result<(),String> {
    if std::env::args().any(|a|a=="--native-smoke") {return Err("原生 smoke 不允许安装更新。".into());}
    apply_update(app,version,true).await
}
/// Isolated native acceptance downloads and verifies with the real plugin,
/// but never executes an installer or touches an installed application's files.
#[tauri::command]
pub async fn verify_app_update_smoke(app:AppHandle,version:String) -> Result<(),String> {
    if !std::env::args().any(|a|a=="--native-smoke") {return Err("此命令仅用于隔离原生验收。".into());}
    apply_update(app,version,false).await
}
async fn apply_update(app:AppHandle,version:String,install:bool) -> Result<(),String> {
    let state=app.state::<UpdateState>();
    let _operation=state.operation.try_lock().map_err(|_|"更新操作正在进行。")?;
    let status=state.status.lock().unwrap().clone();
    if status.phase!="available" || status.version.as_deref()!=Some(&version) {return Err("请先检查更新，再安装当前显示的版本。".into());}
    publish(&app,|s|{s.phase="downloading".into();s.downloaded=0;s.total=None;s.error=None;});
    let result:Result<(),String>=async {
        let mut bridge=None;
        let update=match validate_source(&status.settings.source)? {
            Source::Local(root)=>{
                let (manifest,package)=parse_local(&bounded_file(&root.join("latest.json"),MAX_MANIFEST).await?,&root)?;
                if manifest.version!=version {return Err("发布版本已变化，请重新检查更新。".into());}
                let bytes=bounded_file(&package,MAX_PACKAGE).await?;
                let (endpoint,task)=local_bridge(&manifest,bytes).await?; bridge=Some(task);
                app.updater_builder().endpoints(vec![endpoint]).map_err(|_|"本机更新传输不可用。")?.no_proxy().timeout(Duration::from_secs(30))
                    .build().map_err(|_|"无法初始化本机更新。")?.check().await.map_err(|_|"无法读取本机更新清单。")?
            },
            Source::Online(url)=>check_online(&app,url,status.settings.backup_automatic).await?.0,
        }.ok_or("没有可安装的新版本，请重新检查。")?;
        if update.version!=version {if let Some(task)=bridge {task.abort();}return Err("发布版本已变化，请重新检查更新。".into());}
        if bridge.is_none(){validate_download(&update.download_url)?;}
        let app_progress=app.clone();
        let bytes=update.download(move|length,total|{publish(&app_progress,|s|{s.downloaded=s.downloaded.saturating_add(length as u64);s.total=total;});},||{})
            .await.map_err(|_|"更新下载或签名校验失败，当前版本未改变。请重新检查后重试。".to_string());
        if let Some(task)=bridge {task.abort();}
        let bytes=bytes?;
        if !install {publish(&app,|s|{s.phase="available".into();});return Ok(());}
        publish(&app,|s|{s.phase="installing".into();});
        update.install(bytes).map_err(|_|"无法启动更新安装程序，当前版本仍可使用。".into())
    }.await;
    result.map_err(|error|fail(&app,&error))
}

pub fn watch(app:AppHandle) {
    if std::env::args().any(|a|a=="--native-smoke") {return;}
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(Duration::from_secs(20)).await;
        loop {
            let automatic=app.state::<UpdateState>().status.lock().unwrap().settings.automatic;
            if automatic {let _=check_app_update(app.clone()).await;}
            tokio::time::sleep(Duration::from_secs(30*60)).await;
        }
    });
}

pub fn open(app:&AppHandle) {
    crate::open_account_settings(app,Some("general"));
    let handle=app.clone();tauri::async_runtime::spawn(async move {let _=check_app_update(handle).await;});
}

#[cfg(test)] mod tests {
    use super::*;
    #[test] fn old_settings_keep_safe_defaults_and_notes_only_use_supported_languages(){
        let s:UpdateSettings=serde_json::from_str(r#"{"source":"C:/updates","automatic":false}"#).unwrap();
        assert!(!s.automatic);assert!(s.backup_automatic);assert_eq!(s.notes_language,"auto");
        let notes=notes_languages(&serde_json::json!({"notesByLanguage":{"en":"English","zh-CN":"中文","unknown":"discard"}}));assert_eq!(notes.len(),2);assert_eq!(notes["en"],"English");
    }
    #[test] fn backup_is_limited_to_the_same_public_github_release(){
        let source="https://github.com/wapearoudy/PulseWin/releases/latest/download/latest.json".parse().unwrap();
        assert_eq!(github_repository(&source),Some(("wapearoudy".into(),"PulseWin".into())));
        for url in ["https://example.com/a/b/releases/latest/download/latest.json","https://github.com/a/b/releases/download/v1/latest.json","https://github.com/a/b/releases/latest/download/other.json"]{assert!(github_repository(&url.parse().unwrap()).is_none())}
        let mut asset=serde_json::json!({"assets":[{"name":"latest.json","state":"uploaded","size":500,"url":"https://api.github.com/repos/wapearoudy/PulseWin/releases/assets/42"}]});
        assert!(github_asset(&asset,"wapearoudy","PulseWin").is_ok());
        for url in ["http://api.github.com/repos/wapearoudy/PulseWin/releases/assets/42","https://api.github.com/repos/other/PulseWin/releases/assets/42","https://api.github.com.evil.test/repos/wapearoudy/PulseWin/releases/assets/42","https://api.github.com/repos/wapearoudy/PulseWin/releases/assets/42?token=x"]{asset["assets"][0]["url"]=url.into();assert!(github_asset(&asset,"wapearoudy","PulseWin").is_err())}
    }
    #[test] fn versions_do_not_downgrade_or_compare_lexically() {
        assert!(newer("0.1.10","0.1.9").unwrap());assert!(!newer("0.1.3","0.1.4").unwrap());
        assert!(!newer("0.1.4","0.1.4").unwrap());assert!(!newer("0.1.4-beta.1","0.1.4").unwrap());assert!(newer("garbage","0.1.4").is_err());
    }
    #[test] fn channels_reject_insecure_remote_addresses_and_relative_paths() {
        for value in ["http://example.com/latest.json","file:///C:/updates","relative","https://user:pass@example.com/x","https://example.com/x#secret"] {assert!(validate_source(value).is_err(),"{value}");}
        assert!(validate_source("C:/updates").is_ok());assert!(validate_source("https://example.com/latest.json").is_ok());
    }
    #[test] fn local_manifest_cannot_escape_its_folder_or_omit_signature() {
        for file in ["../other.exe","sub/package.exe","C:\\package.exe","package.msi",""] {
            let data=serde_json::to_vec(&serde_json::json!({"version":"0.1.5","file":file,"signature":"sig"})).unwrap();assert!(parse_local(&data,Path::new("C:/updates")).is_err());
        }
        let data=br#"{"version":"0.1.5","file":"package.exe","signature":"sig"}"#;
        assert_eq!(parse_local(data,Path::new("C:/updates")).unwrap().1,PathBuf::from("C:/updates/package.exe"));
        assert!(parse_local(br#"{"version":"0.1.5","file":"package.exe","signature":""}"#,Path::new("C:/updates")).is_err());
    }
    #[tokio::test] async fn loopback_bridge_only_serves_the_selected_manifest_and_bytes() {
        let manifest=LocalManifest{version:"0.1.5".into(),notes:None,file:"package.exe".into(),signature:"sig".into(),notes_by_language:Default::default()};
        let (endpoint,task)=local_bridge(&manifest,b"signed fixture bytes".to_vec()).await.unwrap();
        let client=reqwest::Client::builder().no_proxy().build().unwrap();
        let value:serde_json::Value=client.get(endpoint.clone()).send().await.unwrap().json().await.unwrap();
        assert_eq!(value["version"],"0.1.5");
        assert_eq!(client.get(value["url"].as_str().unwrap()).send().await.unwrap().bytes().await.unwrap().as_ref(),b"signed fixture bytes");
        assert_eq!(client.get(endpoint.join("/private").unwrap()).send().await.unwrap().status(),404);task.abort();
    }
}
