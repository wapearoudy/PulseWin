//! Pulse 1.9 CapturedTraeReader/CapturedWarpReader (Apache-2.0), the rest of Group E.
//! Trae's cache is a raw JSON array of per-turn counters with **no row id**: equal rows
//! inside one page stay two rows, while overlapping pages are reconciled by content
//! multiset ({A} and {A,B} give A and B, never two A's) instead of concatenated.
//! Warp's snapshot carries `requestsUsed` and `spendCents` and no token figure at all,
//! so its tally is zero by design — inventing one would report work nobody measured.
#![allow(dead_code)] // The dispatcher in token_spend.rs routes these; until it does,
                      // a non-test build has no caller yet.

use super::measured_readers::{count, record, text, timestamp};
use super::*;

/// A captured export is bounded like every other document here; a larger file is
/// refused rather than streamed, because the whole page is one JSON array.
const MAX_DOCUMENT: u64 = 64 * 1024 * 1024;

/// The Group E clients this file owns. Cursor/antigravity/hindsight/mcode stay in
/// `measured_readers`; `captured()` is what marks a source `origin: "export"`.
pub(super) fn captured(agent: &str) -> bool { matches!(agent, "trae" | "warp") }

/// `<config>/trae-cache/sessions` and `<config>/warp-cache`, plus Pulse's own drop
/// folder. `$TOKSCALE_CONFIG_DIR` wins, else `~/.config/tokscale`, exactly as the
/// original's `cacheRoot`. The drop folder lives under `%APPDATA%` on Windows and
/// under `~/Library/Application Support/Pulse` on macOS/Linux; a root is named even
/// when it does not exist, because it is part of the store's identity.
pub(super) fn roots(home: &Path, id: &str, env: &impl Fn(&str) -> Option<String>) -> Vec<PathBuf> {
    let cache = env("TOKSCALE_CONFIG_DIR")
        .filter(|v| !v.trim().is_empty())
        .map(|v| PathBuf::from(v.trim()))
        .unwrap_or_else(|| home.join(".config/tokscale"));
    let mut roots = vec![match id {
        "trae" => cache.join("trae-cache/sessions"),
        "warp" => cache.join("warp-cache"),
        _ => return Vec::new(),
    }];
    roots.push(match env("APPDATA").filter(|v| !v.trim().is_empty()) {
        Some(appdata) => PathBuf::from(appdata).join("PulseWin/UsageImports").join(id),
        None => home.join("Library/Application Support/Pulse/UsageImports").join(id),
    });
    roots
}

/// Trae reads whatever `.json` it is handed and lets the root decide (an array is a
/// page, anything else is nothing). Warp's reader produces no tokens, but its
/// `usage*.json` snapshot is still a candidate so the pane can show that the store
/// exists and say it holds requests and cost only.
pub(super) fn candidate(path: &Path, id: &str) -> bool {
    let extension = path.extension().unwrap_or_default().to_string_lossy().to_ascii_lowercase();
    match id {
        "trae" => extension == "json",
        "warp" => extension == "json"
            && path.file_stem().unwrap_or_default().to_string_lossy().to_ascii_lowercase().starts_with("usage"),
        _ => false,
    }
}

