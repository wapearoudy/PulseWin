//! Alibaba Cloud's Token Plan (Model Studio / Bailian): a five-hour, a weekly
//! and a monthly allowance, each reported by the service as the share of it
//! already used.
//!
//! Read by running Alibaba's own Bailian CLI, `bl`, with the login it already
//! saved: `bl usage token-plan --console-region … --console-site … --output
//! json`. PulseWin passes it no credential and reads none of its files. The
//! international console is asked first and the China mainland one second,
//! because a login belongs to one site and nothing on this machine says which.
//! The shape is second-hand — taken from CodexBar's Alibaba Token Plan provider
//! and its tests, not from a captured run — and the fixture below says so.
//!
//! **Where the CLI is on Windows.** Both of Alibaba's installers are Windows
//! ones: `npm install -g bailian-cli` writes `bl.cmd` and `bailian.cmd` into
//! `%APPDATA%\npm`, and the binary installer writes `bl.exe` and `bailian.exe`
//! into `%LOCALAPPDATA%\bailian-cli\bin` (a junction onto `current`). `PATH` is
//! searched first, then both of those, and a batch file — which Windows cannot
//! start directly — is run through `cmd.exe /c`.
//!
//! **What is left out.** A Team plan's shared credit pool is read by CodexBar
//! from the console with a browser session, on a site the reader picks; that
//! needs a region choice this port has no control for, so it is not read here.
//! A CLI output with no rolling windows says so as "No limits reported". The
//! monthly window needs a recent `bl`: older ones print only the five-hour and
//! weekly figures, and the monthly row is then absent rather than guessed.

use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use chrono::{DateTime, Utc};
use serde_json::{Map, Value};
use tokio::io::AsyncReadExt;

use super::{
    by_window_length, percent_from_fraction, Ctx, FetchFuture, Provider,
};
use crate::credentials;
use crate::model::{ProviderUsage, UsageWindow};

const ID: &str = "alibaba-token-plan";
const NAME: &str = "Alibaba Token Plan";

/// The two consoles, in the CLI's own words for them.
const SITES: [(&str, &str); 2] = [
    ("international", "ap-southeast-1"),
    ("domestic", "cn-beijing"),
];

/// A CLI that runs past this is stopped, and one that prints more than this has
/// the rest dropped — the original's own two ceilings.
const DEADLINE: Duration = Duration::from_secs(15);
const OUTPUT_CEILING: usize = 256 * 1024;

/// A console window must not flash up in front of the panel.
#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

pub struct AlibabaTokenPlan;

impl Provider for AlibabaTokenPlan {
    fn id(&self) -> &'static str {
        ID
    }

    fn name(&self) -> &'static str {
        NAME
    }

    /// Whether the CLI is on this machine — which is the whole of the
    /// credential question here.
    ///
    /// **Not a key and not a file of PulseWin's.** This provider reads nothing
    /// itself: the login is the CLI's own, and running it is the route. So the
    /// presence question is "is `bl` here", and an install PulseWin cannot find
    /// is named by `PULSEWIN_ALIBABA_TOKEN_PLAN_BIN`. Nothing is sent, and
    /// nothing is read, to answer it.
    fn is_configured(&self) -> bool {
        locate_cli().is_some()
    }

    fn fetch(&self, _ctx: Arc<Ctx>) -> FetchFuture {
        Box::pin(async move { fetch_inner().await })
    }
}

/// The CLI's own words for one site's figures.
fn arguments(site: &str, region: &str) -> Vec<String> {
    [
        "usage",
        "token-plan",
        "--console-region",
        region,
        "--console-site",
        site,
        "--output",
        "json",
    ]
    .iter()
    .map(|argument| argument.to_string())
    .collect()
}

async fn fetch_inner() -> ProviderUsage {
    let Some(binary) = locate_cli() else {
        return ProviderUsage::failed(ID, NAME, Reason::LocalLoginMissing.message());
    };

    let mut first_reason: Option<Reason> = None;

    for (site, region) in SITES {
        let usage = match run(&binary, &arguments(site, region)).await {
            Ok(output) => reading_at(&output, Utc::now()),
            Err(reason) => Err(reason),
        };

        match usage {
            Ok(usage) => return usage,
            Err(reason) => {
                // A CLI that cannot be started will not start for the other
                // site either.
                if reason == Reason::LocalLoginMissing {
                    return ProviderUsage::failed(ID, NAME, reason.message());
                }
                match first_reason {
                    None => first_reason = Some(reason),
                    // The site the login does not belong to refuses it; the one
                    // it does belong to says more, whatever it says.
                    Some(first) => {
                        let reported = if reason == Reason::LocalLoginExpired {
                            first
                        } else {
                            reason
                        };
                        return ProviderUsage::failed(ID, NAME, reported.message());
                    }
                }
            }
        }
    }

    ProviderUsage::failed(
        ID,
        NAME,
        first_reason.unwrap_or(Reason::UnreadableReply).message(),
    )
}

