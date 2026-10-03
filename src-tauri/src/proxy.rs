//! Windows system and manual proxy policy.
//!
//! reqwest only honours the `HTTP_PROXY` / `HTTPS_PROXY` environment variables.
//! Most Windows users behind a local proxy (Clash, v2ray, Surge, corporate
//! MITM) configure it in **Settings → Network → Proxy**, which lands in the
//! registry and in WinINET — not in the environment. Without this module the
//! app connects directly and times out on every provider.
//!
//! System mode preserves reqwest environment precedence; a valid manual
//! endpoint overrides both environment and registry settings.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all="camelCase",default)]
pub struct NetworkProxySettings {
    pub mode:String,
    pub kind:String,
    pub host:String,
    pub port:Option<u16>,
}
impl Default for NetworkProxySettings {
    fn default()->Self{Self{mode:"system".into(),kind:"http".into(),host:String::new(),port:None}}
}
impl NetworkProxySettings {
    pub fn validate(&mut self)->Result<(),String>{
        if !["system","manual"].contains(&self.mode.as_str()){return Err("请选择跟随系统或手动代理。".into())}
        if !["http","socks5"].contains(&self.kind.as_str()){return Err("代理类型只支持 HTTP 或 SOCKS5。".into())}
        self.host=self.host.trim().to_owned();
        // Selecting Manual before entering its first endpoint is permitted;
        // as upstream, an unset endpoint continues to follow system settings.
        if self.host.is_empty()&&self.port.is_none(){return Ok(())}
        if self.port.is_none()||self.port==Some(0){return Err("端口必须是 1–65535 的整数。".into())}
        let host=self.host.strip_prefix('[').and_then(|h|h.strip_suffix(']')).unwrap_or(&self.host);
        if host.is_empty()||host.chars().any(|c|c.is_whitespace()||c.is_control())||host.contains(['/', '\\','@','?','#','%']) {
            return Err("请填写代理主机名或 IP，不包含协议、路径或登录信息。".into())
        }
        if (self.host.starts_with('[')||self.host.ends_with(']'))&&host.parse::<std::net::Ipv6Addr>().is_err(){return Err("IPv6 地址无效。".into())}
        if host.contains(':')&&host.parse::<std::net::Ipv6Addr>().is_err(){return Err("IPv6 地址无效；端口请填在端口栏。".into())}
        let url=reqwest::Url::parse(&format!("http://{}:{}",if host.contains(':'){format!("[{host}]")}else{host.to_owned()},self.port.unwrap()))
            .map_err(|_|"代理主机地址无效。")?;
        if url.host_str().is_none()||!url.username().is_empty()||url.password().is_some(){return Err("代理主机地址无效。".into())}
        self.host=host.to_owned();Ok(())
    }
    pub fn normalize(&mut self){if self.validate().is_err(){*self=Self::default();}}
    pub fn endpoint(&self)->Option<String>{
        if self.mode!="manual"{return None}
        let mut valid=self.clone();valid.validate().ok()?;let port=valid.port?;
        let host=if valid.host.contains(':'){format!("[{}]",valid.host)}else{valid.host};
        Some(format!("{}://{host}:{port}",if self.kind=="socks5"{"socks5h"}else{"http"}))
    }
    /// Preserve the caller's allowlisted environment and leading CLI PATH.
    pub fn process_environment(&self,mut base:Vec<(std::ffi::OsString,std::ffi::OsString)>)->Vec<(std::ffi::OsString,std::ffi::OsString)>{
        let Some(url)=self.endpoint() else{return base};
        base.retain(|(key,_)|!is_proxy_key(&key.to_string_lossy()));
        let add=|base:&mut Vec<_>,key:&str,value:&str|base.push((std::ffi::OsString::from(key),std::ffi::OsString::from(value)));
        if self.kind=="http"{add(&mut base,"HTTP_PROXY",&url);add(&mut base,"HTTPS_PROXY",&url);}
        else{add(&mut base,"ALL_PROXY",&url.replace("socks5h://","socks5://"));}
        add(&mut base,"NO_PROXY","localhost,127.0.0.1,::1");base
    }
}
fn is_proxy_key(key:&str)->bool{["HTTP_PROXY","HTTPS_PROXY","ALL_PROXY","NO_PROXY"].iter().any(|p|key.eq_ignore_ascii_case(p))}
pub fn is_loopback(host:&str)->bool{
    let host=host.trim_matches(['[',']']).trim_end_matches('.');
    host.eq_ignore_ascii_case("localhost")||host.to_ascii_lowercase().ends_with(".localhost")||host.parse::<std::net::IpAddr>().is_ok_and(|ip|ip.is_loopback()||matches!(ip,std::net::IpAddr::V6(v) if v.to_ipv4_mapped().is_some_and(|v|v.is_loopback())))
}
/// Explicit manual proxy wins over inherited proxy and bypass lists. Loopback
/// is always direct; it must not leave the machine through a remote proxy.
pub fn configure(mut builder:reqwest::ClientBuilder,settings:&NetworkProxySettings)->Result<reqwest::ClientBuilder,String>{
    let mut checked=settings.clone();checked.validate()?;
    if let Some(endpoint)=checked.endpoint(){
        let url=reqwest::Url::parse(&endpoint).map_err(|_|"代理地址无效。")?;
        builder=builder.no_proxy().proxy(reqwest::Proxy::custom(move|target|{
            if target.host_str().is_some_and(is_loopback){None}else{Some(url.clone())}
        }));
    }else if let Some(url)=system_proxy(){
        builder=configure_system_proxy(builder,&url,system_no_proxy().as_deref());
    }
    Ok(builder)
}
// External WinINET configuration is not validated by our settings command.
// Preserve the old startup policy: malformed registry values must not prevent
// users from reaching Settings to configure a working manual endpoint.
fn configure_system_proxy(builder:reqwest::ClientBuilder,endpoint:&str,bypass:Option<&str>)->reqwest::ClientBuilder{
    match reqwest::Proxy::all(endpoint){
        Ok(proxy)=>builder.proxy(proxy.no_proxy(reqwest::NoProxy::from_string(bypass.unwrap_or_default()))),
        Err(_)=>builder,
    }
}
#[derive(Serialize)]#[serde(rename_all="camelCase")]
pub struct NetworkStatus {pub settings:NetworkProxySettings,pub source:String,pub endpoint:Option<String>,pub manual_ready:bool}
pub fn status(settings:NetworkProxySettings)->NetworkStatus{
    let endpoint=settings.endpoint();let manual_ready=endpoint.is_some();
    let (source,endpoint)=if manual_ready{("手动代理".into(),endpoint)}else{
        let env=["HTTPS_PROXY","https_proxy","HTTP_PROXY","http_proxy","ALL_PROXY","all_proxy"].iter()
            .find_map(|name|std::env::var(name).ok().filter(|v|!v.trim().is_empty()));
        match env {Some(url)=>("环境变量".into(),redacted_endpoint(&url)),None=>match system_proxy(){Some(url)=>("Windows 系统代理".into(),redacted_endpoint(&url)),None=>("系统未设置代理".into(),None)}}
    };
    NetworkStatus{settings,source,endpoint,manual_ready}
}
fn redacted_endpoint(value:&str)->Option<String>{
    let mut url=reqwest::Url::parse(&normalize(value)).ok()?;
    let _=url.set_username("");let _=url.set_password(None);url.set_query(None);url.set_fragment(None);url.set_path("");Some(url.to_string())
}

