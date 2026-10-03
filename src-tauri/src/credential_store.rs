//! Manual credentials are protected for the current Windows user with DPAPI.
//! The complete provider document is encrypted so cookie and vendor-specific
//! fields follow the same storage contract as an API key.

use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::{atomic::{AtomicU64, Ordering}, Mutex};

use serde_json::{Map, Value};

const FORMAT: &str = "pulsewin-dpapi";
const VERSION: u64 = 1;
const MAX_DOCUMENT_BYTES: u64 = 1024 * 1024;
static STORE_LOCK: Mutex<()> = Mutex::new(());
static TEMP_SEQUENCE: AtomicU64 = AtomicU64::new(0);

pub fn valid_provider_id(id: &str) -> bool {
    if id.is_empty() || id.len() > 128 || !id.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'-' || c == b'_') {
        return false;
    }
    let upper = id.to_ascii_uppercase();
    !matches!(upper.as_str(), "CON" | "PRN" | "AUX" | "NUL" | "CLOCK$")
        && !(upper.len() == 4 && (upper.starts_with("COM") || upper.starts_with("LPT")) && matches!(upper.as_bytes()[3], b'1'..=b'9'))
        && !matches!(upper.as_str(), "PANEL" | "PREFERENCES" | "ACCOUNTS" | "SETTINGS" | "DEEPSEEK-BASELINE")
}

/// Recognize PulseWin-owned credential files before the generic provider JSON
/// reader runs. Third-party CLI files and non-secret panel/baseline data are
/// intentionally outside this format.
pub fn provider_at_path(path: &Path) -> Option<&str> {
    if !path.parent()?.file_name()?.to_string_lossy().eq_ignore_ascii_case("PulseWin")
        || !path.extension()?.to_string_lossy().eq_ignore_ascii_case("json") {
        return None;
    }
    let id = path.file_stem()?.to_str()?;
    valid_provider_id(id).then_some(id)
}

fn invalid(message: &str) -> io::Error { io::Error::new(io::ErrorKind::InvalidData, message) }

fn ensure_id(provider: &str) -> io::Result<()> {
    if valid_provider_id(provider) { Ok(()) }
    else { Err(io::Error::new(io::ErrorKind::InvalidInput, "Invalid provider id")) }
}

fn parse_document(bytes: &[u8]) -> io::Result<Value> {
    let value: Value = serde_json::from_slice(bytes).map_err(|_| invalid("Credential file is not valid JSON"))?;
    if !value.is_object() { return Err(invalid("Credential document must be a JSON object")); }
    Ok(value)
}

fn read_bytes(path: &Path) -> io::Result<Option<Vec<u8>>> {
    let metadata = match fs::metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
    };
    if !metadata.is_file() || metadata.len() > MAX_DOCUMENT_BYTES {
        return Err(invalid("Credential file is not a regular file or exceeds the size limit"));
    }
    let bytes = fs::read(path)?;
    if bytes.len() as u64 > MAX_DOCUMENT_BYTES { return Err(invalid("Credential file exceeds the size limit")); }
    Ok(Some(bytes))
}

fn decode(provider: &str, value: Value) -> io::Result<(Value, bool)> {
    if value.get("format").and_then(Value::as_str) != Some(FORMAT) {
        // A damaged encrypted envelope must never be accepted as legacy JSON.
        if value.get("ciphertext").is_some()
            || value.get("format").and_then(Value::as_str).is_some_and(|format| format.starts_with("pulsewin-")) {
            return Err(invalid("Unsupported or damaged credential storage format"));
        }
        return Ok((value, true));
    }
    if value.get("version").and_then(Value::as_u64) != Some(VERSION) {
        return Err(invalid("Unsupported credential storage version"));
    }
    let encoded = value.get("ciphertext").and_then(Value::as_str)
        .ok_or_else(|| invalid("Encrypted credential file has no ciphertext"))?;
    let encrypted = hex_decode(encoded)?;
    let mut plaintext = platform::unprotect(provider, &encrypted)?;
    let parsed = parse_document(&plaintext);
    // DPAPI's own allocation is wiped below; also wipe our temporary copy.
    for byte in &mut plaintext { unsafe { std::ptr::write_volatile(byte, 0); } }
    Ok((parsed?, false))
}

