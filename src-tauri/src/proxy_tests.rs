use super::*;
use tokio::{io::{AsyncReadExt, AsyncWriteExt}, net::{TcpListener, TcpStream}};
use std::{ffi::OsString, time::Duration};

fn manual(kind: &str, port: u16) -> NetworkProxySettings {
    NetworkProxySettings { mode: "manual".into(), kind: kind.into(), host: "127.0.0.1".into(), port: Some(port) }
}
async fn request_headers(stream: &mut TcpStream) -> String {
    let mut bytes = Vec::new();
    loop {
        let mut byte = [0]; stream.read_exact(&mut byte).await.unwrap(); bytes.push(byte[0]);
        if bytes.ends_with(b"\r\n\r\n") { break }
        assert!(bytes.len() < 8192);
    }
    String::from_utf8(bytes).unwrap()
}
async fn answer(stream: &mut TcpStream) {
    stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 5\r\nConnection: close\r\n\r\nusage").await.unwrap();
}

#[test]
fn invalid_endpoints_are_rejected_and_ipv6_is_normalized() {
    for (host, port) in [("", Some(80)), ("example.com", None), ("example.com", Some(0)), ("https://example.com", Some(80)), ("a:80", Some(80)), ("a/b", Some(80)), ("user@host", Some(80)), ("a b", Some(80)), ("%61", Some(80)), ("[broken]", Some(80))] {
        let mut value = manual("http", 80); value.host = host.into(); value.port = port;
        assert!(value.validate().is_err(), "{host} {port:?}"); assert!(value.endpoint().is_none());
    }
    let mut value = manual("socks5", 1080); value.host = " [::1] ".into(); value.validate().unwrap();
    assert_eq!(value.host, "::1"); assert_eq!(value.endpoint().as_deref(), Some("socks5h://[::1]:1080"));
    value.mode = "direct".into(); assert!(value.validate().is_err());
    value.mode = "manual".into(); value.kind = "ftp".into(); assert!(value.validate().is_err());
    let mut initial = NetworkProxySettings { mode: "manual".into(), ..Default::default() };
    assert!(initial.validate().is_ok()); assert!(initial.endpoint().is_none());
}

#[test]
fn helpers_override_only_proxy_keys_and_preserve_cli_path() {
    let base: Vec<(OsString, OsString)> = [("PATH", "cli-first;system"), ("USERPROFILE", "profile"), ("http_proxy", "old-http"), ("HTTPS_PROXY", "old-https"), ("all_proxy", "old-socks"), ("No_Proxy", "*")].into_iter().map(|(k,v)| (k.into(),v.into())).collect();
    for kind in ["http", "socks5"] {
        let env: std::collections::HashMap<_,_> = manual(kind,1080).process_environment(base.clone()).into_iter().map(|(k,v)| (k.to_string_lossy().to_string(),v.to_string_lossy().to_string())).collect();
        assert_eq!(env["PATH"], "cli-first;system"); assert_eq!(env["USERPROFILE"], "profile");
        assert_eq!(env["NO_PROXY"], "localhost,127.0.0.1,::1");
        assert!(!env.contains_key("http_proxy")); assert!(!env.contains_key("No_Proxy"));
        if kind == "http" { assert_eq!(env["HTTPS_PROXY"], "http://127.0.0.1:1080"); assert!(!env.contains_key("ALL_PROXY")); }
        else { assert_eq!(env["ALL_PROXY"], "socks5://127.0.0.1:1080"); assert!(!env.contains_key("HTTPS_PROXY")); }
    }
    assert_eq!(NetworkProxySettings::default().process_environment(base.clone()), base);
}

#[test]
fn loopback_aliases_are_local_and_proxy_credentials_never_reach_status() {
    for host in ["localhost", "LOCALHOST.", "editor.localhost", "127.4.3.2", "::1", "[::1]", "::ffff:127.0.0.1"] { assert!(is_loopback(host), "{host}"); }
    for host in ["localhost.evil.com", "example.com", "10.0.0.1", "192.168.0.1", "::2"] { assert!(!is_loopback(host)); }
    for endpoint in ["http://alice:secret@example.com:80/path?key=secret#secret", "socks5h://alice:secret@example.com:1080"] {
        let redacted = redacted_endpoint(endpoint).unwrap(); assert!(!redacted.contains("alice")); assert!(!redacted.contains("secret")); assert!(redacted.contains("example.com"));
    }
}

#[tokio::test]
async fn http_proxy_receives_absolute_target_without_local_dns() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let config = manual("http",listener.local_addr().unwrap().port());
    let server = tokio::spawn(async move { let (mut stream,_) = listener.accept().await.unwrap(); let headers = request_headers(&mut stream).await; answer(&mut stream).await; headers });
    let client = configure(reqwest::Client::builder().timeout(Duration::from_secs(3)),&config).unwrap().build().unwrap();
    assert_eq!(client.get("http://pulsewin-test.invalid/usage").send().await.unwrap().text().await.unwrap(), "usage");
    assert!(server.await.unwrap().starts_with("GET http://pulsewin-test.invalid/usage HTTP/1.1"));
}

