//! Starting a usage window as soon as it resets.
//!
//! **Why this exists.** Claude Code's five-hour window, and Codex's five hours
//! and week, start at the first message after a reset rather than at the reset
//! itself. Somebody who comes back three hours after a reset starts a fresh
//! window then, and if they run dry waits all of it out — where a window
//! started at the reset would already be three hours through. So PulseWin
//! sends one short message through the provider's own command-line tool, which
//! starts the clock at the reset.
//!
//! **The one place this app acts rather than reads.** Everything else here
//! fetches figures. This sends. It is therefore off by default, per provider,
//! and only after the reader has been shown what it means (the confirmation in
//! Settings); it holds no credential of its own, because the tool sends as
//! whoever that tool is already signed in as. Only the first account of each
//! provider is ever started, for the same reason.
//!
//! **By the clock, not by guessing from the reply.** How a window that has
//! reset but not restarted is reported differs by provider and is documented
//! by neither. What is certain is the reset time the last reading stated. So
//! each eligible window's reset is remembered, a timer is set for just after
//! it ([`is_due`]), and at that moment one message is sent — unless a reading
//! since shows a later reset, which means the reader has used it already.
//!
//! The two halves are kept apart on purpose: [`PrimerHours`], [`eligible`],
//! [`is_due`] and [`kept`] decide *when* and are pure, so they are tested
//! without a clock, a disk or a process. [`start`] and everything below it
//! decide *what is sent*.

use std::collections::{BTreeMap, BTreeSet};
use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use chrono::Timelike;
use serde_json::Value;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt};

use crate::credentials;

/// The providers the starter acts for. Claude Code's week resets at a fixed
/// hour whatever anybody does, so only its five hours are started; Codex's
/// five hours and week both wait for a first message, so both are.
pub const PROVIDERS: [&str; 2] = ["claude-code", "codex"];

/// A reading can arrive a little after the reset it stated; the window is
/// given this long to be over before it is started again.
pub const GRACE: i64 = 60;

/// A message takes seconds; a tool stuck on a prompt nobody will answer must
/// not hold anything up for longer than this.
pub const DEADLINE_SECS: u64 = 120;

/// Past this the rest of a tool's output is dropped. The reply is one word;
/// anything past a few kilobytes is a transcript or a debug dump, and holding
/// all of it would only be a way to run out of memory.
pub const OUTPUT_CEILING: usize = 64 * 1024;

/// Hours of the day the starter may act in, by the machine's own clock.
///
/// A window started at 3 a.m. resets at 8 — while somebody is asleep — and
/// again five hours later. Limiting it to waking hours costs nothing that
/// matters and keeps the account from looking like a machine that never
/// sleeps. `start == end` means all day; `start > end` is an overnight range.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct PrimerHours {
    /// 0–23. Equal to `end` means all day.
    pub start: u32,
    /// 0–23, exclusive.
    pub end: u32,
}

impl Default for PrimerHours {
    fn default() -> Self {
        Self { start: 7, end: 23 }
    }
}

impl PrimerHours {
    /// Whether the hour of the day is inside the range. Half-open, so with
    /// 07:00–23:00 the 07 hour counts and the 23 hour does not.
    pub fn contains_hour(&self, hour: u32) -> bool {
        if self.start == self.end {
            return true;
        }
        if self.start < self.end {
            (self.start..self.end).contains(&hour)
        } else {
            hour >= self.start || hour < self.end
        }
    }

    /// Whether `now` — epoch seconds — falls inside the range, read on this
    /// machine's clock. The local hour is the point: the reader set these
    /// hours thinking of their own morning, not of UTC.
    pub fn contains(&self, now: i64) -> bool {
        match local(now) {
            Some(at) => self.contains_hour(at.hour()),
            None => false,
        }
    }

    /// The next moment at or after `now` that is inside these hours.
    ///
    /// A reset that happens at 3 a.m. is not skipped, it is moved: the window
    /// is started when the allowed hours begin, so the reader wakes to a
    /// window that is already counting. The wall clock is resolved through the
    /// local zone rather than by adding hours, because a day the clocks move
    /// does not have the same number of hours in it.
    pub fn next_after(&self, now: i64) -> i64 {
        let Some(at) = local(now) else {
            return now;
        };
        if self.contains(now) {
            return now;
        }
        let start = self.start.min(23);
        let today = at.date_naive().and_hms_opt(start, 0, 0).and_then(local_instant);
        if let Some(today) = today.filter(|today| *today > now) {
            return today;
        }
        let tomorrow = at.date_naive() + chrono::Duration::days(1);
        tomorrow
            .and_hms_opt(start, 0, 0)
            .and_then(local_instant)
            .unwrap_or(now + 86_400)
    }
}

fn local(now: i64) -> Option<chrono::DateTime<chrono::Local>> {
    chrono::DateTime::from_timestamp(now, 0).map(|at| at.with_timezone(&chrono::Local))
}

fn local_instant(naive: chrono::NaiveDateTime) -> Option<i64> {
    use chrono::TimeZone;
    // A wall-clock time that the zone skips (the hour a spring-forward drops)
    // has no instant; the earliest reading that does exist is the one the
    // clock shows next.
    chrono::Local
        .from_local_datetime(&naive)
        .earliest()
        .map(|at| at.timestamp())
}

/// The window's reset as epoch seconds, when the port's RFC3339 string parses.
fn reset_epoch(window: &crate::model::UsageWindow) -> Option<i64> {
    window
        .resets_at
        .as_deref()
        .and_then(|text| chrono::DateTime::parse_from_rfc3339(text).ok())
        .map(|at| at.timestamp())
}

