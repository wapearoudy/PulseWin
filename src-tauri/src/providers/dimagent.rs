//! DimAgent's plan credits, read through the `dim` CLI's own login.
//!
//! **Nothing here holds a DimAgent credential.** The account is whatever
//! `dim auth login` last saved, and this provider asks the CLI for a JSON
//! summary rather than reading, copying or storing any part of that login — the
//! profile's `localLogin` credential and the `dim` process are the whole of it.
//!
//! `dim usage --json` answers with three separate things, and they must not be
//! folded together:
//!
//! - **A subscription bucket** — the plan's own credits, with an expiry.
//! - **Add-on buckets** — separately bought packs with their *own* expiry dates.
//!   They are never added to the subscription's denominator: a purchase does not
//!   grow the plan, and a pack expiring is not the plan resetting.
//! - **Feature meters** — `web_search` among them, an allowance of its own,
//!   neither a plan credit nor a top-up.
//!
//! Three rules about figures worth keeping:
//!
//! - A bucket states `total_units`/`used_units`/`remaining_units` as numbers, and
//!   **all three must agree with reality before anything is drawn**: a missing
//!   total, a negative usage or a non-finite number is "no limits reported", not
//!   zero used.
//! - **An expiry date makes the remainder the thing that expires.** A bucket
//!   that has expired is gone; a bucket still short of its expiry keeps its
//!   share, and is reported with the date its remainder disappears.
//! - Token windows inside a bucket (`window_states`) name their own duration in
//!   hours, which is the only statement of their length the reply makes. A
//!   window whose expiry has passed is dropped, exactly as an expired bucket is.

use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use serde_json::Value;

use super::{percent_from_scale, Ctx, FetchFuture, Provider};
use crate::credentials;
use crate::model::{ProviderUsage, UsageWindow, WindowKind};

const ID: &str = "dimagent";
const NAME: &str = "DimAgent";

/// How long the CLI is given before it is dropped. Long enough for a cold
/// start and a network round trip, short enough that a fetch every minute does
/// not pile up behind a CLI that will never answer.
const DEADLINE: Duration = Duration::from_secs(20);

/// Nothing this provider reports approaches this; the ceiling is here so a CLI
/// that streams forever cannot exhaust the process's memory.
const OUTPUT_CEILING: usize = 256 * 1024;

/// The highest window length worth drawing, in seconds: a year. Anything longer
/// is not a quota window whatever the reply says.
const LONGEST_WINDOW: f64 = 366.0 * 86_400.0;

/// The arguments the reference runs. One line, no shell.
fn arguments() -> Vec<String> {
    vec!["usage".to_string(), "--json".to_string()]
}

pub struct DimAgent;

impl Provider for DimAgent {
    fn id(&self) -> &'static str {
        ID
    }

    fn name(&self) -> &'static str {
        NAME
    }

    /// **The CLI's presence is the credential.** There is no key to look for:
    /// the login `dim auth login` saved is the CLI's own, and the state folder
    /// it keeps that login in is the only local evidence this port can see.
    fn is_configured(&self) -> bool {
        locate_cli().is_some() || state_folder().is_some()
    }

    fn fetch(&self, _ctx: Arc<Ctx>) -> FetchFuture {
        Box::pin(async move { fetch_inner().await })
    }
}

async fn fetch_inner() -> ProviderUsage {
    if let Some(named) = credentials::env_override(ID, "bin") {
        let path = credentials::expand_tilde(named.trim());
        if !path.is_file() {
            return ProviderUsage::failed(ID, NAME, "the program named for DimAgent is not there");
        }
    }

    let bytes = match run().await {
        Ok(bytes) => bytes,
        Err(Failure::NotInstalled) => {
            return ProviderUsage::failed(
                ID,
                NAME,
                "the dim CLI was not found, so its usage cannot be read",
            )
        }
        Err(Failure::TimedOut) => {
            return ProviderUsage::failed(ID, NAME, "the dim CLI did not answer in time")
        }
        Err(Failure::StartFailed) => {
            return ProviderUsage::failed(ID, NAME, "the dim CLI could not be started")
        }
        Err(Failure::Exited(stderr)) => {
            return ProviderUsage::failed(ID, NAME, exit_message(&stderr))
        }
    };

    match usage_from(&bytes, now()) {
        Ok(usage) => usage,
        Err(reason) => ProviderUsage::failed(ID, NAME, reason),
    }
}