/// One export page. Trae: `extra_info` carries the four real counters and nothing
/// else is folded in; `session_id` names a session and is never a row identity, so
/// no `deduplication_id` is set and equal rows survive. Warp: no token count exists
/// in the format, so the page is read only far enough to say so.
pub(super) fn read(path: &Path, id: &str, cancel: &AtomicBool) -> Result<(Vec<SpendRecord>, Vec<String>), String> {
    check(cancel)?;
    if id == "warp" {
        return Ok((Vec::new(), vec!["Warp：快照只报告请求数与花费，没有 token 计数，因此不产生 token 用量。".into()]));
    }
    let bytes = page_bytes(path, cancel)?;
    let data = bytes.strip_prefix(&[0xef, 0xbb, 0xbf]).unwrap_or(&bytes);
    let root: Value = serde_json::from_slice(data)
        .map_err(|error| format!("Trae：导出页无法解码（{error}）；计数可能不完整。"))?;
    let Some(sessions) = root.as_array() else { return Ok((Vec::new(), Vec::new())); };
    // The page's own bytes are its identity. Two byte-identical exports are one
    // page, and this is how `reconcile` recognises them across different paths.
    let scope = format!("trae-page:{}-{:016x}", bytes.len(), digest(&bytes));
    let mut records = Vec::new();
    for session in sessions {
        check(cancel)?;
        if records.len() >= MAX_RECORDS { return Err("Trae：记录超过读取上限。".into()); }
        // A session id is required; a page row that names none cannot be placed.
        let Some(session_id) = session.get("session_id").and_then(text) else { continue };
        // `usage_time` is epoch seconds by schema; a non-positive value is the
        // store's "unset", not 1970-01-01.
        let Some(at) = session.get("usage_time").and_then(|v| timestamp(v, false)).filter(|at| at.timestamp() > 0)
        else { continue };
        let extra = session.get("extra_info").cloned().unwrap_or(Value::Null);
        let tally = Tally {
            input: count(&extra["input_token"]),
            cache_write: count(&extra["cache_write_token"]),
            cache_read: count(&extra["cache_read_token"]),
            output: count(&extra["output_token"]),
            ..Default::default()
        };
        // An all-zero row asserts no usage; anything else in `extra_info` (a
        // prelude count, a dollar figure) states no bucket and is left alone.
        if tally.total() == 0 { continue; }
        let mut entry = record("trae", model_name(session), at, session_id, tally, 0, None);
        entry.source_scope = Some(scope.clone());
        records.push(entry);
    }
    check(cancel)?;
    Ok((records, Vec::new()))
}

/// Multiset union across the pages of one store, ported from
/// `CapturedSupport.reconcile`.
///
/// A row found in more than one page is kept **once per the most any single page
/// holds**; two equal rows inside one page are kept, because that page says there
/// were two. Byte-identical pages are folded outright first — a copied export is
/// one page, not an overlap. When a row appears across pages more often than any
/// one page holds it, the shared region is unconfirmed and the whole read is
/// flagged rather than added again.
pub(super) fn reconcile(files: Vec<Vec<SpendRecord>>, notes: &mut Vec<String>, cancel: &AtomicBool) -> Result<Vec<SpendRecord>, String> {
    let mut seen: HashSet<String> = HashSet::new();
    let mut pages: Vec<Vec<SpendRecord>> = Vec::new();
    for file in files {
        check(cancel)?;
        let scope = file.first().and_then(|r| r.source_scope.clone()).unwrap_or_default();
        if !scope.is_empty() && !seen.insert(scope) { continue; }
        pages.push(file);
    }
    let mut order: Vec<String> = Vec::new();
    let mut representative: HashMap<String, SpendRecord> = HashMap::new();
    let mut maximum: HashMap<String, usize> = HashMap::new();
    let mut totals: HashMap<String, usize> = HashMap::new();
    for page in pages {
        check(cancel)?;
        let mut counts: HashMap<String, usize> = HashMap::new();
        for entry in page {
            check(cancel)?;
            let key = signature(&entry)?;
            *counts.entry(key.clone()).or_default() += 1;
            if !representative.contains_key(&key) { order.push(key.clone()); representative.insert(key, entry); }
        }
        for (key, count) in counts {
            let top = maximum.entry(key.clone()).or_default();
            *top = (*top).max(count);
            *totals.entry(key).or_default() += count;
        }
    }
    let mut records = Vec::new();
    let mut overlapped = false;
    for key in order {
        check(cancel)?;
        let emit = maximum[&key];
        if totals[&key] > emit { overlapped = true; }
        let Some(entry) = representative.get(&key) else { continue };
        for _ in 0..emit {
            if records.len() >= MAX_RECORDS { return Err("Trae：记录超过读取上限。".into()); }
            let mut copy = entry.clone();
            // The page digest was scaffolding for this fold; it is not a scope the
            // user should ever see on a record.
            copy.source_scope = None;
            records.push(copy);
        }
    }
    if overlapped {
        notes.push("Trae：多个导出页存在无法确认的重叠；按每个文件实测重复次数的最大值合并，计数可能不完整。".into());
    }
    check(cancel)?;
    Ok(records)
}

