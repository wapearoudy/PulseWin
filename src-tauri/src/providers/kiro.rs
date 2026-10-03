//! Kiro's subscription credits, read through Kiro CLI's native ACP process.
//!
//! **The credential is Kiro's own and never leaves it.** There is no key here
//! and no token file: PulseWin starts `kiro-cli`, completes the Agent Client
//! Protocol handshake, asks for the account's usage, and tears the helper down.
//! Kiro owns authentication and refresh — which is what the original means by
//! "uses Kiro CLI's signed-in session without reading its credentials", and
//! what decides this provider's `is_configured`: the CLI being here is the
//! whole of the credential question.
//!
//! **Where the CLI is on Windows.** Kiro ships a Windows CLI
//! (<https://kiro.dev/changelog/cli/2-0/>), and its two installs are `PATH` and
//! the folder the official MSI writes: `%LOCALAPPDATA%\Kiro-Cli\kiro-cli.exe`
//! sits beside the CLI's own session database (`data.sqlite3`), and
//! `C:\Program Files\Kiro-Cli\kiro-cli.exe` is the machine-wide one. `PATH` is
//! searched first, then both of those, and a batch file — which Windows cannot
//! start directly — is run through `cmd.exe /c`.
//!
//! **The exchange is two requests, in this order.** Kiro installs its auth
//! connection while handling `initialize`, so asking for usage before that
//! reply arrives races with it. Messages are newline-delimited JSON on the
//! child's stdin and stdout; stderr is drained while the exchange runs, because
//! a helper that writes more than a pipe buffer to a reader nobody is holding
//! parks for ever — the deadlock `alibaba_token_plan` documents at length.
//!
//! **What is left out.** The original gives each pool an id built from its
//! resource type and a repeat counter, so a saved pin or an alert record keeps
//! pointing at the same allowance when the CLI reorders its pools. This port's
//! `UsageWindow` has no id — nothing here pins or alerts by one — so the pool's
//! own name is carried on the row's detail line instead and the counter has
//! nowhere to go. The arithmetic, the labels, the reset and the reason set are
//! the original's.
//!
//! VERIFY ON A REAL MACHINE: `_kiro/account/getUsage` is Kiro's own internal
//! ACP method, undocumented outside its CLI, and no machine here has `kiro-cli`
//! installed. Set `PULSEWIN_DEBUG=1` to dump the reply.

use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, Lines};
use tokio::process::{ChildStdin, ChildStdout};

use super::{by_window_length, percent_from_fraction, Ctx, FetchFuture, Provider};
use crate::credentials;
use crate::model::{ProviderUsage, UsageWindow};

const ID: &str = "kiro";
const NAME: &str = "Kiro";

/// The usage method Kiro's own ACP server answers.
const USAGE_METHOD: &str = "_kiro/account/getUsage";

/// The original gives each request twenty seconds. Here that is spent on the
/// **whole exchange**, not on each of its two halves: a tray refresh that waits
/// forty seconds for one provider has stopped being a tray refresh.
const DEADLINE: Duration = Duration::from_secs(20);

/// A console window must not flash up in front of the panel.
#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

pub struct Kiro;

impl Provider for Kiro {
    fn id(&self) -> &'static str {
        ID
    }

    fn name(&self) -> &'static str {
        NAME
    }

    /// Whether the CLI is on this machine — which is the whole of the
    /// credential question here, exactly as it is for the Bailian CLI.
    ///
    /// **Not a key and not a file of PulseWin's.** This provider reads no
    /// credential itself: the login is Kiro's, and running the CLI is the
    /// route. So the presence question is "is `kiro-cli` here", and an install
    /// PulseWin cannot find is named by `PULSEWIN_KIRO_BIN`. Nothing is sent,
    /// and nothing is read, to answer it.
    ///
    /// **The folder the CLI keeps its own state in counts too.** The original's
    /// discovery list names `~/.kiro` for Kiro, and it is there for a reason
    /// worth keeping: a machine where `kiro-cli` was installed somewhere this
    /// port does not look still has that folder, and drawing the card then is
    /// what lets it say *which* command to run rather than leaving the reader
    /// with no ring and no reason.
    fn is_configured(&self) -> bool {
        locate_cli().is_some() || state_folder().is_some()
    }

    fn fetch(&self, _ctx: Arc<Ctx>) -> FetchFuture {
        Box::pin(async move { fetch_inner().await })
    }
}

