//! User extensions: programs a user drops into a folder that report one
//! account's usage.
//!
//! **Out of process, and host-drawn.** PulseWin starts the program, reads one
//! JSON object from its standard output and draws that with its own ring and
//! card. Nothing is loaded into PulseWin, nothing the program sends is drawn as
//! it sent it, and PulseWin hands it no credential: the program owns its own
//! login. The contract is the original's `Docs/extensions.md`; this file is its
//! port, and it is split the way the original's one Swift file is:
//!
//! 1. [`Extension`] — one extension that resolved: what to call it, which
//!    program to run, from where, and for how long at most.
//! 2. [`ExtensionCatalog`] — finding them: which folders hold a manifest and
//!    whether it can be used.
//! 3. [`ExtensionReport`] — what one program printed, turned into a reading.
//!
//! Running one lives in [`crate::extension_runner`].

use std::collections::HashSet;
use std::path::{Component, Path, PathBuf};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::model::{parse_reset, CreditRemaining, ProviderUsage, UsageMetric, UsageMetricPoint, UsageWindow};

// ---------------------------------------------------------------------------
// Part 1 — the resolved extension
// ---------------------------------------------------------------------------

/// One program in the extensions folder, resolved and known to be runnable.
///
/// Every field here has already been checked by [`ExtensionCatalog::scan_in`],
/// so a value of this type means "the folder holds a manifest this build reads,
/// and its program is a real executable file inside that folder".
#[derive(Debug, Clone, PartialEq)]
pub struct Extension {
    /// From the manifest. Also the account's slot, so it has to survive being
    /// written into an account id: see [`ExtensionCatalog::is_valid_id`].
    pub id: String,
    /// What the rail's card and Settings call it. The program's own word for
    /// itself, so it is never translated.
    pub name: String,
    /// The folder the manifest was found in, with links resolved.
    pub directory: PathBuf,
    /// The program, resolved, and known to be inside `directory`.
    pub executable: PathBuf,
    /// How long a run may take before it is stopped.
    pub timeout: Duration,
}

/// The provider name an extension's account id is built from.
pub const ACCOUNT_PROVIDER: &str = "extension";

/// The account id for an extension, from its manifest id.
///
/// **Hex, because the port's account ids already are.** `accounts::valid`
/// requires an added account's id to be `<provider>--account-<hex>`; a manifest
/// id may contain dots and dashes, and a dot is not even a legal character in a
/// credential file name here. Encoding the id keeps one rule for every account
/// id in the port, and makes the manifest id recoverable with
/// [`id_from_account_id`].
pub fn account_id(manifest_id: &str) -> String {
    let mut suffix = String::with_capacity(manifest_id.len() * 2);
    for byte in manifest_id.as_bytes() {
        suffix.push_str(&format!("{byte:02x}"));
    }
    format!("{ACCOUNT_PROVIDER}{}{suffix}", crate::accounts::SEPARATOR)
}

/// The manifest id behind a stored account id, if that is what it is.
pub fn id_from_account_id(account: &str) -> Option<String> {
    let suffix = account.strip_prefix(&format!("{ACCOUNT_PROVIDER}{}", crate::accounts::SEPARATOR))?;
    if suffix.is_empty() || suffix.len() % 2 != 0 || !suffix.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return None;
    }
    let bytes = (0..suffix.len())
        .step_by(2)
        .map(|index| u8::from_str_radix(&suffix[index..index + 2], 16).ok())
        .collect::<Option<Vec<u8>>>()?;
    let id = String::from_utf8(bytes).ok()?;
    ExtensionCatalog::is_valid_id(&id).then_some(id)
}

impl Extension {
    pub fn account_id(&self) -> String {
        account_id(&self.id)
    }
}

/// Whether `path` is a file a program can be started from.
///
/// Unix asks the mode bits, as the original does. Windows has no such bit, so
/// it asks what Windows itself uses to decide: the extension a shell would
/// hand to an interpreter. A `.cmd`/`.bat` counts, and the runner wraps it in
/// `cmd.exe`, because `CreateProcess` cannot start one directly.
pub fn is_executable(path: &Path) -> bool {
    let Ok(metadata) = std::fs::metadata(path) else { return false };
    if !metadata.is_file() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        metadata.permissions().mode() & 0o111 != 0
    }
    #[cfg(not(unix))]
    {
        path.extension()
            .map(|extension| extension.to_string_lossy().to_ascii_lowercase())
            .is_some_and(|extension| matches!(extension.as_str(), "exe" | "cmd" | "bat" | "com"))
    }
}

// ---------------------------------------------------------------------------
// Part 2 — finding them
// ---------------------------------------------------------------------------

/// Finding extensions: which folders hold a manifest, and whether it can be
/// used.
///
/// **Reading a manifest never runs anything.** A folder dropped into the
/// extensions directory is listed in Settings and does nothing else until it is
/// switched on there — the same rule as every built-in provider, which is not
/// fetched while it is off.
pub struct ExtensionCatalog;