fn now() -> i64 {
    chrono::Utc::now().timestamp()
}

// ---------------------------------------------------------------- the process

#[derive(Debug, PartialEq, Eq)]
enum Failure {
    NotInstalled,
    StartFailed,
    TimedOut,
    /// It ran and refused: the CLI's own words are the only diagnosis there is.
    Exited(String),
}

/// Where the CLI is, or `None` when it is nowhere this port knows to look.
fn locate_cli() -> Option<PathBuf> {
    if let Some(named) = credentials::env_override(ID, "bin") {
        let path = credentials::expand_tilde(named.trim());
        return path.is_file().then_some(path);
    }
    locate(&candidate_dirs(), &|path| path.is_file())
}

/// The first file among the candidates. The check is injected so a test can
/// drive the search without installing anything.
fn locate(dirs: &[PathBuf], exists: &dyn Fn(&Path) -> bool) -> Option<PathBuf> {
    for dir in dirs {
        for name in names() {
            let path = dir.join(name);
            if exists(&path) {
                return Some(path);
            }
        }
    }
    None
}

/// The names a `dim` install goes by. Windows resolves a bare name through
/// `PATHEXT`, but the file itself is `dim.exe` (or a wrapper script), and the
/// search below joins names to folders rather than asking the shell.
fn names() -> &'static [&'static str] {
    if cfg!(windows) {
        &["dim.exe", "dim.cmd", "dim.bat"]
    } else {
        &["dim"]
    }
}

/// The folders a `dim` install lives in, in the order the reference prefers
/// them.
///
/// **The mise shim is named outright.** A GUI launch does not inherit a
/// terminal's mise activation, so a CLI installed through mise is on no `PATH`
/// this process can see — and the shim is a fixed, known location.
fn candidate_dirs() -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = Vec::new();

    if let Some(path) = std::env::var_os("PATH") {
        dirs.extend(std::env::split_paths(&path));
    }

    // The mise shim, which is where a mise-installed CLI answers from.
    if let Some(home) = std::env::var_os("USERPROFILE")
        .or_else(|| std::env::var_os("HOME"))
        .filter(|value| !value.is_empty())
    {
        let home = PathBuf::from(home);
        dirs.push(home.join(".local/share/mise/shims"));
        dirs.push(home.join(".local/bin"));
        dirs.push(home.join(".dimcode/bin"));
    }

    // A per-user install from npm or a standalone download.
    if let Some(app_data) = std::env::var_os("LOCALAPPDATA").filter(|value| !value.is_empty()) {
        let app_data = PathBuf::from(app_data);
        dirs.push(app_data.join("Programs/dim"));
        dirs.push(app_data.join("dim"));
    }

    if let Some(roaming) = std::env::var_os("APPDATA").filter(|value| !value.is_empty()) {
        dirs.push(PathBuf::from(roaming).join("npm"));
    }

    // `ProgramFiles` last: a machine-wide install is unusual for this CLI.
    if let Some(program_files) = std::env::var_os("ProgramFiles").filter(|value| !value.is_empty()) {
        dirs.push(PathBuf::from(program_files).join("dim"));
    }

    dirs.retain(|dir| !dir.as_os_str().is_empty());
    dirs
}

/// The folder the CLI keeps its own state in (`discoveryPaths: [".dimcode"]`).
///
/// A presence hint and never permission to read: nothing here opens it, and no
/// part of the login is copied out of it.
fn state_folder() -> Option<PathBuf> {
    credentials::home_relative(&[".dimcode"])
        .into_iter()
        .find(|path| path.is_dir())
}

