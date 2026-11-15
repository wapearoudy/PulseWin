//! DeepSeek console sign-ins kept in Chromium's local storage (Pulse 1.12 renewal path).
//! Pulse stores the sign-in encrypted; this importer only copies a token the user already
//! signed in with, then the caller verifies it against the console before saving.
use std::{collections::BTreeSet, fs, path::{Path, PathBuf}};
const MAX_FILE: u64 = 32 * 1024 * 1024;
const MAX_TOKEN: usize = 4096;
/// Where each Chromium-based browser keeps its profiles, under one base folder.
///
/// **ego lite is here for the same reason the others are: it is Chromium
/// underneath.** The original reads it from `Citro Labs/ego lite` inside the
/// platform's application-support folder (1.13.0), and a Chromium app on Windows
/// keeps its profiles one step further down, in `User Data` — the shape every
/// other entry here already has. The folder without `User Data` is listed too,
/// because the profile rule below (`Default` or `Profile N`) is what decides
/// whether there is anything to read, and an ego lite build that keeps its
/// profiles where its macOS one does must not be missed. A folder that is not
/// there is dropped, so a browser the reader does not have costs nothing.
fn roots_in(base: &Path) -> Vec<(String, PathBuf)> {
    [
        ("Chrome", base.join("Google/Chrome/User Data")),
        ("Edge", base.join("Microsoft/Edge/User Data")),
        ("Brave", base.join("BraveSoftware/Brave-Browser/User Data")),
        ("Chromium", base.join("Chromium/User Data")),
        ("ego lite", base.join("Citro Labs/ego lite/User Data")),
        ("ego lite", base.join("Citro Labs/ego lite")),
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
/// Every console sign-in this machine's browsers still hold, with the browser
/// it came from. The caller verifies each one against the console before it
/// saves any of them, so an expired token costs one request, not a wrong answer.
pub fn deepseek_tokens() -> Vec<(String, String)> {
    match std::env::var_os("LOCALAPPDATA").map(PathBuf::from) {
        Some(base) => tokens_in(&base),
        None => vec![],
    }
}

/// The same list read from one base folder, so a fixture can stand in for a
/// machine's browsers without a real profile.
fn tokens_in(base: &Path) -> Vec<(String, String)> {
    let mut found: Vec<(String, String)> = vec![];
    for (browser, root) in roots_in(base) {
        for profile in profiles(&root) {
            for file in leveldb_files(&profile) {
                let Ok(bytes) = fs::read(&file) else { continue };
                if let Some(token) = token_near(&bytes) {
                    // ego lite can be found in both places it keeps its profiles,
                    // and one sign-in verified twice is still one sign-in.
                    if found.iter().any(|(_, seen)| *seen == token) {
                        continue;
                    }
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

    // --- which folders are read --------------------------------------------

    static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

    /// A base folder that stands in for `%LOCALAPPDATA%`.
    fn sandbox(label: &str) -> PathBuf {
        let suffix = format!("{label}-{}-{}", std::process::id(), NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed));
        let candidate = std::env::temp_dir().join(format!("pulsewin-browser-{suffix}"));
        if fs::create_dir_all(&candidate).is_ok() {
            return candidate;
        }
        // Sandboxed test binaries can be denied writes outside the workspace.
        let fallback = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("target")
            .join("test-scratch")
            .join(format!("pulsewin-browser-{suffix}"));
        fs::create_dir_all(&fallback).unwrap_or_else(|error| panic!("create_dir_all {}: {error:?}", fallback.display()));
        fallback
    }

    /// One LevelDB log in one profile, holding a console sign-in the way the
    /// browser writes it: the site, then the token.
    fn session(base: &Path, browser: &str, profile: &str, name: &str, token: &str) {
        let directory = base.join(browser).join(profile).join("Local Storage").join("leveldb");
        fs::create_dir_all(&directory).unwrap();
        let contents = format!("_https://platform.deepseek.com\u{1}userToken{{\"value\":\"{token}\"}}");
        fs::write(directory.join(name), contents).unwrap();
    }

    /// A JWT-shaped token; `seed` makes each browser's sign-in its own.
    fn token(seed: u8) -> String {
        format!("eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiJpZA{seed}e30.c2lnbmF0dXJlLXRlc3Q")
    }

    #[test]
    fn reads_the_session_ego_lite_keeps() {
        // ego lite's folders are `Profile N`: there is no `Default` to fall back on.
        let base = sandbox("ego");
        session(&base, "Citro Labs/ego lite/User Data", "Profile 3", "000003.log", &token(9));
        assert_eq!(tokens_in(&base), vec![("ego lite".to_string(), token(9))]);
        let _ = fs::remove_dir_all(&base);
    }

    #[test]
    fn reads_the_ego_lite_session_wherever_it_keeps_its_profiles() {
        let base = sandbox("ego-flat");
        session(&base, "Citro Labs/ego lite", "Default", "000003.ldb", &token(7));
        assert_eq!(tokens_in(&base), vec![("ego lite".to_string(), token(7))]);
        let _ = fs::remove_dir_all(&base);
    }

    #[test]
    fn still_reads_the_browsers_it_read_before() {
        let base = sandbox("four");
        for (index, browser) in [
            "Google/Chrome/User Data",
            "Microsoft/Edge/User Data",
            "BraveSoftware/Brave-Browser/User Data",
            "Chromium/User Data",
        ]
        .into_iter()
        .enumerate()
        {
            session(&base, browser, "Default", "000003.log", &token(index as u8));
        }
        let found: Vec<String> = tokens_in(&base).into_iter().map(|(browser, _)| browser).collect();
        assert_eq!(found, vec!["Chrome", "Edge", "Brave", "Chromium"]);
        let _ = fs::remove_dir_all(&base);
    }

    #[test]
    fn one_sign_in_kept_in_two_places_is_reported_once() {
        let base = sandbox("ego-twice");
        session(&base, "Citro Labs/ego lite/User Data", "Profile 1", "000003.log", &token(5));
        session(&base, "Citro Labs/ego lite", "Profile 1", "000003.log", &token(5));
        assert_eq!(tokens_in(&base), vec![("ego lite".to_string(), token(5))]);
        let _ = fs::remove_dir_all(&base);
    }

    #[test]
    fn a_folder_with_nothing_signed_in_reads_as_nothing() {
        let base = sandbox("ego-empty");
        // The browser is installed and has a profile, but no console sign-in.
        fs::create_dir_all(base.join("Citro Labs/ego lite/User Data/Profile 2")).unwrap();
        assert!(tokens_in(&base).is_empty());
        // A base folder holding no browser at all answers the same way.
        assert!(tokens_in(&base.join("Citro Labs")).is_empty());
        let _ = fs::remove_dir_all(&base);
    }
}