impl ExtensionCatalog {
    pub fn manifest_name() -> &'static str {
        "pulse-extension.json"
    }

    /// The only schema this build reads, for the manifest and the report both.
    pub fn schema_version() -> i64 {
        1
    }

    /// Where extensions live: PulseWin's own data folder plus `Extensions`, so
    /// it is somewhere nothing else writes to and a user who asks can be told
    /// exactly where to look.
    pub fn folder() -> Option<PathBuf> {
        crate::settings::directory().map(|base| base.join("Extensions"))
    }

    pub fn default_timeout() -> Duration {
        Duration::from_secs(20)
    }

    /// A pass waits for every extension it asks, so the ceiling is a ceiling on
    /// how late every other ring can be.
    pub fn timeout_range() -> std::ops::RangeInclusive<f64> {
        1.0..=60.0
    }

    /// Letters, digits and `.-_`, lowercase, starting with a letter or digit.
    ///
    /// **Not a style rule.** The id becomes an account id, and the port's
    /// account ids are split on `#`-style separators — `accounts::SEPARATOR`
    /// here — while rail slots are split on `@`. Either character in here would
    /// cut the account in two when it is read back.
    pub fn is_valid_id(id: &str) -> bool {
        let count = id.chars().count();
        if count == 0 || count > 64 {
            return false;
        }
        let mut characters = id.chars();
        let Some(first) = characters.next() else { return false };
        if !first.is_ascii_lowercase() && !first.is_ascii_digit() {
            return false;
        }
        id.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '.' | '-' | '_'))
    }

    /// The app's own extensions folder.
    pub fn scan() -> Scan {
        match Self::folder() {
            Some(folder) => Self::scan_in(&folder),
            None => Scan::default(),
        }
    }

    /// Every folder directly inside `folder`, in name order. Hidden entries and
    /// plain files are skipped: a stray file in the folder is not a broken
    /// extension.
    ///
    /// **A missing folder is an empty scan, not a failure.** Someone who never
    /// added an extension has no folder, and that is the normal state.
    pub fn scan_in(folder: &Path) -> Scan {
        let Ok(entries) = std::fs::read_dir(folder) else { return Scan::default() };
        let mut folders: Vec<(String, PathBuf)> = entries
            .filter_map(Result::ok)
            .filter_map(|entry| {
                let name = entry.file_name().to_string_lossy().into_owned();
                if hidden(&entry.path(), &name) {
                    return None;
                }
                // `metadata` follows a link, so a folder that is itself a link
                // is still a folder the user put here.
                std::fs::metadata(entry.path()).ok().filter(std::fs::Metadata::is_dir)?;
                Some((name, entry.path()))
            })
            .collect();
        // Case-insensitive first, the way a Windows folder lists them; the
        // exact name breaks a tie so the order never depends on read order.
        folders.sort_by(|(left, _), (right, _)| {
            left.to_lowercase().cmp(&right.to_lowercase()).then_with(|| left.cmp(right))
        });

        let mut scan = Scan::default();
        let mut taken: HashSet<String> = HashSet::new();
        for (name, directory) in folders {
            match Self::load(&directory) {
                Ok(found) if taken.contains(&found.id) => {
                    // The first folder in name order keeps it. Two programs
                    // answering as one account would draw whichever ran last.
                    scan.problems.push(Problem { folder: name, reason: Reason::DuplicateId(found.id) });
                }
                Ok(found) => {
                    taken.insert(found.id.clone());
                    scan.extensions.push(found);
                }
                Err(reason) => scan.problems.push(Problem { folder: name, reason }),
            }
        }
        scan
    }

    /// The extension behind a stored account id, from a fresh scan.
    ///
    /// A stored id is all a saved preference carries, so this is how a refresh
    /// gets from "this account is switched on" back to a program to run.
    pub fn find(account: &str) -> Option<Extension> {
        Self::scan().extensions.into_iter().find(|extension| extension.account_id() == account)
    }

    fn load(directory: &Path) -> Result<Extension, Reason> {
        let manifest_path = directory.join(Self::manifest_name());
        if !manifest_path.is_file() {
            return Err(Reason::NoManifest);
        }
        let Ok(bytes) = std::fs::read(&manifest_path) else { return Err(Reason::UnreadableManifest) };
        let Ok(manifest) = serde_json::from_slice::<Manifest>(&bytes) else {
            return Err(Reason::UnreadableManifest);
        };

        if manifest.schema_version != Self::schema_version() {
            return Err(Reason::UnsupportedSchema(manifest.schema_version));
        }
        if !Self::is_valid_id(&manifest.id) {
            return Err(Reason::InvalidId);
        }

        let name = manifest.name.trim();
        if name.is_empty() {
            return Err(Reason::MissingName);
        }

        // Resolved before it is compared, so neither `..` nor a link can make a
        // program outside the folder look like one inside it. The folder
        // Settings shows is the folder whose program runs.
        let root = resolve(directory);
        let Some(executable) = resolve_program(&root, &manifest.executable) else {
            return Err(Reason::ExecutableOutsideFolder);
        };
        if executable == root || !executable.starts_with(&root) {
            return Err(Reason::ExecutableOutsideFolder);
        }
        if !is_executable(&executable) {
            return Err(Reason::ExecutableMissing);
        }

        Ok(Extension {
            id: manifest.id,
            name: clip(name, 60),
            directory: root,
            executable,
            timeout: timeout(manifest.timeout_seconds),
        })
    }
}

/// The manifest as written. Strict on purpose: a field of the wrong type is a
/// manifest Pulse cannot read, not a manifest with one field missing — the same
/// answer the original's `JSONDecoder` gives.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Manifest {
    schema_version: i64,
    id: String,
    name: String,
    executable: String,
    #[serde(default)]
    timeout_seconds: Option<f64>,
}

/// What a scan found: the extensions that can be used, and why each folder that
/// could not be was turned away.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Scan {
    pub extensions: Vec<Extension>,
    pub problems: Vec<Problem>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Problem {
    /// The folder's name, which is all the reader needs to find it.
    pub folder: String,
    pub reason: Reason,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Reason {
    NoManifest,
    UnreadableManifest,
    UnsupportedSchema(i64),
    InvalidId,
    MissingName,
    ExecutableOutsideFolder,
    ExecutableMissing,
    DuplicateId(String),
}

impl Reason {
    /// The reader is told what is wrong with *their* folder, in the port's own
    /// voice: plain, one sentence, and naming no service.
    pub fn message(&self) -> String {
        match self {
            Self::NoManifest => "这个文件夹里没有 pulse-extension.json。".into(),
            Self::UnreadableManifest => "pulse-extension.json 不是有效的 JSON，或缺少字段。".into(),
            Self::UnsupportedSchema(version) => {
                format!("它是按扩展格式版本 {version} 写的，此版本 Pulse 不读取。")
            }
            Self::InvalidId => "它的 id 只能是小写字母、数字、点、短横线和下划线，最长 64 个字符。".into(),
            Self::MissingName => "它的名称是空的。".into(),
            Self::ExecutableOutsideFolder => "它的程序必须位于自己的文件夹内。".into(),
            Self::ExecutableMissing => "它的程序不在，或不可执行。".into(),
            Self::DuplicateId(id) => format!("另一个扩展已经使用了 id {id}。"),
        }
    }
}

/// The three ways a run can end without a report. They live here rather than in
/// the runner because the reading and the wording are the same promise: the
/// reader is told what happened, and nothing is invented to fill the gap.
pub const MISSING: &str = "无法运行这个扩展的程序，请检查它的文件夹。";
pub const STOPPED: &str = "扩展运行出错，已退出。";
pub const TIMED_OUT: &str = "扩展没有在规定时间内返回结果。";

/// Hidden means hidden: a name Windows and macOS both treat as "not the user's
/// folder" — a dot name everywhere, and the hidden attribute on Windows.
fn hidden(path: &Path, name: &str) -> bool {
    if name.starts_with('.') {
        return true;
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        const FILE_ATTRIBUTE_HIDDEN: u32 = 0x2;
        if std::fs::metadata(path).is_ok_and(|meta| meta.file_attributes() & FILE_ATTRIBUTE_HIDDEN != 0) {
            return true;
        }
    }
    let _ = path;
    false
}

/// The absolute, link-free form of `path`.
///
/// Falls back to the path as given when it cannot be resolved — the folder may
/// have been deleted between the listing and this call, and "it is gone" is
/// still a useful answer to the comparison that follows.
fn resolve(path: &Path) -> PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_| lexical(path))
}

