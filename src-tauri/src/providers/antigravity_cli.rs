//! Reading Antigravity's allowances through `agy`, the CLI the product ships.
//!
//! **Two routes, one mapping.** The language server in [`super::antigravity`]
//! answers a protobuf-JSON reply with `remainingFraction`; `agy` answers an
//! envelope whose usage payload is the same figures in snake_case
//! (`remaining_fraction`). The CLI payload is translated into the server's
//! shape here and then handed to [`super::antigravity::windows_from`], so the
//! two routes cannot drift: a window the app draws and a window the CLI draws
//! come from one function.
//!
//! **Where the binary is looked for.** `agy` is the CLI's own name; the editor's
//! `antigravity` command is a different program and is never asked. On Windows
//! an install lands on `PATH` (npm, mise, scoop), in the per-user programs
//! folder, or in `~/.local/bin`; Gemini's own copy sits in `~/.gemini/bin`. A
//! copy found in any of them is asked the same way.
//!
//! **A version is read before the question is put.** `agy` answers `/usage`
//! itself from 1.1.11; older builds print a turn instead, which reads as a
//! reply that is not an answer. The version gate turns that into "too old to
//! ask" rather than a parse failure.
//!
//! **Nothing here needs the network of a test**, or an `agy` of one:
//! [`locate_in`] takes the filesystem as a closure, [`answers_usage`] takes a
//! printed version, and [`answer_from`] takes the bytes a run would have
//! returned.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use serde_json::{json, Value};
use tokio::io::{AsyncRead, AsyncReadExt};

use crate::model::UsageWindow;

/// The first version that answers `/usage` with figures of its own. Older
/// builds print a turn: the question is understood, the answer is not usage.
pub const MINIMUM_VERSION: [u32; 3] = [1, 1, 11];

/// A run asks a local binary one question, so the ceiling is about a hung
/// process, not about patience.
const USAGE_DEADLINE: Duration = Duration::from_secs(30);
const VERSION_DEADLINE: Duration = Duration::from_secs(10);

/// Pipes are read to here and no further: a CLI that starts printing forever
/// must not become a memory leak in the panel.
const OUTPUT_CEILING: usize = 512 * 1024;
const VERSION_CEILING: usize = 4 * 1024;

/// Where the CLI writes its log. Every run writes a new file and repoints
/// `cli.log` at it, so the original sends the log to the null device; on Windows
/// that device is `NUL`.
#[cfg(windows)]
const NULL_DEVICE: &str = "NUL";
#[cfg(not(windows))]
const NULL_DEVICE: &str = "/dev/null";

/// The arguments the original passes. `-p` is the one-shot prompt, and the
/// output format is what makes the answer a document instead of prose.
pub fn arguments() -> Vec<String> {
    [
        "--log-file",
        NULL_DEVICE,
        "-p",
        "/usage",
        "--output-format",
        "json",
    ]
    .iter()
    .map(|argument| argument.to_string())
    .collect()
}

/// The names the binary may carry. On Windows an install is an `.exe` or a
/// batch shim; the bare name is still looked for, because a test fixture or a
/// hand-placed copy may be either.
fn binary_names() -> &'static [&'static str] {
    #[cfg(windows)]
    {
        &["agy.exe", "agy.cmd", "agy.bat", "agy"]
    }
    #[cfg(not(windows))]
    {
        &["agy"]
    }
}

