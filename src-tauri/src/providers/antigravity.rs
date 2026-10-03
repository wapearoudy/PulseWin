//! Antigravity's limits, read from a language server running on this PC.
//!
//! The odd one out. There is no account endpoint to ask and no stored login to
//! borrow: Antigravity starts a `language_server` process of its own and talks
//! to it over the loopback interface, and that process is the only thing that
//! knows the quota. So this is the one provider whose figures exist **only
//! while something of Antigravity's is running**, which is what "Open
//! Antigravity to see its usage." says rather than dressing it up as a failure.
//!
//! Three things have to be found, and not one of them can be assumed:
//!
//! - **the process**, which lives inside an install folder rather than on
//!   `PATH`;
//! - **the port**, because the server is started with `--https_server_port 0`,
//!   meaning "take any free one" — it is a different port on every launch, so
//!   anything hardcoded is wrong by the next restart;
//! - **the CSRF token**, a per-launch UUID passed on the command line. Without
//!   it the server answers `unauthenticated`.
//!
//! **More than one process can match, and most of them are the wrong one.**
//! `language_server` is Codeium's binary and other editors ship the same one,
//! so the original identifies a candidate by **the product in its own path**,
//! never by the executable's name. Every candidate is tried, and every port
//! each of them listens on.
//!
//! ## What is different on Windows, and why
//!
//! The original reads the process table with `/bin/ps -axww -o pid=,command=`
//! and the listening ports with `/usr/sbin/lsof`. Windows has neither, so the
//! same two facts are collected with the tools a Windows machine has:
//! `Get-CimInstance Win32_Process` for the command lines (through an absolute
//! `powershell.exe`, so nothing on `PATH` can stand in for it) and
//! `netstat -ano -p tcp` for the ports. Both are read by parsers that are pure
//! functions below, so the shapes are fixture-tested even though no Antigravity
//! is installed on the machine this was written on.
//!
//! Four further differences, each deliberate:
//!
//! - **Antigravity in the path is the whole identity test.** The original
//!   separates `Antigravity.app` from `Antigravity IDE.app` and asks the app
//!   first. The two products report the same account-wide quota — the original
//!   says so itself — and their Windows install folders are not a pair this
//!   port can name from evidence, so every candidate is simply tried in the
//!   order the machine lists them.
//! - **Both schemes are tried per port.** The original posts `https` to a
//!   self-signed loopback listener. Current Antigravity builds also bind the
//!   same JSON-RPC service **in the clear** on a second port, and the one
//!   public report of this route on Windows is of `https` answering nothing on
//!   any of them. The RPC, the method and the header are identical either way;
//!   only the scheme differs, and the original already treats a port it cannot
//!   reach as "not this one, try the next".
//! - **The reset stamp is read by the port's own `parse_reset`**, which also
//!   accepts fractional seconds. The original's `ISO8601DateFormatter` is set
//!   to `.withInternetDateTime` alone and silently drops such a stamp.
//! - Bucket identity, model scope, reported duration and explicit exhausted
//!   status are retained for the panel and alert/cache reconciliation.
//! - **`agy` is not looked for.** Antigravity 2.0's CLI embeds the same RPC
//!   surface, but it postdates the original and is a different product from
//!   the one whose two windows this reads.
//!
//! VERIFY ON A REAL MACHINE: no Antigravity is installed on the machine this
//! was written on, so the two discovery commands have never been run for real.
//! Set `PULSEWIN_DEBUG=1` to dump the quota reply.

use std::collections::BTreeMap;
use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use serde_json::Value;
use tokio::io::AsyncReadExt;

use super::{by_window_length, percent_from_fraction, Ctx, FetchFuture, Provider};
use crate::credentials;
use crate::model::{parse_reset, ProviderUsage, UsageWindow};

const ID: &str = "antigravity";
const NAME: &str = "Antigravity";

/// The RPCs this uses. Antigravity is built on Codeium's language server,
/// hence the `exa.` package and the `x-codeium-` header.
const QUOTA_RPC: &str = "exa.language_server_pb.LanguageServerService/RetrieveUserQuotaSummary";
const STATUS_RPC: &str = "exa.language_server_pb.LanguageServerService/GetUserStatus";
const CSRF_HEADER: &str = "x-codeium-csrf-token";

/// The original's own `timeoutInterval`, per request.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(6);

/// The two discovery commands. PowerShell's first start is the slow one.
const DISCOVERY_DEADLINE: Duration = Duration::from_secs(10);

/// A helper that prints more than this has the rest dropped. Both pipes are
/// still drained past it, because a reader that walks away is the deadlock
/// wearing a different hat.
const OUTPUT_CEILING: usize = 256 * 1024;

/// A console window must not flash up in front of the panel.
#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

pub struct Antigravity;

impl Provider for Antigravity {
    fn id(&self) -> &'static str {
        ID
    }

    fn name(&self) -> &'static str {
        NAME
    }

    /// Whether Antigravity is installed here — **a hint, never permission to
    /// read**, which is what the original calls its own discovery list.
    ///
    /// A folder check and nothing more: this is asked on every refresh, ahead
    /// of the fetch, so it may not run the two discovery commands. The figures
    /// themselves need the app *running*, and that question is answered in the
    /// fetch, where its answer can be said on the card.
    ///
    /// An install PulseWin cannot recognise is named by
    /// `PULSEWIN_ANTIGRAVITY_HOME`.
    fn is_configured(&self) -> bool {
        if let Some(named) = credentials::env_override(ID, "home") {
            return !named.trim().is_empty() && credentials::expand_tilde(named.trim()).exists();
        }
        installed(&candidate_roots(), &|path| path.exists())
    }

    fn fetch(&self, _ctx: Arc<Ctx>) -> FetchFuture {
        Box::pin(async move { fetch_inner().await })
    }
}

// ---------------------------------------------------------------------------
// The reasons, in the original's own set
// ---------------------------------------------------------------------------