/// `HKCU\Software\Microsoft\Windows\CurrentVersion\Internet Settings`
#[cfg(windows)]
const INTERNET_SETTINGS: &str = r"Software\Microsoft\Windows\CurrentVersion\Internet Settings";

/// True when a proxy is already supplied through the environment, in which case
/// the registry must not override it.
pub fn env_proxy_present() -> bool {
    ["HTTPS_PROXY", "https_proxy", "HTTP_PROXY", "http_proxy", "ALL_PROXY", "all_proxy"]
        .iter()
        .any(|name| std::env::var(name).map(|v| !v.trim().is_empty()).unwrap_or(false))
}

/// Normalize a bare `host:port` into a URL, leaving explicit schemes alone.
fn normalize(value: &str) -> String {
    let value = value.trim();
    let lower = value.to_ascii_lowercase();
    if lower.starts_with("http://")
        || lower.starts_with("https://")
        || lower.starts_with("socks4://")
        || lower.starts_with("socks5://")
        || lower.starts_with("socks5h://")
    {
        value.to_string()
    } else {
        format!("http://{value}")
    }
}

/// Parse the `ProxyServer` registry value.
///
/// Windows writes either a single `host:port`:
/// ```text
/// 127.0.0.1:7897
/// ```
/// or a per-scheme list:
/// ```text
/// http=127.0.0.1:7897;https=127.0.0.1:7897
/// ```
/// HTTPS is preferred when both are present, since every provider call is HTTPS.
pub fn parse_proxy_server(raw: &str) -> Option<String> {
    let raw = raw.trim();
    if raw.is_empty() {
        return None;
    }

    if !raw.contains('=') {
        return Some(normalize(raw));
    }

    let mut http = None;
    let mut https = None;

    for part in raw.split(';') {
        let part = part.trim();
        if part.is_empty() {
            continue;
        }
        let Some((scheme, value)) = part.split_once('=') else {
            continue;
        };
        match scheme.trim().to_ascii_lowercase().as_str() {
            "https" => https = Some(normalize(value)),
            "http" => http = Some(normalize(value)),
            // ftp= and socks= entries are irrelevant to the provider calls.
            _ => {}
        }
    }

    https.or(http)
}