/// `CapturedSupport`'s content identity for one row: what the row says, never where
/// it was read from. A JSON tuple rather than `:`-joined text, so a model or session
/// name containing the separator cannot collide with another row.
fn signature(entry: &SpendRecord) -> Result<String, String> {
    serde_json::to_string(&(
        &entry.session, entry.source_timestamp, &entry.model,
        entry.tally.input, entry.tally.cache_write, entry.tally.cache_read, entry.tally.output,
    )).map_err(|error| error.to_string())
}

/// The model a Trae row belongs to. An empty `model_name` means Auto mode picked the
/// model per turn, so the row is filed under `trae-<mode>`; the true per-turn model is
/// not recoverable from the cache and is not guessed.
fn model_name(session: &Value) -> String {
    if let Some(name) = session.get("model_name").and_then(text) { return normalized(&name); }
    if let Some(mode) = session.get("mode").and_then(text) { return format!("trae-{mode}"); }
    "trae-unknown".into()
}

/// Spell a reported name the way the price list spells it, and leave anything
/// unrecognised exactly as written so it is counted and reported unpriced rather
/// than disguised under a name nobody published.
fn normalized(name: &str) -> String {
    let trimmed = name.trim();
    let lower = trimmed.to_ascii_lowercase();
    if lower.starts_with("gpt-5") { return spaced(&lower); }
    if lower.starts_with("gemini 3.1") || lower.starts_with("gemini-3.1") { return spaced(&lower); }
    if lower == "glm 5.1" || lower == "glm-5.1" { return "glm-5.1".into(); }
    if lower.starts_with("claude sonnet 4.5") { return spaced(&lower.replace("4.5", "4-5")); }
    if lower.starts_with("claude sonnet 4.6") { return spaced(&lower.replace("4.6", "4-6")); }
    trimmed.to_owned()
}

/// `spaced`: spaces and underscores become dashes, empty pieces dropped.
fn spaced(value: &str) -> String {
    value.split(|c| c == ' ' || c == '_').filter(|part| !part.is_empty()).collect::<Vec<_>>().join("-")
}

fn page_bytes(path: &Path, cancel: &AtomicBool) -> Result<Vec<u8>, String> {
    let size = fs::metadata(path).map_err(|error| format!("Trae：导出页无法读取（{error}）。"))?.len();
    if size > MAX_DOCUMENT { return Err("Trae：导出页超过 64 MiB，未读取；计数可能不完整。".into()); }
    check(cancel)?;
    let data = fs::read(path).map_err(|error| format!("Trae：导出页无法读取（{error}）。"))?;
    check(cancel)?;
    Ok(data)
}