/// The variables a helper keeps. **An allow-list, not a deny-list**: what is not
/// named here does not cross into it, so no key, cookie or credential of
/// PulseWin's own reaches a program this runs on the reader's behalf.
const KEPT: [&str; 30] = [
    "SYSTEMROOT",
    "SYSTEMDRIVE",
    "WINDIR",
    "COMSPEC",
    "PATHEXT",
    "TEMP",
    "TMP",
    "PROGRAMDATA",
    "PROGRAMFILES",
    "PROGRAMFILES(X86)",
    "COMMONPROGRAMFILES",
    "NUMBER_OF_PROCESSORS",
    "PROCESSOR_ARCHITECTURE",
    "OS",
    // Where the CLI's own login lives.
    "USERPROFILE",
    "HOMEDRIVE",
    "HOMEPATH",
    "APPDATA",
    "LOCALAPPDATA",
    // Its clock, its language, and the proxy the network needs.
    "LANG",
    "LC_ALL",
    "LC_CTYPE",
    "TZ",
    "HTTP_PROXY",
    "HTTPS_PROXY",
    "ALL_PROXY",
    "NO_PROXY",
    "http_proxy",
    "https_proxy",
    "all_proxy",
];

fn kept(name: &OsStr) -> bool {
    let name = name.to_string_lossy();
    !name.is_empty()
        && KEPT
            .iter()
            .any(|kept| name.eq_ignore_ascii_case(kept))
        // The `no_proxy` spelling pairs with the lower-case proxies above; it is
        // off the end of `KEPT` only to keep that array's length explicit.
        || name.eq_ignore_ascii_case("no_proxy")
}

/// Only what the CLI needs to find itself, its login and the network, with the
/// folder it was found in — and the mise shims — at the front of `PATH`.
fn environment(binary: &Path) -> Vec<(OsString, OsString)> {
    let mut environment: Vec<(OsString, OsString)> = std::env::vars_os()
        .filter(|(name, _)| kept(name))
        .collect();

    let mut dirs: Vec<PathBuf> = Vec::new();
    if let Some(folder) = binary.parent() {
        dirs.push(folder.to_path_buf());
    }
    if let Some(home) = std::env::var_os("USERPROFILE")
        .or_else(|| std::env::var_os("HOME"))
        .filter(|value| !value.is_empty())
    {
        dirs.push(PathBuf::from(home).join(".local/share/mise/shims"));
    }
    if let Some(path) = std::env::var_os("PATH") {
        dirs.extend(std::env::split_paths(&path));
    }
    if let Some(root) = std::env::var_os("SYSTEMROOT") {
        dirs.push(PathBuf::from(root).join("System32"));
    }

    let mut joined = OsString::new();
    for (index, dir) in dirs.iter().enumerate() {
        if index > 0 {
            joined.push(if cfg!(windows) { ";" } else { ":" });
        }
        joined.push(dir.as_os_str());
    }
    environment.push((OsString::from("PATH"), joined));

    crate::preferences::load().network_proxy.process_environment(environment)
}

/// The command that runs `binary`, wrapped in `cmd.exe` when it is a batch file,
/// which `CreateProcess` cannot start on its own.
fn command_for(binary: &Path) -> tokio::process::Command {
    let batch = binary
        .extension()
        .map(|extension| {
            let extension = extension.to_string_lossy().to_lowercase();
            extension == "cmd" || extension == "bat"
        })
        .unwrap_or(false);

    let mut command = if batch {
        let mut command = tokio::process::Command::new("cmd.exe");
        command.arg("/c").arg(binary);
        command
    } else {
        tokio::process::Command::new(binary)
    };

    command
        .args(arguments())
        .env_clear()
        .envs(environment(binary))
        // A run that is dropped — the deadline below — takes the child with it.
        .kill_on_drop(true);

    #[cfg(windows)]
    command.creation_flags(CREATE_NO_WINDOW);

    command
}

#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// Ask the CLI, and read at most `OUTPUT_CEILING` bytes of what it says.
async fn run() -> Result<Vec<u8>, Failure> {
    let Some(binary) = locate_cli() else {
        return Err(Failure::NotInstalled);
    };

    let mut command = command_for(&binary);
    command
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());

    let child = command.spawn().map_err(|_| Failure::StartFailed)?;

    match tokio::time::timeout(DEADLINE, collect(child)).await {
        Ok(outcome) => outcome,
        // The future is dropped, and `kill_on_drop` takes the child with it.
        Err(_) => Err(Failure::TimedOut),
    }
}