/// Whether starting this window is what the feature is for.
///
/// **Account-wide limits only.** A limit scoped to one model (Claude's Opus or
/// Sonnet week, Codex's per-model windows) is a different allowance; starting
/// it would spend the wrong one, so a window with a `scope` is never eligible.
///
/// **Decided from the duration the provider stated, not from a kind.** The
/// original asked its model for `kind == .fiveHour` or `.weekly`, which this
/// port does not carry: its `WindowKind` is a coarse bucket (limit, balance,
/// top-up, …) that says what a number means, not how long the window is. What
/// this port does have, and only when the provider itself said so, is
/// `window_seconds`. So 18,000 seconds is the five-hour window and 604,800 the
/// week, and a label of exactly "5h" or "7d" is accepted as a fallback for the
/// older replies that carry no duration — nothing is inferred from a longer
/// label like "7d Opus", and no duration is guessed from one.
///
/// The original also refused windows it had *estimated*; this port has no
/// estimated windows at all, so there is nothing extra to refuse.
pub fn eligible(provider: &str, window: &crate::model::UsageWindow) -> bool {
    if window.scope.is_some() {
        return false;
    }
    let seconds = window.window_seconds.or_else(|| label_seconds(&window.label));
    match (provider, seconds) {
        // Claude Code's week resets at a fixed hour whatever anybody does, so
        // only the five hours are worth starting.
        ("claude-code", Some(seconds)) => within(seconds, 18_000.0),
        // Codex's five hours and week both wait for a first message.
        ("codex", Some(seconds)) => within(seconds, 18_000.0) || within(seconds, 604_800.0),
        _ => false,
    }
}

/// A stated duration, compared with enough slack for the fractional seconds
/// some replies carry (18000.0, 1.8e4, 17999.999) and no more.
fn within(seconds: f64, expected: f64) -> bool {
    seconds.is_finite() && (seconds - expected).abs() <= 1.0
}

/// The duration a label names, for replies that carry no `window_seconds`.
/// Exact matches only, so "7d Opus" is not read as the account's week.
fn label_seconds(label: &str) -> Option<f64> {
    let label = label.trim().to_ascii_lowercase().replace(' ', "");
    match label.as_str() {
        "5h" => Some(18_000.0),
        "7d" | "weekly" | "week" => Some(604_800.0),
        _ => None,
    }
}

/// Whether a window whose last known reset was `reset` should be started now.
/// `current` is the window as the latest reading has it.
///
/// `reset` alone is not enough: the reset has to have passed by [`GRACE`], it
/// has to be inside the allowed hours, and the latest reading must not show a
/// **later** reset — a later one means somebody used the window since, so
/// there is nothing left to start.
pub fn is_due(
    reset: Option<i64>,
    current: Option<&crate::model::UsageWindow>,
    now: i64,
    hours: &PrimerHours,
) -> bool {
    let Some(reset) = reset else {
        return false;
    };
    if now < reset.saturating_add(GRACE) {
        return false;
    }
    if !hours.contains(now) {
        return false;
    }
    if current
        .and_then(reset_epoch)
        .is_some_and(|next| next > reset.saturating_add(GRACE))
    {
        return false;
    }
    true
}

/// The noted resets of the providers that will still be started.
///
/// **A reset nobody will act on is not something to wake for.** Turning the
/// starter off, or the account, used to leave its noted resets behind; once
/// they had passed, every timer came due at once, each fire skipped them, and
/// the loop woke again five seconds later for as long as the app ran. Keys are
/// `"{account_id}|{window_id}"`, so a provider's prefix is its first account's
/// id and nothing else — an added account's `codex--account-…` key is dropped,
/// which is correct because only the first account is ever started.
pub fn kept(
    resets: &BTreeMap<String, i64>,
    starting: &BTreeSet<&str>,
) -> BTreeMap<String, i64> {
    let prefixes: Vec<String> = starting.iter().map(|provider| format!("{provider}|")).collect();
    resets
        .iter()
        .filter(|(key, _)| prefixes.iter().any(|prefix| key.starts_with(prefix.as_str())))
        .map(|(key, reset)| (key.clone(), *reset))
        .collect()
}

// ---------------------------------------------------------------------------
// What is sent
// ---------------------------------------------------------------------------

/// What became of one attempt. The raw strings are the original's, so a value
/// written by one build is still readable by the other and by the pane.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    /// The tool ran and reported success. The window is counting.
    Sent,
    /// The tool is not installed anywhere this port looks.
    ToolMissing,
    /// It ran and failed — signed out, offline, a model gone.
    Failed,
    /// It did not finish inside [`DEADLINE_SECS`].
    TimedOut,
}

impl Outcome {
    pub fn as_str(self) -> &'static str {
        match self {
            Outcome::Sent => "sent",
            Outcome::ToolMissing => "toolMissing",
            Outcome::Failed => "failed",
            Outcome::TimedOut => "timedOut",
        }
    }

    /// The outcome a stored string names, for a value written by an older
    /// build. Unknown text is `None` rather than a guess, so the pane can say
    /// nothing instead of something false.
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "sent" => Some(Outcome::Sent),
            "toolMissing" => Some(Outcome::ToolMissing),
            "failed" => Some(Outcome::Failed),
            "timedOut" => Some(Outcome::TimedOut),
            _ => None,
        }
    }
}

/// Where the tools are run from: empty, and PulseWin's own, so no project's
/// settings or instructions are picked up. Claude Code loads `project`
/// settings, and the point of this folder is that there are none to load.
pub fn directory() -> PathBuf {
    crate::settings::directory()
        .unwrap_or_else(std::env::temp_dir)
        .join("Window starter")
}

/// `claude -p` with Haiku, and with everything that could do more than answer
/// one word turned off.
///
/// **`--tools ""`** is what stops the model from running anything. **`--strict-mcp-config`**
/// is what keeps the reader's MCP servers out of it. **`--setting-sources project`**
/// loads only the empty folder's settings, which is to say none: the reader's
/// own hooks — sounds, notifications, PulseWin's own status line — do not run
/// for this. **`--no-session-persistence`** is why nothing is left behind.
/// Do not shorten this list; each entry is one of those four promises.
pub fn claude_arguments() -> Vec<String> {
    [
        "-p",
        "hi",
        "--model",
        "haiku",
        "--no-session-persistence",
        "--tools",
        "",
        "--strict-mcp-config",
        "--setting-sources",
        "project",
    ]
    .iter()
    .map(|argument| argument.to_string())
    .collect()
}