/// The three ways this ends without figures.
///
/// **`NotAnswering` is not a fault to be told about on a timer.** The first
/// attempt at this in the original reported "couldn't read the reply", which
/// sits beside "the service didn't respond" in its failure classification, and
/// the banner the fix was written to stop went on firing about an app that was
/// open. Both of these are answers, not failures.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Reason {
    /// Nothing of Antigravity's is running.
    NotRunning,
    /// Something is, and every helper it runs refused this RPC.
    NotAnswering,
    NoLimitsReported,
}

impl Reason {
    /// The original's own copy, sentence for sentence.
    fn message(self) -> &'static str {
        match self {
            Reason::NotRunning => "Open Antigravity to see its usage.",
            Reason::NotAnswering => {
                "Antigravity is open but didn't answer. Restarting it usually helps."
            }
            Reason::NoLimitsReported => "No limits reported.",
        }
    }
}

async fn fetch_inner() -> ProviderUsage {
    let servers = discover().await;
    if servers.is_empty() {
        return ProviderUsage::failed(ID, NAME, Reason::NotRunning.message());
    }

    let Some(client) = loopback_client() else {
        return ProviderUsage::failed(ID, NAME, Reason::NotRunning.message());
    };

    // An answer with no limits in it is a real answer, but not a reason to
    // stop: with two servers up it is what the wrong one says. Held, and
    // reported only if nothing better turns up.
    let mut answered_empty = false;
    // Whether anything answered at all. A server that refused this RPC is
    // still Antigravity running, and reporting "not running" for it would say
    // the app is not there while it is.
    let mut something_answered = false;

    for server in &servers {
        // A server listens on more than one port and only one of them speaks
        // this. Which is which isn't advertised, so they are tried.
        for port in &server.ports {
            match ask(&client, *port, &server.token).await {
                Ok(windows) if !windows.is_empty() => {
                    // A second call, because the quota reply doesn't name the
                    // plan. Its absence is not worth failing over.
                    let plan = plan(&client, *port, &server.token).await;
                    return ProviderUsage::ok(ID, NAME, windows).with_plan(plan);
                }
                Ok(_) => {
                    answered_empty = true;
                    something_answered = true;
                }
                // This port answered, but not with this service — try the next
                // one. A closed port and a port serving something else are the
                // same answer here.
                Err(Ask::WrongPort) => continue,
                // The other of Antigravity's servers answers 401 to this RPC.
                // That is this process saying "not me", not the account being
                // refused, so it is worth no more than a closed port.
                Err(Ask::Refused) => {
                    something_answered = true;
                    continue;
                }
                // A 200 whose body would not decode. Also not a reason to stop:
                // "every candidate is tried" has to mean every candidate, or
                // the first odd body ends the search and the server that would
                // have answered is never asked.
                Err(Ask::Unreadable) => {
                    something_answered = true;
                    continue;
                }
            }
        }
    }

    if answered_empty {
        return ProviderUsage::failed(ID, NAME, Reason::NoLimitsReported.message());
    }

    let reason = if something_answered {
        Reason::NotAnswering
    } else {
        Reason::NotRunning
    };
    ProviderUsage::failed(ID, NAME, reason.message())
}

/// The client the loopback server is reached with.
///
/// **Its own client, not the shared one**, for two reasons that both come from
/// the server being on this machine. It signs its own certificate, so nothing
/// can vouch for it; and it is on `127.0.0.1`, which no proxy has any business
/// in front of — a reader running Clash or a corporate MITM would otherwise
/// have this request leave the machine to reach a server inside it.
///
/// `danger_accept_invalid_certs` is set here and **nowhere else in the port**:
/// no other provider talks to a host the reader did not name, on an address no
/// certificate can be issued for.
fn loopback_client() -> Option<reqwest::Client> {
    reqwest::Client::builder()
        .danger_accept_invalid_certs(true)
        .no_proxy()
        .timeout(REQUEST_TIMEOUT)
        .connect_timeout(REQUEST_TIMEOUT)
        .user_agent(concat!("PulseWin/", env!("CARGO_PKG_VERSION")))
        .build()
        .ok()
}

// ---------------------------------------------------------------------------
// Asking it
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Ask {
    /// This port answered, but not with this service — try the next one.
    WrongPort,
    Refused,
    Unreadable,
}

/// The URL one RPC lives at. Loopback is written as `127.0.0.1` and never as
/// `localhost`, because the certificate the server signs is for that address.
fn rpc_url(scheme: &str, port: u16, method: &str) -> String {
    format!("{scheme}://127.0.0.1:{port}/{method}")
}

/// One RPC's reply, or why there is none.
///
/// **Both schemes, in that order.** `https` is the original's route and is
/// tried first; a build that serves the same service in the clear on this port
/// is reached by the second attempt. A transport failure moves to the next
/// scheme rather than ending the port, and a status that is neither a success
/// nor a refusal means this port is somebody else's — which is exactly what
/// the original reads it as.
async fn post(
    client: &reqwest::Client,
    port: u16,
    token: &str,
    method: &str,
) -> Result<Value, Ask> {
    const SCHEMES: [&str; 2] = ["https", "http"];

    for scheme in SCHEMES {
        let response = client
            .post(rpc_url(scheme, port, method))
            .header("Content-Type", "application/json")
            .header(CSRF_HEADER, token)
            .body("{}")
            .send()
            .await;

        let Ok(response) = response else {
            continue;
        };

        let status = response.status().as_u16();
        let body = response.text().await.unwrap_or_default();

        match status {
            200 => {
                return serde_json::from_str::<Value>(&body).map_err(|_| Ask::Unreadable);
            }
            401 | 403 => return Err(Ask::Refused),
            _ => continue,
        }
    }

    Err(Ask::WrongPort)
}

async fn ask(client: &reqwest::Client, port: u16, token: &str) -> Result<Vec<UsageWindow>, Ask> {
    match post(client, port, token, QUOTA_RPC).await {
        Err(failure) => Err(failure),
        // A body that is JSON but not a quota summary decodes in the original
        // to a reply with every field absent, which is a reply with no
        // windows — not an unreadable one. Only a body that is not JSON at all
        // is `Unreadable`, and `post` has already said so.
        Ok(reply) => {
            // The one place the reply itself exists, and the only thing that
            // can be looked at when this provider is being checked against a
            // real Antigravity.
            super::claude_code::debug_dump(ID, &reply);
            Ok(windows_from(&reply))
        }
    }
}