// ---------------------------------------------------------------------------
// The CLI
// ---------------------------------------------------------------------------

/// The reasons a run can leave nothing to read, in the original's own set.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Reason {
    /// The program is not here, or will not start.
    LocalLoginMissing,
    /// It is here, and says its login has gone.
    LocalLoginExpired,
    /// It ran past its time limit.
    Unreachable,
    UnreadableReply,
    ServerError,
    NoLimitsReported,
}

impl Reason {
    /// Every sentence names the CLI, because every remedy here is a command to
    /// run rather than a field to fill in.
    fn message(self) -> &'static str {
        match self {
            Reason::LocalLoginMissing => {
                "the Bailian CLI (bl) was not found — install it and run `bl auth login`"
            }
            Reason::LocalLoginExpired => {
                "the Bailian CLI's saved login has expired — run `bl auth login` again"
            }
            Reason::Unreachable => "the Bailian CLI did not answer in time",
            Reason::UnreadableReply => "unreadable reply",
            Reason::ServerError => "the service returned an error",
            Reason::NoLimitsReported => "no limits reported",
        }
    }
}

/// The names the CLI installs under: the short alias the original runs, then
/// the full one, which both Windows installers write beside it.
const COMMANDS: [&str; 2] = ["bl", "bailian"];

/// The extensions a Windows install gives either name. npm writes `bl.cmd`,
/// the binary installer `bl.exe`; `bailian.ps1` is left out because running one
/// would need a second interpreter between this and the CLI.
#[cfg(windows)]
const EXTENSIONS: [&str; 3] = ["exe", "cmd", "bat"];

/// The names each folder is asked for, in the order they are tried in it.
#[cfg(windows)]
fn names() -> Vec<String> {
    COMMANDS
        .iter()
        .flat_map(|command| EXTENSIONS.iter().map(move |extension| format!("{command}.{extension}")))
        .collect()
}

#[cfg(not(windows))]
fn names() -> Vec<String> {
    COMMANDS.iter().map(|command| command.to_string()).collect()
}

/// The folders the CLI is looked for in: `PATH` first, then the two trees its
/// own installers use, then the folders the original names.
fn candidate_dirs() -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = Vec::new();

    if let Some(path) = std::env::var_os("PATH") {
        dirs.extend(std::env::split_paths(&path));
    }

    // The binary installer: `%LOCALAPPDATA%\bailian-cli\bin\bl.exe`, with
    // `current` beside it in case the junction is not there.
    let local = std::env::var_os("LOCALAPPDATA")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .or_else(dirs::data_local_dir);
    if let Some(local) = local {
        let root = local.join("bailian-cli");
        dirs.push(root.join("bin"));
        dirs.push(root.join("current"));
    }

    // `npm install -g bailian-cli`.
    dirs.extend(credentials::config_relative(&["npm"]));
    dirs.extend(credentials::home_relative(&[".local", "bin"]));
    dirs.extend(credentials::home_relative(&[".bun", "bin"]));
    dirs.retain(|dir| !dir.as_os_str().is_empty());
    dirs
}

/// The first candidate that is a file. The check is injected so a test can
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

/// Where the CLI is, or `None` when it is nowhere this port knows to look.
fn locate_cli() -> Option<PathBuf> {
    // An install PulseWin does not know about is named outright.
    if let Some(named) = credentials::env_override(ID, "bin") {
        let path = credentials::expand_tilde(named.trim());
        return path.is_file().then_some(path);
    }
    locate(&candidate_dirs(), &|path| path.is_file())
}