/// The program a manifest names, resolved, or `None` when the manifest points
/// outside its own folder.
///
/// **A missing file still has to be checked.** The program may not be there
/// yet — that is `ExecutableMissing`, not an escape — so the folder that holds
/// it is resolved and the name appended, rather than giving up on the whole
/// path.
fn resolve_program(root: &Path, relative: &str) -> Option<PathBuf> {
    let named = Path::new(relative);
    if named.is_absolute() || relative.starts_with('/') || relative.starts_with('\\') {
        return None;
    }
    let joined = lexical(&root.join(named));
    if let Ok(resolved) = std::fs::canonicalize(&joined) {
        return Some(resolved);
    }
    let parent = std::fs::canonicalize(joined.parent()?).ok()?;
    Some(parent.join(joined.file_name()?))
}

/// `.` and `..` folded away without touching the disk, so a path that does not
/// exist yet can still be compared against the folder it claims to be in.
fn lexical(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                if !out.pop() {
                    out.push(Component::ParentDir.as_os_str());
                }
            }
            other => out.push(other.as_os_str()),
        }
    }
    out
}

fn timeout(stated: Option<f64>) -> Duration {
    let range = ExtensionCatalog::timeout_range();
    let seconds = stated.filter(|value| value.is_finite()).unwrap_or(ExtensionCatalog::default_timeout().as_secs_f64());
    Duration::from_secs_f64(seconds.clamp(*range.start(), *range.end()))
}

/// `text` cut to `max` characters, counted the way a reader counts them.
pub(crate) fn clip(text: &str, max: usize) -> String {
    text.chars().take(max).collect()
}

// ---------------------------------------------------------------------------
// Part 3 — what a program printed
// ---------------------------------------------------------------------------

/// What an extension prints: one JSON object.
///
/// **PulseWin still invents nothing.** A limit is drawn from a percentage the
/// program states, or from a used amount and a limit it states; one with
/// neither is left off rather than drawn at zero, and a report with neither a
/// limit nor a balance left says so.
///
/// **A balance is money and nothing more.** A relay that sells prepaid credit
/// reports what is left and no allowance, so the report carries the amount and
/// its currency, and the ring it gets is every API account's.
///
/// **Metrics are an addition, and cannot take the reading down.** A metric
/// PulseWin cannot use — a value written as text, a list that is not a list —
/// drops that metric, or the list, and leaves the limits and the balance as
/// they were.
pub struct ExtensionReport;

impl ExtensionReport {
    /// As many as the card has room for beside a few limits and a balance. The
    /// rest are left off, in the program's order.
    pub fn metric_limit() -> usize {
        4
    }

    /// The newest points of a series, at most. Older ones are left off rather
    /// than thinned: a point PulseWin kept is a point the program stated.
    pub fn history_limit() -> usize {
        120
    }

    /// More than any honest report needs. Past it the bytes are dropped and the
    /// report fails to parse, rather than PulseWin holding whatever a runaway
    /// program cares to write.
    pub fn output_ceiling() -> usize {
        256 * 1024
    }

    /// Read one report. `account_id` and `name` are what the reading will be
    /// filed under, so a failure names the account that failed.
    pub fn reading(report_json: &[u8], account_id: &str, name: &str) -> ProviderUsage {
        let Ok(report) = serde_json::from_slice::<Report>(report_json) else {
            return failed(account_id, name, UNREADABLE);
        };
        if report.schema_version != ExtensionCatalog::schema_version() {
            return failed(account_id, name, UNREADABLE);
        }

        // Named failures only, each mapped to wording that names no service.
        // Anything else is a reply PulseWin cannot read.
        match report.status.as_deref().unwrap_or("ok") {
            "ok" => {}
            "signedOut" => return failed(account_id, name, "扩展表示它的登录已过期。"),
            "unreachable" => return failed(account_id, name, "服务没有响应。"),
            "rateLimited" => return failed(account_id, name, "检查过于频繁，稍后再试。"),
            "serverError" => return failed(account_id, name, "服务返回了一个错误。"),
            "noLimits" => return failed(account_id, name, NO_LIMITS),
            _ => return failed(account_id, name, UNREADABLE),
        }

        let mut seen: HashSet<String> = HashSet::new();
        let mut windows: Vec<UsageWindow> = Vec::new();
        for (index, limit) in report.limits.unwrap_or_default().iter().enumerate() {
            let Some(percent) = used_percent(limit) else { continue };
            let label = limit.label.trim();
            if label.is_empty() {
                continue;
            }
            // Pinning a ring to a limit is by id, so it has to stay put across
            // runs — the program's own if it gave one, else its place.
            let id = format!(
                "extension.{}",
                limit.id.as_deref().map_or_else(|| index.to_string(), |id| clip(id, 64))
            );
            if !seen.insert(id.clone()) {
                continue;
            }
            // A length only when the program stated one. Without it the window
            // clock has nothing to divide by and draws nothing.
            let seconds = limit.window_seconds.filter(|seconds| *seconds > 0);
            windows.push(UsageWindow {
                id: Some(id),
                scope: None,
                kind: crate::model::WindowKind::Other,
                is_exhausted: percent >= 100.0,
                window_seconds: seconds.map(|seconds| seconds as f64),
                label: clip(label, 60),
                // The figure the program stated, kept past 100 rather than
                // flattened: "spent" is the flag, and how far past is the news.
                percent_used: Some(percent),
                resets_at: limit.resets_at.as_deref().and_then(|text| parse_reset(&Value::String(text.to_owned()))),
                detail: None,
            });
        }

        let money = report.balance.as_ref().and_then(credit);
        if windows.is_empty() && money.is_none() {
            return failed(account_id, name, NO_LIMITS);
        }

        let plan = report
            .plan
            .as_deref()
            .map(|plan| clip(plan.trim(), 60))
            .filter(|plan| !plan.is_empty());
        let mut usage = ProviderUsage::ok(account_id, name, windows);
        usage.plan = plan;
        usage.credit_remaining = money;
        usage.source = Some("扩展程序自带的登录".into());
        usage.metrics = metrics(report.metrics.map(|lossy| lossy.0).unwrap_or_default());
        usage
    }
}

const UNREADABLE: &str = "无法读取扩展的输出。";
const NO_LIMITS: &str = "扩展没有报告任何额度。";