/// The plan's name — "Pro", and whatever the other tiers are called.
///
/// `GetUserStatus` answers with a good deal more than this, the account's name
/// and email address among it. Only the plan's name is read: the rest is the
/// reader's, not ours, and nothing here has any use for it.
async fn plan(client: &reqwest::Client, port: u16, token: &str) -> Option<String> {
    let reply = post(client, port, token, STATUS_RPC).await.ok()?;
    let name = reply
        .pointer("/userStatus/planStatus/planInfo/planName")?
        .as_str()?;
    (!name.is_empty()).then(|| name.to_string())
}

// ---------------------------------------------------------------------------
// Reading the reply
// ---------------------------------------------------------------------------

/// The quota summary's windows, in the original's own order.
///
/// The groups are the model groups — "Gemini", "Claude and GPT" — and each
/// holds its own buckets. Groups keep the reply's order; **within a group the
/// shortest window comes first**, which is the order the other providers'
/// limits arrive in and the order they are useful in: the one about to bite
/// comes first.
fn windows_from(reply: &Value) -> Vec<UsageWindow> {
    let Some(groups) = reply
        .pointer("/response/groups")
        .and_then(Value::as_array)
    else {
        return Vec::new();
    };

    let mut windows: Vec<UsageWindow> = Vec::new();
    for group in groups {
        let scope = model_group(group.get("displayName").and_then(Value::as_str));

        let rows: Vec<(i64, UsageWindow)> = group
            .get("buckets")
            .and_then(Value::as_array)
            .map(|buckets| {
                buckets
                    .iter()
                    .filter_map(|bucket| window_from(bucket, scope.clone()))
                    .collect()
            })
            .unwrap_or_default();

        windows.extend(by_window_length(rows));
    }

    windows
}

fn window_from(bucket: &Value, scope: Option<String>) -> Option<(i64, UsageWindow)> {
    // A bucket with no id of its own is not a pool this build can name, which
    // is what the original's decoder requires the field for. An empty id is
    // still an id.
    let bucket_id = bucket.get("bucketId").and_then(Value::as_str)?;

    let remaining = bucket
        .get("remainingFraction")
        .and_then(Value::as_f64)
        .filter(|fraction| fraction.is_finite())?;

    let (label, seconds) = length(bucket.get("window").and_then(Value::as_str))?;

    // **The only provider that reports what is *left* rather than what is
    // gone.** Everything downstream is in terms of what is gone.
    let used = (1.0 - remaining).clamp(0.0, 1.0);

    let resets_at = bucket
        .get("resetTime")
        .and_then(Value::as_str)
        .and_then(|text| parse_reset(&Value::String(text.to_string())));

    Some((
        seconds,
        UsageWindow::new(label, Some(percent_from_fraction(used)))
            .with_id(Some(bucket_id.to_owned()))
            .with_scope(scope.clone())
            .with_duration(Some(seconds as f64))
            .with_exhausted(remaining <= 0.0)
            .with_reset(resets_at)
            .with_detail(scope),
    ))
}

/// A bucket's window, as the length the panel draws and the seconds it sorts
/// by.
///
/// `5h` and `weekly` are what the server sends today; the numbered forms are
/// there so a new window length is understood rather than dropped. **A window
/// with no length cannot be named or sorted, and inventing one would put a
/// figure under a heading that isn't true**, so anything unrecognised is left
/// out rather than guessed at.
///
/// The heading itself is this port's one spelling of a length
/// (`super::humanize_window_seconds`), because the compact forms are what the
/// panel draws where the original builds a longer translated name.
fn length(window: Option<&str>) -> Option<(String, i64)> {
    let window = window?.to_lowercase();

    match window.as_str() {
        "5h" => return Some((super::humanize_window_seconds(5 * 3_600), 5 * 3_600)),
        "weekly" => return Some((super::humanize_window_seconds(7 * 86_400), 7 * 86_400)),
        "daily" => return Some((super::humanize_window_seconds(86_400), 86_400)),
        "monthly" => return Some((super::humanize_window_seconds(30 * 86_400), 30 * 86_400)),
        _ => {}
    }

    let mut characters = window.chars();
    let unit = characters.next_back()?;
    let count: i64 = characters.as_str().parse().ok()?;
    if count <= 0 {
        return None;
    }

    // A named length is named, whatever spelling of it arrived: five hours is
    // the five-hour window and seven days is the weekly one, so `5h` and
    // `weekly` stay the headings the other providers use.
    let seconds = match unit {
        'h' => count * 3_600,
        'd' => count * 86_400,
        _ => return None,
    };
    Some((super::humanize_window_seconds(seconds), seconds))
}

/// "Gemini Models" → "Gemini".
///
/// The group's name is what the limit is scoped to, and it is shown after the
/// window's own name — "5h · Gemini" — where the trailing "models" is a word
/// the row can't spare.
fn model_group(name: Option<&str>) -> Option<String> {
    let name = name?.trim();
    if name.is_empty() {
        return None;
    }

    let words: Vec<&str> = name.split(' ').filter(|word| !word.is_empty()).collect();
    match words.last() {
        Some(last) if words.len() > 1 && last.eq_ignore_ascii_case("models") => {
            Some(words[..words.len() - 1].join(" "))
        }
        _ => Some(name.to_string()),
    }
}

// ---------------------------------------------------------------------------
// Finding it
// ---------------------------------------------------------------------------

/// A language server that might be able to answer.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Server {
    ports: Vec<u16>,
    token: String,
}

/// Every language server on this PC that might be able to answer, best first.
async fn discover() -> Vec<Server> {
    let printed = run(&powershell(), &process_arguments(), DISCOVERY_DEADLINE).await;
    let candidates = candidates_from_processes(&String::from_utf8_lossy(
        printed.as_deref().unwrap_or_default(),
    ));
    if candidates.is_empty() {
        return Vec::new();
    }

    let printed = run(&netstat(), &port_arguments(), DISCOVERY_DEADLINE).await;
    let ports = ports_by_pid(&String::from_utf8_lossy(
        printed.as_deref().unwrap_or_default(),
    ));

    servers_from(&candidates, &ports)
}

