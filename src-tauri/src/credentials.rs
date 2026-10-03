//! Locating and reading the credential files that the AI coding tools already
//! store on disk. PulseWin never asks you to paste a token if the tool itself
//! has already logged in — it reuses the same files the CLI/IDE writes.
//!
//! Every path is resolved with `dirs` so the code also builds on macOS/Linux
//! for development, but Windows (`%USERPROFILE%`, `%APPDATA%`, `%LOCALAPPDATA%`)
//! is the target platform.

use std::path::{Path, PathBuf};

use serde_json::Value;

/// Environment variable prefix that overrides on-disk discovery, one per provider.
/// e.g. `PULSEWIN_CLAUDE_TOKEN`, `PULSEWIN_CURSOR_COOKIE`.
pub const ENV_PREFIX: &str = "PULSEWIN_";

pub fn home_dir() -> Option<PathBuf> {
    #[cfg(test)]
    if let Some(directory) = std::env::var_os("PULSEWIN_TEST_HOME") {
        return Some(PathBuf::from(directory));
    }
    dirs::home_dir()
}

pub fn config_dir() -> Option<PathBuf> {
    #[cfg(test)]
    if let Some(directory) = std::env::var_os("PULSEWIN_TEST_CONFIG") {
        return Some(PathBuf::from(directory));
    }
    dirs::config_dir()
}

pub fn data_local_dir() -> Option<PathBuf> {
    #[cfg(test)]
    if let Some(directory) = std::env::var_os("PULSEWIN_TEST_LOCAL_CONFIG") {
        return Some(PathBuf::from(directory));
    }
    dirs::data_local_dir()
}

pub fn cache_dir() -> Option<PathBuf> {
    #[cfg(test)]
    if let Some(directory) = std::env::var_os("PULSEWIN_TEST_CACHE") {
        return Some(PathBuf::from(directory));
    }
    dirs::cache_dir()
}

/// Read and parse a JSON file, returning `None` for missing/corrupt files
/// rather than erroring — a tool simply not being installed is normal.
pub fn read_json(path: &Path) -> Option<Value> {
    if let Some(provider) = crate::credential_store::provider_at_path(path) {
        return crate::credential_store::read_at(path, provider).ok().flatten();
    }
    let text = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(&text).ok()
}

/// First path in `candidates` that exists on disk.
pub fn first_existing(candidates: &[PathBuf]) -> Option<PathBuf> {
    candidates.iter().find(|p| p.is_file()).cloned()
}

/// Build `%USERPROFILE%\.name`-style candidates plus a `$HOME`-relative fallback.
pub fn home_relative(parts: &[&str]) -> Vec<PathBuf> {
    let mut out = Vec::new();
    if let Some(home) = home_dir() {
        let mut p = home;
        for part in parts {
            p.push(part);
        }
        out.push(p);
    }
    out
}

/// Build `%APPDATA%\name\...` candidates.
pub fn config_relative(parts: &[&str]) -> Vec<PathBuf> {
    let mut out = Vec::new();
    if let Some(base) = config_dir() {
        let mut p = base;
        for part in parts {
            p.push(part);
        }
        out.push(p);
    }
    // Some tools use ~/.config on Windows too (when run under MSYS/Git Bash).
    out.extend(home_relative(&{
        let mut v = vec![".config"];
        v.extend_from_slice(parts);
        v
    }));
    out
}

/// Look up `PULSEWIN_{PROVIDER}_{KEY}`, then the generic `PULSEWIN_{KEY}`.
/// Returns the first non-empty value found.
pub fn env_override(provider: &str, key: &str) -> Option<String> {
    let specific = format!(
        "{}{}_{}",
        ENV_PREFIX,
        provider.to_uppercase().replace('-', "_"),
        key.to_uppercase()
    );
    let generic = format!("{}{}", ENV_PREFIX, key.to_uppercase());
    for name in [specific, generic] {
        if let Ok(v) = std::env::var(&name) {
            let v = v.trim().to_string();
            if !v.is_empty() {
                return Some(v);
            }
        }
    }
    None
}

/// Walk a dotted path through nested JSON: `dig(&v, "claudeAiOauth.accessToken")`.
pub fn dig<'a>(value: &'a Value, path: &str) -> Option<&'a Value> {
    let mut current = value;
    for segment in path.split('.') {
        current = match current {
            Value::Object(map) => map.get(segment)?,
            Value::Array(items) => items.get(segment.parse::<usize>().ok()?)?,
            _ => return None,
        };
    }
    Some(current)
}

/// Same as [`dig`] but coerced to an owned `String`.
pub fn dig_str(value: &Value, path: &str) -> Option<String> {
    match dig(value, path)? {
        Value::String(s) if !s.trim().is_empty() => Some(s.clone()),
        Value::Number(n) => Some(n.to_string()),
        _ => None,
    }
}

/// Same as [`dig`] but coerced to `f64`, accepting numeric strings.
pub fn dig_f64(value: &Value, path: &str) -> Option<f64> {
    match dig(value, path)? {
        Value::Number(n) => n.as_f64(),
        Value::String(s) => s.trim().parse::<f64>().ok(),
        _ => None,
    }
}

/// Try several dotted paths in order, returning the first hit.
pub fn dig_first_str(value: &Value, paths: &[&str]) -> Option<String> {
    paths.iter().find_map(|p| dig_str(value, p))
}