/// `codex exec`, ephemeral, read-only, in PulseWin's own folder, at the lowest
/// reasoning effort — and with `model` when one cheaper than the account's
/// default was found.
///
/// **`--ephemeral`** is what stops a session being written. **`-s read-only`**
/// is what stops it changing a file. **`model_reasoning_effort="low"`** is what
/// keeps one word from costing a reasoning pass. Do not shorten this list
/// either.
pub fn codex_arguments(model: Option<&str>, folder: &Path) -> Vec<String> {
    let mut arguments: Vec<String> = [
        "exec",
        "--ephemeral",
        "--skip-git-repo-check",
        "-s",
        "read-only",
        "-C",
    ]
    .iter()
    .map(|argument| argument.to_string())
    .collect();
    arguments.push(folder.to_string_lossy().into_owned());
    arguments.push("-c".to_string());
    arguments.push("model_reasoning_effort=\"low\"".to_string());
    if let Some(model) = model {
        arguments.push("-m".to_string());
        arguments.push(model.to_string());
    }
    arguments.push("hi".to_string());
    arguments
}

/// The id a model entry is named by, whichever of the two fields carries it.
fn model_id(model: &Value) -> Option<&str> {
    model
        .get("id")
        .and_then(Value::as_str)
        .or_else(|| model.get("model").and_then(Value::as_str))
}

/// The cheapest model in Codex's own list: one named Luna — the fast, efficient
/// tier at the time of writing — else one described as fast, efficient or mini.
/// `None` keeps the account's default, which still costs next to nothing for
/// one word.
///
/// Read from the list each time rather than written down here, because OpenAI
/// renames these tiers. A hidden entry is not on offer to this account, so it
/// is skipped even when its name matches.
pub fn cheapest_codex_model(reply: &Value) -> Option<String> {
    let models = reply
        .get("data")
        .and_then(Value::as_array)
        .or_else(|| {
            reply
                .get("result")
                .and_then(|result| result.get("data"))
                .and_then(Value::as_array)
        })?;
    let offered: Vec<&Value> = models
        .iter()
        .filter(|model| model.get("hidden").and_then(Value::as_bool) != Some(true))
        .collect();
    if let Some(luna) = offered.iter().find(|model| {
        model_id(model).is_some_and(|id| id.to_ascii_lowercase().contains("luna"))
    }) {
        return model_id(luna).map(str::to_owned);
    }
    let light = offered.iter().find(|model| {
        let words = format!(
            "{} {}",
            model
                .get("description")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_ascii_lowercase(),
            model_id(model).unwrap_or("").to_ascii_lowercase(),
        );
        ["fast", "efficient", "mini"]
            .iter()
            .any(|word| words.contains(word))
    });
    light.and_then(|model| model_id(model)).map(str::to_owned)
}

/// The folder Claude Code keeps for a working directory: the path with
/// everything but ASCII letters and digits turned into dashes.
///
/// This is Claude Code's own naming rule, not PulseWin's. It is needed to find
/// the empty folder that a run leaves behind, so it has to match exactly.
pub fn claude_project_folder(directory: &Path, home: &Path) -> PathBuf {
    let name: String = directory
        .to_string_lossy()
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() {
                character
            } else {
                '-'
            }
        })
        .collect();
    home.join(".claude").join("projects").join(name)
}

/// Claude Code makes a project folder for the starter's directory even when
/// told not to keep the session — empty, but listed with the reader's real
/// ones. It is removed afterwards when it holds no file at all; anything with
/// a file in it is left exactly as it is, because at that point it is not
/// PulseWin's to delete.
fn remove_empty_claude_project(directory: &Path, home: &Path) {
    let project = claude_project_folder(directory, home);
    if holds_a_file(&project) {
        return;
    }
    let _ = std::fs::remove_dir_all(&project);
}

/// Whether anything in the tree is a regular file. Symbolic links are not
/// followed and not counted, which is what the original's resource check did:
/// a link is neither the file it points at nor a file of its own here.
fn holds_a_file(path: &Path) -> bool {
    let Ok(entries) = std::fs::read_dir(path) else {
        return false;
    };
    for entry in entries.flatten() {
        let Ok(kind) = entry.file_type() else {
            return true;
        };
        if kind.is_file() {
            return true;
        }
        if kind.is_dir() && holds_a_file(&entry.path()) {
            return true;
        }
    }
    false
}

/// Locate the tool and run it. Nothing here records anything: the caller owns
/// the outcome, because only it knows which provider and which account.
pub async fn start(provider: &str, folder: &Path) -> Outcome {
    // A folder that cannot be made is not a reason to send from somewhere
    // else: the whole point of it is that nothing of the reader's is in it.
    let _ = std::fs::create_dir_all(folder);
    match provider {
        "claude-code" => {
            let home = credentials::home_dir().unwrap_or_default();
            let extra = vec![home.join(".claude").join("local").join("claude")];
            let Some(binary) = locate("claude", &extra) else {
                return Outcome::ToolMissing;
            };
            let outcome = run(&binary, &claude_arguments(), folder).await;
            remove_empty_claude_project(folder, &home);
            outcome
        }
        "codex" => {
            let Some(binary) = locate("codex", &[]) else {
                return Outcome::ToolMissing;
            };
            // The model list is a courtesy: if asking for it fails, the
            // account's default is used and the message still goes.
            let model = codex_model_list(&binary, folder)
                .await
                .and_then(|reply| cheapest_codex_model(&reply));
            run(&binary, &codex_arguments(model.as_deref(), folder), folder).await
        }
        _ => Outcome::Failed,
    }
}

// ---------------------------------------------------------------------------
// Where the tool is
// ---------------------------------------------------------------------------

/// A run past this is stopped. Asking for the model list is a conversation
/// with a helper, so it gets its own, shorter limit than the message itself.
const MODELS_DEADLINE: Duration = Duration::from_secs(20);

/// A console window must not flash up in front of the panel.
#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// The extensions a Windows install gives a command. `ps1` is left out on
/// purpose: running one would need a second interpreter between this and the
/// tool, and that interpreter is not what the tool's own installer signed up
/// for.
#[cfg(windows)]
const EXTENSIONS: [&str; 3] = ["exe", "cmd", "bat"];

/// The file names a folder is asked for, in the order they are tried in it.
#[cfg(windows)]
fn names(name: &str) -> Vec<String> {
    EXTENSIONS
        .iter()
        .map(|extension| format!("{name}.{extension}"))
        .collect()
}

#[cfg(not(windows))]
fn names(name: &str) -> Vec<String> {
    vec![name.to_string()]
}