/// Wait for the CLI, keeping the first `OUTPUT_CEILING` bytes of its stdout and
/// the whole of its stderr — the stderr is short and is the diagnosis.
async fn collect(child: tokio::process::Child) -> Result<Vec<u8>, Failure> {
    let output = child
        .wait_with_output()
        .await
        .map_err(|_| Failure::StartFailed)?;

    if output.stdout.len() > OUTPUT_CEILING {
        return Err(Failure::Exited(
            "the dim CLI said more than this reader will hold".to_string(),
        ));
    }

    if !output.status.success() {
        // Both streams can carry the reason: the CLI writes its complaint to
        // stderr, but a wrapper script's shell writes to stdout.
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
        return Err(Failure::Exited(if stderr.is_empty() { stdout } else { stderr }));
    }

    Ok(output.stdout)
}

/// What the CLI's refusal means, in the reader's words.
///
/// The distinctions matter: a missing login is the reader's to fix, a token that
/// expired is a command to run, and a 429 is neither — telling somebody their
/// credential is wrong because the service was busy sends them to the wrong
/// screen.
fn exit_message(stderr: &str) -> String {
    let text = stderr.to_lowercase();

    if text.contains("not logged in") || text.contains("not signed in") {
        return "the dim CLI has no saved login; run dim auth login".to_string();
    }
    if text.contains("dim auth login") || text.contains("unauthorized") || text.contains("token expired") {
        return "the dim CLI's login expired; run dim auth login".to_string();
    }
    if text.contains("429") || text.contains("rate limit") {
        return "DimAgent rate limited the read; try again shortly".to_string();
    }
    if stderr.trim().is_empty() {
        return "the dim CLI stopped without saying why".to_string();
    }

    // Its own words, first line only: a stack trace says nothing a reader can act on.
    let first = stderr
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or("the dim CLI stopped without saying why");
    format!("the dim CLI reported: {first}")
}

// ------------------------------------------------------------------ the reply