/// The folders looked in, in order.
///
/// `PATH` first, because that is what the reader's own shell would run, then the
/// places a per-user install lands without ever touching `PATH`: the programs
/// folder npm and the Windows installer use, `~/.local/bin` where mise and pipx
/// put shims, mise's own shim folder, and Gemini's bundled copy under
/// `~/.gemini/bin`.
pub fn search_dirs() -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = std::env::var_os("PATH")
        .map(|path| std::env::split_paths(&path).collect())
        .unwrap_or_default();

    if let Some(local) = std::env::var_os("LOCALAPPDATA") {
        let local = PathBuf::from(local);
        dirs.push(local.join("Programs"));
        dirs.push(local);
    }
    if let Some(roaming) = std::env::var_os("APPDATA") {
        let roaming = PathBuf::from(roaming);
        dirs.push(roaming.join("Programs"));
        dirs.push(roaming);
    }
    if let Some(home) = std::env::var_os("USERPROFILE") {
        let home = PathBuf::from(home);
        dirs.push(home.join(".local").join("bin"));
        dirs.push(home.join(".local").join("share").join("mise").join("shims"));
        dirs.push(home.join(".gemini").join("bin"));
    }
    dirs
}

/// The `agy` this machine has, if it has one.
pub fn locate() -> Option<PathBuf> {
    locate_in(&search_dirs(), &|path| path.is_file())
}

/// The one location rule, with the filesystem passed in so a test can lay a
/// machine out and ask what would be found.
pub fn locate_in(dirs: &[PathBuf], is_executable: &dyn Fn(&Path) -> bool) -> Option<PathBuf> {
    for dir in dirs {
        for name in binary_names() {
            let candidate = dir.join(name);
            if is_executable(&candidate) {
                return Some(candidate);
            }
        }
    }
    None
}

/// Whether a `--version` print comes from a build that answers `/usage` itself.
///
/// Three numbers compared as numbers, because `1.1.10` is older than `1.1.11`
/// and a string comparison says the opposite. Two components are enough (the
/// original accepts `2.0`), a missing component counts as zero, and a part with
/// no digits in it is not a version: `agy version 1.3.5` prints prose, and prose
/// is not read as a number to be guessed at.
pub fn answers_usage(printed: &str) -> bool {
    let parts: Option<Vec<u32>> = printed
        .trim()
        .split('.')
        .map(|part| {
            let digits: String = part.chars().take_while(char::is_ascii_digit).collect();
            digits.parse::<u32>().ok()
        })
        .collect();

    let Some(parts) = parts else {
        return false;
    };
    if parts.len() < 2 {
        return false;
    }

    for index in 0..parts.len().max(MINIMUM_VERSION.len()) {
        let found = parts.get(index).copied().unwrap_or(0);
        let minimum = MINIMUM_VERSION.get(index).copied().unwrap_or(0);
        if found != minimum {
            return found > minimum;
        }
    }
    true
}

/// Why a run produced no figures. A clock that ran out is kept apart from an
/// exit that came back, because the reader's next step differs: wait, or look at
/// what the CLI says.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Fault {
    Unreadable,
    Unreachable,
}

/// Why the CLI was never asked. Not a fault of the account: this machine has no
/// `agy`, or has one from before the question could be understood.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NotAsked {
    Missing,
    Outdated,
}

/// What a run amounted to.
#[derive(Debug, Clone, PartialEq)]
pub enum Answer {
    Windows(Vec<UsageWindow>),
    NoLimits,
    Failed(Fault),
    NotAsked(NotAsked),
}

/// What an answer body amounts to.
///
/// The envelope is read rather than the tab-separated `response` text beside
/// it: percentages in that text are already rounded (0.998 prints as "100%"),
/// and a rounded percentage is a number Pulse would then have to believe.
pub fn answer_from(body: &[u8]) -> Answer {
    let Ok(reply) = serde_json::from_slice::<Value>(body) else {
        return Answer::Failed(Fault::Unreadable);
    };
    if reply.get("status").and_then(Value::as_str) != Some("SUCCESS") {
        return Answer::Failed(Fault::Unreadable);
    }
    let Some(usage) = reply
        .get("command")
        .and_then(|command| command.get("data"))
        .filter(|data| !data.is_null())
    else {
        return Answer::Failed(Fault::Unreadable);
    };

    let windows = super::windows_from(&as_quota_reply(usage));
    if windows.is_empty() {
        Answer::NoLimits
    } else {
        Answer::Windows(windows)
    }
}