/// Convert the `ProxyOverride` value into a value reqwest's `NoProxy` accepts.
///
/// Windows uses `;` and the literal token `<local>`; reqwest uses `,`.
pub fn parse_proxy_override(raw: &str) -> Option<String> {
    let parts: Vec<&str> = raw
        .split(';')
        .map(str::trim)
        .filter(|s| !s.is_empty() && !s.eq_ignore_ascii_case("<local>"))
        .collect();

    if parts.is_empty() {
        None
    } else {
        Some(parts.join(","))
    }
}

/// The configured system proxy URL, or `None` when there is none.
///
/// Returns `None` on non-Windows targets, and whenever an environment proxy is
/// already set so reqwest's own handling takes precedence.
#[cfg(windows)]
pub fn system_proxy() -> Option<String> {
    if env_proxy_present() {
        return None;
    }

    let hkcu = winreg::RegKey::predef(winreg::enums::HKEY_CURRENT_USER);
    let key = hkcu.open_subkey(INTERNET_SETTINGS).ok()?;

    let enabled: u32 = key.get_value("ProxyEnable").unwrap_or(0);
    if enabled == 0 {
        return None;
    }

    let server: String = key.get_value("ProxyServer").ok()?;
    parse_proxy_server(&server)
}

/// The `ProxyOverride` no-proxy list, when the system proxy is in use.
#[cfg(windows)]
pub fn system_no_proxy() -> Option<String> {
    if env_proxy_present() {
        return None;
    }

    let hkcu = winreg::RegKey::predef(winreg::enums::HKEY_CURRENT_USER);
    let key = hkcu.open_subkey(INTERNET_SETTINGS).ok()?;

    let enabled: u32 = key.get_value("ProxyEnable").unwrap_or(0);
    if enabled == 0 {
        return None;
    }

    let raw: String = key.get_value("ProxyOverride").ok()?;
    parse_proxy_override(&raw)
}

#[cfg(not(windows))]
pub fn system_proxy() -> Option<String> {
    None
}

#[cfg(not(windows))]
pub fn system_no_proxy() -> Option<String> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_bare_host_port() {
        assert_eq!(
            parse_proxy_server("127.0.0.1:7897").as_deref(),
            Some("http://127.0.0.1:7897")
        );
    }

    #[test]
    fn keeps_explicit_scheme() {
        assert_eq!(
            parse_proxy_server("socks5://127.0.0.1:1080").as_deref(),
            Some("socks5://127.0.0.1:1080")
        );
    }

    #[test]
    fn prefers_https_from_scheme_list() {
        let raw = "http=127.0.0.1:8080;https=127.0.0.1:8443";
        assert_eq!(
            parse_proxy_server(raw).as_deref(),
            Some("http://127.0.0.1:8443")
        );
    }

    #[test]
    fn falls_back_to_http_when_no_https_entry() {
        let raw = "http=10.0.0.1:3128;ftp=10.0.0.1:3128";
        assert_eq!(
            parse_proxy_server(raw).as_deref(),
            Some("http://10.0.0.1:3128")
        );
    }

    #[test]
    fn trims_whitespace_around_entries() {
        let raw = " http = 127.0.0.1:7897 ; https = 127.0.0.1:7898 ";
        assert_eq!(
            parse_proxy_server(raw).as_deref(),
            Some("http://127.0.0.1:7898")
        );
    }

    #[test]
    fn empty_proxy_is_none() {
        assert!(parse_proxy_server("").is_none());
        assert!(parse_proxy_server("   ").is_none());
    }

    #[test]
    fn scheme_list_without_known_schemes_is_none() {
        assert!(parse_proxy_server("ftp=10.0.0.1:21").is_none());
    }

    #[test]
    fn converts_override_to_comma_list_and_drops_local_token() {
        let raw = "localhost;127.*;10.*;<local>";
        assert_eq!(
            parse_proxy_override(raw).as_deref(),
            Some("localhost,127.*,10.*")
        );
    }

    #[test]
    fn override_with_only_local_token_is_none() {
        assert!(parse_proxy_override("<local>").is_none());
        assert!(parse_proxy_override("").is_none());
    }
}

#[cfg(test)]
#[path="proxy_tests.rs"]
mod network_tests;