/// A path given outright, plus the same path with a Windows extension. The
/// original's `extra` entries are full paths to a binary; on Windows the same
/// install is `claude.cmd` or `codex.exe`, so the bare path alone would miss
/// every install that a package manager made.
fn extra_names(path: &Path) -> Vec<PathBuf> {
    #[allow(unused_mut)]
    let mut out = vec![path.to_path_buf()];
    #[cfg(windows)]
    if path.extension().is_none() {
        for extension in EXTENSIONS {
            out.push(path.with_extension(extension));
        }
    }
    out
}

/// The `PATH` folders, in their own order. A GUI app inherits almost none of
/// what a terminal has, which is why the folders below exist at all — but what
/// it did inherit is still the first place to look.
fn path_dirs() -> Vec<PathBuf> {
    std::env::var_os("PATH")
        .map(|path| std::env::split_paths(&path).collect())
        .unwrap_or_default()
}

/// Folders that hold a package-manager install of a command directly.
///
/// npm's global folder is `%APPDATA%\npm` on Windows, which is where
/// `claude.cmd` and `codex.cmd` land; the rest are the folders the tools' own
/// installers and the other package managers use. The macOS ones are kept so a
/// development machine finds the same install a terminal does — they simply do
/// not exist on Windows.
fn install_dirs() -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = Vec::new();
    dirs.push(PathBuf::from("/opt/homebrew/bin"));
    dirs.push(PathBuf::from("/usr/local/bin"));
    dirs.extend(credentials::config_relative(&["npm"]));
    dirs.extend(credentials::home_relative(&[".local", "bin"]));
    dirs.extend(credentials::home_relative(&[".bun", "bin"]));
    dirs.extend(credentials::home_relative(&[".volta", "bin"]));
    dirs.extend(credentials::home_relative(&[".npm-global", "bin"]));
    dirs.extend(credentials::home_relative(&[".local", "share", "pnpm"]));
    dirs.retain(|dir| !dir.as_os_str().is_empty());
    dirs
}

/// The command under every Node version manager's version folders, newest
/// version first.
///
/// An npm-installed tool is a script with an interpreter in the same folder,
/// so a version manager keeps each Node version's copy apart and the newest is
/// the one the reader is most likely to be running. `versions` lists a folder,
/// so a test needs no disk; the listing is compared lexically, which is the
/// original's rule and is exact for the `v<major>.<minor>.<patch>` names these
/// managers use.
fn managed_dirs(name: &str, home: &Path, versions: &dyn Fn(&Path) -> Vec<String>) -> Vec<PathBuf> {
    const MANAGERS: [(&str, &str); 4] = [
        (".nvm/versions/node", "bin"),
        ("Library/Application Support/fnm/node-versions", "installation/bin"),
        (".local/share/fnm/node-versions", "installation/bin"),
        (".local/share/mise/installs/node", "bin"),
    ];
    let mut out = Vec::new();
    for (root, bin) in MANAGERS {
        let root = home.join(root);
        let mut listed = versions(&root);
        listed.sort();
        for version in listed.iter().rev() {
            let folder = root.join(version).join(bin);
            for name in names(name) {
                out.push(folder.join(&name));
            }
        }
    }
    out
}

/// Every candidate in the order it is tried: `PATH` first, then the paths
/// given outright, then the usual folders, then the version managers.
fn candidates(
    name: &str,
    path_dirs: &[PathBuf],
    extra: &[PathBuf],
    install: &[PathBuf],
    managed: &[PathBuf],
) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for dir in path_dirs {
        for name in names(name) {
            out.push(dir.join(&name));
        }
    }
    for path in extra {
        out.extend(extra_names(path));
    }
    for dir in install {
        for name in names(name) {
            out.push(dir.join(&name));
        }
    }
    out.extend(managed.iter().cloned());
    out
}

/// The first candidate that is a file. The check is injected so a test can
/// drive the search without installing anything.
fn locate_in(listed: &[PathBuf], exists: &dyn Fn(&Path) -> bool) -> Option<PathBuf> {
    listed.iter().find(|path| exists(path)).cloned()
}

/// Where a command-line tool is, or `None` when it is nowhere this port knows
/// to look.
pub fn locate(name: &str, extra: &[PathBuf]) -> Option<PathBuf> {
    let versions = |dir: &Path| -> Vec<String> {
        std::fs::read_dir(dir)
            .map(|entries| {
                entries
                    .flatten()
                    .filter(|entry| entry.file_type().map(|kind| kind.is_dir()).unwrap_or(false))
                    .filter_map(|entry| entry.file_name().into_string().ok())
                    .collect()
            })
            .unwrap_or_default()
    };
    let managed = credentials::home_dir()
        .map(|home| managed_dirs(name, &home, &versions))
        .unwrap_or_default();
    let listed = candidates(name, &path_dirs(), extra, &install_dirs(), &managed);
    locate_in(&listed, &|path| path.is_file())
}

// ---------------------------------------------------------------------------
// Running it
// ---------------------------------------------------------------------------

/// The variables a helper keeps. **An allow-list, not a deny-list**: what is
/// not named here does not cross into it, so no key, cookie or credential of
/// PulseWin's own reaches a program this runs on the reader's behalf.
///
/// The two tool-specific entries are the tools' own config folders — where
/// each keeps the login this feature deliberately does not hold. They are
/// settings, not secrets.
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
    // The reader's home, which is where both tools keep their login.
    "USERPROFILE",
    "HOMEDRIVE",
    "HOMEPATH",
    "APPDATA",
    "LOCALAPPDATA",
    "CLAUDE_CONFIG_DIR",
    "CODEX_HOME",
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

fn kept_variable(name: &OsStr) -> bool {
    let name = name.to_string_lossy();
    !name.is_empty() && KEPT.iter().any(|kept| name.eq_ignore_ascii_case(kept))
}