async fn fetch_inner() -> ProviderUsage {
    let result = match usage().await {
        Ok(result) => result,
        Err(failure) => {
            return ProviderUsage::failed(ID, NAME, reason_for_failure(&failure).message())
        }
    };

    super::claude_code::debug_dump(ID, &result);

    reading(&result)
}

// ---------------------------------------------------------------------------
// The CLI
// ---------------------------------------------------------------------------

/// The reasons a run can leave nothing to read, in the original's own set.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Reason {
    /// `kiro-cli` is absent, or not where this port knows to look.
    NotInstalled,
    /// It is here and too old for the native usage method.
    VersionUnsupported,
    /// It is here and has no session.
    SignInRequired,
    /// It is here and did not answer.
    Unreachable,
    UnreadableReply,
    NoLimitsReported,
}

impl Reason {
    /// The original's own copy, sentence for sentence.
    fn message(self) -> &'static str {
        match self {
            Reason::NotInstalled => "Kiro CLI isn't installed.",
            Reason::VersionUnsupported => "Update Kiro CLI to read subscription usage.",
            Reason::SignInRequired => "Sign in to Kiro CLI to see usage.",
            Reason::Unreachable => "The service didn't respond.",
            Reason::UnreadableReply => "Couldn't read the reply.",
            Reason::NoLimitsReported => "No limits reported.",
        }
    }
}

/// The four ways the exchange itself fails, as the original names them.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Failure {
    ExecutableNotFound,
    StartFailed,
    TimedOut,
    Closed,
    /// The server answered with a JSON-RPC error, or a message of its own.
    Server(String),
}

fn reason_for_failure(failure: &Failure) -> Reason {
    match failure {
        Failure::ExecutableNotFound => Reason::NotInstalled,
        Failure::Server(message) => reason_for_message(Some(message)),
        Failure::StartFailed | Failure::TimedOut | Failure::Closed => Reason::Unreachable,
    }
}

/// What a server message means.
///
/// **A sign-in phrase first and a version phrase second**, which is the
/// original's order: a CLI that says both — "unsupported: sign in again" — is
/// one the reader can fix by signing in, and telling them to update would send
/// them to a download page that changes nothing.
fn reason_for_message(message: Option<&str>) -> Reason {
    let text = message.unwrap_or("").to_lowercase();

    const SIGN_IN: [&str; 3] = ["sign in", "not authenticated", "login"];
    if SIGN_IN.iter().any(|phrase| text.contains(phrase)) {
        return Reason::SignInRequired;
    }

    const UNSUPPORTED: [&str; 3] = ["method not found", "unsupported", "agent-engine"];
    if UNSUPPORTED.iter().any(|phrase| text.contains(phrase)) {
        return Reason::VersionUnsupported;
    }

    Reason::UnreadableReply
}

// ---------------------------------------------------------------------------
// The reply
// ---------------------------------------------------------------------------

/// The mapping, kept apart from the process so a fixture can drive it.
///
/// `result` is the ACP **result object** of `_kiro/account/getUsage` — the
/// `result` member of the JSON-RPC reply, not the whole reply.
fn reading(result: &Value) -> ProviderUsage {
    // The original's decoder requires `success` to be there and to be a bool;
    // a reply without it is one this build cannot read rather than one that
    // says "no".
    let Some(success) = result.get("success").and_then(Value::as_bool) else {
        return ProviderUsage::failed(ID, NAME, Reason::UnreadableReply.message());
    };
    if !success {
        let message = result.get("message").and_then(Value::as_str);
        return ProviderUsage::failed(ID, NAME, reason_for_message(message).message());
    }

    let Some(data) = result.get("data").and_then(Value::as_object) else {
        return ProviderUsage::failed(ID, NAME, Reason::NoLimitsReported.message());
    };
    // `usageBreakdowns` is not optional in the original's decoder: a payload
    // object without it is unreadable rather than empty.
    let Some(breakdowns) = data.get("usageBreakdowns").and_then(Value::as_array) else {
        return ProviderUsage::failed(ID, NAME, Reason::UnreadableReply.message());
    };

    let windows = windows_from(
        breakdowns,
        data.get("billingCycleReset").and_then(Value::as_str),
    );
    if windows.is_empty() {
        return ProviderUsage::failed(ID, NAME, Reason::NoLimitsReported.message());
    }

    let plan = data
        .get("planName")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|plan| !plan.is_empty())
        .map(str::to_string);

    ProviderUsage::ok(ID, NAME, windows).with_plan(plan)
}