#[tokio::test]
async fn https_uses_connect_to_the_configured_proxy() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let config = manual("http",listener.local_addr().unwrap().port());
    let server = tokio::spawn(async move { let (mut stream,_) = listener.accept().await.unwrap(); let headers = request_headers(&mut stream).await; stream.write_all(b"HTTP/1.1 502 Test endpoint\r\nContent-Length: 0\r\n\r\n").await.unwrap(); headers });
    let client = configure(reqwest::Client::builder().timeout(Duration::from_secs(3)),&config).unwrap().build().unwrap();
    assert!(client.get("https://pulsewin-test.invalid/usage").send().await.is_err());
    assert!(server.await.unwrap().starts_with("CONNECT pulsewin-test.invalid:443 HTTP/1.1"));
}

#[tokio::test]
async fn socks5_sends_remote_domain_and_requires_no_authentication() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let config = manual("socks5",listener.local_addr().unwrap().port());
    let server = tokio::spawn(async move {
        let (mut stream,_) = listener.accept().await.unwrap(); let mut greeting=[0;2]; stream.read_exact(&mut greeting).await.unwrap(); assert_eq!(greeting[0],5);
        let mut methods=vec![0;greeting[1] as usize]; stream.read_exact(&mut methods).await.unwrap(); assert!(methods.contains(&0)); stream.write_all(&[5,0]).await.unwrap();
        let mut header=[0;5]; stream.read_exact(&mut header).await.unwrap(); assert_eq!(&header[..4], &[5,1,0,3]);
        let mut domain=vec![0;header[4] as usize]; stream.read_exact(&mut domain).await.unwrap(); let mut port=[0;2];stream.read_exact(&mut port).await.unwrap();assert_eq!(u16::from_be_bytes(port),80);
        stream.write_all(&[5,0,0,1,127,0,0,1,0,80]).await.unwrap(); let headers=request_headers(&mut stream).await; answer(&mut stream).await;
        (String::from_utf8(domain).unwrap(), headers)
    });
    let client = configure(reqwest::Client::builder().timeout(Duration::from_secs(3)),&config).unwrap().build().unwrap();
    assert_eq!(client.get("http://pulsewin-test.invalid/usage").send().await.unwrap().text().await.unwrap(),"usage");
    let (domain, headers)=server.await.unwrap(); assert_eq!(domain,"pulsewin-test.invalid");assert!(headers.starts_with("GET /usage HTTP/1.1"));
}

#[tokio::test]
async fn loopback_targets_bypass_http_and_socks5_proxies() {
    for kind in ["http","socks5"] {
        let proxy=TcpListener::bind("127.0.0.1:0").await.unwrap(); let direct=TcpListener::bind("127.0.0.1:0").await.unwrap();
        let config=manual(kind,proxy.local_addr().unwrap().port()); let target=format!("http://{}/usage",direct.local_addr().unwrap());
        let server=tokio::spawn(async move { let (mut stream,_)=direct.accept().await.unwrap();request_headers(&mut stream).await;answer(&mut stream).await; });
        let client=configure(reqwest::Client::builder().timeout(Duration::from_secs(3)),&config).unwrap().build().unwrap();
        assert_eq!(client.get(target).send().await.unwrap().text().await.unwrap(),"usage");server.await.unwrap();
        assert!(tokio::time::timeout(Duration::from_millis(30),proxy.accept()).await.is_err());
    }
}

#[tokio::test]
async fn gateway_client_still_refuses_to_forward_keys_through_redirects() {
    let listener=TcpListener::bind("127.0.0.1:0").await.unwrap(); let config=manual("http",listener.local_addr().unwrap().port());
    let server=tokio::spawn(async move { let (mut stream,_)=listener.accept().await.unwrap();let headers=request_headers(&mut stream).await;stream.write_all(b"HTTP/1.1 302 Found\r\nLocation: http://other.invalid/secret\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").await.unwrap(); assert!(tokio::time::timeout(Duration::from_millis(30),listener.accept()).await.is_err()); headers });
    let ctx=crate::providers::Ctx::with_network(&config).unwrap();let response=ctx.gateway_client.get("http://gateway.invalid/usage").header("Authorization","Bearer synthetic-key").send().await.unwrap();
    assert_eq!(response.status(),302);assert!(server.await.unwrap().contains("Bearer synthetic-key"));
}

#[test]
fn malformed_external_system_proxy_does_not_block_client_initialization() {
    for endpoint in ["http://", "http://["] {
        assert!(reqwest::Proxy::all(endpoint).is_err(), "{endpoint}");
        assert!(configure_system_proxy(reqwest::Client::builder(), endpoint, None).build().is_ok());
    }
}
#[tokio::test]
async fn a_valid_system_proxy_still_applies_its_bypass_list() {
    let proxy=TcpListener::bind("127.0.0.1:0").await.unwrap();
    let direct=TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint=format!("http://{}",proxy.local_addr().unwrap());
    let target=format!("http://{}/usage",direct.local_addr().unwrap());
    let server=tokio::spawn(async move {let (mut stream,_)=direct.accept().await.unwrap();request_headers(&mut stream).await;answer(&mut stream).await;});
    let client=configure_system_proxy(reqwest::Client::builder().no_proxy().timeout(Duration::from_secs(3)),&endpoint,Some("127.0.0.1")).build().unwrap();
    assert_eq!(client.get(target).send().await.unwrap().text().await.unwrap(),"usage");server.await.unwrap();
    assert!(tokio::time::timeout(Duration::from_millis(30),proxy.accept()).await.is_err());
}