/// The CLI's usage payload written in the language server's spelling, so both
/// routes end in one window builder. The names are the whole translation:
/// `name`→`displayName`, `id`→`bucketId`, `remaining_fraction`→
/// `remainingFraction`, `reset_time`→`resetTime`.
fn as_quota_reply(usage: &Value) -> Value {
    let groups: Vec<Value> = usage
        .get("groups")
        .and_then(Value::as_array)
        .map(|groups| {
            groups
                .iter()
                .map(|group| {
                    let buckets: Vec<Value> = group
                        .get("buckets")
                        .and_then(Value::as_array)
                        .map(|buckets| {
                            buckets
                                .iter()
                                .map(|bucket| {
                                    json!({
                                        "bucketId": bucket.get("id").cloned().unwrap_or(Value::Null),
                                        "window": bucket.get("window").cloned().unwrap_or(Value::Null),
                                        "remainingFraction": bucket.get("remaining_fraction").cloned().unwrap_or(Value::Null),
                                        "resetTime": bucket.get("reset_time").cloned().unwrap_or(Value::Null),
                                    })
                                })
                                .collect()
                        })
                        .unwrap_or_default();
                    json!({
                        "displayName": group.get("name").cloned().unwrap_or(Value::Null),
                        "buckets": buckets,
                    })
                })
                .collect()
        })
        .unwrap_or_default();

    json!({ "response": { "groups": groups } })
}

/// Ask the CLI: one run for the version, then one for the figures.
pub async fn ask() -> Answer {
    let Some(binary) = locate() else {
        return Answer::NotAsked(NotAsked::Missing);
    };

    match run(
        &binary,
        &["--version".to_string()],
        VERSION_DEADLINE,
        VERSION_CEILING,
        None,
    )
    .await
    {
        Run::Success(printed) if answers_usage(&String::from_utf8_lossy(&printed)) => {}
        // A binary that cannot be started at all is not installed, whatever the
        // folder it was found in says.
        Run::CouldNotStart => return Answer::NotAsked(NotAsked::Missing),
        _ => return Answer::NotAsked(NotAsked::Outdated),
    }

    // From a temporary folder: the CLI looks for project files around the folder
    // it is started in, and the panel is not a project.
    let folder = std::env::temp_dir();
    match run(
        &binary,
        &arguments(),
        USAGE_DEADLINE,
        OUTPUT_CEILING,
        Some(&folder),
    )
    .await
    {
        Run::Success(body) => answer_from(&body),
        Run::CouldNotStart => Answer::NotAsked(NotAsked::Missing),
        Run::TimedOut => Answer::Failed(Fault::Unreachable),
        Run::Exited => Answer::Failed(Fault::Unreadable),
    }
}

/// What a run of the binary amounted to. [`ask`] keeps these four apart because
/// the reader's next step depends on which one happened.
enum Run {
    Success(Vec<u8>),
    Exited,
    TimedOut,
    CouldNotStart,
}

/// One run, bounded in time and in bytes.
async fn run(
    binary: &Path,
    arguments: &[String],
    deadline: Duration,
    ceiling: usize,
    current_dir: Option<&Path>,
) -> Run {
    let mut command = command_for(binary);
    command.args(arguments);
    if let Some(directory) = current_dir {
        command.current_dir(directory);
    }

    let child = match command.spawn() {
        Ok(child) => child,
        Err(_) => return Run::CouldNotStart,
    };
    let collected = match tokio::time::timeout(deadline, collect(child, ceiling)).await {
        Ok(collected) => collected,
        // The child was created with `kill_on_drop`, so the deadline reaping it
        // is what stops the process.
        Err(_) => return Run::TimedOut,
    };

    if collected.success {
        Run::Success(collected.stdout)
    } else {
        Run::Exited
    }
}