/// FNV-1a 64-bit: a file's byte identity for the "copied page" fold. Only ever
/// compared against another whole file of the same store, never used as a row id.
fn digest(bytes: &[u8]) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in bytes { hash ^= *byte as u64; hash = hash.wrapping_mul(0x0000_0100_0000_01b3); }
    hash
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicUsize;

    static NEXT: AtomicUsize = AtomicUsize::new(0);

    fn temp_dir(label: &str) -> PathBuf {
        let suffix = format!("{label}-{}-{}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed));
        let candidate = std::env::temp_dir().join(format!("pulsewin-{suffix}"));
        if fs::create_dir_all(&candidate).is_ok() { return candidate; }
        // Sandboxed test binaries can be denied writes outside the workspace.
        let fallback = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target").join("test-scratch").join(format!("pulsewin-{suffix}"));
        fs::create_dir_all(&fallback).unwrap_or_else(|error| panic!("create_dir_all {}: {error:?}", fallback.display()));
        fallback
    }

    fn home() -> PathBuf { temp_dir("captured-extra-home") }

    fn page(dir: &Path, name: &str, body: &str) -> PathBuf {
        let path = dir.join(name);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, body).unwrap();
        path
    }

    const TRAE_PAGE: &str = r#"[
      {"session_id":"t1","model_name":"GPT-5.4","usage_time":1717200000,"dollar_float":0.5,
       "extra_info":{"input_token":100,"output_token":20,"cache_read_token":30,"cache_write_token":40,"prelude_token":999}},
      {"session_id":"t1","model_name":"GPT-5.4","usage_time":1717200000,
       "extra_info":{"input_token":100,"output_token":20,"cache_read_token":30,"cache_write_token":40}},
      {"session_id":"t2","model_name":"","mode":"auto","usage_time":1717203600,
       "extra_info":{"input_token":10,"output_token":5}},
      {"session_id":"","model_name":"GPT-5.4","usage_time":1717203600,"extra_info":{"input_token":1}},
      {"session_id":"t4","model_name":"Mystery X","usage_time":0,"extra_info":{"input_token":9}},
      {"session_id":"t5","model_name":"Claude Sonnet 4.5","usage_time":1717207200,
       "extra_info":{"input_token":7,"output_token":3,"cache_read_token":0,"cache_write_token":0}}
    ]"#;

    #[test]
    fn roots_follow_tokscale_cache_and_import_drop() {
        let home = home();
        let cache = |key: &str| (key == "TOKSCALE_CONFIG_DIR").then(|| "/opt/tokscale".to_string());
        assert_eq!(roots(&home, "trae", &cache), vec![
            PathBuf::from("/opt/tokscale/trae-cache/sessions"),
            home.join("Library/Application Support/Pulse/UsageImports/trae"),
        ]);
        assert_eq!(roots(&home, "warp", &cache), vec![
            PathBuf::from("/opt/tokscale/warp-cache"),
            home.join("Library/Application Support/Pulse/UsageImports/warp"),
        ]);
        // The Windows profile relocates Pulse's own drop folder.
        let appdata = |key: &str| (key == "APPDATA").then(|| "C:\\Users\\me\\AppData\\Roaming".to_string());
        assert_eq!(roots(&home, "warp", &appdata)[1], PathBuf::from("C:\\Users\\me\\AppData\\Roaming/PulseWin/UsageImports/warp"));
        // Defaults when no override is set.
        assert_eq!(roots(&home, "trae", &|_: &str| None)[0], home.join(".config/tokscale/trae-cache/sessions"));
        assert!(roots(&home, "cursor", &|_: &str| None).is_empty());
        let _ = fs::remove_dir_all(&home);
    }

    #[test]
    fn candidate_accepts_trae_json_and_warp_usage_json_only() {
        // Separators are the host's: a `Path` splits on `/` here and on `\\` on Windows,
        // and a forward slash is a separator on both.
        assert!(candidate(Path::new("C:/tokscale/trae-cache/sessions/page.json"), "trae"));
        assert!(!candidate(Path::new("C:/tokscale/trae-cache/sessions/page.jsonl"), "trae"));
        assert!(candidate(Path::new("C:/tokscale/warp-cache/usage.json"), "warp"));
        assert!(!candidate(Path::new("C:/tokscale/warp-cache/notes.json"), "warp"));
        // Trae takes any `.json` it is handed — the root already scoped the store, and
        // the page's root decides whether it holds anything. Warp is narrower: only the
        // `usage*` snapshot is its own.
        assert!(candidate(Path::new("C:/tokscale/trae-cache/sessions/anything.json"), "trae"));
        assert!(!candidate(Path::new("C:/tokscale/trae-cache/sessions/page.csv"), "trae"));
        assert!(!candidate(Path::new("C:/tokscale/warp-cache/page.json"), "warp"));
        assert!(!candidate(Path::new("C:/tokscale/warp-cache/usage.jsonl"), "warp"));
        assert!(!candidate(Path::new("page.json"), "cursor"));
    }

    #[test]
    fn trae_keeps_equal_rows_in_one_page_and_names_only_stated_sessions() {
        let home = home();
        let path = page(&home, "page.json", TRAE_PAGE);
        let (records, notes) = read(&path, "trae", &AtomicBool::new(false)).unwrap();
        // t1 twice (no row id), t2 auto, t5; the blank session and the zero time go.
        assert_eq!(records.len(), 4, "{records:?}");
        assert!(notes.is_empty(), "{notes:?}");
        assert!(records.iter().all(|record| record.deduplication_id.is_none()));
        let same_second: Vec<_> = records.iter().filter(|record| record.session == "t1").collect();
        assert_eq!(same_second.len(), 2);
        assert!(same_second.iter().all(|record| record.tally
            == Tally { input: 100, output: 20, cache_write: 40, cache_read: 30, ..Default::default() }));
        // `prelude_token` states no bucket and is left out; the dollar figure too.
        assert!(same_second.iter().all(|record| record.tally.total() == 190));
        assert_eq!(records.iter().find(|record| record.model == "trae-auto").unwrap().tally.total(), 15);
        let claude = records.iter().find(|record| record.model == "claude-sonnet-4-5").unwrap();
        assert_eq!(claude.tally.total(), 10);
        assert!(!claude.aggregate_timing);
        assert_eq!(claude.source_timestamp, Some(1_717_207_200));
        // A non-array root is not a page at all, and is not an error either.
        let other = page(&home, "notes.json", r#"{"notes":"something else"}"#);
        assert!(read(&other, "trae", &AtomicBool::new(false)).unwrap().0.is_empty());
        let serialized = serde_json::to_string(&records).unwrap();
        assert!(!serialized.contains("prelude_token"));
        let _ = fs::remove_dir_all(&home);
    }

    #[test]
    fn trae_overlapping_pages_are_reconciled_and_marked_incomplete() {
        let home = home();
        let shared = r#"{"session_id":"t1","model_name":"GPT-5.4","usage_time":1717200000,"extra_info":{"input_token":100,"output_token":20,"cache_read_token":30,"cache_write_token":40}}"#;
        let extra = r#"{"session_id":"t2","model_name":"GPT-5.4","usage_time":1717209000,"extra_info":{"input_token":5,"output_token":5}}"#;
        let a = page(&home, "sessions/a.json", &format!("[{shared}]"));
        let b = page(&home, "sessions/b.json", &format!("[{shared},{extra}]"));
        let pages = vec![
            read(&a, "trae", &AtomicBool::new(false)).unwrap().0,
            read(&b, "trae", &AtomicBool::new(false)).unwrap().0,
        ];
        let mut notes = Vec::new();
        let records = reconcile(pages, &mut notes, &AtomicBool::new(false)).unwrap();
        // A once, then B: never 2A + B. The unconfirmed overlap is stated.
        assert_eq!(records.len(), 2, "{records:?}");
        assert_eq!(records.iter().filter(|record| record.session == "t1").count(), 1);
        assert!(records.iter().any(|record| record.session == "t2" && record.tally.total() == 10));
        assert_eq!(records.iter().map(|record| record.tally.total()).sum::<u64>(), 200);
        assert!(notes.iter().any(|note| note.contains("无法确认的重叠")), "{notes:?}");
        // The page digest is scaffolding, not a scope the user sees.
        assert!(records.iter().all(|record| record.source_scope.is_none()));
        let _ = fs::remove_dir_all(&home);
    }

    #[test]
    fn trae_byte_identical_pages_are_one_page_not_an_overlap() {
        let home = home();
        let a = page(&home, "sessions/page.json", TRAE_PAGE);
        let b = page(&home, "sessions/page-copy.json", TRAE_PAGE);
        let pages = vec![
            read(&a, "trae", &AtomicBool::new(false)).unwrap().0,
            read(&b, "trae", &AtomicBool::new(false)).unwrap().0,
        ];
        let mut notes = Vec::new();
        let records = reconcile(pages, &mut notes, &AtomicBool::new(false)).unwrap();
        // A copied page is the same page: four rows, and nothing to flag.
        assert_eq!(records.len(), 4, "{records:?}");
        assert!(notes.is_empty(), "{notes:?}");
        // Two rows that only look equal across *different* pages are an overlap.
        let c = page(&home, "sessions/c.json", r#"[{"session_id":"t1","model_name":"GPT-5.4","usage_time":1717200000,"extra_info":{"input_token":1}}]"#);
        let d = page(&home, "sessions/d.json", r#"[{"session_id":"t1","model_name":"GPT-5.4","usage_time":1717200000,"extra_info":{"input_token":1}}, {"session_id":"t9","model_name":"GPT-5.4","usage_time":1717200600,"extra_info":{"input_token":2}}]"#);
        let mut notes = Vec::new();
        let records = reconcile(vec![
            read(&c, "trae", &AtomicBool::new(false)).unwrap().0,
            read(&d, "trae", &AtomicBool::new(false)).unwrap().0,
        ], &mut notes, &AtomicBool::new(false)).unwrap();
        assert_eq!(records.len(), 2);
        assert!(notes.iter().any(|note| note.contains("无法确认的重叠")), "{notes:?}");
        let _ = fs::remove_dir_all(&home);
    }

    #[test]
    fn warp_reports_requests_and_cost_but_yields_no_tokens() {
        let home = home();
        let path = page(&home, "warp-cache/usage.json", r#"{"syncedAt":"2024-06-01T00:00:00Z","usage":{"requestsUsed":42,"spendCents":1234},"workspaces":[{"id":"ws 1","name":"Work","requestsUsed":10,"spendCents":99}]}"#);
        let (records, notes) = read(&path, "warp", &AtomicBool::new(false)).unwrap();
        // Zero tokens, and the reason is stated rather than shown as a silent zero.
        assert!(records.is_empty());
        assert_eq!(notes.len(), 1);
        assert!(notes[0].contains("没有 token 计数"), "{notes:?}");
        // The root is still named so the pane can say the snapshot exists.
        assert!(roots(&home, "warp", &|_: &str| None).iter().any(|root| root.ends_with("warp-cache")));
        let _ = fs::remove_dir_all(&home);
    }

    #[test]
    fn no_message_text_or_secret_reaches_a_record() {
        let home = home();
        // Everything a Trae page can carry besides counters: a prompt, a completion,
        // a token, a cookie. Only the four counters and the declared names survive.
        let path = page(&home, "page.json", r#"[{"session_id":"t1","model_name":"GPT-5.4","usage_time":1717200000,
          "extra_info":{"input_token":100,"output_token":20,"cache_read_token":30,"cache_write_token":40},
          "prompt":"SECRET PROMPT BODY","completion":"SECRET COMPLETION BODY",
          "api_key":"SECRET KEY","cookie":"SECRET COOKIE"}]"#);
        let (records, _) = read(&path, "trae", &AtomicBool::new(false)).unwrap();
        let serialized = serde_json::to_string(&records).unwrap();
        for secret in ["SECRET PROMPT BODY", "SECRET COMPLETION BODY", "SECRET KEY", "SECRET COOKIE", "api_key", "cookie"] {
            assert!(!serialized.contains(secret), "{secret} leaked into {serialized}");
        }
        assert_eq!(records[0].tally.total(), 190);
        let _ = fs::remove_dir_all(&home);
    }

    #[test]
    fn cancellation_and_unreadable_pages_are_safe() {
        let home = home();
        let path = page(&home, "page.json", TRAE_PAGE);
        assert_eq!(read(&path, "trae", &AtomicBool::new(true)).unwrap_err(), "cancelled");
        assert_eq!(reconcile(vec![Vec::new()], &mut Vec::new(), &AtomicBool::new(true)).unwrap_err(), "cancelled");
        assert!(read(&home.join("absent.json"), "trae", &AtomicBool::new(false)).is_err());
        let _ = fs::remove_dir_all(&home);
    }
}