pub fn dig_first_f64(value: &Value, paths: &[&str]) -> Option<f64> {
    paths.iter().find_map(|p| dig_f64(value, p))
}

/// Expand a leading `~` to the user's home directory.
pub fn expand_tilde(raw: &str) -> PathBuf {
    if let Some(rest) = raw.strip_prefix("~/").or_else(|| raw.strip_prefix("~\\")) {
        if let Some(home) = home_dir() {
            return home.join(rest);
        }
    }
    PathBuf::from(raw)
}

// ---------------------------------------------------------------------------
// Per-provider credential locations
// ---------------------------------------------------------------------------

/// Claude Code CLI: `~/.claude/.credentials.json` (OAuth), with `~/.claude.json`
/// as a legacy fallback that sometimes carries the same block.
pub fn claude_credential_paths() -> Vec<PathBuf> {
    let mut v = home_relative(&[".claude", ".credentials.json"]);
    v.extend(home_relative(&[".claude.json"]));
    v
}

/// Codex CLI: `~/.codex/auth.json`.
pub fn codex_credential_paths() -> Vec<PathBuf> {
    let mut v = home_relative(&[".codex", "auth.json"]);
    v.extend(home_relative(&[".codex", "config.json"]));
    v
}

/// Gemini CLI: `~/.gemini/oauth_creds.json`.
pub fn gemini_credential_paths() -> Vec<PathBuf> {
    let mut v = home_relative(&[".gemini", "oauth_creds.json"]);
    v.extend(home_relative(&[".gemini", "settings.json"]));
    v
}

/// GitHub Copilot's editor extensions persist their OAuth token in a hosts file.
pub fn copilot_credential_paths() -> Vec<PathBuf> {
    let mut v = config_relative(&["GitHub Copilot", "hosts.json"]);
    v.extend(config_relative(&["github-copilot", "hosts.json"]));
    // VS Code / VS Code Insiders global storage.
    v.extend(config_relative(&["Code", "User", "globalStorage", "github.copilot", "hosts.json"]));
    v.extend(config_relative(&[
        "Code - Insiders",
        "User",
        "globalStorage",
        "github.copilot",
        "hosts.json",
    ]));
    // gh CLI token file is a valid fallback for api.github.com calls.
    v.extend(config_relative(&["GitHub CLI", "hosts.yml"]));
    v.extend(home_relative(&[".config", "gh", "hosts.yml"]));
    v
}

/// Cursor stores its session in a SQLite key/value store, so we can only point
/// at it; the actual token has to come from `PULSEWIN_CURSOR_COOKIE` or a
/// pasted value in Settings. This path is used by the "find my data" hint in UI.
pub fn cursor_state_db_paths() -> Vec<PathBuf> {
    let mut v = config_relative(&["Cursor", "User", "globalStorage", "state.vscdb"]);
    v.extend(config_relative(&["Cursor Nightly", "User", "globalStorage", "state.vscdb"]));
    v
}

/// Resolve a token for a provider: env override first, then the first readable
/// credential file, walking the given dotted JSON paths.
pub fn resolve_token(
    provider: &str,
    paths: &[PathBuf],
    dotted_paths: &[&str],
) -> Result<(String, PathBuf), String> {
    if let Some(token) = env_override(provider, "token") {
        return Ok((token, PathBuf::from(format!("<env:{provider}>"))));
    }

    if let Some(token) = crate::settings::read_credential_checked(provider).map_err(|error| error.to_string())?.api_key.filter(|s| !s.trim().is_empty()) {
        let path = crate::settings::credential_file(provider).ok_or("Invalid provider id")?;
        return Ok((token, path));
    }

    let path = first_existing(paths).ok_or_else(|| {
        format!(
            "no credentials found (looked for: {})",
            paths
                .iter()
                .map(|p| p.display().to_string())
                .collect::<Vec<_>>()
                .join(", ")
        )
    })?;

    let json = read_json(&path)
        .ok_or_else(|| format!("could not parse {}", path.display()))?;

    let token = dig_first_str(&json, dotted_paths)
        .ok_or_else(|| format!("no token field in {}", path.display()))?;

    Ok((token, path))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn digs_nested_paths() {
        let v = json!({ "a": { "b": [ { "c": "deep" } ] } });
        assert_eq!(dig_str(&v, "a.b.0.c").as_deref(), Some("deep"));
        assert!(dig_str(&v, "a.missing.c").is_none());
    }

    #[test]
    fn dig_str_skips_empty_strings() {
        let v = json!({ "token": "   " });
        assert!(dig_str(&v, "token").is_none());
    }

    #[test]
    fn dig_f64_accepts_numeric_strings() {
        let v = json!({ "used": "42.5" });
        assert_eq!(dig_f64(&v, "used"), Some(42.5));
    }

    #[test]
    fn dig_first_returns_first_hit() {
        let v = json!({ "second": 7 });
        assert_eq!(dig_first_f64(&v, &["first", "second"]), Some(7.0));
    }

    #[test]
    fn expands_tilde() {
        if home_dir().is_some() {
            let p = expand_tilde("~/.claude/.credentials.json");
            assert!(p.is_absolute());
            assert!(p.ends_with(".claude/.credentials.json") || p.ends_with(r".claude\.credentials.json"));
        }
    }
}