fn failed(account_id: &str, name: &str, message: &str) -> ProviderUsage {
    ProviderUsage::failed(account_id, name, message)
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Report {
    schema_version: i64,
    #[serde(default)]
    status: Option<String>,
    #[serde(default)]
    plan: Option<String>,
    #[serde(default)]
    limits: Option<Vec<Limit>>,
    #[serde(default)]
    balance: Option<Balance>,
    #[serde(default)]
    metrics: Option<Lossy<Metric>>,
}

/// A list whose bad entries are skipped, and which is empty rather than an
/// error when it is not a list at all.
///
/// **The whole reason `metrics` cannot take a reading down.** `metrics[]`
/// arrived after the first extensions were written, so a program that emits
/// something this build does not understand still has its limits and its
/// balance read exactly as before.
struct Lossy<T>(Vec<T>);

impl<'de, T: serde::de::DeserializeOwned> Deserialize<'de> for Lossy<T> {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = Value::deserialize(deserializer)?;
        let Value::Array(entries) = value else { return Ok(Self(Vec::new())) };
        let mut kept = Vec::new();
        for entry in entries {
            // One bad entry steps past itself; it never stops the list.
            if let Ok(element) = serde_json::from_value::<T>(entry) {
                kept.push(element);
            }
        }
        Ok(Self(kept))
    }
}

/// Every field optional, so a wrong type drops this metric and nothing else.
#[derive(Default)]
struct Metric {
    id: Option<String>,
    label: Option<String>,
    value: Option<f64>,
    unit: Option<String>,
    history: Option<Lossy<Point>>,
}

impl<'de> Deserialize<'de> for Metric {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = Value::deserialize(deserializer)?;
        // Not an object at all is the one thing that drops the entry: every
        // field of an object is read with "this one may be wrong".
        let Value::Object(fields) = value else {
            return Err(serde::de::Error::custom("a metric must be an object"));
        };
        let text = |key: &str| fields.get(key).and_then(Value::as_str).map(str::to_owned);
        Ok(Self {
            id: text("id"),
            label: text("label"),
            value: fields.get("value").and_then(Value::as_f64),
            unit: text("unit"),
            history: fields
                .get("history")
                .and_then(|history| serde_json::from_value::<Lossy<Point>>(history.clone()).ok()),
        })
    }
}

/// One point of a series. Both halves are required, so half a point is no
/// point at all.
#[derive(Deserialize)]
struct Point {
    t: String,
    v: f64,
}

/// Optional fields, so a balance missing one is dropped rather than taking the
/// limits beside it down with it.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Balance {
    #[serde(default)]
    amount: Option<f64>,
    #[serde(default)]
    currency: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Limit {
    #[serde(default)]
    id: Option<String>,
    label: String,
    #[serde(default)]
    used_percent: Option<f64>,
    #[serde(default)]
    used: Option<f64>,
    #[serde(default)]
    limit: Option<f64>,
    #[serde(default)]
    resets_at: Option<String>,
    #[serde(default)]
    window_seconds: Option<i64>,
}

/// The metrics PulseWin can draw, in the program's order: each with a label and
/// a value that is a number, no id twice, at most [`ExtensionReport::metric_limit`].
fn metrics(reported: Vec<Metric>) -> Vec<UsageMetric> {
    let mut seen: HashSet<String> = HashSet::new();
    let mut kept: Vec<UsageMetric> = Vec::new();
    for (index, metric) in reported.into_iter().enumerate() {
        if kept.len() >= ExtensionReport::metric_limit() {
            break;
        }
        let Some(value) = metric.value.filter(|value| value.is_finite()) else { continue };
        let Some(label) = metric.label.as_deref().map(str::trim).filter(|label| !label.is_empty()) else {
            continue;
        };
        let id = metric.id.as_deref().map_or_else(|| index.to_string(), |id| clip(id, 64));
        if !seen.insert(id.clone()) {
            continue;
        }
        let unit = metric
            .unit
            .as_deref()
            .map(str::trim)
            .map(|unit| clip(unit, 16))
            .filter(|unit| !unit.is_empty());
        kept.push(UsageMetric {
            id,
            label: clip(label, 60),
            value,
            unit,
            history: history(metric.history.map(|lossy| lossy.0).unwrap_or_default()),
        });
    }
    kept
}

/// The points with a time and a number, oldest first, one per moment — the
/// first stated wins — and only the newest [`ExtensionReport::history_limit`].
fn history(points: Vec<Point>) -> Vec<UsageMetricPoint> {
    let mut seen: HashSet<chrono::DateTime<chrono::Utc>> = HashSet::new();
    let mut dated: Vec<(chrono::DateTime<chrono::Utc>, UsageMetricPoint)> = Vec::new();
    for point in points {
        if !point.v.is_finite() {
            continue;
        }
        let Some(at) = instant(&point.t) else { continue };
        if !seen.insert(at) {
            continue;
        }
        dated.push((at, UsageMetricPoint { at: stated(&point.t), value: point.v }));
    }
    dated.sort_by_key(|(at, _)| *at);
    let mut kept: Vec<UsageMetricPoint> = dated.into_iter().map(|(_, point)| point).collect();
    if kept.len() > ExtensionReport::history_limit() {
        kept.drain(..kept.len() - ExtensionReport::history_limit());
    }
    kept
}

/// The amount as stated, in a currency named by its ISO code. Below zero is
/// kept — some services run an account negative and keep serving it — but a
/// figure that is not a number, or a currency that is not a code, is no balance
/// at all.
fn credit(balance: &Balance) -> Option<CreditRemaining> {
    let amount = balance.amount.filter(|amount| amount.is_finite())?;
    let currency = balance.currency.as_deref()?.trim().to_ascii_uppercase();
    let code = currency.len() == 3 && currency.bytes().all(|byte| byte.is_ascii_uppercase());
    code.then_some(CreditRemaining { amount, currency })
}

/// The percentage as stated, or the used amount over the stated limit. Nothing
/// negative, nothing that is not a number, and no division by a limit of zero.
///
/// **The stated figure is returned as stated.** Dividing it by 100 and
/// multiplying the result back is not the inverse in binary floating point:
/// 110 becomes 1.1 (already not exact) and 1.1 * 100 is 110.00000000000001, so
/// the pane would print a limit as "110.00000000000001%". Everything else
/// derives from this one figure.
fn used_percent(limit: &Limit) -> Option<f64> {
    if let Some(percent) = limit.used_percent {
        return (percent.is_finite() && percent >= 0.0).then_some(percent);
    }
    let used = limit.used?;
    let total = limit.limit?;
    if !used.is_finite() || !total.is_finite() || used < 0.0 || total <= 0.0 {
        return None;
    }
    Some(used / total * 100.0)
}

/// ISO 8601, with or without fractional seconds.
fn instant(text: &str) -> Option<chrono::DateTime<chrono::Utc>> {
    if let Ok(at) = chrono::DateTime::parse_from_rfc3339(text) {
        return Some(at.with_timezone(&chrono::Utc));
    }
    // The shapes `model::parse_reset` also takes: no offset, or a bare date.
    let normalized = parse_reset(&Value::String(text.to_owned()))?;
    chrono::DateTime::parse_from_rfc3339(&normalized).ok().map(|at| at.with_timezone(&chrono::Utc))
}