fn encode(provider: &str, document: &Value) -> io::Result<Vec<u8>> {
    if !document.is_object() { return Err(invalid("Credential document must be a JSON object")); }
    let mut plaintext = serde_json::to_vec(document).map_err(|_| invalid("Could not encode credential document"))?;
    let protected = platform::protect(provider, &plaintext);
    for byte in &mut plaintext { unsafe { std::ptr::write_volatile(byte, 0); } }
    let ciphertext = protected?;
    let envelope = serde_json::to_vec_pretty(&serde_json::json!({
        "format": FORMAT, "version": VERSION, "ciphertext": hex_encode(&ciphertext)
    })).map_err(|_| invalid("Could not encode encrypted credential file"))?;
    if envelope.len() as u64 > MAX_DOCUMENT_BYTES { return Err(invalid("Encrypted credential file exceeds the size limit")); }
    Ok(envelope)
}

/// Reads and atomically migrates legacy JSON. A migration failure is an error,
/// leaves the original bytes intact, and never returns an unprotected fallback.
pub fn read_at(path: &Path, provider: &str) -> io::Result<Option<Value>> {
    ensure_id(provider)?;
    let _guard = STORE_LOCK.lock().map_err(|_| io::Error::new(io::ErrorKind::Other, "Credential store is unavailable"))?;
    let Some(bytes) = read_bytes(path)? else { return Ok(None); };
    let (document, legacy) = decode(provider, parse_document(&bytes)?)?;
    if legacy {
        let encrypted = encode(provider, &document)?;
        atomic_write(path, &encrypted, Some(&bytes))?;
    }
    Ok(Some(document))
}

/// Updates canonical fields without discarding cookie, site, organization or
/// other provider-specific fields. Saving first decodes the old document; a
/// corrupt/foreign-user file is never silently replaced by an empty one.
pub fn update_at(path: &Path, provider: &str, api_key: Option<&str>, base_url: Option<&str>) -> io::Result<()> {
    ensure_id(provider)?;
    let _guard = STORE_LOCK.lock().map_err(|_| io::Error::new(io::ErrorKind::Other, "Credential store is unavailable"))?;
    let previous = read_bytes(path)?;
    let mut document = match previous.as_deref() {
        Some(bytes) => decode(provider, parse_document(bytes)?)?.0,
        None => Value::Object(Map::new()),
    };
    let object = document.as_object_mut().ok_or_else(|| invalid("Credential document must be a JSON object"))?;
    // Canonical fields supersede spelling aliases, including when cleared.
    // Cookie/userId/site and other vendor-specific data remain untouched.
    for alias in ["api_key", "key", "token"] { object.remove(alias); }
    for alias in ["base_url", "serverAddress", "address"] { object.remove(alias); }
    set_field(object, "apiKey", api_key);
    set_field(object, "baseUrl", base_url);
    let encrypted = encode(provider, &document)?;
    atomic_write(path, &encrypted, previous.as_deref())
}

fn set_field(object: &mut Map<String, Value>, field: &str, value: Option<&str>) {
    if let Some(value) = value.map(str::trim).filter(|value| !value.is_empty()) {
        object.insert(field.to_string(), Value::String(value.to_string()));
    } else { object.remove(field); }
}

pub(crate) fn atomic_write(path: &Path, encrypted: &[u8], expected: Option<&[u8]>) -> io::Result<()> {
    let parent = path.parent().ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "Credential path has no directory"))?;
    fs::create_dir_all(parent)?;
    let unique = TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let temporary: PathBuf = parent.join(format!(".credential-{}-{unique}.tmp", std::process::id()));
    let result = (|| {
        let mut file = OpenOptions::new().write(true).create_new(true).open(&temporary)?;
        file.write_all(encrypted)?;
        file.sync_all()?;
        drop(file);
        if read_bytes(path)?.as_deref() != expected {
            return Err(io::Error::new(io::ErrorKind::Other, "Credential file changed while it was being saved; retry"));
        }
        platform::replace(&temporary, path)
    })();
    if result.is_err() { let _ = fs::remove_file(&temporary); }
    result
}