/// Every pool Kiro reports is a monthly allowance.
///
/// The payload states no window length at all: the original names the kind
/// `.monthly` outright and marks `reportsLength: false`, so the thirty days is
/// a sort key and never a length anything is divided by. Nothing here divides
/// by it either.
const WINDOW_SECONDS: i64 = 30 * 86_400;

fn windows_from(breakdowns: &[Value], billing_cycle_reset: Option<&str>) -> Vec<UsageWindow> {
    let reset = billing_cycle_reset.and_then(billing_reset);
    let rows: Vec<(i64, UsageWindow)> = breakdowns
        .iter()
        .filter_map(|item| window_from(item, reset.clone()))
        .map(|window| (WINDOW_SECONDS, window))
        .collect();

    // Every pool is the same length, so this is the port's own shortest-first
    // rule doing nothing but holding the reply's order — the sort is stable,
    // and the original draws these in the order the CLI sent them.
    by_window_length(rows)
}

fn window_from(item: &Value, reset: Option<String>) -> Option<UsageWindow> {
    // `hasLimit` absent means "there is one": only an explicit false says the
    // pool has no ceiling, and a pool with no ceiling has no denominator.
    if item.get("hasLimit").and_then(Value::as_bool) == Some(false) {
        return None;
    }

    let limit = number(item.get("limit")).filter(|limit| limit.is_finite() && *limit > 0.0)?;

    // What is spent, or the share the service states instead — the CLI does
    // not always send both, and a pool with neither cannot be drawn.
    let used = match number(item.get("used")).filter(|used| used.is_finite()) {
        Some(used) => used,
        None => number(item.get("percentage")).filter(|pct| pct.is_finite())? / 100.0 * limit,
    };

    // The pool's name, which the panel shows after the window's own. The
    // resource type is the provider-owned identity and the display name is
    // the words for it; either will do, and neither is required.
    let scope = text(item.get("displayName")).or_else(|| text(item.get("resourceType")));

    Some(
        UsageWindow::new("Monthly", Some(percent_from_fraction((used / limit).clamp(0.0, 1.0))))
            .with_reset(reset)
            .with_detail(scope),
    )
}

/// A figure Kiro sends as a number. A word is not a figure.
fn number(value: Option<&Value>) -> Option<f64> {
    value.and_then(Value::as_f64)
}

/// A field that may be absent, blank or whitespace — and is none of those
/// things when the original's own `stableIDComponent` would have taken it.
fn text(value: Option<&Value>) -> Option<String> {
    value
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|text| !text.is_empty())
        .map(str::to_string)
}

/// `billingCycleReset` is a bare date, and the original reads it at midnight
/// UTC with a POSIX formatter pinned to `en_US_POSIX` and GMT.
fn billing_reset(text: &str) -> Option<String> {
    let date = chrono::NaiveDate::parse_from_str(text.trim(), "%Y-%m-%d").ok()?;
    Some(
        date.and_hms_opt(0, 0, 0)?
            .and_utc()
            .to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
    )
}

// ---------------------------------------------------------------------------
// The exchange
// ---------------------------------------------------------------------------

/// The CLI's own words for the handshake the original performs.
fn arguments() -> Vec<String> {
    ["acp", "--agent-engine", "v3", "--auth-method", "cli"]
        .iter()
        .map(|argument| argument.to_string())
        .collect()
}

/// The `initialize` parameters.
///
/// The original announces itself as `Pulse`/`0.1`. This port announces itself
/// instead: it is a different program, and claiming another one's name to a
/// server that may one day gate on it is the kind of thing that is only ever
/// discovered after it has broken.
fn initialize_params() -> Value {
    json!({
        "protocolVersion": 1,
        "clientCapabilities": {},
        "clientInfo": { "name": "PulseWin", "version": env!("CARGO_PKG_VERSION") }
    })
}

/// One JSON-RPC request as the newline-delimited line ACP is written in.
fn request_line(id: u64, method: &str, params: &Value) -> Option<String> {
    serde_json::to_string(&json!({
        "jsonrpc": "2.0",
        "id": id,
        "method": method,
        "params": params,
    }))
    .ok()
}