/// Only what the tool needs to find itself, its login and the network, with the
/// folder it was found in at the front of `PATH`.
///
/// The leading folder is what the original does for every helper it starts: an
/// npm install is a script whose interpreter sits beside it, and a GUI app's
/// `PATH` is the one Windows booted with.
fn environment(binary: &Path) -> Vec<(OsString, OsString)> {
    let mut environment: Vec<(OsString, OsString)> = std::env::vars_os()
        .filter(|(name, _)| kept_variable(name))
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
/// an npm-installed `claude.cmd` is.
///
/// Standard input is not set here: the message runs with none, while asking for
/// the model list needs a pipe to write the handshake into.
fn command_for(binary: &Path, folder: &Path) -> tokio::process::Command {
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
        // The starter's own empty folder, so no project's settings or
        // instructions are picked up from wherever PulseWin happens to run.
        .current_dir(folder)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        // A run that is dropped — the deadline below — takes the child with it,
        // so a tool that ignores everything does not stay running.
        .kill_on_drop(true);

    #[cfg(windows)]
    command.creation_flags(CREATE_NO_WINDOW);

    command
}

/// The tool's output, or why there is none.
async fn run(binary: &Path, arguments: &[String], folder: &Path) -> Outcome {
    let mut command = command_for(binary, folder);
    command
        .args(arguments)
        // Nothing to answer with, so a tool that asks gets EOF rather than
        // blocking on a terminal that is not there.
        .stdin(Stdio::null());
    let child = match command.spawn() {
        Ok(child) => child,
        // Not found, or not startable — either way there is nothing the reader
        // can do from inside the pane but check the tool is installed.
        Err(_) => return Outcome::ToolMissing,
    };

    match tokio::time::timeout(Duration::from_secs(DEADLINE_SECS), collect(child)).await {
        Ok(collected) if collected.success => Outcome::Sent,
        Ok(_) => Outcome::Failed,
        Err(_) => Outcome::TimedOut,
    }
}

struct Collected {
    #[allow(dead_code)]
    stdout: Vec<u8>,
    #[allow(dead_code)]
    stderr: String,
    success: bool,
}

/// Drains both pipes at once, and then waits for the process.
///
/// **Both at once.** Reading one to EOF and only then reading the other
/// deadlocks the moment the child writes more than a pipe buffer to the one
/// nobody is reading — a transcript, a debug dump — and past [`OUTPUT_CEILING`]
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

/// The models the account may use, by talking to `codex app-server`.
///
/// This is Codex's own documented protocol, and it is the reason PulseWin
/// needs no Codex credential to pick a model: the helper is already signed in.
/// The handshake is the one the original sends — `initialize`, then
/// `initialized`, then `model/list`.
///
/// **Every failure is the same failure: no model.** The caller falls back to
/// the account's default, which is still nearly free for one word, so a helper
/// that is too old, refuses, or hangs must not stop the message that this
/// feature exists to send. Standard error is drained into nothing for the same
/// reason it is drained elsewhere: a helper that fills that pipe would wait for
/// a reader for ever.
async fn codex_model_list(binary: &Path, folder: &Path) -> Option<Value> {
    let mut command = command_for(binary, folder);
    command.stdin(Stdio::piped());
    let mut child = command.spawn().ok()?;
    let mut stdin = child.stdin.take()?;
    let stdout = child.stdout.take()?;
    if let Some(mut pipe) = child.stderr.take() {
        tokio::spawn(async move {
            let mut sink = Vec::new();
            drain(&mut Some(&mut pipe), &mut sink).await;
        });
    }

    let handshake = [
        serde_json::json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"clientInfo":{"name":"PulseWin","title":"PulseWin","version":"0.1"}}}),
        serde_json::json!({"jsonrpc":"2.0","method":"initialized","params":{}}),
        serde_json::json!({"jsonrpc":"2.0","id":2,"method":"model/list","params":{}}),
    ];
    let mut text = String::new();
    for message in handshake {
        text.push_str(&message.to_string());
        text.push('\n');
    }
    if stdin.write_all(text.as_bytes()).await.is_err() || stdin.flush().await.is_err() {
        return None;
    }

    let reply = tokio::time::timeout(MODELS_DEADLINE, async move {
        let mut reader = tokio::io::BufReader::new(stdout);
        let mut line = String::new();
        loop {
            line.clear();
            match reader.read_line(&mut line).await {
                Ok(0) | Err(_) => return None,
                Ok(_) => {}
            }
            let Ok(message) = serde_json::from_str::<Value>(line.trim()) else {
                continue;
            };
            if message.get("id").and_then(Value::as_i64) == Some(2) {
                return Some(message);
            }
        }
    })
    .await;

    // The helper is a child of this process for the length of one question;
    // it must not outlive the question. Waiting after the kill is what keeps it
    // from being left behind as a process nobody reaps.
    let _ = child.start_kill();
    let _ = tokio::time::timeout(Duration::from_secs(2), child.wait()).await;
    reply.ok().flatten()
}

// ---------------------------------------------------------------------------
// The scheduler — the original's `WindowPrimer`, driven by the refresh cycle
// ---------------------------------------------------------------------------

/// How long the loop may sleep before it looks again.
///
/// The original hung a one-shot `Timer` on the main run loop and let
/// observation call it back. There is no observation here, so the timer
/// itself has to be the thing that looks: it is set for the soonest moment
/// something could be due and capped so a change in the readings (a provider
/// used in the meantime) is noticed without waiting out a five-hour window.
pub const MAX_WAIT_SECS: i64 = 300;

/// The soonest moment anything could come due, in epoch seconds, or `None`
/// when nothing is waiting.
///
/// This is the original's `schedule()`, minus the timer: a reset that passed
/// outside the allowed hours moves to when they open, and a reset that has not
/// happened yet is waited for with [`GRACE`] on top. The result is never in
/// the past and never sooner than [`MIN_WAIT_SECS`] from `now`, because a
/// timer that fires immediately in a loop is the bug the original's `max(…, 5)`
/// was there to prevent.
pub fn next_wake(resets: &BTreeMap<String, i64>, now: i64, hours: &PrimerHours) -> Option<i64> {
    resets
        .values()
        .map(|reset| hours.next_after((reset.saturating_add(GRACE)).max(now)))
        .min()
        .map(|soonest| soonest.clamp(now.saturating_add(MIN_WAIT_SECS), now.saturating_add(MAX_WAIT_SECS)))
}

/// The shortest gap between two looks. The original used five seconds; the
/// refresh cycle this runs beside is measured in minutes, so anything shorter
/// only costs a wake-up.
pub const MIN_WAIT_SECS: i64 = 5;

/// The account whose windows a provider's key belongs to.
///
/// **Only the first account of each provider is ever started** — the tools
/// send as whoever they are signed in as, which is that account — so this is
/// the provider's own id and never an added account's `…--account-…` id.
fn key_account(key: &str) -> Option<&str> {
    key.split_once('|').map(|(account, _)| account)
}

