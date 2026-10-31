//! DeepSeek console sign-ins kept in Chromium's local storage (Pulse 1.12 renewal path).
//! Pulse stores the sign-in encrypted; this importer only copies a token the user already
//! signed in with, then the caller verifies it against the console before saving.
use std::{collections::BTreeSet, fs, path::{Path, PathBuf}};
const MAX_FILE: u64 = 32 * 1024 * 1024;
const MAX_TOKEN: usize = 4096;
fn roots() -> Vec<(String, PathBuf)> {
    let Some(base) = std::env::var_os("LOCALAPPDATA").map(PathBuf::from) else { return vec![] };
    [
        ("Chrome", base.join("Google/Chrome/User Data")),
        ("Edge", base.join("Microsoft/Edge/User Data")),
        ("Brave", base.join("BraveSoftware/Brave-Browser/User Data")),
        ("Chromium", base.join("Chromium/User Data")),
    ]
    .into_iter()
    .filter(|(_, path)| path.is_dir())
    .map(|(name, path)| (name.to_string(), path))
    .collect()
}
fn profiles(root: &Path) -> Vec<PathBuf> {
    let mut found = vec![];
    let Ok(entries) = fs::read_dir(root) else { return found };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if name != "Default" && !name.starts_with("Profile ") {
            continue;
        }
        let path = entry.path().join("Local Storage/leveldb");
        if path.is_dir() {
            found.push(entry.path());
        }
    }
    found
}
fn leveldb_files(profile: &Path) -> Vec<PathBuf> {
    let directory = profile.join("Local Storage/leveldb");
    let Ok(entries) = fs::read_dir(directory) else { return vec![] };
    let mut files: Vec<_> = entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| {
            path.extension().and_then(|e| e.to_str()).is_some_and(|e| e == "ldb" || e == "log")
                && path.metadata().is_ok_and(|m| m.len() <= MAX_FILE)
        })
        .collect();
    files.sort_by_key(|path| std::cmp::Reverse(path.metadata().and_then(|m| m.modified()).ok()));
    files
}
fn token_near(bytes: &[u8]) -> Option<String> {
    // LevelDB keeps keys and values as UTF-16LE in some versions and UTF-8 in others;
    // accept either encoding and only return candidates that look like a JWT.
    let ascii = String::from_utf8_lossy(bytes);
    let utf16: String = bytes
        .chunks_exact(2)
        .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
        .map(|unit| char::from_u32(unit as u32).unwrap_or('\0'))
        .collect();
    let mut seen = BTreeSet::new();
    for text in [ascii.as_ref(), utf16.as_str()] {
        if !text.contains("platform.deepseek.com") && !text.contains("userToken") {
            continue;
        }
        let mut start = 0;
        while let Some(index) = text[start..].find("eyJ") {
            let index = start + index;
            let candidate: String = text[index..]
                .chars()
                .take_while(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
                .collect();
            if candidate.len() > 40 && candidate.matches('.').count() >= 2 && candidate.len() <= MAX_TOKEN {
                seen.insert(candidate);
            }
            start = index + 3;
        }
    }
    seen.into_iter().next()
}
pub fn deepseek_tokens() -> Vec<(String, String)> {
    let mut found = vec![];
    for (browser, root) in roots() {
        for profile in profiles(&root) {
            for file in leveldb_files(&profile) {
                let Ok(bytes) = fs::read(&file) else { continue };
                if let Some(token) = token_near(&bytes) {
                    found.push((browser.clone(), token));
                }
            }
        }
    }
    found
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn finds_a_jwt_in_utf8_and_utf16_leveldb_bytes() {
        let jwt = "eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiIxMjM0NTY3ODkwIn0.c2lnbmF0dXJlLXRlc3Q";
        let utf8 = format!("_https://platform.deepseek.com\u{1}userToken{{\"value\":\"{jwt}\"}}");
        assert_eq!(token_near(utf8.as_bytes()).as_deref(), Some(jwt));
        let mut utf16 = vec![];
        for unit in format!("platform.deepseek.com userToken {jwt}").encode_utf16() {
            utf16.extend_from_slice(&unit.to_le_bytes());
        }
        assert_eq!(token_near(&utf16).as_deref(), Some(jwt));
    }
    #[test]
    fn ignores_files_without_a_console_key() {
        assert!(token_near(b"eyJhbGciOiJIUzI1NiJ9.only-two-parts").is_none());
    }
}