/// What one line off the helper's stdout means for the request `id` waits on.
#[derive(Debug, PartialEq)]
enum Reply {
    Result(Value),
    /// A JSON-RPC error, carrying the server's own words.
    Failure(String),
    /// A line for somebody else: a notification, another request's reply, or
    /// something that is not JSON at all. ACP servers emit all three.
    Ignore,
}

fn reply_for(line: &str, id: u64) -> Reply {
    let Ok(message) = serde_json::from_str::<Value>(line) else {
        return Reply::Ignore;
    };
    if message.get("id").and_then(Value::as_u64) != Some(id) {
        return Reply::Ignore;
    }
    if let Some(error) = message.get("error") {
        let said = error
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or("unknown");
        return Reply::Failure(said.to_string());
    }
    Reply::Result(message.get("result").cloned().unwrap_or(Value::Null))
}

/// A live ACP helper: its pipes, and the id counter.
struct Helper {
    /// Held for no other reason than that the process lives exactly as long as
    /// this does: `kill_on_drop` fires when the `Child` is dropped, and a
    /// `Child` that was never stored would be dropped the moment it was
    /// started.
    _child: tokio::process::Child,
    stdin: ChildStdin,
    lines: Lines<BufReader<ChildStdout>>,
    pump: Option<tokio::task::JoinHandle<()>>,
    next_id: u64,
}

impl Helper {
    /// Send one request and wait for the reply that carries its id.
    async fn request(&mut self, method: &str, params: &Value) -> Result<Value, Failure> {
        let id = self.next_id;
        self.next_id += 1;

        let Some(line) = request_line(id, method, params) else {
            return Err(Failure::StartFailed);
        };
        for part in [line.as_bytes(), b"\n".as_slice()] {
            self.stdin.write_all(part).await.map_err(|_| Failure::Closed)?;
        }
        self.stdin.flush().await.map_err(|_| Failure::Closed)?;

        loop {
            match self.lines.next_line().await {
                Ok(Some(line)) => match reply_for(&line, id) {
                    Reply::Result(result) => return Ok(result),
                    Reply::Failure(message) => return Err(Failure::Server(message)),
                    Reply::Ignore => continue,
                },
                // A helper whose stdout has closed will not answer, and one
                // whose read failed will not either.
                Ok(None) | Err(_) => return Err(Failure::Closed),
            }
        }
    }

    /// Stop draining. The child goes with the `Helper`, `kill_on_drop` and all.
    fn shut_down(&mut self) {
        if let Some(pump) = self.pump.take() {
            pump.abort();
        }
    }
}

/// stderr drained as it arrives and **thrown away**.
///
/// The discarding is the point, not a shortcut: a helper that writes more than
/// a pipe buffer to a reader nobody is holding parks for ever, and the request
/// waiting on it waits for ever with it. This is what the original's
/// `drain(_:)` does, and it keeps its words for nothing — a helper's stderr is
/// not part of any of the reasons this provider reports.
fn pump_stderr(pipe: Option<tokio::process::ChildStderr>) -> Option<tokio::task::JoinHandle<()>> {
    let mut pipe = pipe?;

    Some(tokio::spawn(async move {
        use tokio::io::AsyncReadExt;

        let mut chunk = [0u8; 8 * 1024];
        loop {
            match pipe.read(&mut chunk).await {
                Ok(0) | Err(_) => return,
                // Past the ceiling the bytes are dropped, never the reading.
                Ok(_) => continue,
            }
        }
    }))
}

/// The whole exchange: start the CLI, handshake, ask for usage, tear it down.
async fn exchange(binary: &Path) -> Result<Value, Failure> {
    let mut command = command_for(binary);
    command
        .args(arguments())
        // **Piped, not null.** Unlike every other helper this port runs, this
        // one is a conversation: ACP is written to its stdin.
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    let mut child = command.spawn().map_err(|_| Failure::StartFailed)?;
    let Some(stdin) = child.stdin.take() else {
        return Err(Failure::StartFailed);
    };
    let Some(stdout) = child.stdout.take() else {
        return Err(Failure::StartFailed);
    };
    let pump = pump_stderr(child.stderr.take());

    let mut helper = Helper {
        _child: child,
        stdin,
        lines: BufReader::new(stdout).lines(),
        pump,
        next_id: 1,
    };

    let initialized = helper.request("initialize", &initialize_params()).await;
    let outcome = match initialized {
        Ok(_) => helper.request(USAGE_METHOD, &json!({})).await,
        Err(failure) => Err(failure),
    };

    helper.shut_down();
    outcome
}