/// The variables a helper keeps. **An allow-list, not a deny-list**: what is
/// not named here does not cross into it, so no key, cookie or cloud credential
/// of PulseWin's own reaches a program this runs on the reader's behalf.
const KEPT: [&str; 33] = [
    // Where Windows keeps what a program cannot run without.
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
    // The CLI's own home, and the two folders its login lives under. The
    // last two are the CLI's own settings, which are how a reader who moved
    // them is still found.
    "USERPROFILE",
    "HOMEDRIVE",
    "HOMEPATH",
    "APPDATA",
    "LOCALAPPDATA",
    "BAILIAN_CONFIG_DIR",
    "BAILIAN_SHARE_DIR",
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
    "no_proxy",
];

fn kept(name: &OsStr) -> bool {
    let name = name.to_string_lossy();
    !name.is_empty() && KEPT.iter().any(|kept| name.eq_ignore_ascii_case(kept))
}

/// Only what the CLI needs to find itself, its login and the network, with the
/// folder it was found in at the front of `PATH`.
///
/// The leading folder is what the original does for every helper it starts: an
/// npm install is a script whose interpreter sits beside it, and a GUI app's
/// `PATH` is the one Windows booted with.
fn environment(binary: &Path) -> Vec<(OsString, OsString)> {
    let mut environment: Vec<(OsString, OsString)> = std::env::vars_os()
        .filter(|(name, _)| kept(name))
        .collect();

    let mut dirs: Vec<PathBuf> = Vec::new();
    if let Some(folder) = binary.parent() {
        dirs.push(folder.to_path_buf());
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

/// The command that runs `binary`, wrapped in `cmd.exe` when it is a batch file
/// — which `CreateProcess` cannot start on its own, and which is exactly what
/// an npm-installed `bl.cmd` is.
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
        .env_clear()
        .envs(environment(binary))
        // Nothing to answer with, so a CLI that asks gets EOF rather than
        // blocking on a terminal that is not there.
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        // A run that is dropped — the deadline below — takes the child with it,
        // so a CLI that ignores everything does not stay running.
        .kill_on_drop(true);

    #[cfg(windows)]
    command.creation_flags(CREATE_NO_WINDOW);

    command
}

/// The CLI's stdout, or why there is none.
async fn run(binary: &Path, arguments: &[String]) -> Result<Vec<u8>, Reason> {
    let child = match command_for(binary).args(arguments).spawn() {
        Ok(child) => child,
        Err(_) => return Err(Reason::LocalLoginMissing),
    };

    match tokio::time::timeout(DEADLINE, collect(child)).await {
        Ok(collected) if collected.success => Ok(collected.stdout),
        Ok(collected) => Err(reason_for_exit(&collected.stderr)),
        Err(_) => Err(Reason::Unreachable),
    }
}

struct Collected {
    stdout: Vec<u8>,
    stderr: String,
    success: bool,
}

/// Drains both pipes at once, and then waits for the process.
///
/// **Both at once.** Reading one to EOF and only then reading the other
/// deadlocks the moment the child writes more than a pipe buffer to the one
/// nobody is reading — a debug build, a TLS dump — and past the output ceiling
/// the bytes are dropped but the pipe is still drained, because a reader that
/// walks away is the same deadlock wearing a different hat.
async fn collect(mut child: tokio::process::Child) -> Collected {
    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    let mut out_pipe = child.stdout.take();
    let mut err_pipe = child.stderr.take();

    tokio::join!(
        drain(&mut out_pipe, &mut stdout),
        drain(&mut err_pipe, &mut stderr),
    );

    let success = child
        .wait()
        .await
        .map(|status| status.success())
        .unwrap_or(false);

    Collected {
        stdout,
        stderr: String::from_utf8_lossy(&stderr).into_owned(),
        success,
    }
}

async fn drain<R>(pipe: &mut Option<R>, into: &mut Vec<u8>)
where
    R: tokio::io::AsyncRead + Unpin,
{
    let Some(pipe) = pipe.as_mut() else {
        return;
    };

    let mut chunk = [0u8; 8 * 1024];
    loop {
        match pipe.read(&mut chunk).await {
            Ok(0) | Err(_) => return,
            Ok(read) => {
                if into.len() < OUTPUT_CEILING {
                    into.extend_from_slice(&chunk[..read]);
                }
            }
        }
    }
}

/// Why the CLI stopped, as far as its own words say. Matched on sign-in phrases
/// only: a CLI too old for the command prints its usage and exits non-zero too,
/// and telling that reader to sign in again would be a remedy that changes
/// nothing.
fn reason_for_exit(stderr: &str) -> Reason {
    const PHRASES: [&str; 12] = [
        "not signed in",
        "not logged in",
        "unauthorized",
        "unauthenticated",
        "login required",
        "please login",
        "please log in",
        "bl login",
        "token expired",
        "session expired",
        "credentials expired",
        "needlogin",
    ];

    let said = stderr.to_lowercase();
    if PHRASES.iter().any(|phrase| said.contains(phrase)) {
        Reason::LocalLoginExpired
    } else {
        Reason::UnreadableReply
    }
}

// ---------------------------------------------------------------------------
// Reading the output
// ---------------------------------------------------------------------------

fn reading_at(bytes: &[u8], now: DateTime<Utc>) -> Result<ProviderUsage, Reason> {
    let Ok(raw) = serde_json::from_slice::<Value>(bytes) else {
        return Err(Reason::UnreadableReply);
    };
    let tree = super::alibaba_coding_plan::console::expanded(&raw);
    let Some(root) = tree.as_object() else {
        return Err(Reason::UnreadableReply);
    };

    let windows = rolling_windows(root, Some(1.0), now);
    if !windows.is_empty() {
        return Ok(ProviderUsage::ok(ID, NAME, windows));
    }

    match super::alibaba_coding_plan::console::failure(root) {
        Some(super::alibaba_coding_plan::console::Failure::SignedOut) => {
            Err(Reason::LocalLoginExpired)
        }
        Some(super::alibaba_coding_plan::console::Failure::Failed) => Err(Reason::ServerError),
        None => Err(Reason::NoLimitsReported),
    }
}

/// One of the rolling windows of a Token Plan's personal usage, which the CLI
/// prints and Qwen Cloud's console returns in the same words.
struct Rolling {
    key: &'static str,
    reset: &'static str,
    label: &'static str,
    seconds: i64,
}

/// A share used, as a ratio, and a reset in epoch milliseconds, per window. A
/// month is a billing month, so its length is a sort key and nothing more.
const ROLLING: [Rolling; 3] = [
    Rolling {
        key: "per5HourPercentage",
        reset: "per5HourResetTime",
        label: "5h",
        seconds: 5 * 3_600,
    },
    Rolling {
        key: "per1WeekPercentage",
        reset: "per1WeekResetTime",
        label: "7d",
        seconds: 7 * 86_400,
    },
    Rolling {
        key: "per1MonthPercentage",
        reset: "per1MonthResetTime",
        label: "Monthly",
        seconds: 30 * 86_400,
    },
];

/// The rolling windows of a Token Plan's personal usage.
///
/// A negative figure is never read. `ceiling` is where the CLI's are held to 1:
/// its unit is not documented, and a figure past a ratio's range may be a
/// percentage — left off rather than misread a hundredfold.
///
/// Shared with Qwen Cloud, whose console answers in the same words, as the
/// original shares them.
pub(super) fn rolling_windows(
    tree: &Map<String, Value>,
    ceiling: Option<f64>,
    now: DateTime<Utc>,
) -> Vec<UsageWindow> {
    let Some(usage) = super::alibaba_coding_plan::console::first_object_in(tree, true, &|object| {
        ROLLING.iter().any(|window| object.contains_key(window.key))
    }) else {
        return Vec::new();
    };

    let mut rows: Vec<(i64, UsageWindow)> = Vec::new();
    for window in &ROLLING {
        let Some(ratio) =
            super::alibaba_coding_plan::console::number(usage.get(window.key))
        else {
            continue;
        };
        if ratio < 0.0 {
            continue;
        }
        if let Some(ceiling) = ceiling {
            if ratio > ceiling {
                continue;
            }
        }

        // Epoch milliseconds only: a reset in the past is a stale figure, not a
        // new window, and is left off rather than moved forward.
        let reset = super::alibaba_coding_plan::console::number(usage.get(window.reset))
            .and_then(|milliseconds| DateTime::from_timestamp_millis(milliseconds as i64))
            .filter(|at| *at > now);

        rows.push((
            window.seconds,
            UsageWindow::new(window.label, Some(percent_from_fraction(ratio)))
                .with_reset(reset.map(stamp)),
        ));
    }

    by_window_length(rows)
}

fn stamp(at: DateTime<Utc>) -> String {
    at.to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn at(text: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(text).unwrap().with_timezone(&Utc)
    }

    fn as_map(value: Value) -> Map<String, Value> {
        match value {
            Value::Object(map) => map,
            other => panic!("expected an object, got {other}"),
        }
    }

    /// The shape the CLI prints, as CodexBar's tests describe it.
    fn fixture() -> Value {
        json!({
            "code": "success",
            "data": {
                "per5HourPercentage": 0.42,
                "per5HourResetTime": 1_800_000_000_000i64,
                "per1WeekPercentage": "0.25",
                "per1WeekResetTime": 1_800_000_000_000i64,
                "per1MonthPercentage": 1.5,
                "per1MonthResetTime": 1_800_000_000_000i64
            }
        })
    }

    #[test]
    fn runs_the_cli_in_its_own_words() {
        assert_eq!(
            arguments("international", "ap-southeast-1"),
            vec![
                "usage",
                "token-plan",
                "--console-region",
                "ap-southeast-1",
                "--console-site",
                "international",
                "--output",
                "json"
            ]
        );
        assert_eq!(SITES[0], ("international", "ap-southeast-1"));
        assert_eq!(SITES[1], ("domestic", "cn-beijing"));
    }

    #[test]
    fn reads_the_three_rolling_windows() {
        let windows = rolling_windows(&as_map(fixture()), Some(1.0), at("2026-10-02T00:00:00Z"));

        // The month's figure is past a ratio's range, and is left off rather
        // than misread a hundredfold.
        let labels: Vec<&str> = windows.iter().map(|w| w.label.as_str()).collect();
        assert_eq!(labels, vec!["5h", "7d"]);
        assert_eq!(windows[0].percent_used, Some(42.0));
        // A share that arrives as a string is a share.
        assert_eq!(windows[1].percent_used, Some(25.0));
        assert_eq!(
            windows[0].resets_at.as_deref(),
            Some("2027-01-15T08:00:00Z")
        );
    }

    #[test]
    fn a_figure_out_of_range_and_one_below_zero_are_both_left_off() {
        let out_of_range = json!({
            "per5HourPercentage": 1.5,
            "per1WeekPercentage": -0.2,
            "per1MonthPercentage": 0.5
        });
        let windows = rolling_windows(
            &as_map(out_of_range),
            Some(1.0),
            at("2026-10-02T00:00:00Z"),
        );
        let labels: Vec<&str> = windows.iter().map(|w| w.label.as_str()).collect();
        assert_eq!(labels, vec!["Monthly"]);

        // The same reply read without the ceiling the CLI is held to.
        let windows = rolling_windows(
            &as_map(json!({ "per5HourPercentage": 1.5 })),
            None,
            at("2026-10-02T00:00:00Z"),
        );
        assert_eq!(windows[0].percent_used, Some(100.0));
    }

    #[test]
    fn a_reset_in_the_past_is_dropped_rather_than_moved_forward() {
        let stale = json!({
            "per5HourPercentage": 0.5,
            "per5HourResetTime": 1_700_000_000_000i64
        });
        let windows = rolling_windows(
            &as_map(stale),
            Some(1.0),
            at("2026-10-02T00:00:00Z"),
        );
        assert_eq!(windows[0].percent_used, Some(50.0));
        assert!(windows[0].resets_at.is_none());
    }

    #[test]
    fn an_output_with_no_rolling_windows_says_so() {
        assert_eq!(
            reading_at(br#"{"code":"success"}"#, Utc::now()).unwrap_err(),
            Reason::NoLimitsReported
        );
        // Not JSON at all, and JSON that is not an object.
        assert_eq!(
            reading_at(b"", Utc::now()).unwrap_err(),
            Reason::UnreadableReply
        );
        assert_eq!(
            reading_at(b"[1, 2]", Utc::now()).unwrap_err(),
            Reason::UnreadableReply
        );
    }

    #[test]
    fn a_cli_that_says_it_is_signed_out_is_a_lapsed_login() {
        assert_eq!(
            reading_at(br#"{"code":"ConsoleNeedLogin"}"#, Utc::now()).unwrap_err(),
            Reason::LocalLoginExpired
        );
        assert_eq!(
            reading_at(br#"{"statusCode":500,"message":"boom"}"#, Utc::now()).unwrap_err(),
            Reason::ServerError
        );
    }

    #[test]
    fn a_reading_with_figures_is_not_read_from_the_frame_around_them() {
        let usage = reading_at(
            fixture().to_string().as_bytes(),
            at("2026-10-02T00:00:00Z"),
        )
        .expect("a reading");
        assert_eq!(usage.windows.len(), 2);
        assert!(usage.error.is_none());
    }

    #[test]
    fn only_a_sign_in_phrase_is_a_lapsed_login() {
        // A CLI too old for the command prints its usage and exits non-zero;
        // telling that reader to sign in again would change nothing.
        assert_eq!(
            reason_for_exit("error: unknown command 'token-plan'\nUsage: bl usage ..."),
            Reason::UnreadableReply
        );
        assert_eq!(
            reason_for_exit("Error: not signed in. Run `bl auth login`."),
            Reason::LocalLoginExpired
        );
        assert_eq!(
            reason_for_exit("TOKEN EXPIRED"),
            Reason::LocalLoginExpired
        );
        assert_eq!(reason_for_exit(""), Reason::UnreadableReply);
    }

    #[test]
    fn the_environment_a_helper_keeps_is_an_allow_list() {
        // What PulseWin holds for other providers never crosses.
        assert!(!kept(OsStr::new("PULSEWIN_CURSOR_COOKIE")));
        assert!(!kept(OsStr::new("PULSEWIN_ALIBABA_TOKEN_PLAN_BIN")));
        assert!(!kept(OsStr::new("DASHSCOPE_API_KEY")));
        assert!(!kept(OsStr::new("ALIBABA_CLOUD_ACCESS_KEY_SECRET")));
        assert!(!kept(OsStr::new("OPENAI_API_KEY")));
        assert!(!kept(OsStr::new("")));

        // Where Windows keeps what a program cannot run without, the CLI's own
        // home, and the proxy.
        assert!(kept(OsStr::new("SystemRoot")));
        assert!(kept(OsStr::new("USERPROFILE")));
        assert!(kept(OsStr::new("LOCALAPPDATA")));
        assert!(kept(OsStr::new("BAILIAN_CONFIG_DIR")));
        assert!(kept(OsStr::new("HTTPS_PROXY")));
        assert!(kept(OsStr::new("https_proxy")));
        // `PATH` is not inherited but rebuilt, so it is not on the list.
        assert!(!kept(OsStr::new("PATH")));
    }

    #[test]
    fn the_cli_is_looked_for_in_the_order_the_folders_are_given() {
        let dirs = vec![PathBuf::from("C:\\first"), PathBuf::from("C:\\second")];
        let wanted = dirs[1].join(names().first().expect("a name to look for"));
        let found = locate(&dirs, &|path| path == wanted).unwrap();
        assert_eq!(found, wanted);

        // A second name in the same folder, and nothing at all.
        let second = dirs[0].join(names().get(1).expect("a second name"));
        assert_eq!(locate(&dirs, &|path| path == second).unwrap(), second);
        assert!(locate(&dirs, &|_| false).is_none());

        // Every folder is asked for every name before the next folder is.
        let asked = std::cell::RefCell::new(Vec::new());
        let _ = locate(&dirs, &|path| {
            asked.borrow_mut().push(path.to_path_buf());
            false
        });
        let asked = asked.into_inner();
        let reached_second = asked
            .iter()
            .position(|path| path.starts_with(&dirs[1]))
            .expect("the second folder is asked");
        assert_eq!(reached_second, names().len());
    }

    #[test]
    fn a_batch_file_is_run_through_the_command_interpreter() {
        // npm's shim cannot be started by `CreateProcess` directly.
        let batch = command_for(Path::new("C:\\Users\\me\\AppData\\Roaming\\npm\\bl.cmd"));
        if cfg!(windows) {
            assert_eq!(batch.as_std().get_program(), OsStr::new("cmd.exe"));
        }

        let binary = command_for(Path::new(
            "C:\\Users\\me\\AppData\\Local\\bailian-cli\\bin\\bl.exe",
        ));
        assert_eq!(
            binary.as_std().get_program(),
            OsStr::new("C:\\Users\\me\\AppData\\Local\\bailian-cli\\bin\\bl.exe")
        );
    }

    #[test]
    fn every_reason_names_the_command_to_run() {
        assert!(Reason::LocalLoginMissing.message().contains("`bl auth login`"));
        assert!(Reason::LocalLoginExpired.message().contains("`bl auth login`"));
        // A run that never answered names the program rather than a command.
        assert!(Reason::Unreachable.message().contains("Bailian CLI"));
        assert_eq!(Reason::UnreadableReply.message(), "unreadable reply");
        assert_eq!(Reason::NoLimitsReported.message(), "no limits reported");
    }
}