/// The reading, or why there is none.
///
/// `Err` carries a message for the card; the two `Err("no limits reported")`
/// cases are the reference's `noLimitsReported`, and unreadable JSON is its
/// `unreadableReply`.
fn usage_from(bytes: &[u8], now: i64) -> Result<ProviderUsage, String> {
    let reply: Value = serde_json::from_slice(bytes).map_err(|_| unreadable())?;
    // A reply that is not an object is not this reader's JSON, even when it
    // parses: an array or a bare number says nothing about any limit.
    if !reply.is_object() {
        return Err(unreadable());
    }

    let credits = match reply.get("credits") {
        Some(value) if value.is_null() => None,
        Some(value) if value.is_object() => Some(value),
        // `"credits": 42` is not a reply this reader understands, and reading it
        // as "no credits" would report an account with a plan as having none.
        Some(_) => return Err(unreadable()),
        None => None,
    };

    let mut windows: Vec<UsageWindow> = Vec::new();

    if let Some(bucket) = credits.and_then(|credits| credits.get("subscription_bucket")) {
        if let Some(window) = bucket_window(bucket, "dimagent.subscription", WindowKind::Credits, now) {
            windows.push(window.clone());
            windows.extend(token_windows(bucket, "dimagent.subscription", now));
        }
    }

    // Packs have separate expiry dates; never add them to the subscription
    // denominator, and never let one shadow an id already taken.
    if let Some(packs) = credits
        .and_then(|credits| credits.get("addon_buckets"))
        .and_then(Value::as_array)
    {
        for pack in packs {
            let Some(id) = pack.get("id").and_then(dig_id) else {
                continue;
            };
            let window_id = format!("dimagent.addon.{id}");
            let Some(window) = bucket_window(pack, &window_id, WindowKind::TopUp, now) else {
                continue;
            };
            if windows.iter().any(|window| window.id.as_deref() == Some(window_id.as_str())) {
                continue;
            }
            windows.push(window);
            windows.extend(token_windows(pack, &window_id, now));
        }
    }

    if let Some(meters) = reply.get("feature_meters").and_then(Value::as_array) {
        for meter in meters {
            if meter.get("feature_key").and_then(Value::as_str) != Some("web_search") {
                continue;
            }
            // An unlimited meter has no percentage worth drawing.
            if meter.get("unlimited").and_then(Value::as_bool) == Some(true) {
                continue;
            }
            if windows
                .iter()
                .any(|window| window.id.as_deref() == Some("dimagent.web_search"))
            {
                continue;
            }
            let Some(amounts) = amounts(
                meter.get("total_used").and_then(as_number),
                meter.get("total_remaining").and_then(as_number),
                meter.get("total_allowance").and_then(as_number),
            ) else {
                continue;
            };

            windows.push(UsageWindow {
                id: Some("dimagent.web_search".to_string()),
                scope: Some("Web Search".to_string()),
                kind: WindowKind::Credits,
                is_exhausted: false,
                window_seconds: None,
                label: "Web Search".to_string(),
                percent_used: Some(amounts.percent),
                resets_at: meter
                    .get("period_end")
                    .and_then(Value::as_str)
                    .and_then(rfc3339),
                detail: Some(amounts.detail("")),
            });
        }
    }

    if windows.is_empty() {
        return Err(no_limits());
    }

    let plan = reply
        .get("subscription")
        .and_then(|subscription| subscription.get("product"))
        .and_then(|product| product.get("name"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .map(str::to_string);

    Ok(ProviderUsage::ok(ID, NAME, windows).with_plan(plan))
}

fn unreadable() -> String {
    "the dim CLI's reply could not be read".to_string()
}

fn no_limits() -> String {
    "no DimAgent limits were reported".to_string()
}

/// One bucket as a window, or `None` when the reply does not describe one.
///
/// **A bucket has to be usable before it is drawn**: active or exhausted, with a
/// total and a usage that agree with each other. Only an exhausted bucket may
/// omit its remainder; one that expires still owes a remainder, because the
/// thing that expires is the remainder itself.
fn bucket_window(bucket: &Value, id: &str, kind: WindowKind, now: i64) -> Option<UsageWindow> {
    let status = bucket.get("status").and_then(Value::as_str)?;
    if status != "active" && status != "exhausted" {
        return None;
    }

    let amounts = amounts(
        bucket.get("used_units").and_then(as_number),
        bucket.get("remaining_units").and_then(as_number),
        bucket.get("total_units").and_then(as_number),
    )?;

    let expiry = bucket.get("expires_at").and_then(Value::as_str);

    // An expired bucket is gone, not empty: drawing it as 0% left says the plan
    // still has something to spend.
    if let Some(expiry) = expiry {
        let instant = rfc3339(expiry).and_then(|text| rfc3339_instant(&text));
        if instant.map(|at| at <= now).unwrap_or(false) {
            return None;
        }
        // A bucket that expires must say what expires.
        if bucket.get("remaining_units").and_then(as_number).is_none() {
            return None;
        }
    }

    let detail = {
        let mut text = amounts.detail("credits");
        if let Some(expiry) = expiry.and_then(rfc3339) {
            text = format!("{text}, expiring {expiry}");
        }
        text
    };

    Some(UsageWindow {
        id: Some(id.to_string()),
        scope: None,
        kind,
        is_exhausted: status == "exhausted",
        // A credit pool is not a timed allowance, and its expiry is not a reset.
        window_seconds: None,
        label: match kind {
            WindowKind::TopUp => "Top-up".to_string(),
            _ => "Credits".to_string(),
        },
        percent_used: Some(amounts.percent),
        resets_at: None,
        detail: Some(detail),
    })
}

/// The token windows a bucket carries: a length in hours, a cap, and how much of
/// it is used.
///
/// The duration is the only statement of the window's length, so a window whose
/// length cannot be read is dropped rather than guessed at — and one whose
/// expiry has passed is dropped as well, exactly as an expired bucket is.
fn token_windows(bucket: &Value, parent: &str, now: i64) -> Vec<UsageWindow> {
    let Some(states) = bucket.get("window_states").and_then(Value::as_array) else {
        return Vec::new();
    };

    let mut occurrences: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
    let mut windows = Vec::new();

    for state in states {
        let Some(hours) = state.get("window_duration_hours").and_then(as_number) else {
            continue;
        };
        if !hours.is_finite() || hours <= 0.0 || hours * 3600.0 > LONGEST_WINDOW {
            continue;
        }
        let seconds = (hours * 3600.0).round() as i64;
        if seconds <= 0 {
            continue;
        }

        let Some(amounts) = amounts(
            state.get("window_token_used").and_then(as_number),
            None,
            state.get("window_token_cap").and_then(as_number),
        ) else {
            continue;
        };

        let key = match state.get("id").and_then(dig_id) {
            Some(id) => format!("id:{id}"),
            None => format!("seconds:{seconds}"),
        };
        let seen = occurrences.entry(key.clone()).or_insert(0);
        let occurrence = *seen;
        *seen += 1;

        let reset = state.get("window_expires_at").and_then(Value::as_str);
        let reset_at = reset.and_then(rfc3339);
        // A window that has already reset is not a window the reader is inside.
        if let Some(at) = reset_at.as_deref().and_then(rfc3339_instant) {
            if at <= now {
                continue;
            }
        } else if reset.is_some() {
            // It named an expiry this reader cannot parse; drawing it would put
            // an unreadable clock on the card.
            continue;
        }

        windows.push(UsageWindow {
            id: Some(format!("{parent}.tokens.{key}.{occurrence}")),
            scope: None,
            kind: WindowKind::Limit,
            is_exhausted: false,
            window_seconds: Some(seconds as f64),
            label: label_for(seconds),
            percent_used: Some(amounts.percent),
            resets_at: reset_at,
            detail: None,
        });
    }

    windows
}

/// The three figures a bucket states, with the percentage derived from them.
///
/// **Every one of them has to agree with reality.** A total of zero cannot be a
/// denominator, and a negative or non-finite usage is a reply this reader does
/// not understand — reporting it as "0% used" would draw a full ring for a
/// bucket that is broken.
struct Amounts {
    used: f64,
    remaining: Option<f64>,
    limit: f64,
    percent: f64,
}

impl Amounts {
    /// "88,000 / 100,000" — the figures the reply gave, in the reply's own units.
    fn detail(&self, unit: &str) -> String {
        let mut text = format!("{:.0} / {:.0}", self.used, self.limit);
        if !unit.is_empty() {
            text.push(' ');
            text.push_str(unit);
        }
        if let Some(remaining) = self.remaining {
            text.push_str(&format!(" ({:.0} left)", remaining));
        }
        text
    }
}

fn amounts(used: Option<f64>, remaining: Option<f64>, total: Option<f64>) -> Option<Amounts> {
    let used = used.filter(|value| value.is_finite() && *value >= 0.0)?;
    let limit = total.filter(|value| value.is_finite() && *value > 0.0)?;
    let remaining = match remaining {
        Some(value) if value.is_finite() && value >= 0.0 => Some(value),
        Some(_) => return None,
        None => None,
    };

    Some(Amounts {
        used,
        remaining,
        limit,
        percent: percent_from_scale((used / limit) * 100.0),
    })
}

/// The short label a window of this length goes by.
fn label_for(seconds: i64) -> String {
    match seconds {
        18_000 => "5h".to_string(),
        86_400 => "1d".to_string(),
        604_800 => "7d".to_string(),
        other => super::humanize_window_seconds(other),
    }
}

/// A number that may arrive as JSON `1` or `1.0`.
fn as_number(value: &Value) -> Option<f64> {
    value
        .as_f64()
        .or_else(|| value.as_str().and_then(|text| text.trim().parse().ok()))
}

/// An id that may arrive as a number or a string.
fn dig_id(value: &Value) -> Option<String> {
    match value {
        Value::Number(number) => Some(number.to_string()),
        Value::String(text) => Some(text.trim().to_string()).filter(|text| !text.is_empty()),
        _ => None,
    }
}

/// An RFC3339 timestamp, normalised the way the rest of the port states times.
fn rfc3339(text: &str) -> Option<String> {
    let parsed = chrono::DateTime::parse_from_rfc3339(text.trim()).ok()?;
    Some(parsed.to_utc().to_rfc3339_opts(chrono::SecondsFormat::Secs, true))
}

fn rfc3339_instant(text: &str) -> Option<i64> {
    chrono::DateTime::parse_from_rfc3339(text.trim())
        .ok()
        .map(|parsed| parsed.timestamp())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The shape observed from `dimcode` 0.5.21, identities removed and figures
    /// replaced.
    fn sample() -> &'static str {
        r#"{"subscription":{"product":{"name":"Max plan"}},
         "credits":{"subscription_bucket":{"id":1,"status":"active","total_units":100000,
           "used_units":88000,"remaining_units":12000,"expires_at":"2026-11-13T02:33:55.398Z"},
           "addon_buckets":[]},
         "feature_meters":[{"feature_key":"web_search","total_allowance":5500,
           "total_used":100,"total_remaining":5400,"unlimited":false,"period_end":"2026-11-13T02:33:55.398Z"}]}"#
    }

    /// Well before the sample's expiry.
    const NOW: i64 = 1_793_059_200;

    #[test]
    fn the_subscription_and_search_are_two_separate_windows() {
        let usage = usage_from(sample().as_bytes(), NOW).unwrap();

        assert_eq!(usage.plan.as_deref(), Some("Max plan"));
        assert_eq!(usage.windows.len(), 2);

        let credits = &usage.windows[0];
        assert_eq!(credits.id.as_deref(), Some("dimagent.subscription"));
        assert_eq!(credits.percent_used, Some(88.0));
        assert_eq!(credits.is_exhausted, false);
        // A credit pool is not a timed allowance: no reset, and the expiry is
        // stated in the detail line rather than as a clock.
        assert!(credits.resets_at.is_none());
        assert!(credits.window_seconds.is_none());
        assert!(credits.detail.as_deref().unwrap().contains("expiring"));

        let search = &usage.windows[1];
        assert_eq!(search.id.as_deref(), Some("dimagent.web_search"));
        assert_eq!(search.scope.as_deref(), Some("Web Search"));
        assert_eq!(search.percent_used, Some(percent_from_scale(100.0 / 5500.0 * 100.0)));
    }

    #[test]
    fn an_add_on_is_its_own_window_and_not_part_of_the_plan() {
        let json = r#"{"credits":{"addon_buckets":[{"id":9,"status":"active","total_units":1000,
            "used_units":200,"remaining_units":800,"expires_at":"2026-12-01T00:00:00Z"}]}}"#;
        let usage = usage_from(json.as_bytes(), NOW).unwrap();

        assert_eq!(usage.windows.len(), 1);
        assert_eq!(usage.windows[0].id.as_deref(), Some("dimagent.addon.9"));
        assert_eq!(usage.windows[0].kind, WindowKind::TopUp);
        assert_eq!(usage.windows[0].label, "Top-up");
        assert!(usage.windows[0].resets_at.is_none());
    }

    #[test]
    fn windows_that_are_missing_invalid_expired_or_unlimited_are_never_a_zero() {
        let cases = [
            "{}",
            r#"{"credits":{"subscription_bucket":{"status":"active","total_units":0,"used_units":0}}}"#,
            r#"{"credits":{"subscription_bucket":{"status":"active","total_units":100,"used_units":-1}}}"#,
            r#"{"credits":{"subscription_bucket":{"status":"active","total_units":100,"used_units":1,
                "expires_at":"2020-01-01T00:00:00Z"}}}"#,
            r#"{"feature_meters":[{"feature_key":"web_search","unlimited":true,
                "total_allowance":100,"total_used":0}]}"#,
        ];

        for case in cases {
            assert_eq!(usage_from(case.as_bytes(), NOW).unwrap_err(), no_limits(), "{case}");
        }
    }

    #[test]
    fn an_expiring_bucket_without_a_reported_remainder_is_not_accepted() {
        for remainder in ["", r#","remaining_units":null"#] {
            let json = format!(
                r#"{{"credits":{{"subscription_bucket":{{"status":"active","total_units":100,
                 "used_units":95,"expires_at":"2026-11-13T02:33:55.398Z"{remainder}}}}}}}"#
            );
            assert_eq!(usage_from(json.as_bytes(), NOW).unwrap_err(), no_limits());
        }
    }

    #[test]
    fn a_malformed_reply_fails_rather_than_reading_as_an_empty_account() {
        for case in ["not json", "[]", r#"{"credits":42}"#] {
            assert_eq!(usage_from(case.as_bytes(), NOW).unwrap_err(), unreadable(), "{case}");
        }
    }

    #[test]
    fn token_windows_take_their_length_from_the_reply() {
        let json = r#"{"credits":{"subscription_bucket":{"status":"active","total_units":100,
            "used_units":10,"window_states":[
              {"id":1,"window_duration_hours":5,"window_token_cap":1000,"window_token_used":250,
               "window_expires_at":"2026-11-13T02:33:55.398Z"},
              {"id":2,"window_duration_hours":168,"window_token_cap":20000,"window_token_used":5000}
            ]}}}"#;
        let usage = usage_from(json.as_bytes(), NOW).unwrap();

        let labels: Vec<&str> = usage.windows.iter().map(|w| w.label.as_str()).collect();
        assert_eq!(labels, vec!["Credits", "5h", "7d"]);
        assert_eq!(usage.windows[1].percent_used, Some(25.0));
        assert_eq!(usage.windows[1].window_seconds, Some(18_000.0));
        assert_eq!(
            usage.windows[1].id.as_deref(),
            Some("dimagent.subscription.tokens.id:1.0")
        );
        // The second window named no expiry, and is still a window.
        assert!(usage.windows[2].resets_at.is_none());
    }

    #[test]
    fn a_window_past_its_reset_is_dropped() {
        let json = r#"{"credits":{"subscription_bucket":{"status":"active","total_units":100,
            "used_units":10,"window_states":[{"window_duration_hours":5,"window_token_cap":1000,
            "window_token_used":250,"window_expires_at":"2020-01-01T00:00:00Z"}]}}}"#;
        let usage = usage_from(json.as_bytes(), NOW).unwrap();
        assert_eq!(usage.windows.len(), 1);
        assert_eq!(usage.windows[0].label, "Credits");
    }

    #[test]
    fn an_exhausted_bucket_is_flagged_and_a_missing_one_is_not() {
        let json = r#"{"credits":{"subscription_bucket":{"status":"exhausted","total_units":100,
            "used_units":100,"remaining_units":0}}}"#;
        let usage = usage_from(json.as_bytes(), NOW).unwrap();
        assert!(usage.windows[0].is_exhausted);
        assert_eq!(usage.windows[0].percent_used, Some(100.0));

        let pending = r#"{"credits":{"subscription_bucket":{"status":"pending","total_units":100,
            "used_units":1,"remaining_units":99}}}"#;
        assert_eq!(usage_from(pending.as_bytes(), NOW).unwrap_err(), no_limits());
    }

    #[test]
    fn a_cli_refusal_keeps_its_distinct_remedy() {
        assert!(exit_message("Error: not logged in").contains("dim auth login"));
        assert!(exit_message("run dim auth login first").contains("expired"));
        assert!(exit_message("HTTP 429 Too Many Requests").contains("rate limited"));
        assert!(exit_message("connection refused").contains("the dim CLI reported: connection refused"));
        assert!(exit_message("   ").contains("without saying why"));
    }

    #[test]
    fn the_helper_gets_its_own_folder_and_the_proxies_but_no_borrowed_secrets() {
        let environment = environment(Path::new("/example/bin/dim"));
        let named = |name: &str| {
            environment
                .iter()
                .find(|(key, _)| key.to_string_lossy().eq_ignore_ascii_case(name))
                .map(|(_, value)| value.to_string_lossy().to_string())
        };

        // A variable that is not on the allow-list never crosses.
        assert!(named("PULSEWIN_TEST_SECRET").is_none());
        // The folder it was found in leads `PATH`.
        assert!(named("PATH").unwrap().starts_with("/example/bin"));
    }

    #[test]
    fn the_cli_is_reported_as_missing_rather_than_reported_as_empty() {
        let dirs = vec![PathBuf::from("/nothing/here")];
        assert!(locate(&dirs, &|_| false).is_none());
        // The first candidate that exists wins, in the order the search lists them.
        let found = locate(&dirs, &|path| {
            path.file_name()
                .map(|name| name.to_string_lossy().starts_with("dim"))
                .unwrap_or(false)
        });
        assert!(found.is_some());
    }

    #[test]
    fn the_arguments_are_the_reference_ones() {
        assert_eq!(arguments(), vec!["usage".to_string(), "--json".to_string()]);
    }

    #[test]
    fn the_window_ceiling_is_a_year_of_seconds() {
        // The longest length the reply can state and still be drawn as a window.
        assert!(LONGEST_WINDOW >= 86_400.0 * 365.0);
    }
}