/// The window a key names, for asking the latest reading whether the reset it
/// stated is still the coming one.
fn key_window<'a>(key: &str, windows: &'a [crate::model::UsageWindow]) -> Option<&'a crate::model::UsageWindow> {
    let id = key.split_once('|')?.1;
    windows.iter().find(|window| window.id.as_deref() == Some(id))
}

/// What one look at the readings concluded.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Due {
    /// Provider ids that should be started now, in [`PROVIDERS`] order.
    pub providers: Vec<String>,
    /// The keys whose reset was acted on, already removed by the caller.
    pub keys: Vec<String>,
}

/// What a look decided, given the readings, the switches, and the noted resets.
///
/// Pure, so the whole of "who is due" is tested without a clock, a disk or a
/// process — the same split the original makes between `record`/`fire` and the
/// timers they hang off. `resets` is the note from previous looks, which this
/// call also *updates*: every eligible window still heading for a future reset
/// is noted, exactly as the original's `record()` does, because a window first
/// seen after its reset would otherwise never be started.
pub fn review(
    resets: &mut BTreeMap<String, i64>,
    readings: &[crate::model::ProviderUsage],
    primed: &[String],
    enabled: &[String],
    attempted: &std::collections::HashMap<String, i64>,
    hours: &PrimerHours,
    now: i64,
) -> Due {
    // Neither switch is a default. With nothing turned on this does nothing at
    // all, which is why it can be called from every refresh unconditionally.
    let starting: BTreeSet<&str> = PROVIDERS
        .iter()
        .copied()
        .filter(|provider| primed.iter().any(|id| id == provider) && enabled.iter().any(|id| id == provider))
        .collect();

    // A reset nobody will act on is not something to wake for: drop the notes
    // of every provider that is off now, before anything is compared with them.
    *resets = kept(resets, &starting);

    let mut due = Due::default();
    for provider in PROVIDERS {
        if !starting.contains(provider) {
            continue;
        }
        let Some(reading) = readings.iter().find(|reading| reading.id == provider) else {
            continue;
        };
        // Note what is coming, then ask which notes are due. Order matters:
        // a window whose reset is in the past is not re-noted, and one whose
        // reset just moved later is not due — `is_due` reads the latest
        // reading and sees the later reset.
        for window in &reading.windows {
            if !eligible(provider, window) {
                continue;
            }
            let Some(reset) = reset_epoch(window) else { continue };
            if reset <= now {
                continue;
            }
            let key = format!("{}|{}", reading.id, window.id.as_deref().unwrap_or(""));
            resets.insert(key, reset);
        }
        let mut fired = false;
        for (key, reset) in resets.iter() {
            if key_account(key) != Some(reading.id.as_str()) {
                continue;
            }
            if !is_due(Some(*reset), key_window(key, &reading.windows), now, hours) {
                continue;
            }
            // **A failure is not retried until the next reset.** The note says
            // when the reset was, and `primer_run_times` says when this
            // provider was last sent for — an attempt after that reset means
            // this one has been answered, whatever came of it, and sending
            // again would only repeat a tool that is not installed once per
            // look for the rest of the day.
            if attempted
                .get(provider)
                .is_some_and(|at| *at >= reset.saturating_add(GRACE))
            {
                continue;
            }
            due.keys.push(key.clone());
            fired = true;
        }
        if fired {
            due.providers.push((*provider).to_owned());
        }
    }
    for key in &due.keys {
        resets.remove(key);
    }
    due
}

/// What the app must do after a look: send, then record what came of it.
///
/// Split from the look itself so the decision stays pure and the sending is
/// one place. The caller owns the persistence, because the preferences are the
/// caller's.
pub struct Done {
    pub provider: String,
    pub outcome: Outcome,
}

