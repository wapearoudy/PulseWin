//! Windows system-proxy discovery.
//!
//! reqwest only honours the `HTTP_PROXY` / `HTTPS_PROXY` environment variables.
//! Most Windows users behind a local proxy (Clash, v2ray, Surge, corporate
//! MITM) configure it in **Settings → Network → Proxy**, which lands in the
//! registry and in WinINET — not in the environment. Without this module the
//! app connects directly and times out on every provider.
//!
//! Environment variables still win when set, matching reqwest's own behaviour.

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