/// The usage reply, or why there is none.
async fn usage() -> Result<Value, Failure> {
    let Some(binary) = locate_cli() else {
        return Err(Failure::ExecutableNotFound);
    };

    match tokio::time::timeout(DEADLINE, exchange(&binary)).await {
        Ok(outcome) => outcome,
        // The helper is dropped with the future that was waiting on it, and
        // `kill_on_drop` takes the child with it.
        Err(_) => Err(Failure::TimedOut),
    }
}

// ---------------------------------------------------------------------------
// Finding the CLI
// ---------------------------------------------------------------------------

/// The name the CLI installs under on both platforms.
const COMMANDS: [&str; 1] = ["kiro-cli"];

/// The extensions a Windows install gives it — both are `kiro-cli.exe` on the
/// two documented installs, but an npm-style shim would be a `.cmd` and
/// `CreateProcess` cannot start one directly.
#[cfg(windows)]
const EXTENSIONS: [&str; 3] = ["exe", "cmd", "bat"];

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
/// own installers use.
fn candidate_dirs() -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = Vec::new();

    if let Some(path) = std::env::var_os("PATH") {
        dirs.extend(std::env::split_paths(&path));
    }

    // The per-user install: `%LOCALAPPDATA%\Kiro-Cli\kiro-cli.exe`, beside the
    // CLI's own session database.
    let local = std::env::var_os("LOCALAPPDATA")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .or_else(dirs::data_local_dir);
    if let Some(local) = local {
        dirs.push(local.join("Kiro-Cli"));
    }

    // The machine-wide MSI install.
    if let Some(program_files) = std::env::var_os("ProgramFiles").filter(|value| !value.is_empty()) {
        dirs.push(PathBuf::from(program_files).join("Kiro-Cli"));
    }

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

/// The folder `kiro-cli` keeps its own state in, which the original's
/// discovery list names as `~/.kiro`.
///
/// A presence hint and never permission to read: nothing here opens it, and
/// the fetch reads nothing out of it — Kiro's login is Kiro's, and the CLI is
/// what this provider asks.
fn state_folder() -> Option<PathBuf> {
    credentials::home_relative(&[".kiro"])
        .into_iter()
        .find(|path| path.is_dir())
}

/// The variables a helper keeps. **An allow-list, not a deny-list**: what is
/// not named here does not cross into it, so no key, cookie or cloud credential
/// of PulseWin's own reaches a program this runs on the reader's behalf.
const KEPT: [&str; 31] = [
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
    // The CLI's own home. Kiro keeps its session under `%LOCALAPPDATA%`, so
    // that folder is not a convenience here — it is where the login lives.
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

    environment
}