/// Look, send for whoever is due, and report what to remember.
///
/// **Never blocks a refresh past one look.** The callers spawn this, and the
/// only thing it waits on is the provider's own tool — with a deadline in
/// [`start`] — so a tool that hangs cannot hold a ring on screen.
pub async fn advance(
    resets: &mut BTreeMap<String, i64>,
    readings: &[crate::model::ProviderUsage],
    primed: &[String],
    enabled: &[String],
    attempted: &std::collections::HashMap<String, i64>,
    hours: &PrimerHours,
    folder: &Path,
    now: i64,
) -> Vec<Done> {
    let due = review(resets, readings, primed, enabled, attempted, hours, now);
    let mut done = Vec::new();
    for provider in due.providers {
        let outcome = start(&provider, folder).await;
        done.push(Done { provider, outcome });
    }
    done
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{UsageWindow, WindowKind};
    use serde_json::json;

    /// A local wall-clock moment. The hours rules are read on the reader's own
    /// clock, so a test that fixed them in UTC would be testing a different
    /// rule from the one that ships.
    fn at(hour: u32, minute: u32, day: u32) -> i64 {
        use chrono::TimeZone;
        chrono::Local
            .with_ymd_and_hms(2026, 6, day, hour, minute, 0)
            .earliest()
            .expect("a local wall-clock time")
            .timestamp()
    }

    /// The same moment as the RFC3339 string a reading carries.
    fn rfc(epoch: i64) -> String {
        chrono::DateTime::from_timestamp(epoch, 0)
            .expect("a timestamp")
            .to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
    }

    fn window(seconds: Option<f64>, label: &str) -> UsageWindow {
        UsageWindow::new(label, Some(10.0)).with_duration(seconds)
    }

    // MARK: Hours

    #[test]
    fn daytime_overnight_and_all_day_hours() {
        let day = PrimerHours { start: 7, end: 23 };
        assert!(day.contains_hour(7));
        assert!(!day.contains_hour(23));
        assert!(!day.contains_hour(3));
        let night = PrimerHours { start: 22, end: 6 };
        assert!(night.contains_hour(23));
        assert!(night.contains_hour(2));
        assert!(!night.contains_hour(12));
        assert!(PrimerHours { start: 9, end: 9 }.contains_hour(3));
        assert_eq!(PrimerHours::default(), PrimerHours { start: 7, end: 23 });
    }

    #[test]
    fn the_clock_is_read_in_local_time() {
        let day = PrimerHours { start: 7, end: 23 };
        assert!(day.contains(at(7, 0, 1)));
        assert!(!day.contains(at(23, 0, 1)));
        assert!(!day.contains(at(3, 0, 1)));
    }

    #[test]
    fn outside_the_hours_the_next_start_is_when_they_open() {
        let day = PrimerHours { start: 7, end: 23 };
        assert_eq!(day.next_after(at(3, 0, 1)), at(7, 0, 1));
        assert_eq!(day.next_after(at(23, 30, 1)), at(7, 0, 2));
        assert_eq!(day.next_after(at(12, 0, 1)), at(12, 0, 1));
        // All day: the moment asked about is already inside.
        let all = PrimerHours { start: 9, end: 9 };
        assert_eq!(all.next_after(at(3, 0, 1)), at(3, 0, 1));
    }

    // MARK: When

    #[test]
    fn due_once_the_reset_and_its_grace_have_passed_inside_the_hours() {
        let hours = PrimerHours { start: 7, end: 23 };
        let reset = at(10, 0, 1);
        assert!(!is_due(Some(reset), None, at(10, 0, 1), &hours));
        assert!(!is_due(Some(reset), None, reset + 59, &hours));
        assert!(is_due(Some(reset), None, reset + GRACE, &hours));
        assert!(is_due(Some(reset), None, at(10, 2, 1), &hours));
        assert!(!is_due(None, None, at(10, 2, 1), &hours));
    }

    #[test]
    fn not_due_outside_the_hours() {
        let hours = PrimerHours { start: 7, end: 23 };
        assert!(!is_due(Some(at(1, 0, 1)), None, at(2, 0, 1), &hours));
    }

    #[test]
    fn not_due_when_a_reading_since_shows_a_later_reset() {
        let hours = PrimerHours { start: 7, end: 23 };
        let reset = at(10, 0, 1);
        let restarted = window(Some(18_000.0), "5h").with_reset(Some(rfc(at(15, 30, 1))));
        assert!(!is_due(Some(reset), Some(&restarted), at(11, 0, 1), &hours));
        // The same reset, still reported, is not a restart.
        let stale = window(Some(18_000.0), "5h").with_reset(Some(rfc(reset)));
        assert!(is_due(Some(reset), Some(&stale), at(11, 0, 1), &hours));
    }

    // MARK: Which

    #[test]
    fn claude_five_hours_only_codex_five_hours_and_week_nothing_scoped() {
        assert!(eligible("claude-code", &window(Some(18_000.0), "5h")));
        assert!(!eligible("claude-code", &window(Some(604_800.0), "7d")));
        assert!(eligible("codex", &window(Some(18_000.0), "5h")));
        assert!(eligible("codex", &window(Some(604_800.0), "7d")));
        let scoped = window(Some(604_800.0), "7d").with_scope(Some("Spark".to_owned()));
        assert!(!eligible("codex", &scoped));
        assert!(!eligible("cursor", &window(Some(18_000.0), "5h")));
        // A stated duration is the provider's own word and is taken as such;
        // the label is only a fallback for replies that carry none, and then
        // it has to match exactly, so a model's own week is never read as the
        // account's.
        assert!(!eligible("claude-code", &window(None, "7d Opus")));
        assert!(!eligible("codex", &window(None, "7d Opus")));
        // A reply with no duration is read from the label the provider gave.
        assert!(eligible("claude-code", &window(None, "5h")));
        assert!(eligible("codex", &window(None, "7d")));
        assert!(!eligible("codex", &window(None, "Monthly")));
        // A balance is not a usage window: it carries no duration and its
        // label is not one either.
        assert!(!eligible(
            "codex",
            &window(None, "Balance").with_kind(WindowKind::Balance)
        ));
    }

    // MARK: What is sent

    #[test]
    fn claude_is_asked_with_haiku_no_tools_no_mcp_no_user_settings_nothing_saved() {
        assert_eq!(
            claude_arguments(),
            vec![
                "-p",
                "hi",
                "--model",
                "haiku",
                "--no-session-persistence",
                "--tools",
                "",
                "--strict-mcp-config",
                "--setting-sources",
                "project",
            ]
        );
    }

    #[test]
    fn codex_is_asked_ephemerally_read_only_at_low_effort_with_the_model_when_found() {
        let folder = Path::new("/tmp/starter");
        let arguments = codex_arguments(Some("gpt-5.6-luna"), folder);
        assert_eq!(arguments.first().map(String::as_str), Some("exec"));
        assert_eq!(arguments.last().map(String::as_str), Some("hi"));
        assert!(arguments.contains(&"--ephemeral".to_string()));
        assert!(arguments.contains(&"--skip-git-repo-check".to_string()));
        assert!(arguments.windows(2).any(|pair| pair == ["-m", "gpt-5.6-luna"]));
        assert!(arguments.windows(2).any(|pair| pair == ["-s", "read-only"]));
        assert!(arguments.windows(2).any(|pair| pair == ["-C", "/tmp/starter"]));
        assert!(arguments
            .windows(2)
            .any(|pair| pair == ["-c", "model_reasoning_effort=\"low\""]));
        assert!(!codex_arguments(None, folder).contains(&"-m".to_string()));
    }

    #[test]
    fn the_cheapest_codex_model_is_luna_else_one_called_fast_else_none() {
        let current = json!({"data": [
            {"id": "gpt-5.6-sol", "description": "Older coding model for complex work."},
            {"id": "gpt-5.6-luna", "description": "Older fast and efficient model."},
            {"id": "gpt-5.6-terra", "description": "Older balanced model."},
        ]});
        assert_eq!(cheapest_codex_model(&current).as_deref(), Some("gpt-5.6-luna"));

        let renamed = json!({"data": [
            {"id": "gpt-7-big", "description": "Frontier model."},
            {"id": "gpt-7-swift", "description": "Fast and cheap."},
        ]});
        assert_eq!(cheapest_codex_model(&renamed).as_deref(), Some("gpt-7-swift"));

        // `model` is the other field the id can arrive in.
        let other = json!({"result": {"data": [{"model": "gpt-7-mini", "description": "Small."}]}});
        assert_eq!(cheapest_codex_model(&other).as_deref(), Some("gpt-7-mini"));

        // Hidden entries are not on offer, even by name.
        let hidden = json!({"data": [{"id": "gpt-5.6-luna", "hidden": true, "description": "fast"}]});
        assert_eq!(cheapest_codex_model(&hidden), None);
        assert_eq!(
            cheapest_codex_model(&json!({"data": [{"id": "only", "description": "A model."}]})),
            None
        );
        assert_eq!(cheapest_codex_model(&json!("nope")), None);
        assert_eq!(cheapest_codex_model(&json!({})), None);
    }

    #[test]
    fn claudes_project_folder_for_the_starters_directory_is_named_its_way() {
        let folder = Path::new("/Users/me/Library/Application Support/Pulse/Window starter");
        let home = Path::new("/Users/me");
        assert_eq!(
            claude_project_folder(folder, home),
            home.join(".claude")
                .join("projects")
                .join("-Users-me-Library-Application-Support-Pulse-Window-starter")
        );
    }

    // MARK: Where the tool is

    #[test]
    fn the_tool_is_looked_for_in_the_order_the_folders_are_given() {
        let path_dirs = vec![PathBuf::from("/usr/bin")];
        let extra = vec![PathBuf::from("/Users/me/.claude/local/claude")];
        let install = vec![
            PathBuf::from("/opt/homebrew/bin"),
            PathBuf::from("/Users/me/.local/bin"),
        ];
        let managed = vec![PathBuf::from(
            "/Users/me/.nvm/versions/node/v24.1.0/bin/claude",
        )];
        let listed = candidates("claude", &path_dirs, &extra, &install, &managed);

        let name = names("claude")[0].clone();
        assert_eq!(listed[0], Path::new("/usr/bin").join(&name));
        // The given path comes before the folders, and its Windows extensions
        // follow it rather than replacing it.
        assert!(listed.contains(&PathBuf::from("/Users/me/.claude/local/claude")));
        assert!(listed.iter().any(|path| path.ends_with(&name)));
        let homebrew = listed
            .iter()
            .position(|path| path == &Path::new("/opt/homebrew/bin").join(&name))
            .expect("the homebrew folder is listed");
        let given = listed
            .iter()
            .position(|path| path == Path::new("/Users/me/.claude/local/claude"))
            .expect("the given path is listed");
        assert!(given < homebrew);
        assert_eq!(listed.last(), managed.last());
    }

    #[test]
    fn node_version_managers_are_searched_newest_version_first() {
        let home = Path::new("/Users/me");
        let versions = |dir: &Path| -> Vec<String> {
            if dir.ends_with(".nvm/versions/node") {
                vec!["v20.0.0".to_string(), "v24.1.0".to_string()]
            } else {
                Vec::new()
            }
        };
        let listed = managed_dirs("claude", home, &versions);
        let name = names("claude")[0].clone();
        assert_eq!(
            listed,
            vec![
                home.join(".nvm/versions/node/v24.1.0/bin").join(&name),
                home.join(".nvm/versions/node/v20.0.0/bin").join(&name),
            ]
        );
    }

    #[test]
    fn the_first_existing_candidate_wins_and_none_is_none() {
        let listed = vec![
            PathBuf::from("/nowhere/claude"),
            PathBuf::from("/usr/bin/claude"),
        ];
        assert_eq!(
            locate_in(&listed, &|path| path == Path::new("/usr/bin/claude")),
            Some(PathBuf::from("/usr/bin/claude"))
        );
        assert_eq!(locate_in(&listed, &|_| false), None);
    }

    #[test]
    fn only_what_the_tool_needs_crosses_into_it() {
        assert!(kept_variable(OsStr::new("SYSTEMROOT")));
        assert!(kept_variable(OsStr::new("userprofile")));
        assert!(kept_variable(OsStr::new("HTTPS_PROXY")));
        assert!(!kept_variable(OsStr::new("ANTHROPIC_API_KEY")));
        assert!(!kept_variable(OsStr::new("OPENAI_API_KEY")));
        assert!(!kept_variable(OsStr::new("PULSEWIN_CODEX_TOKEN")));
        assert!(!kept_variable(OsStr::new("PULSEWIN_TEST_HOME")));
        // `PATH` is rebuilt with the tool's own folder first, never inherited.
        assert!(!kept_variable(OsStr::new("PATH")));
    }

    // MARK: The bookkeeping

    #[test]
    fn a_provider_no_longer_started_keeps_no_noted_reset_to_wake_for() {
        let reset = 1_800_000_000i64;
        let mut noted = BTreeMap::new();
        noted.insert("claude-code|claudeCode.five_hour".to_string(), reset);
        noted.insert("codex|primary_window".to_string(), reset);
        noted.insert("codex|secondary_window".to_string(), reset);
        // An added account is never started, so its reset is not kept either.
        noted.insert("codex--account-ab|primary_window".to_string(), reset);

        let codex: BTreeSet<&str> = ["codex"].into_iter().collect();
        let surviving = kept(&noted, &codex);
        assert_eq!(
            surviving.keys().cloned().collect::<Vec<_>>(),
            vec!["codex|primary_window", "codex|secondary_window"]
        );
        assert!(kept(&noted, &BTreeSet::new()).is_empty());

        let both: BTreeSet<&str> = ["codex", "claude-code"].into_iter().collect();
        assert_eq!(kept(&noted, &both).len(), 3);
    }

    #[test]
    fn outcomes_keep_the_strings_an_older_build_wrote() {
        assert_eq!(Outcome::Sent.as_str(), "sent");
        assert_eq!(Outcome::ToolMissing.as_str(), "toolMissing");
        assert_eq!(Outcome::Failed.as_str(), "failed");
        assert_eq!(Outcome::TimedOut.as_str(), "timedOut");
        for outcome in [
            Outcome::Sent,
            Outcome::ToolMissing,
            Outcome::Failed,
            Outcome::TimedOut,
        ] {
            assert_eq!(Outcome::parse(outcome.as_str()), Some(outcome));
        }
        assert_eq!(Outcome::parse(""), None);
        assert_eq!(Outcome::parse("SENT"), None);
    }

    #[test]
    fn the_ceilings_are_the_originals() {
        assert_eq!(GRACE, 60);
        assert_eq!(DEADLINE_SECS, 120);
        assert_eq!(OUTPUT_CEILING, 64 * 1024);
        assert_eq!(PROVIDERS, ["claude-code", "codex"]);
    }
}