/// A child's pipes drained to their ceilings and its exit status.
struct Collected {
    stdout: Vec<u8>,
    success: bool,
}

async fn collect(mut child: tokio::process::Child, ceiling: usize) -> Collected {
    let mut stdout: Option<tokio::process::ChildStdout> = child.stdout.take();
    let mut stderr: Option<tokio::process::ChildStderr> = child.stderr.take();
    let mut out = Vec::new();
    let mut err = Vec::new();

    // Both pipes are drained while the child runs: a helper that fills the
    // stderr pipe while stdout is read first would deadlock.
    let (_, _, status) = tokio::join!(
        drain(&mut stdout, &mut out, ceiling),
        drain(&mut stderr, &mut err, ceiling),
        child.wait()
    );

    Collected {
        stdout: out,
        success: status.map(|status| status.success()).unwrap_or(false),
    }
}

async fn drain<R: AsyncRead + Unpin>(pipe: &mut Option<R>, into: &mut Vec<u8>, ceiling: usize) {
    let Some(pipe) = pipe else {
        return;
    };
    let mut chunk = [0u8; 8 * 1024];
    while into.len() < ceiling {
        match pipe.read(&mut chunk).await {
            Ok(0) | Err(_) => return,
            Ok(read) => into.extend_from_slice(&chunk[..read]),
        }
    }
}

/// A command that starts the CLI the way the reader's shell would have: the
/// folder it was found in leads `PATH`, because an npm or mise install is a
/// wrapper whose interpreter sits beside it.
fn command_for(binary: &Path) -> tokio::process::Command {
    let batch = binary
        .extension()
        .map(|extension| {
            let extension = extension.to_string_lossy().to_ascii_lowercase();
            extension == "cmd" || extension == "bat"
        })
        .unwrap_or(false);

    let mut command = if batch {
        // A batch shim is not executable on its own: `cmd.exe /c` is what
        // Windows would do with it in a shell.
        let mut command = tokio::process::Command::new("cmd.exe");
        command.arg("/c").arg(binary);
        command
    } else {
        tokio::process::Command::new(binary)
    };

    command
        .env_clear()
        .envs(environment(binary))
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    #[cfg(windows)]
    command.creation_flags(super::CREATE_NO_WINDOW);
    command
}

/// The variables the CLI needs to find its own login and the network, and
/// nothing else: a GUI app inherits whatever launched it, and an unrelated
/// token in the environment is not this provider's to hand over.
fn environment(binary: &Path) -> Vec<(OsString, OsString)> {
    let mut environment: Vec<(OsString, OsString)> = std::env::vars_os()
        .filter(|(name, _)| super::kept(name))
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

    crate::preferences::load()
        .network_proxy
        .process_environment(environment)
}