/// The command that runs `binary`, wrapped in `cmd.exe` when it is a batch
/// file, which `CreateProcess` cannot start on its own.
///
/// stdin is left for the caller: the exchange pipes it.
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
        // A run that is dropped — the deadline above — takes the child with it,
        // so a CLI that ignores everything does not stay running.
        .kill_on_drop(true);

    #[cfg(windows)]
    command.creation_flags(CREATE_NO_WINDOW);

    command
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The shape the ACP server answers `_kiro/account/getUsage` with.
    fn fixture() -> Value {
        json!({
            "success": true,
            "data": {
                "planName": "Kiro Pro",
                "billingCycleReset": "2026-11-01",
                "usageBreakdowns": [
                    { "resourceType": "AGENTIC_REQUEST", "displayName": "Vibe Requests",
                      "used": 120.0, "limit": 500.0, "hasLimit": true },
                    { "resourceType": "CREDIT", "displayName": "Credits",
                      "used": 0.0, "limit": 1000.0 }
                ]
            }
        })
    }

    #[test]
    fn runs_the_cli_in_its_own_words() {
        assert_eq!(
            arguments(),
            vec!["acp", "--agent-engine", "v3", "--auth-method", "cli"]
        );
        assert_eq!(USAGE_METHOD, "_kiro/account/getUsage");
    }

    /// The handshake the original performs, protocol version and all.
    #[test]
    fn the_handshake_is_the_one_the_original_sends() {
        let params = initialize_params();
        assert_eq!(params["protocolVersion"], json!(1));
        assert_eq!(params["clientCapabilities"], json!({}));
        assert!(params["clientInfo"]["name"].is_string());
        assert!(params["clientInfo"]["version"].is_string());
    }

    #[test]
    fn reads_the_breakdowns_the_reset_and_the_plan() {
        let usage = reading(&fixture());
        assert!(usage.error.is_none());
        assert_eq!(usage.plan.as_deref(), Some("Kiro Pro"));
        assert_eq!(usage.windows.len(), 2);

        // Every pool is a monthly allowance, and the pool's own name rides
        // along on the detail line.
        assert_eq!(usage.windows[0].label, "Monthly");
        assert_eq!(usage.windows[0].percent_used, Some(24.0));
        assert_eq!(usage.windows[0].detail.as_deref(), Some("Vibe Requests"));
        assert_eq!(usage.windows[1].percent_used, Some(0.0));
        assert_eq!(usage.windows[1].detail.as_deref(), Some("Credits"));
    }

    /// A bare date is midnight UTC — the original's formatter is pinned to
    /// `en_US_POSIX` and GMT, so the reader's own clock never moves it.
    #[test]
    fn the_billing_reset_is_a_bare_date_at_midnight_utc() {
        let usage = reading(&fixture());
        assert_eq!(
            usage.windows[0].resets_at.as_deref(),
            Some("2026-11-01T00:00:00Z")
        );
        assert!(billing_reset("not a date").is_none());
        assert!(billing_reset("2026-11-01T05:00:00Z").is_none());
    }

    /// The CLI does not always send both figures, and a percentage stands in
    /// for what is spent when it is the one that arrived.
    #[test]
    fn a_percentage_stands_in_for_a_used_figure() {
        let reply = json!({
            "success": true,
            "data": { "usageBreakdowns": [ { "limit": 200.0, "percentage": 25.0 } ] }
        });
        let usage = reading(&reply);
        assert_eq!(usage.windows[0].percent_used, Some(25.0));
        // No scope in the payload, so the row carries no second line.
        assert!(usage.windows[0].detail.is_none());
    }

    /// A pool with no ceiling has no denominator, and one with neither figure
    /// cannot be drawn at all.
    #[test]
    fn a_pool_without_a_denominator_is_left_off() {
        let reply = json!({
            "success": true,
            "data": { "usageBreakdowns": [
                { "resourceType": "A", "limit": 100.0, "used": 10.0, "hasLimit": false },
                { "resourceType": "B", "limit": 0.0, "used": 10.0 },
                { "resourceType": "C", "limit": 100.0 },
                { "resourceType": "D" },
                { "resourceType": "E", "limit": 100.0, "used": 10.0 }
            ] }
        });
        let usage = reading(&reply);
        assert_eq!(usage.windows.len(), 1);
        assert_eq!(usage.windows[0].detail.as_deref(), Some("E"));
    }

    /// A spend past its own limit is a full ring and no more.
    #[test]
    fn a_spend_past_the_limit_clamps_to_a_full_ring() {
        let reply = json!({
            "success": true,
            "data": { "usageBreakdowns": [ { "limit": 100.0, "used": 250.0 } ] }
        });
        assert_eq!(reading(&reply).windows[0].percent_used, Some(100.0));
    }

    /// The display name wins over the resource type, and a blank one is not a
    /// name at all.
    #[test]
    fn the_pools_name_is_its_display_name_first() {
        let reply = json!({
            "success": true,
            "data": { "usageBreakdowns": [
                { "resourceType": "AGENTIC_REQUEST", "displayName": "   ",
                  "limit": 10.0, "used": 1.0 }
            ] }
        });
        assert_eq!(
            reading(&reply).windows[0].detail.as_deref(),
            Some("AGENTIC_REQUEST")
        );
    }

    #[test]
    fn a_reply_this_build_cannot_read_is_unreadable() {
        // `success` is required and is a bool.
        assert!(reading(&json!({})).error.is_some());
        assert!(reading(&json!({ "success": "yes" })).error.is_some());
        // An answer, not a fault: nothing to draw.
        assert_eq!(
            reading(&json!({ "success": true })).error.as_deref(),
            Some("No limits reported.")
        );
        assert_eq!(
            reading(&json!({ "success": true, "data": { "usageBreakdowns": [] } }))
                .error
                .as_deref(),
            Some("No limits reported.")
        );
        // A payload object without the breakdowns is unreadable rather than
        // empty — the original's decoder requires the field.
        assert_eq!(
            reading(&json!({ "success": true, "data": { "planName": "Pro" } }))
                .error
                .as_deref(),
            Some("Couldn't read the reply.")
        );
    }

    /// What the server said, in the original's own order of guesses.
    #[test]
    fn a_server_message_is_read_for_what_it_names() {
        let refused = json!({ "success": false, "message": "Not authenticated. Run kiro-cli login." });
        assert_eq!(reading(&refused).error.as_deref(), Some("Sign in to Kiro CLI to see usage."));

        let old = json!({ "success": false, "message": "Method not found: _kiro/account/getUsage" });
        assert_eq!(
            reading(&old).error.as_deref(),
            Some("Update Kiro CLI to read subscription usage.")
        );

        // A sign-in phrase first, even when both are there.
        let both = json!({ "success": false, "message": "unsupported: sign in again" });
        assert_eq!(reading(&both).error.as_deref(), Some("Sign in to Kiro CLI to see usage."));

        // Anything else is a reply this build cannot read.
        assert_eq!(
            reading(&json!({ "success": false, "message": "boom" })).error.as_deref(),
            Some("Couldn't read the reply.")
        );
        assert_eq!(
            reading(&json!({ "success": false })).error.as_deref(),
            Some("Couldn't read the reply.")
        );
    }

    #[test]
    fn every_reason_names_what_to_do_about_it() {
        assert_eq!(Reason::NotInstalled.message(), "Kiro CLI isn't installed.");
        assert_eq!(Reason::SignInRequired.message(), "Sign in to Kiro CLI to see usage.");
        assert_eq!(
            Reason::VersionUnsupported.message(),
            "Update Kiro CLI to read subscription usage."
        );
        assert_eq!(Reason::NoLimitsReported.message(), "No limits reported.");
        assert_eq!(Reason::Unreachable.message(), "The service didn't respond.");
        assert_eq!(Reason::UnreadableReply.message(), "Couldn't read the reply.");

        assert_eq!(
            reason_for_failure(&Failure::ExecutableNotFound),
            Reason::NotInstalled
        );
        assert_eq!(
            reason_for_failure(&Failure::Server("Not authenticated — run kiro-cli login".into())),
            Reason::SignInRequired
        );
        // The original's three phrases, kept verbatim: "sign in", "not
        // authenticated" and "login". A server that says "log in" — two words —
        // is therefore not read as a signed-out CLI, which is the original's
        // behaviour and not a phrasing this port is free to invent around.
        assert_eq!(
            reason_for_failure(&Failure::Server("please log in".into())),
            Reason::UnreadableReply
        );
        for failure in [Failure::StartFailed, Failure::TimedOut, Failure::Closed] {
            assert_eq!(reason_for_failure(&failure), Reason::Unreachable);
        }
    }

    /// ACP is newline-delimited JSON, and the line carries everything the
    /// server needs to answer it.
    #[test]
    fn a_request_is_one_line_of_json_rpc() {
        let line = request_line(1, "initialize", &json!({ "protocolVersion": 1 })).unwrap();
        assert!(!line.contains('\n'));
        let parsed: Value = serde_json::from_str(&line).unwrap();
        assert_eq!(parsed["jsonrpc"], json!("2.0"));
        assert_eq!(parsed["id"], json!(1));
        assert_eq!(parsed["method"], json!("initialize"));
        assert_eq!(parsed["params"]["protocolVersion"], json!(1));
    }

    /// A reply is read by id, and everything else off the pipe is passed over:
    /// a server that emits notifications would otherwise hang the request.
    #[test]
    fn only_a_reply_carrying_this_id_is_read() {
        assert_eq!(
            reply_for(r#"{"jsonrpc":"2.0","id":2,"result":{"success":true}}"#, 2),
            Reply::Result(json!({ "success": true }))
        );
        // A result that is absent is an empty one, not a failure.
        assert_eq!(
            reply_for(r#"{"jsonrpc":"2.0","id":2}"#, 2),
            Reply::Result(Value::Null)
        );
        // A JSON-RPC error carries the server's own words.
        assert_eq!(
            reply_for(
                r#"{"jsonrpc":"2.0","id":2,"error":{"code":-32601,"message":"Method not found"}}"#,
                2
            ),
            Reply::Failure("Method not found".into())
        );
        assert_eq!(
            reply_for(r#"{"jsonrpc":"2.0","id":2,"error":{}}"#, 2),
            Reply::Failure("unknown".into())
        );

        // Somebody else's reply, a notification, and a line that is not JSON.
        assert_eq!(
            reply_for(r#"{"jsonrpc":"2.0","id":3,"result":{}}"#, 2),
            Reply::Ignore
        );
        assert_eq!(
            reply_for(r#"{"jsonrpc":"2.0","method":"session/update","params":{}}"#, 2),
            Reply::Ignore
        );
        assert_eq!(reply_for("not json at all", 2), Reply::Ignore);
        assert_eq!(reply_for("", 2), Reply::Ignore);
    }

    #[test]
    fn the_cli_is_looked_for_in_the_order_the_folders_are_given() {
        let dirs = vec![PathBuf::from("C:\\first"), PathBuf::from("C:\\second")];
        let wanted = dirs[1].join(names().first().expect("a name to look for"));
        assert_eq!(locate(&dirs, &|path| path == wanted).unwrap(), wanted);

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

    /// The per-user install is the one the CLI's own session database sits
    /// beside, so it is looked for wherever `PATH` did not answer.
    #[test]
    fn the_windows_installs_are_looked_for_by_their_own_names() {
        if cfg!(windows) {
            assert!(names().contains(&"kiro-cli.exe".to_string()));
        }
        assert!(names().contains(&"kiro-cli.exe".to_string()) || names().contains(&"kiro-cli".to_string()));
        // `PATH` is where the search starts.
        let dirs = candidate_dirs();
        assert!(!dirs.is_empty());
    }

    /// The presence hint the original's discovery list carries, and the folder
    /// it is: `~/.kiro` and nothing else. Nothing is opened to answer it.
    #[test]
    fn the_state_folder_is_a_presence_hint_and_nothing_more() {
        match state_folder() {
            Some(path) => {
                assert!(path.ends_with(".kiro"));
                assert!(path.is_dir());
            }
            // A machine that has never run the CLI has no such folder, which
            // is an answer and not a failure.
            None => assert!(credentials::home_relative(&[".kiro"])
                .iter()
                .all(|path| !path.is_dir())),
        }
    }

    #[test]
    fn a_batch_file_is_run_through_the_command_interpreter() {
        let batch = command_for(Path::new("C:\\Users\\me\\AppData\\Roaming\\npm\\kiro-cli.cmd"));
        if cfg!(windows) {
            assert_eq!(batch.as_std().get_program(), OsStr::new("cmd.exe"));
        }

        let binary = command_for(Path::new("C:\\Program Files\\Kiro-Cli\\kiro-cli.exe"));
        assert_eq!(
            binary.as_std().get_program(),
            OsStr::new("C:\\Program Files\\Kiro-Cli\\kiro-cli.exe")
        );
    }

    #[test]
    fn the_environment_a_helper_keeps_is_an_allow_list() {
        // What PulseWin holds for other providers never crosses.
        assert!(!kept(OsStr::new("PULSEWIN_CURSOR_COOKIE")));
        assert!(!kept(OsStr::new("PULSEWIN_KIRO_BIN")));
        assert!(!kept(OsStr::new("OPENAI_API_KEY")));
        assert!(!kept(OsStr::new("AWS_SECRET_ACCESS_KEY")));
        assert!(!kept(OsStr::new("")));

        // Where the CLI's own login lives, and what Windows cannot run without.
        assert!(kept(OsStr::new("LOCALAPPDATA")));
        assert!(kept(OsStr::new("SystemRoot")));
        assert!(kept(OsStr::new("USERPROFILE")));
        assert!(kept(OsStr::new("HTTPS_PROXY")));
        // `PATH` is not inherited but rebuilt, so it is not on the list.
        assert!(!kept(OsStr::new("PATH")));
    }

    /// The folder the CLI was found in leads `PATH`, the way every helper the
    /// original starts is set up.
    #[test]
    fn the_folder_the_cli_was_found_in_leads_the_path() {
        let binary = Path::new("C:\\Tools\\Kiro-Cli\\kiro-cli.exe");
        let environment = environment(binary);
        let path = environment
            .iter()
            .find(|(name, _)| name == OsStr::new("PATH"))
            .map(|(_, value)| value.clone())
            .expect("a PATH");
        let path = path.to_string_lossy();
        let mut parts = path.split(if cfg!(windows) { ';' } else { ':' });
        assert_eq!(parts.next(), Some("C:\\Tools\\Kiro-Cli"));
        assert!(path.contains("System32") || !cfg!(windows));
    }
}