fn hex_encode(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes { out.push(DIGITS[(byte >> 4) as usize] as char); out.push(DIGITS[(byte & 15) as usize] as char); }
    out
}

fn hex_decode(text: &str) -> io::Result<Vec<u8>> {
    if text.is_empty() || text.len() % 2 != 0 || text.len() as u64 > MAX_DOCUMENT_BYTES {
        return Err(invalid("Encrypted credential ciphertext is invalid"));
    }
    text.as_bytes().chunks_exact(2).map(|pair| {
        let a = (pair[0] as char).to_digit(16).ok_or_else(|| invalid("Encrypted credential ciphertext is invalid"))?;
        let b = (pair[1] as char).to_digit(16).ok_or_else(|| invalid("Encrypted credential ciphertext is invalid"))?;
        Ok(((a << 4) | b) as u8)
    }).collect()
}

#[cfg(windows)]
mod platform {
    use super::*;
    use std::ffi::c_void;
    use std::os::windows::ffi::OsStrExt;

    #[repr(C)]
    struct DataBlob { size: u32, data: *mut u8 }

    #[link(name = "crypt32")]
    extern "system" {
        fn CryptProtectData(input: *const DataBlob, description: *const u16, entropy: *const DataBlob,
            reserved: *mut c_void, prompt: *mut c_void, flags: u32, output: *mut DataBlob) -> i32;
        fn CryptUnprotectData(input: *const DataBlob, description: *mut *mut u16, entropy: *const DataBlob,
            reserved: *mut c_void, prompt: *mut c_void, flags: u32, output: *mut DataBlob) -> i32;
    }
    #[link(name = "kernel32")]
    extern "system" {
        fn LocalFree(memory: *mut c_void) -> *mut c_void;
        fn MoveFileExW(existing: *const u16, new: *const u16, flags: u32) -> i32;
    }

    fn crypt(provider: &str, bytes: &[u8], decrypt: bool) -> io::Result<Vec<u8>> {
        let size = u32::try_from(bytes.len()).map_err(|_| invalid("Credential data is too large"))?;
        let entropy_bytes = format!("PulseWin/manual-credential/v1/{provider}").into_bytes();
        let input = DataBlob { size, data: bytes.as_ptr() as *mut u8 };
        let entropy = DataBlob { size: entropy_bytes.len() as u32, data: entropy_bytes.as_ptr() as *mut u8 };
        let mut output = DataBlob { size: 0, data: std::ptr::null_mut() };
        // CRYPTPROTECT_UI_FORBIDDEN, without CRYPTPROTECT_LOCAL_MACHINE:
        // bind the ciphertext to this Windows user and refuse prompts.
        let success = unsafe {
            if decrypt { CryptUnprotectData(&input, std::ptr::null_mut(), &entropy, std::ptr::null_mut(), std::ptr::null_mut(), 1, &mut output) }
            else { CryptProtectData(&input, std::ptr::null(), &entropy, std::ptr::null_mut(), std::ptr::null_mut(), 1, &mut output) }
        };
        if success == 0 {
            let cause = io::Error::last_os_error();
            return Err(io::Error::new(io::ErrorKind::PermissionDenied,
                format!("Windows credential {} failed ({cause}); use the Windows account that saved it", if decrypt { "decryption" } else { "encryption" })));
        }
        if output.data.is_null() { return Err(invalid("Windows returned an empty credential buffer")); }
        let copied = unsafe { std::slice::from_raw_parts(output.data, output.size as usize).to_vec() };
        unsafe {
            if decrypt { for index in 0..output.size as usize { std::ptr::write_volatile(output.data.add(index), 0); } }
            LocalFree(output.data as *mut c_void);
        }
        Ok(copied)
    }
    pub fn protect(provider: &str, bytes: &[u8]) -> io::Result<Vec<u8>> { crypt(provider, bytes, false) }
    pub fn unprotect(provider: &str, bytes: &[u8]) -> io::Result<Vec<u8>> { crypt(provider, bytes, true) }
    pub fn replace(temporary: &Path, target: &Path) -> io::Result<()> {
        let source: Vec<u16> = temporary.as_os_str().encode_wide().chain(Some(0)).collect();
        let target: Vec<u16> = target.as_os_str().encode_wide().chain(Some(0)).collect();
        // Same-directory replacement; no copy/delete or deferred reboot path.
        if unsafe { MoveFileExW(source.as_ptr(), target.as_ptr(), 1 | 8) } == 0 {
            Err(io::Error::last_os_error())
        } else { Ok(()) }
    }
}