/// The pids worth asking and each one's CSRF token.
///
/// `printed` is what `Get-CimInstance … | ConvertTo-Json` wrote: one object
/// when a single process matched and an array when several did, which is why
/// both are read.
fn candidates_from_processes(printed: &str) -> Vec<(u32, String)> {
    let trimmed = printed.trim();
    if trimmed.is_empty() {
        return Vec::new();
    }
    let Ok(parsed) = serde_json::from_str::<Value>(trimmed) else {
        return Vec::new();
    };

    let processes: Vec<&Value> = match &parsed {
        Value::Array(items) => items.iter().collect(),
        other => vec![other],
    };

    processes
        .iter()
        .filter_map(|process| {
            let command_line = process.get("CommandLine").and_then(Value::as_str)?;
            if !is_antigravity(command_line) {
                return None;
            }
            let pid = process.get("ProcessId").and_then(Value::as_u64)?;
            let token = flag_value(command_line, "--csrf_token")?;
            Some((pid as u32, token))
        })
        .collect()
}

/// Does this process belong to Antigravity?
///
/// **The product's own path, never the executable's name.**
/// `language_server` is Codeium's binary and Antigravity is not the only thing
/// that ships it — asking the wrong one for Antigravity's quota is what the
/// original's bundle-fragment test exists to prevent, and on Windows the two
/// things that say "this is Antigravity's" are the same two the app itself
/// passes: its own data directory, or its install folder in the path.
fn is_antigravity(command_line: &str) -> bool {
    if flag_value(command_line, "--app_data_dir")
        .is_some_and(|value| value.eq_ignore_ascii_case("antigravity"))
    {
        return true;
    }

    let lowered = command_line.to_lowercase();
    lowered.contains(r"\antigravity\") || lowered.contains("/antigravity/")
}

/// The value of a `--flag` in a command line, written either `--flag value` or
/// `--flag=value`.
///
/// **Both spellings, because the platforms differ**: the original reads
/// macOS's `--csrf_token <uuid>` from `ps`, and Windows is written
/// `--csrf_token=<uuid>`. A flag that merely *starts* with this one —
/// `--csrf_token_backup` — is not this flag, which is what the character after
/// it is checked for.
fn flag_value(command_line: &str, flag: &str) -> Option<String> {
    let mut rest = command_line;

    loop {
        let at = rest.find(flag)?;
        let after = &rest[at + flag.len()..];

        let mut characters = after.chars();
        let value = match characters.next() {
            Some('=') => characters.as_str(),
            Some(character) if character.is_whitespace() => characters.as_str().trim_start(),
            _ => {
                rest = after;
                continue;
            }
        };

        let token: String = value
            .chars()
            .take_while(|character| !character.is_whitespace())
            .collect();
        if !token.is_empty() {
            return Some(token);
        }
        rest = after;
    }
}

/// Every loopback port each pid is listening on.
///
/// Read from `netstat -ano -p tcp`, one row per socket:
///
/// ```text
///   TCP    127.0.0.1:58493        0.0.0.0:0              LISTENING       19644
///   TCP    [::1]:62406            [::]:0                 LISTENING       19644
/// ```
///
/// **A listening row is recognised by its foreign address, not by the word
/// "LISTENING".** That word is translated on a Windows running in another
/// language, and a parser that matched it would find nothing at all on those
/// machines — silently, and only for the readers least able to report it. A
/// socket waiting for connections always has a wildcard peer; one that has a
/// real peer is a conversation, not a listener.
fn ports_by_pid(printed: &str) -> BTreeMap<u32, Vec<u16>> {
    let mut by_pid: BTreeMap<u32, Vec<u16>> = BTreeMap::new();

    for line in printed.lines() {
        let fields: Vec<&str> = line.split_whitespace().collect();
        if fields.len() < 5 || !fields[0].eq_ignore_ascii_case("TCP") {
            continue;
        }
        if !is_wildcard(fields[2]) {
            continue;
        }

        let Some(host) = fields[1].rsplit_once(':').map(|(host, _)| host) else {
            continue;
        };
        if !is_loopback(host) {
            continue;
        }

        let (Some(port), Ok(pid)) = (
            fields[1].rsplit(':').next().and_then(|p| p.parse::<u16>().ok()),
            fields[4].parse::<u32>(),
        ) else {
            continue;
        };

        let ports = by_pid.entry(pid).or_default();
        if !ports.contains(&port) {
            ports.push(port);
        }
    }

    for ports in by_pid.values_mut() {
        ports.sort_unstable();
    }
    by_pid
}

/// A socket with no peer yet: `0.0.0.0:0`, `[::]:0` or `*:*`.
fn is_wildcard(foreign: &str) -> bool {
    matches!(foreign, "0.0.0.0:0" | "[::]:0" | "*:*") || foreign.ends_with(":0") && foreign.starts_with('[')
}

/// The addresses a language server of Antigravity's would be bound to.
fn is_loopback(host: &str) -> bool {
    matches!(host, "127.0.0.1" | "0.0.0.0" | "[::1]" | "[::]")
}

/// A process listening on nothing cannot be asked anything, so it is dropped
/// here rather than being tried and timing out.
fn servers_from(candidates: &[(u32, String)], ports: &BTreeMap<u32, Vec<u16>>) -> Vec<Server> {
    candidates
        .iter()
        .filter_map(|(pid, token)| {
            let ports = ports.get(pid)?.clone();
            if ports.is_empty() {
                return None;
            }
            Some(Server {
                ports,
                token: token.clone(),
            })
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Is it installed here?
// ---------------------------------------------------------------------------

/// The folder names Antigravity's own Windows installers write — the app and
/// the IDE, which install side by side and to different folders.
const INSTALL_FOLDERS: [&str; 2] = ["Antigravity", "Antigravity IDE"];

/// Where an install puts itself: the per-user program folder, the two
/// machine-wide ones, and the app's own data folder, which is there once it
/// has run at least once.
fn candidate_roots() -> Vec<PathBuf> {
    let mut roots: Vec<PathBuf> = Vec::new();

    let local = std::env::var_os("LOCALAPPDATA")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .or_else(dirs::data_local_dir);
    if let Some(local) = local {
        roots.push(local.join("Programs"));
        roots.push(local);
    }

    for name in ["ProgramFiles", "ProgramFiles(x86)"] {
        if let Some(dir) = std::env::var_os(name).filter(|value| !value.is_empty()) {
            roots.push(PathBuf::from(dir));
        }
    }

    // The app's own data folder, beside `%APPDATA%`: an Electron app writes
    // one the first time it runs, which is the moment its quota becomes
    // something this port can read.
    roots.extend(credentials::config_relative(&["Antigravity"]));
    roots.extend(credentials::config_relative(&["Antigravity IDE"]));

    roots.retain(|root| !root.as_os_str().is_empty());
    roots
}

/// Whether the folder or the program beside it is there. The check is injected
/// so a test can drive the search without installing anything.
fn installed(roots: &[PathBuf], exists: &dyn Fn(&Path) -> bool) -> bool {
    roots.iter().any(|root| {
        INSTALL_FOLDERS
            .iter()
            .any(|folder| exists(&root.join(folder)) || exists(&root.join(format!("{folder}.exe"))))
    })
}

// ---------------------------------------------------------------------------
// Running the two commands
// ---------------------------------------------------------------------------

/// The process query, in the machine's own words.
///
/// **Single quotes only, deliberately.** This string is handed to
/// `powershell.exe` as one `CreateProcess` argument, and a query carrying
/// double quotes is a query whose meaning depends on how the child's command
/// line is re-split — the quoting rule that differs between runtimes. It
/// filters in PowerShell rather than in WQL so that the same expression covers
/// both candidate process names, and a machine with several matching processes
/// is the case that matters.
const PROCESS_QUERY: &str = "Get-CimInstance Win32_Process | Where-Object { $_.Name -match 'language_server|antigravity' } | Select-Object ProcessId,CommandLine | ConvertTo-Json -Compress";

fn process_arguments() -> Vec<String> {
    ["-NoProfile", "-NonInteractive", "-Command", PROCESS_QUERY]
        .iter()
        .map(|argument| argument.to_string())
        .collect()
}

fn port_arguments() -> Vec<String> {
    ["-ano", "-p", "tcp"]
        .iter()
        .map(|argument| argument.to_string())
        .collect()
}

/// Windows PowerShell, out of `%SystemRoot%` rather than off `PATH`.
///
/// A program this port starts to *read the machine* must be the machine's own:
/// a `powershell.exe` earlier on `PATH` would be somebody else's, and it would
/// be handed the answer to every question asked here. `AntigravityQuotaWatcher`
/// carries a whole module for the same reason.
fn powershell() -> PathBuf {
    if let Some(root) = std::env::var_os("SystemRoot") {
        let path = PathBuf::from(root)
            .join("System32")
            .join("WindowsPowerShell")
            .join("v1.0")
            .join("powershell.exe");
        if path.is_file() {
            return path;
        }
    }
    PathBuf::from("powershell.exe")
}

fn netstat() -> PathBuf {
    if let Some(root) = std::env::var_os("SystemRoot") {
        let path = PathBuf::from(root).join("System32").join("netstat.exe");
        if path.is_file() {
            return path;
        }
    }
    PathBuf::from("netstat.exe")
}

/// The variables a helper keeps. **An allow-list, not a deny-list**: what is
/// not named here does not cross into it, so no key, cookie or cloud credential
/// of PulseWin's own reaches a program this runs on the reader's behalf.
const KEPT: [&str; 32] = [
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
    // PowerShell finds its own modules beside itself, but a machine that has
    // moved them says so here, and `Get-CimInstance` is such a module.
    "PSMODULEPATH",
    // The reader's own profile — which is where the process table's command
    // lines point, and what makes a path readable at all.
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

/// Only what a system command needs to run, with `System32` on `PATH` so a
/// program named rather than pathed is still found.
fn environment() -> Vec<(OsString, OsString)> {
    let mut environment: Vec<(OsString, OsString)> = std::env::vars_os()
        .filter(|(name, _)| kept(name))
        .collect();

    let mut dirs: Vec<PathBuf> = Vec::new();
    if let Some(root) = std::env::var_os("SYSTEMROOT") {
        dirs.push(PathBuf::from(root).join("System32"));
    }
    if let Some(path) = std::env::var_os("PATH") {
        dirs.extend(std::env::split_paths(&path));
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

/// One bounded run of a system command, and its stdout when it succeeded.
async fn run(program: &Path, arguments: &[String], deadline: Duration) -> Option<Vec<u8>> {
    let mut command = tokio::process::Command::new(program);
    command
        .args(arguments)
        .env_clear()
        .envs(environment())
        // Nothing to answer with, so a command that asks gets EOF rather than
        // blocking on a terminal that is not there.
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        // A run that is dropped — the deadline below — takes the child with it.
        .kill_on_drop(true);

    #[cfg(windows)]
    command.creation_flags(CREATE_NO_WINDOW);

    let child = command.spawn().ok()?;

    match tokio::time::timeout(deadline, collect(child)).await {
        Ok(collected) if collected.success => Some(collected.stdout),
        _ => None,
    }
}

struct Collected {
    stdout: Vec<u8>,
    success: bool,
}

/// Drains both pipes at once, and then waits for the process.
///
/// **Both at once.** Reading one to EOF and only then reading the other
/// deadlocks the moment the child writes more than a pipe buffer to the one
/// nobody is reading — and past the output ceiling the bytes are dropped but
/// the pipe is still drained, because a reader that walks away is the same
/// deadlock wearing a different hat.
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

    Collected { stdout, success }
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

#[cfg(test)]
mod tests {
    use super::*;

    /// The shape `RetrieveUserQuotaSummary` answers with, as the original's
    /// fixture describes it: two model groups, a five-hour and a weekly bucket
    /// in each.
    fn fixture() -> Value {
        serde_json::json!({
            "response": {
                "groups": [
                    {
                        "displayName": "Gemini Models",
                        "buckets": [
                            { "bucketId": "gemini-weekly", "window": "weekly",
                              "remainingFraction": 0.4, "resetTime": "2026-10-09T00:00:00Z" },
                            { "bucketId": "gemini-5h", "window": "5h",
                              "remainingFraction": 0.75, "resetTime": "2026-10-02T22:00:00Z" }
                        ]
                    },
                    {
                        "displayName": "Claude and GPT Models",
                        "buckets": [
                            { "bucketId": "3p-5h", "window": "5h", "remainingFraction": 0.0 },
                            { "bucketId": "3p-weekly", "window": "weekly",
                              "remainingFraction": 1.0 }
                        ]
                    }
                ]
            }
        })
    }

    #[test]
    fn the_rpcs_are_the_ones_antigravity_answers() {
        assert_eq!(
            QUOTA_RPC,
            "exa.language_server_pb.LanguageServerService/RetrieveUserQuotaSummary"
        );
        assert_eq!(
            STATUS_RPC,
            "exa.language_server_pb.LanguageServerService/GetUserStatus"
        );
        assert_eq!(CSRF_HEADER, "x-codeium-csrf-token");
        // Loopback written as the address the server signs its own certificate
        // for.
        assert_eq!(
            rpc_url("https", 58493, QUOTA_RPC),
            "https://127.0.0.1:58493/exa.language_server_pb.LanguageServerService/RetrieveUserQuotaSummary"
        );
    }

    /// Four windows, the short one first inside each group, and the group's
    /// name carried on the row.
    #[test]
    fn reads_the_two_model_groups_and_their_buckets() {
        let windows = windows_from(&fixture());
        let rows: Vec<(&str, Option<f64>, Option<&str>)> = windows
            .iter()
            .map(|w| {
                (
                    w.label.as_str(),
                    w.percent_used,
                    w.detail.as_deref(),
                )
            })
            .collect();

        assert_eq!(
            rows,
            vec![
                ("5h", Some(25.0), Some("Gemini")),
                ("7d", Some(60.0), Some("Gemini")),
                ("5h", Some(100.0), Some("Claude and GPT")),
                ("7d", Some(0.0), Some("Claude and GPT")),
            ]
        );
    }

    /// The reset the server states, on the window it belongs to.
    #[test]
    fn the_reset_is_read_from_the_bucket() {
        let windows = windows_from(&fixture());
        assert_eq!(
            windows[0].resets_at.as_deref(),
            Some("2026-10-02T22:00:00Z")
        );
        // A bucket that states none carries none.
        assert!(windows[2].resets_at.is_none());
    }

    /// **The one provider that reports what is *left*.** Everything downstream
    /// is in terms of what is gone.
    ///
    /// The rows come back shortest-window-first, which is the order the
    /// original draws them in — the two five-hour buckets ahead of the weekly
    /// one, whatever order the reply listed them in.
    #[test]
    fn what_is_left_is_turned_into_what_is_gone() {
        let reply = serde_json::json!({
            "response": { "groups": [ { "buckets": [
                { "bucketId": "a", "window": "5h", "remainingFraction": 0.25 },
                { "bucketId": "b", "window": "weekly", "remainingFraction": 1.0 },
                { "bucketId": "c", "window": "5h", "remainingFraction": 0.0 }
            ] } ] }
        });
        let windows = windows_from(&reply);
        assert_eq!(windows[0].percent_used, Some(75.0));
        assert_eq!(windows[1].percent_used, Some(100.0));
        assert_eq!(windows[2].percent_used, Some(0.0));
    }

    /// A fraction outside its own range is not allowed to draw a ring past
    /// full or below empty.
    #[test]
    fn a_fraction_out_of_range_is_clamped() {
        let reply = serde_json::json!({
            "response": { "groups": [ { "buckets": [
                { "bucketId": "a", "window": "5h", "remainingFraction": -0.5 },
                { "bucketId": "b", "window": "weekly", "remainingFraction": 1.5 }
            ] } ] }
        });
        let windows = windows_from(&reply);
        assert_eq!(windows[0].percent_used, Some(100.0));
        assert_eq!(windows[1].percent_used, Some(0.0));
    }

    #[test]
    fn a_group_named_models_loses_that_word() {
        assert_eq!(model_group(Some("Gemini Models")).as_deref(), Some("Gemini"));
        assert_eq!(
            model_group(Some("Claude and GPT Models")).as_deref(),
            Some("Claude and GPT")
        );
        // A name that is not "… Models" is left exactly as it is.
        assert_eq!(model_group(Some("Gemini")).as_deref(), Some("Gemini"));
        assert_eq!(model_group(Some("Models")).as_deref(), Some("Models"));
        // Nothing to scope the row to.
        assert_eq!(model_group(Some("   ")), None);
        assert_eq!(model_group(None), None);
    }

    /// A bucket the server cannot describe is left out rather than guessed at.
    #[test]
    fn a_bucket_this_build_cannot_read_is_left_off() {
        let reply = serde_json::json!({
            "response": { "groups": [ { "buckets": [
                { "window": "5h", "remainingFraction": 0.5 },
                { "bucketId": "no-fraction", "window": "5h" },
                { "bucketId": "no-window", "remainingFraction": 0.5 },
                { "bucketId": "unknown-window", "window": "fortnight", "remainingFraction": 0.5 },
                { "bucketId": "good", "window": "5h", "remainingFraction": 0.5 }
            ] } ] }
        });
        let windows = windows_from(&reply);
        assert_eq!(windows.len(), 1);
        assert_eq!(windows[0].percent_used, Some(50.0));
    }

    /// `5h` and `weekly` are today's spellings; the numbered forms are there so
    /// a new length is understood rather than dropped, and a length that is not
    /// a whole number of hours or days is not invented.
    #[test]
    fn the_numbered_window_forms_are_understood() {
        let cases: [(&str, Option<(&str, i64)>); 9] = [
            ("5h", Some(("5h", 5 * 3_600))),
            ("weekly", Some(("7d", 7 * 86_400))),
            ("daily", Some(("1d", 86_400))),
            ("monthly", Some(("30d", 30 * 86_400))),
            ("3h", Some(("3h", 10_800))),
            ("2d", Some(("2d", 172_800))),
            ("7d", Some(("7d", 604_800))),
            ("0h", None),
            ("fortnight", None),
        ];

        for (window, wanted) in cases {
            assert_eq!(
                length(Some(window)),
                wanted.map(|(label, seconds)| (label.to_string(), seconds)),
                "reading {window:?}"
            );
        }
        assert_eq!(length(None), None);
        assert_eq!(length(Some("")), None);
        // A negative length is not a window.
        assert_eq!(length(Some("-3h")), None);
    }

    /// The plan's name, out of a reply that carries a good deal more.
    #[test]
    fn the_plan_name_comes_from_the_status_reply() {
        let reply = serde_json::json!({
            "userStatus": {
                "email": "reader@example.com",
                "planStatus": { "planInfo": { "planName": "Pro" } }
            }
        });
        assert_eq!(
            reply.pointer("/userStatus/planStatus/planInfo/planName").and_then(Value::as_str),
            Some("Pro")
        );
        // An empty name is no name.
        let empty = serde_json::json!({ "userStatus": { "planStatus": { "planInfo": { "planName": "" } } } });
        assert_eq!(
            empty.pointer("/userStatus/planStatus/planInfo/planName").and_then(Value::as_str),
            Some("")
        );
        // And a reply without one is not a failure.
        assert!(serde_json::json!({ "userStatus": {} })
            .pointer("/userStatus/planStatus/planInfo/planName")
            .is_none());
    }

    /// **The product's own path, never the executable's name.** Codeium's
    /// `language_server` ships with other editors, and asking one of those for
    /// Antigravity's quota is what this test is here for.
    #[test]
    fn only_a_process_of_antigravity_is_a_candidate() {
        let printed = serde_json::json!([
            { "ProcessId": 1, "CommandLine": "C:\\Windows\\System32\\other\\language_server.exe --csrf_token=aaa" },
            { "ProcessId": 2, "CommandLine": "C:\\Apps\\Antigravity\\language_server.exe --csrf_token=bbb" },
            { "ProcessId": 3, "CommandLine": "C:\\Apps\\Antigravity\\x.exe --app_data_dir antigravity --csrf_token=ccc" },
            { "ProcessId": 4, "CommandLine": "C:\\Apps\\Antigravity IDE\\language_server.exe" }
        ])
        .to_string();

        let candidates = candidates_from_processes(&printed);
        assert_eq!(
            candidates,
            vec![(2, "bbb".to_string()), (3, "ccc".to_string())]
        );
    }

    /// A candidate without a token is not a candidate: the server refuses the
    /// RPC without one, so asking is a request that cannot succeed.
    #[test]
    fn a_candidate_without_a_token_is_not_asked() {
        let printed = serde_json::json!([
            { "ProcessId": 7, "CommandLine": "C:\\Apps\\Antigravity\\language_server.exe" }
        ])
        .to_string();
        assert!(candidates_from_processes(&printed).is_empty());
    }

    /// Windows writes `--csrf_token=<uuid>` where macOS writes
    /// `--csrf_token <uuid>`, and a flag that merely starts with this one is
    /// not this one.
    ///
    /// **The two spellings read their value differently, and that is the
    /// point.** A space is what separates a value written as the next word;
    /// after an `=` the value begins immediately, so `--csrf_token=` with
    /// nothing behind it is a flag with no value rather than a flag whose
    /// value is whatever argument came next.
    #[test]
    fn a_flag_is_read_in_either_spelling() {
        assert_eq!(
            flag_value("ls --csrf_token=abc123 --other", "--csrf_token").as_deref(),
            Some("abc123")
        );
        assert_eq!(
            flag_value("ls --csrf_token abc123 --other", "--csrf_token").as_deref(),
            Some("abc123")
        );
        assert_eq!(
            flag_value("ls --csrf_token   abc123", "--csrf_token").as_deref(),
            Some("abc123")
        );
        // A longer flag that begins with this one is not this one.
        assert_eq!(
            flag_value("ls --csrf_token_backup=zzz --csrf_token=real", "--csrf_token").as_deref(),
            Some("real")
        );
        // Nothing behind the `=`, and nothing behind the flag at all.
        assert_eq!(flag_value("ls --csrf_token=", "--csrf_token"), None);
        assert_eq!(flag_value("ls --csrf_token= --next=1", "--csrf_token"), None);
        assert_eq!(flag_value("ls --csrf_token", "--csrf_token"), None);
        assert_eq!(flag_value("ls", "--csrf_token"), None);
        // A path fragment is not a flag value: the app's own directory marker
        // is compared exactly.
        assert_eq!(
            flag_value("ls --app_data_dir antigravity", "--app_data_dir").as_deref(),
            Some("antigravity")
        );
    }

    /// A process list that is empty, unparseable, or a single object rather
    /// than an array — `ConvertTo-Json` writes the last when one process
    /// matched, which is the common case.
    #[test]
    fn one_matching_process_is_not_an_array() {
        let single = serde_json::json!({
            "ProcessId": 9,
            "CommandLine": "C:\\Apps\\Antigravity\\language_server.exe --csrf_token=solo"
        })
        .to_string();
        assert_eq!(
            candidates_from_processes(&single),
            vec![(9, "solo".to_string())]
        );

        assert!(candidates_from_processes("").is_empty());
        assert!(candidates_from_processes("   ").is_empty());
        assert!(candidates_from_processes("not json").is_empty());
        assert!(candidates_from_processes("[]").is_empty());
    }

    /// Only the rows that are waiting for connections, and only for the
    /// processes the port asked about — and the word "LISTENING" is never what
    /// decides it.
    #[test]
    fn the_listening_ports_are_read_by_their_own_shape() {
        let printed = "\
Active Connections

  Proto  Local Address          Foreign Address        State           PID
  TCP    127.0.0.1:58493        0.0.0.0:0              LISTENING       19644
  TCP    127.0.0.1:62406        93.184.216.34:443      ESTABLISHED     19644
  TCP    [::1]:62407            [::]:0                 LISTENING       19644
  TCP    127.0.0.1:62407        0.0.0.0:0              LISTENING       19644
  TCP    0.0.0.0:135            0.0.0.0:0              LISTENING       2548
  TCP    192.168.1.5:5000       0.0.0.0:0              LISTENING       19644
  UDP    127.0.0.1:9999         *:*                                    19644";

        let ports = ports_by_pid(printed);
        assert_eq!(ports.get(&19644), Some(&vec![58493u16, 62407]));
        // Another process's sockets are another process's.
        assert_eq!(ports.get(&2548), Some(&vec![135u16]));
    }

    /// A server listening on nothing cannot be asked anything, so it is dropped
    /// rather than tried and timed out.
    #[test]
    fn a_process_listening_on_nothing_is_not_asked() {
        let candidates = vec![
            (1u32, "one".to_string()),
            (2u32, "two".to_string()),
            (3u32, "three".to_string()),
        ];
        let mut ports: BTreeMap<u32, Vec<u16>> = BTreeMap::new();
        ports.insert(2, vec![62406]);
        ports.insert(3, Vec::new());

        assert_eq!(
            servers_from(&candidates, &ports),
            vec![Server {
                ports: vec![62406],
                token: "two".to_string()
            }]
        );
        assert!(servers_from(&candidates, &BTreeMap::new()).is_empty());
    }

    /// A quota reply with no groups in it is a reply with no windows, which is
    /// what `fetch_inner` reports as "no limits" rather than as a fault.
    #[test]
    fn a_reply_without_groups_has_no_windows() {
        assert!(windows_from(&serde_json::json!({})).is_empty());
        assert!(windows_from(&serde_json::json!({ "response": {} })).is_empty());
        assert!(windows_from(&serde_json::json!({ "response": { "groups": [] } })).is_empty());
        assert!(windows_from(&serde_json::json!({ "response": { "groups": [ {} ] } })).is_empty());
        assert!(windows_from(&serde_json::json!({ "groups": [ { "buckets": [
            { "bucketId": "a", "window": "5h", "remainingFraction": 0.5 }
        ] } ] }))
        .is_empty());
    }

    /// The three ways this ends without figures, in the original's own words.
    /// **Neither of the first two is a failure to be told about on a timer.**
    #[test]
    fn the_reasons_are_the_originals_three() {
        assert_eq!(
            Reason::NotRunning.message(),
            "Open Antigravity to see its usage."
        );
        assert_eq!(
            Reason::NotAnswering.message(),
            "Antigravity is open but didn't answer. Restarting it usually helps."
        );
        assert_eq!(Reason::NoLimitsReported.message(), "No limits reported.");
    }

    /// The commands are the two the machine actually has, and the process
    /// query carries no double quote — the character that would make its
    /// meaning depend on how the child's command line is re-split.
    #[test]
    fn the_discovery_commands_are_built_for_windows() {
        let arguments = process_arguments();
        assert_eq!(arguments[0], "-NoProfile");
        assert_eq!(arguments[1], "-NonInteractive");
        assert_eq!(arguments[2], "-Command");
        assert!(arguments[3].contains("Get-CimInstance Win32_Process"));
        assert!(arguments[3].contains("ConvertTo-Json"));
        assert!(!arguments[3].contains('"'));

        assert_eq!(port_arguments(), vec!["-ano", "-p", "tcp"]);
        assert!(powershell().to_string_lossy().ends_with("powershell.exe"));
        assert!(netstat().to_string_lossy().ends_with("netstat.exe"));
    }

    /// An install PulseWin cannot recognise is named by the environment, and
    /// the check never runs either discovery command.
    #[test]
    fn the_install_search_reads_folders_and_nothing_else() {
        let roots = vec![PathBuf::from("C:\\one"), PathBuf::from("C:\\two")];

        // The folder, and the program beside it.
        assert!(installed(&roots, &|path| path == Path::new("C:\\two\\Antigravity")));
        assert!(installed(&roots, &|path| {
            path == Path::new("C:\\one\\Antigravity IDE.exe")
        }));
        // Anything else is not an install.
        assert!(!installed(&roots, &|_| false));
        assert!(!installed(&roots, &|path| path == Path::new("C:\\one\\Something Else")));

        // The roots this machine can actually name.
        assert!(!candidate_roots().is_empty());
    }

    #[test]
    fn the_environment_a_helper_keeps_is_an_allow_list() {
        // What PulseWin holds for other providers never crosses.
        assert!(!kept(OsStr::new("PULSEWIN_CURSOR_COOKIE")));
        assert!(!kept(OsStr::new("PULSEWIN_ANTIGRAVITY_HOME")));
        assert!(!kept(OsStr::new("OPENAI_API_KEY")));
        assert!(!kept(OsStr::new("")));

        // What a system command cannot run without, and where it looks for
        // itself.
        assert!(kept(OsStr::new("SystemRoot")));
        assert!(kept(OsStr::new("PSModulePath")));
        assert!(kept(OsStr::new("USERPROFILE")));
        assert!(kept(OsStr::new("HTTPS_PROXY")));
        // `PATH` is not inherited but rebuilt, so it is not on the list.
        assert!(!kept(OsStr::new("PATH")));
    }

    /// `System32` leads `PATH`, so a program named rather than pathed — the
    /// fallback for both commands — is still the machine's own.
    #[test]
    fn system32_leads_the_path_a_helper_gets() {
        let environment = environment();
        let path = environment
            .iter()
            .find(|(name, _)| name == OsStr::new("PATH"))
            .map(|(_, value)| value.clone())
            .expect("a PATH");
        let path = path.to_string_lossy();
        assert!(path.starts_with("System32") || path.contains("System32"));
    }
}