/// A point's time, as the program stated it when that is already RFC3339 —
/// sub-second precision included, because two points a moment apart are two
/// points — and normalized otherwise so the frontend can parse it.
fn stated(text: &str) -> String {
    if chrono::DateTime::parse_from_rfc3339(text).is_ok() {
        text.to_owned()
    } else {
        parse_reset(&Value::String(text.to_owned())).unwrap_or_else(|| text.to_owned())
    }
}

// ---------------------------------------------------------------------------
// Settings view
// ---------------------------------------------------------------------------

/// What the Settings pane draws: the folder to look in, what was found, and why
/// the rest was turned away.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ScanView {
    pub folder: String,
    pub extensions: Vec<ExtensionView>,
    pub problems: Vec<ProblemView>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExtensionView {
    pub id: String,
    pub name: String,
    pub directory: String,
    pub executable: String,
    pub timeout_seconds: f64,
    /// The id the rest of the app files this extension's account under.
    pub account_id: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProblemView {
    pub folder: String,
    pub message: String,
}

impl Scan {
    pub fn view(&self, folder: &Path) -> ScanView {
        ScanView {
            folder: folder.display().to_string(),
            extensions: self
                .extensions
                .iter()
                .map(|extension| ExtensionView {
                    id: extension.id.clone(),
                    name: extension.name.clone(),
                    directory: extension.directory.display().to_string(),
                    executable: extension.executable.display().to_string(),
                    timeout_seconds: extension.timeout.as_secs_f64(),
                    account_id: extension.account_id(),
                })
                .collect(),
            problems: self
                .problems
                .iter()
                .map(|problem| ProblemView { folder: problem.folder.clone(), message: problem.reason.message() })
                .collect(),
        }
    }
}

// ---------------------------------------------------------------------------
// Tests — the original's `PulseExtensionTests.swift`, case for case
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::sync::atomic::{AtomicUsize, Ordering};

    static NEXT: AtomicUsize = AtomicUsize::new(0);

    fn temp_dir(label: &str) -> PathBuf {
        let suffix = format!("{label}-{}-{}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed));
        let candidate = std::env::temp_dir().join(format!("pulsewin-{suffix}"));
        if fs::create_dir_all(&candidate).is_ok() {
            return candidate;
        }
        // Sandboxed test binaries can be denied writes outside the workspace.
        let fallback = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("target")
            .join("test-scratch")
            .join(format!("pulsewin-{suffix}"));
        fs::create_dir_all(&fallback).unwrap_or_else(|error| panic!("create_dir_all {}: {error:?}", fallback.display()));
        fallback
    }

    /// A program's file name: what Windows and macOS each consider runnable.
    #[cfg(unix)]
    const RUN: &str = "run";
    #[cfg(windows)]
    const RUN: &str = "run.exe";
    /// A file that is present and is not a program.
    #[cfg(unix)]
    const PLAIN: &str = "run";
    #[cfg(windows)]
    const PLAIN: &str = "run.txt";

    struct Sandbox {
        root: PathBuf,
    }

    impl Sandbox {
        fn new(label: &str) -> Self {
            Self { root: temp_dir(label) }
        }

        /// One extension folder, with a manifest and optionally a program.
        #[cfg_attr(windows, allow(dead_code))]
        fn add(&self, folder: &str, manifest: Option<&str>, script: Option<&str>) -> PathBuf {
            self.add_named(folder, manifest, script, RUN, true)
        }

        fn add_named(&self, folder: &str, manifest: Option<&str>, script: Option<&str>, name: &str, executable: bool) -> PathBuf {
            let directory = self.root.join(folder);
            fs::create_dir_all(&directory).unwrap();
            if let Some(manifest) = manifest {
                fs::write(directory.join(ExtensionCatalog::manifest_name()), manifest).unwrap();
            }
            if let Some(script) = script {
                let program = directory.join(name);
                fs::write(&program, script).unwrap();
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    fs::set_permissions(&program, fs::Permissions::from_mode(if executable { 0o755 } else { 0o644 })).unwrap();
                }
            }
            directory
        }

        fn remove(&self) {
            let _ = fs::remove_dir_all(&self.root);
        }
    }

    fn manifest(id: &str, name: &str) -> String {
        manifest_for(id, name, RUN)
    }

    fn manifest_for(id: &str, name: &str, executable: &str) -> String {
        format!(r#"{{"schemaVersion":1,"id":"{id}","name":"{name}","executable":"{executable}"}}"#)
    }

    fn manifest_with_timeout(id: &str, name: &str, seconds: f64) -> String {
        format!(
            r#"{{"schemaVersion":1,"id":"{id}","name":"{name}","executable":"{RUN}","timeoutSeconds":{seconds}}}"#
        )
    }

    fn manifest_with_schema(id: &str, name: &str, schema: i64) -> String {
        format!(r#"{{"schemaVersion":{schema},"id":"{id}","name":"{name}","executable":"{RUN}"}}"#)
    }

    // --- ids ---------------------------------------------------------------

    #[test]
    fn an_id_has_to_survive_being_written_into_an_account_id() {
        for id in ["a", "acme", "acme-quota", "acme.quota_2", "9lives", &"x".repeat(64)] {
            assert!(ExtensionCatalog::is_valid_id(id), "{id} should be a valid id");
        }
        for id in ["", "Acme", "acme#two", "acme@one", "acme quota", "acme/quota", &"x".repeat(65), "-lead", ".lead", "ä"] {
            assert!(!ExtensionCatalog::is_valid_id(id), "{id} should not be a valid id");
        }
    }

    #[test]
    fn the_account_id_round_trips_through_the_manifest_id() {
        let account = account_id("acme-quota");
        assert_eq!(account, "extension--account-61636d652d71756f7461");
        assert_eq!(id_from_account_id(&account).as_deref(), Some("acme-quota"));
        assert!(crate::credential_store::valid_provider_id(&account));
        // Not this shape, or not a valid id inside it: no answer at all.
        assert!(id_from_account_id("extension--account-").is_none());
        assert!(id_from_account_id("extension--account-616").is_none());
        assert!(id_from_account_id("codex--account-ab").is_none());
        assert!(id_from_account_id(&account_id("Acme")).is_none());
    }

    // --- discovery ---------------------------------------------------------

    #[test]
    fn finds_a_valid_extension() {
        let sandbox = Sandbox::new("finds");
        sandbox.add("acme", Some(&manifest_with_timeout("acme", "  Acme Quota  ", 5.0)), Some("#!/bin/sh\necho '{}'\n"));

        let scan = ExtensionCatalog::scan_in(&sandbox.root);
        assert!(scan.problems.is_empty(), "{:?}", scan.problems);
        assert_eq!(scan.extensions.len(), 1);
        let found = &scan.extensions[0];
        assert_eq!(found.id, "acme");
        assert_eq!(found.name, "Acme Quota");
        assert_eq!(found.timeout, Duration::from_secs(5));
        assert_eq!(found.directory, fs::canonicalize(sandbox.root.join("acme")).unwrap());
        assert_eq!(found.account_id(), account_id("acme"));
        sandbox.remove();
    }

    #[test]
    fn clamps_the_timeout_into_its_range() {
        let sandbox = Sandbox::new("timeouts");
        sandbox.add("slow", Some(&manifest_with_timeout("slow", "Slow", 600.0)), Some("#!/bin/sh\n"));
        sandbox.add("fast", Some(&manifest_with_timeout("fast", "Fast", 0.0)), Some("#!/bin/sh\n"));
        sandbox.add("plain", Some(&manifest("plain", "Plain")), Some("#!/bin/sh\n"));

        let scan = ExtensionCatalog::scan_in(&sandbox.root);
        // Keyed by id, as the original's test is: `scan_in` returns folders in
        // name order, not in the order they were created.
        let timeouts: std::collections::HashMap<&str, Duration> =
            scan.extensions.iter().map(|extension| (extension.id.as_str(), extension.timeout)).collect();
        assert_eq!(timeouts["slow"], Duration::from_secs(60));
        assert_eq!(timeouts["fast"], Duration::from_secs(1));
        assert_eq!(timeouts["plain"], ExtensionCatalog::default_timeout());
        sandbox.remove();
    }

    #[test]
    fn reports_every_reason_a_folder_can_be_turned_away_for() {
        let sandbox = Sandbox::new("problems");
        sandbox.add("a-empty", None, None);
        sandbox.add("b-garbage", Some("not json"), Some("#!/bin/sh\n"));
        sandbox.add("c-future", Some(&manifest_with_schema("c-future", "Future", 2)), Some("#!/bin/sh\n"));
        sandbox.add("d-hash", Some(&manifest("acme#two", "Hash")), Some("#!/bin/sh\n"));
        sandbox.add("e-upper", Some(&manifest("Acme", "Upper")), Some("#!/bin/sh\n"));
        sandbox.add("f-noname", Some(&manifest("f-noname", "   ")), Some("#!/bin/sh\n"));
        sandbox.add("g-escape", Some(&manifest_for("g-escape", "Escape", "../a-empty/run")), Some("#!/bin/sh\n"));
        sandbox.add("h-absolute", Some(&manifest_for("h-absolute", "Absolute", "/bin/sh")), Some("#!/bin/sh\n"));
        sandbox.add("i-missing", Some(&manifest("i-missing", "Missing")), None);
        sandbox.add_named(
            "j-notexec",
            Some(&manifest_for("j-notexec", "NotExec", PLAIN)),
            Some("#!/bin/sh\n"),
            PLAIN,
            false,
        );

        let scan = ExtensionCatalog::scan_in(&sandbox.root);
        assert!(scan.extensions.is_empty(), "{:?}", scan.extensions);
        let reported: Vec<(&str, &Reason)> =
            scan.problems.iter().map(|problem| (problem.folder.as_str(), &problem.reason)).collect();
        assert_eq!(
            reported,
            vec![
                ("a-empty", &Reason::NoManifest),
                ("b-garbage", &Reason::UnreadableManifest),
                ("c-future", &Reason::UnsupportedSchema(2)),
                ("d-hash", &Reason::InvalidId),
                ("e-upper", &Reason::InvalidId),
                ("f-noname", &Reason::MissingName),
                ("g-escape", &Reason::ExecutableOutsideFolder),
                ("h-absolute", &Reason::ExecutableOutsideFolder),
                ("i-missing", &Reason::ExecutableMissing),
                ("j-notexec", &Reason::ExecutableMissing),
            ]
        );
        sandbox.remove();
    }

    #[test]
    fn every_reason_has_wording_of_its_own() {
        let messages: Vec<String> = [
            Reason::NoManifest,
            Reason::UnreadableManifest,
            Reason::UnsupportedSchema(7),
            Reason::InvalidId,
            Reason::MissingName,
            Reason::ExecutableOutsideFolder,
            Reason::ExecutableMissing,
            Reason::DuplicateId("acme".into()),
        ]
        .iter()
        .map(Reason::message)
        .collect();
        assert!(messages.iter().all(|message| !message.trim().is_empty()));
        assert!(messages[2].contains('7'));
        assert!(messages[7].contains("acme"));
        let mut unique = messages.clone();
        unique.sort();
        unique.dedup();
        assert_eq!(unique.len(), messages.len());
    }

    #[cfg(unix)]
    #[test]
    fn a_link_out_of_the_folder_is_refused() {
        let sandbox = Sandbox::new("symlink");
        let directory = sandbox.add_named("linked", Some(&manifest_for("linked", "Linked", "tool")), None, "tool", false);
        std::os::unix::fs::symlink("/bin/sh", directory.join("tool")).unwrap();

        let scan = ExtensionCatalog::scan_in(&sandbox.root);
        assert!(scan.extensions.is_empty());
        assert_eq!(scan.problems.len(), 1);
        assert_eq!(scan.problems[0].reason, Reason::ExecutableOutsideFolder);
        sandbox.remove();
    }

    #[test]
    fn the_first_folder_keeps_a_duplicated_id() {
        let sandbox = Sandbox::new("duplicates");
        sandbox.add("one", Some(&manifest("same", "First")), Some("#!/bin/sh\n"));
        sandbox.add("two", Some(&manifest("same", "Second")), Some("#!/bin/sh\n"));

        let scan = ExtensionCatalog::scan_in(&sandbox.root);
        let names: Vec<&str> = scan.extensions.iter().map(|extension| extension.name.as_str()).collect();
        assert_eq!(names, vec!["First"]);
        assert_eq!(scan.problems.len(), 1);
        assert_eq!(scan.problems[0].folder, "two");
        assert_eq!(scan.problems[0].reason, Reason::DuplicateId("same".into()));
        sandbox.remove();
    }

    #[test]
    fn a_folder_that_is_not_there_scans_as_empty() {
        let scan = ExtensionCatalog::scan_in(&std::env::temp_dir().join("pulsewin-no-such-extensions-folder"));
        assert_eq!(scan, Scan::default());
    }

    #[test]
    fn hidden_entries_and_plain_files_are_not_extensions() {
        let sandbox = Sandbox::new("skips");
        sandbox.add(".hidden", Some(&manifest(".hidden", "Hidden")), Some("#!/bin/sh\n"));
        fs::write(sandbox.root.join("stray.txt"), "not a folder").unwrap();
        sandbox.add("real", Some(&manifest("real", "Real")), Some("#!/bin/sh\n"));

        let scan = ExtensionCatalog::scan_in(&sandbox.root);
        let names: Vec<&str> = scan.extensions.iter().map(|extension| extension.name.as_str()).collect();
        assert_eq!(names, vec!["Real"]);
        assert!(scan.problems.is_empty(), "{:?}", scan.problems);
        sandbox.remove();
    }

    #[test]
    fn a_name_longer_than_sixty_characters_is_cut() {
        let sandbox = Sandbox::new("longname");
        let long = "N".repeat(80);
        sandbox.add("long", Some(&manifest("long", &long)), Some("#!/bin/sh\n"));
        let scan = ExtensionCatalog::scan_in(&sandbox.root);
        assert_eq!(scan.extensions[0].name.chars().count(), 60);
        assert_eq!(scan.extensions[0].name, "N".repeat(60));
        sandbox.remove();
    }

    // --- reports -----------------------------------------------------------

    fn reading(json: &str) -> ProviderUsage {
        ExtensionReport::reading(json.as_bytes(), &account_id("acme"), "Acme")
    }

    #[test]
    fn reads_limits_with_and_without_a_stated_length() {
        let usage = reading(
            r#"{"schemaVersion":1,"plan":" Team ","limits":[
                {"id":"month","label":"Monthly requests","usedPercent":42.5,"resetsAt":"2026-10-01T00:00:00Z","windowSeconds":2592000},
                {"label":"Tokens","used":30,"limit":120,"resetsAt":"2026-10-01T00:00:00.250Z"}]}"#,
        );
        assert!(usage.error.is_none());
        assert_eq!(usage.plan.as_deref(), Some("Team"));
        assert_eq!(usage.id, account_id("acme"));
        assert_eq!(usage.name, "Acme");
        let ids: Vec<Option<&str>> = usage.windows.iter().map(|window| window.id.as_deref()).collect();
        assert_eq!(ids, vec![Some("extension.month"), Some("extension.1")]);
        let labels: Vec<&str> = usage.windows.iter().map(|window| window.label.as_str()).collect();
        assert_eq!(labels, vec!["Monthly requests", "Tokens"]);
        assert_eq!(usage.windows[0].percent_used, Some(42.5));
        assert_eq!(usage.windows[0].window_seconds, Some(2_592_000.0));
        assert_eq!(usage.windows[0].resets_at.as_deref(), Some("2026-10-01T00:00:00Z"));
        assert_eq!(usage.windows[1].percent_used, Some(25.0));
        assert_eq!(usage.windows[1].window_seconds, None, "a window with no stated length reports none");
        assert!(usage.windows[1].resets_at.is_some(), "fractional seconds are a time too");
        assert!(!usage.windows[0].is_exhausted);
    }

    #[test]
    fn a_limit_with_no_figure_is_left_off_rather_than_drawn_at_zero() {
        let usage = reading(
            r#"{"schemaVersion":1,"limits":[
                {"label":"No figure"},
                {"label":"Only used","used":10},
                {"label":"Zero limit","used":1,"limit":0},
                {"label":"Negative","usedPercent":-5},
                {"label":"   ","usedPercent":10},
                {"label":"Kept","usedPercent":110}]}"#,
        );
        let labels: Vec<&str> = usage.windows.iter().map(|window| window.label.as_str()).collect();
        assert_eq!(labels, vec!["Kept"]);
        assert_eq!(usage.windows[0].percent_used, Some(110.0));
        assert!(usage.windows[0].is_exhausted, "a limit at or past 100% is spent");
    }

    #[test]
    fn a_percentage_wins_over_a_used_and_limit_pair() {
        let usage = reading(
            r#"{"schemaVersion":1,"limits":[{"label":"Both","usedPercent":50,"used":90,"limit":100}]}"#,
        );
        assert_eq!(usage.windows[0].percent_used, Some(50.0));
    }

    #[test]
    fn a_report_with_no_limits_and_no_balance_says_so() {
        for json in [r#"{"schemaVersion":1,"limits":[]}"#, r#"{"schemaVersion":1}"#] {
            let usage = reading(json);
            assert_eq!(usage.error.as_deref(), Some("扩展没有报告任何额度。"), "{json}");
            assert!(usage.windows.is_empty());
            assert!(usage.credit_remaining.is_none());
        }
    }

    #[test]
    fn a_balance_alone_is_a_reading() {
        let usage = reading(r#"{"schemaVersion":1,"balance":{"amount":16.33538288,"currency":"usd"}}"#);
        assert!(usage.error.is_none());
        assert!(usage.windows.is_empty());
        let money = usage.credit_remaining.as_ref().unwrap();
        assert_eq!(money.amount, 16.33538288);
        assert_eq!(money.currency, "USD");
        assert_eq!(usage.source.as_deref(), Some("扩展程序自带的登录"));
    }

    #[test]
    fn a_balance_below_zero_is_kept_and_does_not_mark_the_account_spent() {
        let usage = reading(
            r#"{"schemaVersion":1,"limits":[{"label":"Daily","usedPercent":30}],"balance":{"amount":-2,"currency":"cny"}}"#,
        );
        let labels: Vec<&str> = usage.windows.iter().map(|window| window.label.as_str()).collect();
        assert_eq!(labels, vec!["Daily"]);
        let money = usage.credit_remaining.as_ref().unwrap();
        assert_eq!(money.amount, -2.0);
        assert_eq!(money.currency, "CNY");
        assert!(!usage.windows[0].is_exhausted);
    }

    #[test]
    fn a_balance_that_is_not_money_is_no_balance_at_all() {
        for balance in [
            r#"{"amount":5,"currency":"dollars"}"#,
            r#"{"amount":5,"currency":""}"#,
            r#"{"amount":5,"currency":"U1D"}"#,
            r#"{"amount":5}"#,
            r#"{"currency":"USD"}"#,
        ] {
            let alone = reading(&format!(r#"{{"schemaVersion":1,"balance":{balance}}}"#));
            assert_eq!(alone.error.as_deref(), Some("扩展没有报告任何额度。"), "{balance}");

            let beside = reading(&format!(
                r#"{{"schemaVersion":1,"balance":{balance},"limits":[{{"label":"x","usedPercent":5}}]}}"#
            ));
            assert_eq!(beside.windows.len(), 1, "{balance}");
            assert!(beside.credit_remaining.is_none(), "{balance}");
        }
    }

    #[test]
    fn a_text_amount_makes_the_reply_unreadable() {
        let usage = reading(r#"{"schemaVersion":1,"balance":{"amount":"5","currency":"USD"}}"#);
        assert_eq!(usage.error.as_deref(), Some("无法读取扩展的输出。"));
    }

    #[test]
    fn every_status_has_its_own_answer() {
        let cases = [
            ("signedOut", "扩展表示它的登录已过期。"),
            ("unreachable", "服务没有响应。"),
            ("rateLimited", "检查过于频繁，稍后再试。"),
            ("serverError", "服务返回了一个错误。"),
            ("noLimits", "扩展没有报告任何额度。"),
            ("somethingElse", "无法读取扩展的输出。"),
        ];
        for (status, message) in cases {
            let usage = reading(&format!(
                r#"{{"schemaVersion":1,"status":"{status}","limits":[{{"label":"x","usedPercent":5}}]}}"#
            ));
            assert_eq!(usage.error.as_deref(), Some(message), "{status}");
            assert!(usage.windows.is_empty(), "{status}");
        }
        // A report with no status is an ordinary success.
        let ok = reading(r#"{"schemaVersion":1,"limits":[{"label":"x","usedPercent":5}]}"#);
        assert!(ok.error.is_none());
    }

    #[test]
    fn a_reply_pulse_cannot_read_says_so() {
        for json in ["", "not json", "[]", r#"{"limits":[]}"#, r#"{"schemaVersion":2,"limits":[]}"#] {
            let usage = reading(json);
            assert_eq!(usage.error.as_deref(), Some("无法读取扩展的输出。"), "{json}");
        }
    }

    #[test]
    fn a_repeated_limit_id_is_drawn_once() {
        let usage = reading(
            r#"{"schemaVersion":1,"limits":[{"id":"a","label":"First","usedPercent":1},{"id":"a","label":"Second","usedPercent":2}]}"#,
        );
        let labels: Vec<&str> = usage.windows.iter().map(|window| window.label.as_str()).collect();
        assert_eq!(labels, vec!["First"]);
    }

    #[test]
    fn reads_metrics_beside_a_balance() {
        let usage = reading(
            r#"{"schemaVersion":1,"balance":{"amount":16.33,"currency":"USD"},"metrics":[
                {"id":"ttft","label":" 首字时间 ","value":320,"unit":"ms","history":[
                    {"t":"2026-10-12T16:00:00+08:00","v":320},
                    {"t":"2026-10-12T14:00:00+08:00","v":410},
                    {"t":"2026-10-12T15:00:00.500+08:00","v":355}]},
                {"label":"Output rate","value":87.4,"unit":"tok/s"},
                {"id":"ok","label":"Success","value":-0.5}]}"#,
        );
        assert!(usage.error.is_none());
        assert_eq!(usage.credit_remaining.as_ref().unwrap().currency, "USD");
        let ids: Vec<&str> = usage.metrics.iter().map(|metric| metric.id.as_str()).collect();
        assert_eq!(ids, vec!["ttft", "1", "ok"]);
        let labels: Vec<&str> = usage.metrics.iter().map(|metric| metric.label.as_str()).collect();
        assert_eq!(labels, vec!["首字时间", "Output rate", "Success"]);
        let values: Vec<f64> = usage.metrics.iter().map(|metric| metric.value).collect();
        assert_eq!(values, vec![320.0, 87.4, -0.5]);
        let units: Vec<Option<&str>> = usage.metrics.iter().map(|metric| metric.unit.as_deref()).collect();
        assert_eq!(units, vec![Some("ms"), Some("tok/s"), None]);
        let series: Vec<f64> = usage.metrics[0].history.iter().map(|point| point.value).collect();
        assert_eq!(series, vec![410.0, 355.0, 320.0], "oldest first");
        assert_eq!(usage.metrics[0].history[1].at, "2026-10-12T15:00:00.500+08:00");
        assert!(usage.metrics[1].history.is_empty());
    }

    #[test]
    fn an_unusable_metric_takes_nothing_else_with_it() {
        let usage = reading(
            r#"{"schemaVersion":1,"limits":[{"label":"Daily","usedPercent":30}],"metrics":[
                {"label":"Text value","value":"320"},
                {"label":"No value"},
                {"value":5},
                {"label":"  ","value":5},
                "not an object",
                {"id":"a","label":"Kept","value":1},
                {"id":"a","label":"Same id","value":2}]}"#,
        );
        let labels: Vec<&str> = usage.windows.iter().map(|window| window.label.as_str()).collect();
        assert_eq!(labels, vec!["Daily"]);
        let metrics: Vec<&str> = usage.metrics.iter().map(|metric| metric.label.as_str()).collect();
        assert_eq!(metrics, vec!["Kept"]);
    }

    #[test]
    fn a_metrics_field_that_is_not_a_list_leaves_the_limits_alone() {
        let usage = reading(
            r#"{"schemaVersion":1,"limits":[{"label":"Daily","usedPercent":30}],"metrics":{"a":1}}"#,
        );
        assert_eq!(usage.windows.len(), 1);
        assert!(usage.metrics.is_empty());
        assert!(usage.error.is_none());
    }

    #[test]
    fn metrics_alone_are_not_a_reading() {
        let usage = reading(r#"{"schemaVersion":1,"metrics":[{"label":"Rate","value":5}]}"#);
        assert_eq!(usage.error.as_deref(), Some("扩展没有报告任何额度。"));
    }

    #[test]
    fn keeps_four_metrics_and_the_newest_hundred_and_twenty_points() {
        let mut history = String::new();
        for minute in 0..200 {
            if minute > 0 {
                history.push(',');
            }
            history.push_str(&format!(
                r#"{{"t":"2026-10-12T{:02}:{:02}:00Z","v":{minute}}}"#,
                minute / 60,
                minute % 60
            ));
        }
        // A point with no time, and a second point at a time already stated:
        // the first stated wins, so 999 never appears.
        history.push_str(r#",{"t":"not a time","v":1},{"t":"2026-10-12T00:00:00Z","v":999}"#);

        let mut metrics = format!(r#"{{"label":"Series","value":1,"history":[{history}]}}"#);
        for index in 0..5 {
            metrics.push_str(&format!(r#",{{"label":"M{index}","value":{index}}}"#));
        }
        let usage = reading(&format!(r#"{{"schemaVersion":1,"balance":{{"amount":1,"currency":"USD"}},"metrics":[{metrics}]}}"#));

        assert_eq!(usage.metrics.len(), ExtensionReport::metric_limit());
        let labels: Vec<&str> = usage.metrics.iter().map(|metric| metric.label.as_str()).collect();
        assert_eq!(labels, vec!["Series", "M0", "M1", "M2"]);
        let series = &usage.metrics[0].history;
        assert_eq!(series.len(), ExtensionReport::history_limit());
        assert_eq!(series.first().unwrap().value, 80.0, "oldest kept");
        assert_eq!(series.last().unwrap().value, 199.0, "newest kept, never thinned");
        assert!(!series.iter().any(|point| point.value == 999.0));
    }

    #[test]
    fn a_unit_longer_than_sixteen_characters_is_cut() {
        let usage = reading(&format!(
            r#"{{"schemaVersion":1,"balance":{{"amount":1,"currency":"USD"}},"metrics":[{{"label":"Rate","value":1,"unit":"{}"}}]}}"#,
            "u".repeat(40)
        ));
        assert_eq!(usage.metrics[0].unit.as_deref().unwrap().chars().count(), 16);
    }
}