/// Whether this machine has an `agy` at all, without asking it anything. The
/// caller uses it to decide whether a second route exists before spending a
/// process on finding out that it does not.
pub fn installed() -> bool {
    locate().is_some()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::WindowKind;

    /// A captured `agy -p /usage --output-format json` answer, in the fields
    /// this build reads. The text `response` beside the envelope is left out on
    /// purpose: it is not read.
    fn captured() -> Value {
        json!({
            "status": "SUCCESS",
            "command": {
                "name": "usage",
                "data": {
                    "groups": [
                        {
                            "name": "Gemini",
                            "buckets": [
                                { "id": "gemini-5h", "window": "5h", "remaining_fraction": 1, "reset_time": "2026-10-18T05:00:00Z" },
                                { "id": "gemini-weekly", "window": "weekly", "remaining_fraction": 0.75, "reset_time": "2026-10-18T10:19:51Z" }
                            ]
                        },
                        {
                            "name": "Claude and GPT",
                            "buckets": [
                                { "id": "3p-5h", "window": "5h", "remaining_fraction": 1, "reset_time": "2026-10-18T05:00:00Z" },
                                { "id": "3p-weekly", "window": "weekly", "remaining_fraction": 0.5, "reset_time": "2026-10-18T10:19:51Z" }
                            ]
                        }
                    ]
                }
            }
        })
    }

    /// The reply an account with a third, weekly-only group gives: no trailing
    /// "models", so the group is its own name.
    fn argon() -> Value {
        json!({
            "status": "SUCCESS",
            "command": {
                "name": "usage",
                "data": {
                    "groups": [
                        {
                            "name": "Gemini",
                            "buckets": [
                                { "id": "gemini-5h", "window": "5h", "remaining_fraction": 1 },
                                { "id": "gemini-weekly", "window": "weekly", "remaining_fraction": 0.75 }
                            ]
                        },
                        {
                            "name": "Claude and GPT",
                            "buckets": [
                                { "id": "3p-5h", "window": "5h", "remaining_fraction": 1 },
                                { "id": "3p-weekly", "window": "weekly", "remaining_fraction": 0.5 }
                            ]
                        },
                        {
                            "name": "Gemini 4 Argon",
                            "buckets": [
                                { "id": "gemini-argon-wtu-weekly", "window": "weekly", "remaining_fraction": 0.4, "reset_time": "2026-10-18T10:19:51Z" }
                            ]
                        }
                    ]
                }
            }
        })
    }

    /// The same account as [`captured`], described the way the language server
    /// describes it: the names differ, the figures do not.
    fn captured_as_server() -> Value {
        json!({
            "response": {
                "groups": [
                    {
                        "displayName": "Gemini",
                        "buckets": [
                            { "bucketId": "gemini-5h", "window": "5h", "remainingFraction": 1.0, "resetTime": "2026-10-18T05:00:00Z" },
                            { "bucketId": "gemini-weekly", "window": "weekly", "remainingFraction": 0.75, "resetTime": "2026-10-18T10:19:51Z" }
                        ]
                    },
                    {
                        "displayName": "Claude and GPT",
                        "buckets": [
                            { "bucketId": "3p-5h", "window": "5h", "remainingFraction": 1.0, "resetTime": "2026-10-18T05:00:00Z" },
                            { "bucketId": "3p-weekly", "window": "weekly", "remainingFraction": 0.5, "resetTime": "2026-10-18T10:19:51Z" }
                        ]
                    }
                ]
            }
        })
    }

    fn body(reply: &Value) -> Vec<u8> {
        serde_json::to_vec(reply).unwrap()
    }

    fn windows(answer: Answer) -> Vec<UsageWindow> {
        match answer {
            Answer::Windows(windows) => windows,
            other => panic!("expected windows, got {other:?}"),
        }
    }

    #[test]
    fn the_arguments_are_the_ones_the_original_passes() {
        assert_eq!(
            arguments(),
            vec![
                "--log-file".to_string(),
                NULL_DEVICE.to_string(),
                "-p".to_string(),
                "/usage".to_string(),
                "--output-format".to_string(),
                "json".to_string(),
            ]
        );
    }

    #[test]
    fn the_run_is_bounded_but_a_usage_question_is_given_time() {
        // The CLI took about eight seconds on the machine it was measured on, so
        // the version question gets ten and the usage question thirty.
        assert_eq!(VERSION_DEADLINE, Duration::from_secs(10));
        assert_eq!(USAGE_DEADLINE, Duration::from_secs(30));
        assert_eq!(VERSION_CEILING, 4 * 1024);
        assert_eq!(OUTPUT_CEILING, 512 * 1024);
    }

    #[test]
    fn only_a_version_that_answers_usage_itself_passes_the_gate() {
        for printed in [
            "1.3.5",
            "1.1.11",
            "1.1.12",
            "1.2.0",
            "2.0",
            "1.2.0-beta",
            "10.0.0",
            " 1.1.11\n",
        ] {
            assert!(answers_usage(printed), "{printed} should pass");
        }
        for printed in [
            "1.1.10",
            "1.1.3",
            "1.0.16",
            "0.9.0",
            "",
            "agy version 1.3.5",
            "1",
            "unknown",
        ] {
            assert!(!answers_usage(printed), "{printed} should not pass");
        }
    }

    /// The point of the translation: a payload from the CLI and the same figures
    /// from the language server become the same windows, ids, scopes, durations
    /// and percentages. If this drifts, the two routes draw different rings.
    #[test]
    fn the_cli_payload_is_the_same_windows_the_app_gives() {
        let from_cli = windows(answer_from(&body(&captured())));
        let from_server = super::super::windows_from(&captured_as_server());

        // The server reads `displayName`/`bucketId`/`remainingFraction`; the CLI
        // reads `name`/`id`/`remaining_fraction`. Translate the fixture so both
        // sides describe the same account.
        let pairs: Vec<(String, Option<String>, Option<f64>, Option<f64>, Option<String>)> = from_cli
            .iter()
            .map(|window| {
                (
                    window.label.clone(),
                    window.scope.clone(),
                    window.percent_used,
                    window.window_seconds,
                    window.id.clone(),
                )
            })
            .collect();
        assert_eq!(
            pairs,
            vec![
                (
                    "5h".to_string(),
                    Some("Gemini".to_string()),
                    Some(0.0),
                    Some(18_000.0),
                    Some("gemini-5h".to_string())
                ),
                (
                    "7d".to_string(),
                    Some("Gemini".to_string()),
                    Some(25.0),
                    Some(604_800.0),
                    Some("gemini-weekly".to_string())
                ),
                (
                    "5h".to_string(),
                    Some("Claude and GPT".to_string()),
                    Some(0.0),
                    Some(18_000.0),
                    Some("3p-5h".to_string())
                ),
                (
                    "7d".to_string(),
                    Some("Claude and GPT".to_string()),
                    Some(50.0),
                    Some(604_800.0),
                    Some("3p-weekly".to_string())
                ),
            ],
            "the order is the answer's own: groups as given, buckets by length"
        );
        assert_eq!(
            from_server, from_cli,
            "one mapping: the same figures from either route are the same windows"
        );
        assert!(
            from_cli.iter().all(|window| window.kind == WindowKind::Other),
            "a bucket is not a balance: the kind stays the neutral one"
        );
    }

    /// Upstream's third group since 2026-11: weekly-only, its own name, so it
    /// can be given a ring of its own.
    #[test]
    fn the_argon_weekly_bucket_keeps_its_own_place() {
        let argon = windows(answer_from(&body(&argon())));
        let group = argon
            .iter()
            .filter(|window| window.scope.as_deref() == Some("Gemini 4 Argon"))
            .collect::<Vec<_>>();
        assert_eq!(group.len(), 1, "a weekly-only group is one window");
        assert_eq!(group[0].id.as_deref(), Some("gemini-argon-wtu-weekly"));
        assert_eq!(group[0].window_seconds, Some(604_800.0));
        assert_eq!(group[0].percent_used, Some(60.0));
        assert!(!group[0].is_exhausted);
        assert_eq!(
            group[0].resets_at.as_deref(),
            Some("2026-10-18T10:19:51Z"),
            "the CLI's reset is read from reset_time"
        );
    }

    #[test]
    fn what_is_left_becomes_what_is_gone() {
        let reply = json!({
            "status": "SUCCESS",
            "command": { "data": { "groups": [ { "name": "Gemini", "buckets": [
                { "id": "a", "window": "5h", "remaining_fraction": 0.4 },
                { "id": "b", "window": "weekly", "remaining_fraction": 0.75 },
                { "id": "c", "window": "weekly", "remaining_fraction": 1 },
                { "id": "d", "window": "weekly", "remaining_fraction": 0 }
            ] } ] } }
        });
        let percent = |id: &str| {
            windows(answer_from(&body(&reply)))
                .into_iter()
                .find(|window| window.id.as_deref() == Some(id))
                .and_then(|window| window.percent_used)
        };
        assert_eq!(percent("a"), Some(60.0));
        assert_eq!(percent("b"), Some(25.0));
        assert_eq!(percent("c"), Some(0.0));
        assert_eq!(percent("d"), Some(100.0));
        assert!(
            windows(answer_from(&body(&reply)))
                .iter()
                .find(|window| window.id.as_deref() == Some("d"))
                .unwrap()
                .is_exhausted
        );
    }

    #[test]
    fn a_reply_that_reports_nothing_says_there_are_no_limits() {
        for data in [json!({ "groups": [] }), json!({})] {
            let reply = json!({ "status": "SUCCESS", "command": { "name": "usage", "data": data } });
            assert_eq!(
                answer_from(&body(&reply)),
                Answer::NoLimits,
                "an answer with no groups is an answer"
            );
        }
    }

    #[test]
    fn a_run_that_did_not_answer_is_a_failure_rather_than_an_empty_reading() {
        assert_eq!(
            answer_from(&body(&json!({ "status": "ERROR", "error": "rate limited" }))),
            Answer::Failed(Fault::Unreadable)
        );
        assert_eq!(
            answer_from(&body(&json!({ "status": "SUCCESS", "response": "a turn" }))),
            Answer::Failed(Fault::Unreadable)
        );
        assert_eq!(
            answer_from(&body(&json!({ "status": "SUCCESS", "command": { "name": "usage", "data": null } }))),
            Answer::Failed(Fault::Unreadable)
        );
        assert_eq!(answer_from(b""), Answer::Failed(Fault::Unreadable));
        assert_eq!(answer_from(b"not json"), Answer::Failed(Fault::Unreadable));
    }

    #[test]
    fn the_locations_searched_are_the_ones_a_machine_uses() {
        let dirs = vec![
            PathBuf::from(r"C:\Program Files\nodejs"),
            PathBuf::from(r"C:\Users\me\AppData\Local\Programs"),
            PathBuf::from(r"C:\Users\me\.local\bin"),
        ];
        let wanted = dirs[2].join(if cfg!(windows) { "agy.exe" } else { "agy" });
        let looks_for = wanted.clone();
        assert_eq!(
            locate_in(&dirs, &|path: &Path| path == looks_for),
            Some(wanted),
            "a copy in a per-user folder is found even when PATH has none"
        );
        assert_eq!(
            locate_in(&dirs, &|_| false),
            None,
            "nothing installed is nothing found"
        );

        // The first folder that has one wins, so an install on PATH is the one
        // the reader's own shell would run.
        let first = dirs[0].join(if cfg!(windows) { "agy.cmd" } else { "agy" });
        let wanted = vec![first.clone(), dirs[2].join("agy.exe")];
        assert_eq!(
            locate_in(&dirs, &|path: &Path| wanted.contains(&path.to_path_buf())),
            Some(first)
        );
    }

    #[test]
    fn the_helper_gets_an_allow_listed_environment_with_its_own_folder_first() {
        let binary = PathBuf::from(r"C:\Users\me\.local\bin\agy.exe");
        let environment = environment(&binary);
        let path = environment
            .iter()
            .find(|(name, _)| name == "PATH")
            .map(|(_, value)| value.to_string_lossy().to_string())
            .expect("a PATH");
        assert!(
            path.starts_with(binary.parent().unwrap().to_str().unwrap()),
            "the folder the CLI was found in leads: {path}"
        );
        assert!(
            environment
                .iter()
                // `PATH` is rebuilt rather than inherited: it leads with the
                // folder the binary was found in.
                .filter(|(name, _)| name != "PATH")
                .all(|(name, _)| super::super::kept(name)),
            "nothing outside the allow list is handed to the CLI"
        );
    }
}