#[cfg(not(windows))]
mod platform {
    use super::*;
    fn unsupported() -> io::Error { io::Error::new(io::ErrorKind::Unsupported, "Manual credential storage requires Windows DPAPI") }
    pub fn protect(_: &str, _: &[u8]) -> io::Result<Vec<u8>> { Err(unsupported()) }
    pub fn unprotect(_: &str, _: &[u8]) -> io::Result<Vec<u8>> { Err(unsupported()) }
    pub fn replace(_: &Path, _: &Path) -> io::Result<()> { Err(unsupported()) }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(name: &str) -> PathBuf {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).parent().unwrap().join("test-results").join("credential-store-tests")
            .join(format!("{}-{}", std::process::id(), TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed))).join("PulseWin");
        fs::create_dir_all(&root).unwrap();
        root.join(format!("{name}.json"))
    }

    #[test]
    fn invalid_ids_cannot_be_paths_or_windows_devices() {
        for id in ["", "../test", "a\\b", "a/b", "a b", "a:b", "test.json", "CON", "prn", "COM1", "LPT9", "panel", "PREFERENCES"] {
            assert!(!valid_provider_id(id));
        }
        for id in ["cursor", "hugging-face", "llm_proxy"] { assert!(valid_provider_id(id)); }
        let path = fixture("invalid");
        assert_eq!(read_at(&path, "../outside").unwrap_err().kind(), io::ErrorKind::InvalidInput);
        assert!(!path.exists());
    }

    #[test]
    fn missing_and_damaged_documents_never_create_or_replace_credentials() {
        let path = fixture("synthetic");
        assert!(read_at(&path, "synthetic").unwrap().is_none());
        assert!(!path.exists());
        for bytes in [b"not-json".as_slice(), br#"{"format":"pulsewin-dpapi","version":99,"ciphertext":"00"}"#.as_slice(), br#"{"ciphertext":"00"}"#.as_slice()] {
            fs::write(&path, bytes).unwrap();
            assert!(read_at(&path, "synthetic").is_err());
            assert_eq!(fs::read(&path).unwrap(), bytes.to_vec());
        }
    }

    #[cfg(windows)]
    #[test]
    fn dpapi_round_trip_never_writes_plaintext() {
        let path = fixture("synthetic");
        update_at(&path, "synthetic", Some("  synthetic-test-secret  "), Some(" https://example.invalid ")).unwrap();
        let stored = fs::read_to_string(&path).unwrap();
        assert!(!stored.contains("synthetic-test-secret"));
        assert!(!stored.contains("example.invalid"));
        let decoded = read_at(&path, "synthetic").unwrap().unwrap();
        assert_eq!(decoded["apiKey"], "synthetic-test-secret");
        assert_eq!(decoded["baseUrl"], "https://example.invalid");
    }

    #[cfg(windows)]
    #[test]
    fn tampering_and_provider_swaps_are_rejected_without_rewriting() {
        let path = fixture("synthetic");
        update_at(&path, "synthetic", Some("synthetic-secret"), None).unwrap();
        assert!(read_at(&path, "other-provider").is_err());
        let mut envelope: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        let ciphertext = envelope["ciphertext"].as_str().unwrap();
        let mut bytes = hex_decode(ciphertext).unwrap();
        let index = bytes.len() / 2;
        bytes[index] ^= 1;
        envelope["ciphertext"] = Value::String(hex_encode(&bytes));
        let tampered = serde_json::to_vec(&envelope).unwrap();
        fs::write(&path, &tampered).unwrap();
        assert!(read_at(&path, "synthetic").is_err());
        assert!(update_at(&path, "synthetic", Some("replacement"), None).is_err());
        assert_eq!(fs::read(&path).unwrap(), tampered);
    }

    #[cfg(windows)]
    #[test]
    fn legacy_migration_preserves_vendor_fields_and_is_idempotent() {
        let path = fixture("cursor");
        let legacy = serde_json::json!({"cookie":"synthetic-cookie", "userId":"synthetic-user", "site":"global", "vendor":{"organization":"test"}, "apiKey":"old-test"});
        fs::write(&path, serde_json::to_vec(&legacy).unwrap()).unwrap();
        assert_eq!(read_at(&path, "cursor").unwrap(), Some(legacy.clone()));
        let migrated = fs::read(&path).unwrap();
        assert!(!String::from_utf8_lossy(&migrated).contains("synthetic-cookie"));
        assert_eq!(read_at(&path, "cursor").unwrap(), Some(legacy));
        assert_eq!(fs::read(&path).unwrap(), migrated);
        update_at(&path, "cursor", Some("new-test"), None).unwrap();
        let updated = read_at(&path, "cursor").unwrap().unwrap();
        assert_eq!(updated["cookie"], "synthetic-cookie");
        assert_eq!(updated["vendor"]["organization"], "test");
        assert_eq!(updated["apiKey"], "new-test");
    }

    #[cfg(windows)]
    #[test]
    fn clearing_canonical_fields_cannot_resurrect_legacy_aliases() {
        let path = fixture("synthetic");
        fs::write(&path, br#"{"api_key":"old-test","serverAddress":"https://old.example.invalid","site":"cn"}"#).unwrap();
        update_at(&path, "synthetic", Some("   "), None).unwrap();
        let value = read_at(&path, "synthetic").unwrap().unwrap();
        assert_eq!(value, serde_json::json!({"site":"cn"}));
    }

    #[cfg(windows)]
    #[test]
    fn migration_write_failure_preserves_old_bytes() {
        let path = fixture("synthetic");
        let legacy = br#"{"apiKey":"synthetic-legacy"}"#;
        fs::write(&path, legacy).unwrap();
        let mut permissions = fs::metadata(&path).unwrap().permissions();
        permissions.set_readonly(true);
        fs::set_permissions(&path, permissions).unwrap();
        assert!(read_at(&path, "synthetic").is_err());
        assert_eq!(fs::read(&path).unwrap(), legacy.to_vec());
        let mut permissions = fs::metadata(&path).unwrap().permissions();
        permissions.set_readonly(false);
        fs::set_permissions(&path, permissions).unwrap();
    }

    #[cfg(windows)]
    #[test]
    fn generic_adapter_json_reader_decrypts_only_pulsewin_credentials() {
        let path = fixture("synthetic");
        update_at(&path, "synthetic", Some("adapter-test-secret"), None).unwrap();
        let json = crate::credentials::read_json(&path).unwrap();
        assert_eq!(json["apiKey"], "adapter-test-secret");
        let baseline = path.with_file_name("deepseek-baseline.json");
        fs::write(&baseline, br#"{"balance":10}"#).unwrap();
        assert_eq!(crate::credentials::read_json(&baseline).unwrap()["balance"], 10);
        assert_eq!(fs::read_to_string(&baseline).unwrap(), r#"{"balance":10}"#);
    }

    #[cfg(not(windows))]
    #[test]
    fn unsupported_platform_never_saves_or_migrates_plaintext() {
        let path = fixture("synthetic");
        assert_eq!(update_at(&path, "synthetic", Some("test"), None).unwrap_err().kind(), io::ErrorKind::Unsupported);
        assert!(!path.exists());
        let legacy = br#"{"apiKey":"synthetic-legacy"}"#;
        fs::write(&path, legacy).unwrap();
        assert_eq!(read_at(&path, "synthetic").unwrap_err().kind(), io::ErrorKind::Unsupported);
        assert_eq!(fs::read(&path).unwrap(), legacy.to_vec());
    }
}
