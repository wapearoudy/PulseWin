//! Opt-in measured token usage. Reads counters only; never retains message text.
//! Claude/Codex semantics follow UsageLedgerReader; Qwen follows QwenSessionReader.
//! Gemini and explicit export/capture formats live in measured_readers.
//! Other source readers
//! are deliberately not represented as implemented until their formats are ported.
use chrono::{DateTime, Local, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs::{self, File};
use std::hash::{Hash, Hasher};
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::sync::{atomic::{AtomicBool, AtomicU64, Ordering}, Arc, Mutex, OnceLock};
use std::time::{Duration, UNIX_EPOCH};

const CACHE_VERSION: u32 = 2;
const TRANSCRIPT_VERSION: u32 = 5;
const MAX_LINE: usize = 8 * 1024 * 1024;
const MAX_FILES: usize = 50_000;
const MAX_RECORDS: usize = 1_000_000;
const AGENTS: [(&str, &str); 11] = [("claude", "Claude Code"), ("codex", "Codex"), ("qwen", "Qwen Code"),
    ("gemini", "Gemini CLI"), ("cursor", "Cursor"), ("antigravity", "Antigravity"), ("hindsight", "Hindsight"), ("mcode", "MCode"),
    ("opencode", "OpenCode"), ("kilo", "Kilo CLI"), ("micode", "MiMo Code")];
#[path = "token_spend/measured_readers.rs"]
mod measured_readers;
#[path = "token_spend/opencode_store.rs"]
mod opencode_store;

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Tally { pub input: u64, pub output: u64, pub cache_write: u64, pub cache_read: u64 }
impl Tally {
    fn total(&self) -> u64 { self.input.saturating_add(self.output).saturating_add(self.cache_write).saturating_add(self.cache_read) }
    fn add(&mut self, other: &Self) {
        self.input = self.input.saturating_add(other.input); self.output = self.output.saturating_add(other.output);
        self.cache_write = self.cache_write.saturating_add(other.cache_write); self.cache_read = self.cache_read.saturating_add(other.cache_read);
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SpendRecord {
    pub agent: String, pub model: String, pub day: String, pub hour: u32,
    pub session: String, pub project: Option<String>, pub tally: Tally,
    pub unclassified_tokens: u64, pub deduplication_id: Option<String>,
    // Only exports that state request timing can contribute to an hour chart.
    #[serde(default)] pub aggregate_timing: bool,
    #[serde(default)] pub source_timestamp: Option<i64>,
    #[serde(default)] pub source_scope: Option<String>,
    pub cost: Option<f64>, pub cost_breakdown: Option<[f64; 4]>, pub model_name: Option<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SpendSource { pub id: String, pub name: String, pub status: String, pub files: usize, pub cached_files: usize, pub records: usize,
    pub origin: String, pub roots: Vec<String> }
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SpendSnapshot {
    pub scanned_at: String, pub records: Vec<SpendRecord>, pub sources: Vec<SpendSource>,
    pub notes: Vec<String>, pub prices_at: Option<String>, pub pricing_status: String,
}
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SpendScan {
    pub id: String, pub status: String, pub current_source: Option<String>, pub source_index: usize,
    pub source_count: usize, pub snapshot: Option<SpendSnapshot>, pub error: Option<String>,
}
struct Run { scan: SpendScan, cancel: Arc<AtomicBool> }
static RUN: OnceLock<Mutex<Option<Run>>> = OnceLock::new();
static NEXT_ID: AtomicU64 = AtomicU64::new(1);
fn state() -> &'static Mutex<Option<Run>> { RUN.get_or_init(|| Mutex::new(None)) }
fn check(cancel: &AtomicBool) -> Result<(), String> { if cancel.load(Ordering::Acquire) { Err("cancelled".into()) } else { Ok(()) } }
fn ensure_opt_in(requested: bool, persisted: bool) -> Result<(), String> {
    if requested && persisted { Ok(()) } else { Err("Token Spend reading is disabled".into()) }
}
fn publish(id: &str, update: impl FnOnce(&mut SpendScan)) {
    if let Ok(mut slot) = state().lock() { if let Some(run) = slot.as_mut().filter(|r| r.scan.id == id && !r.cancel.load(Ordering::Acquire)) { update(&mut run.scan); } }
}

/// The UI persists an independent, default-false opt-in. This guard also means
/// opening the disabled pane cannot discover paths, load caches or fetch prices.
#[tauri::command]
pub async fn spend_begin_scan(enabled: bool, force: bool) -> Result<String, String> {
    ensure_opt_in(enabled, enabled && crate::preferences::load().token_spend_enabled)?;
    let id = format!("{}-{}", Utc::now().timestamp_millis(), NEXT_ID.fetch_add(1, Ordering::Relaxed));
    let cancel = Arc::new(AtomicBool::new(false));
    {
        let mut slot = state().lock().map_err(|e| e.to_string())?;
        if let Some(previous) = slot.take() { previous.cancel.store(true, Ordering::Release); }
        *slot = Some(Run { cancel: cancel.clone(), scan: SpendScan { id: id.clone(), status: "running".into(),
            current_source: Some("公开模型价格".into()), source_index: 0, source_count: AGENTS.len(), snapshot: None, error: None } });
    }
    let task_id = id.clone();
    tokio::spawn(async move {
        let Some(home) = crate::credentials::home_dir() else { publish(&task_id, |s| { s.status = "error".into(); s.error = Some("无法找到用户目录".into()); }); return; };
        let cache = crate::credentials::cache_dir().unwrap_or_else(|| home.join(".cache")).join("PulseWin/token-spend");
        let (prices, prices_at, pricing_status) = fetch_prices(&cache, &cancel).await;
        if check(&cancel).is_err() { return; }
        let scan_id = task_id.clone(); let worker_cancel = cancel.clone();
        let result = tokio::task::spawn_blocking(move || scan_sources(&home, &cache, &prices, force, &worker_cancel, |name, index| {
            publish(&scan_id, |s| { s.current_source = Some(name.into()); s.source_index = index; });
        })).await;
        if check(&cancel).is_err() { return; }
        match result {
            Ok(Ok(mut snapshot)) => { snapshot.prices_at = prices_at; snapshot.pricing_status = pricing_status;
                publish(&task_id, |s| { s.status = "completed".into(); s.current_source = None; s.snapshot = Some(snapshot); }); }
            Ok(Err(error)) => publish(&task_id, |s| { s.status = "error".into(); s.error = Some(error); }),
            Err(error) => publish(&task_id, |s| { s.status = "error".into(); s.error = Some(error.to_string()); }),
        }
    });
    Ok(id)
}
#[tauri::command]
pub fn spend_get_scan(scan_id: String) -> Result<SpendScan, String> {
    state().lock().map_err(|e| e.to_string())?.as_ref().filter(|r| r.scan.id == scan_id).map(|r| r.scan.clone()).ok_or_else(|| "扫描已释放".into())
}
#[tauri::command]
pub fn spend_cancel_scan(scan_id: String) {
    if let Ok(mut slot) = state().lock() { if let Some(run) = slot.as_mut().filter(|r| r.scan.id == scan_id) {
        run.cancel.store(true, Ordering::Release); run.scan.status = "cancelled".into(); run.scan.snapshot = None;
    } }
}
#[tauri::command]
pub fn spend_clear_snapshot() { if let Ok(mut slot) = state().lock() { if let Some(run) = slot.take() { run.cancel.store(true, Ordering::Release); } } }

static CARD_READS: OnceLock<Mutex<Vec<std::sync::Weak<AtomicBool>>>> = OnceLock::new();
pub fn cancel_card_reads() {
    if let Ok(mut reads)=CARD_READS.get_or_init(||Mutex::new(Vec::new())).lock() {
        for cancel in reads.iter().filter_map(std::sync::Weak::upgrade) {cancel.store(true,Ordering::Release);}
        reads.clear();
    }
}
fn card_agent(provider: &str) -> Option<&'static str> {
    match provider {"claude-code"=>Some("claude"),"codex"=>Some("codex"),"opencode-go"=>Some("opencode"),"cursor"=>Some("cursor"),"antigravity"=>Some("antigravity"),_=>None}
}
#[derive(Serialize)] #[serde(rename_all="camelCase")]
pub struct CardHistory {status:&'static str,snapshot:Option<SpendSnapshot>}
/// Same measured readers and public prices as Token Spend, scoped to one card.
/// It never changes/cancels the Settings pane's scan. Opt-out is checked before
/// path discovery/cache/network, and cancels an already-running card read.
#[tauri::command]
pub async fn spend_card_history(provider:String) -> Result<CardHistory,String> {
    let prefs=crate::preferences::load();
    if !prefs.token_spend_enabled || !prefs.enabled_providers.contains(&provider) {
        return Ok(CardHistory{status:"disabled",snapshot:None});
    }
    let Some(agent)=card_agent(&provider) else {return Ok(CardHistory{status:"unavailable",snapshot:None});};
    let cancel=Arc::new(AtomicBool::new(false));
    if let Ok(mut reads)=CARD_READS.get_or_init(||Mutex::new(Vec::new())).lock() {
        reads.retain(|read|read.strong_count()>0);reads.push(Arc::downgrade(&cancel));
    }
    // Close the race with a simultaneous Settings opt-out/provider removal.
    let current=crate::preferences::load();
    if !current.token_spend_enabled || !current.enabled_providers.contains(&provider) {return Ok(CardHistory{status:"disabled",snapshot:None});}
    let home=crate::credentials::home_dir().ok_or("无法找到用户目录")?;
    let cache=crate::credentials::cache_dir().unwrap_or_else(||home.join(".cache")).join("PulseWin/token-spend");
    let (prices,prices_at,pricing_status)=fetch_prices(&cache,&cancel).await;
    check(&cancel)?;
    let worker=cancel.clone();
    let mut snapshot=tokio::task::spawn_blocking(move||scan_roots(&cache,&prices,&worker,|_,_|{},|id|
        if id==agent {roots(&home,id,|key|std::env::var(key).ok())}else{Vec::new()})).await.map_err(|_|"本机记录读取中断")??;
    check(&cancel)?;
    snapshot.sources.retain(|source|source.id==agent);
    snapshot.prices_at=prices_at;snapshot.pricing_status=pricing_status;
    Ok(CardHistory{status:"ready",snapshot:Some(snapshot)})
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct Price { input: f64, output: f64, cache_read: Option<f64>, cache_write: Option<f64>, name: Option<String> }
type Prices = BTreeMap<String, Price>;
#[derive(Serialize, Deserialize)]
struct PriceCache { version: u32, fetched_at: i64, prices: Prices }
fn number(value: &Value) -> Option<f64> { value.as_f64().filter(|v| v.is_finite() && *v >= 0.) }
fn parse_prices(root: &Value) -> Prices {
    let mut prices = Prices::new();
    for vendor in ["anthropic", "openai", "xai", "moonshotai", "zhipuai", "minimax", "deepseek", "google", "xiaomi", "alibaba", "mistral", "meta"] {
        if let Some(models) = root[vendor]["models"].as_object() { for (id, model) in models {
            if prices.contains_key(id) { continue; }
            if let (Some(input), Some(output)) = (number(&model["cost"]["input"]), number(&model["cost"]["output"])) {
                prices.insert(id.clone(), Price { input, output, cache_read: number(&model["cost"]["cache_read"]),
                    cache_write: number(&model["cost"]["cache_write"]), name: model["name"].as_str().map(str::to_owned) });
            }
        } }
    }
    // Plan-only models have no first-party listing. Keep reseller rates
    // namespaced and reachable only from records of the corresponding plan.
    for vendor in ["opencode-go", "kilo", "cline-pass"] {
        if let Some(models) = root[vendor]["models"].as_object() { for (id, model) in models {
            if let (Some(input), Some(output)) = (number(&model["cost"]["input"]), number(&model["cost"]["output"])) {
                prices.insert(format!("{vendor}|{id}"), Price { input, output,
                    cache_read: number(&model["cost"]["cache_read"]), cache_write: number(&model["cost"]["cache_write"]),
                    name: model["name"].as_str().map(str::to_owned) });
            }
        } }
    }
    prices
}
fn price_for<'a>(model: &str, prices: &'a Prices) -> Option<&'a Price> {
    fn dotted(value: &str) -> String {
        let chars: Vec<_> = value.chars().collect(); chars.iter().enumerate().map(|(i, c)| {
            if *c == '-' && i > 0 && i + 1 < chars.len() && chars[i-1].is_ascii_digit() && chars[i+1].is_ascii_digit() { '.' } else { *c }
        }).collect()
    }
    let mut aliases = vec![model.to_owned(), dotted(model)];
    for suffix in ["-medium", "-high", "-low", "-minimal", "-build"] {
        if let Some(base) = model.strip_suffix(suffix) { if model != "grok-build-0.1" { aliases.push(base.into()); aliases.push(dotted(base)); } }
    }
    for alias in aliases { if let Some(found) = prices.get(&alias).or_else(|| prices.iter().find(|(key, _)| key.eq_ignore_ascii_case(&alias)).map(|(_, p)| p)) { return Some(found); } }
    None
}
fn price_for_agent<'a>(model: &str, agent: &str, prices: &'a Prices) -> Option<&'a Price> {
    price_for(model, prices).or_else(|| {
        let vendor=match agent { "opencode"=>"opencode-go", "kilo"=>"kilo", _=>return None };
        price_for(&format!("{vendor}|{model}"), prices)
    })
}
async fn fetch_prices(cache: &Path, cancel: &AtomicBool) -> (Prices, Option<String>, String) {
    if check(cancel).is_err() { return (Prices::new(), None, "cancelled".into()); }
    let saved = fs::read(cache.join("prices-1.json")).ok().and_then(|b| serde_json::from_slice::<PriceCache>(&b).ok()).filter(|c| c.version == CACHE_VERSION);
    let now = Utc::now().timestamp();
    if let Some(ref value) = saved { if now.saturating_sub(value.fetched_at) < 86400 {
        return (value.prices.clone(), DateTime::from_timestamp(value.fetched_at, 0).map(|d| d.to_rfc3339()), "cached".into());
    } }
    let request = async {
        let mut builder = reqwest::Client::builder().timeout(Duration::from_secs(30));
        if let Some(proxy) = crate::proxy::system_proxy() { if let Ok(proxy) = reqwest::Proxy::all(proxy) { builder = builder.proxy(proxy); } }
        let response = builder.build().ok()?.get("https://models.dev/api.json").send().await.ok()?.error_for_status().ok()?;
        // The public table has a bound too; no transcript, key or identity is sent.
        if response.content_length().is_some_and(|n| n > 32 * 1024 * 1024) { return None; }
        let bytes = response.bytes().await.ok()?; if bytes.len() > 32 * 1024 * 1024 { return None; }
        let root: Value = serde_json::from_slice(&bytes).ok()?; let prices = parse_prices(&root);
        if prices.is_empty() { None } else { Some(prices) }
    };
    tokio::pin!(request);
    loop { tokio::select! {
        result = &mut request => {
            if let Some(prices) = result { if check(cancel).is_ok() {
                let value = PriceCache { version: CACHE_VERSION, fetched_at: now, prices: prices.clone() };
                let _ = write_cache(cache, "prices-1.json", &value, cancel);
                return (prices, Some(Utc::now().to_rfc3339()), "fresh".into());
            } }
            return saved.map(|s| (s.prices, DateTime::from_timestamp(s.fetched_at, 0).map(|d| d.to_rfc3339()), "stale".into()))
                .unwrap_or_else(|| (Prices::new(), None, "unavailable".into()));
        }
        _ = tokio::time::sleep(Duration::from_millis(100)) => if check(cancel).is_err() { return (Prices::new(), None, "cancelled".into()); }
    } }
}

#[derive(Serialize, Deserialize)]
struct FileCache { version: u32, agent: String, fingerprint: u64, records: Vec<SpendRecord> }
fn hash(value: impl Hash) -> u64 { let mut h = std::collections::hash_map::DefaultHasher::new(); value.hash(&mut h); h.finish() }
fn stamp(path: &Path) -> std::io::Result<u64> {
    let meta = fs::metadata(path)?;
    let main=(meta.len(), meta.modified()?.duration_since(UNIX_EPOCH).unwrap_or_default().as_nanos());
    // A running SQLite writer can append records without changing the main
    // file. Fingerprint its WAL/journal too; never open with immutable=1.
    let sidecars=if opencode_store::is_database(path) { ["-wal", "-journal"].into_iter().map(|suffix| {
        let mut name=path.as_os_str().to_os_string(); name.push(suffix);
        match fs::metadata(PathBuf::from(name)) {
            Ok(m)=>Ok(Some((m.len(),m.modified()?.duration_since(UNIX_EPOCH).unwrap_or_default().as_nanos()))),
            Err(e) if e.kind()==std::io::ErrorKind::NotFound=>Ok(None),Err(e)=>Err(e)
        }
    }).collect::<std::io::Result<Vec<_>>>()? } else { Vec::new() };
    Ok(hash((path.to_string_lossy().to_string(), main, sidecars)))
}
fn write_cache(dir: &Path, name: &str, value: &impl Serialize, cancel: &AtomicBool) -> Result<(), String> {
    check(cancel)?; let bytes = serde_json::to_vec(value).map_err(|e| e.to_string())?; check(cancel)?;
    fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    // A scan-specific staging file prevents a cancelled predecessor from replacing
    // its successor's cache. No incomplete parse is admitted by the caller.
    let temporary = dir.join(format!("{name}.{}.tmp", NEXT_ID.fetch_add(1, Ordering::Relaxed)));
    fs::write(&temporary, bytes).map_err(|e| e.to_string())?;
    if check(cancel).is_err() { let _ = fs::remove_file(&temporary); return Err("cancelled".into()); }
    let target = dir.join(name);
    // rename on Windows cannot replace a destination; removing this cache is safe,
    // and a cancelled write can never create a completed cache with partial data.
    if target.exists() { fs::remove_file(&target).map_err(|e| e.to_string())?; }
    check(cancel)?; fs::rename(temporary, target).map_err(|e| e.to_string())
}
fn roots(home: &Path, agent: &str, environment: impl Fn(&str) -> Option<String>) -> Vec<PathBuf> {
    match agent {
        "claude" => vec![environment("CLAUDE_CONFIG_DIR").filter(|v| !v.trim().is_empty()).map(PathBuf::from).unwrap_or_else(|| home.join(".claude")).join("projects")],
        "codex" => { let root = environment("CODEX_HOME").filter(|v| !v.trim().is_empty()).map(PathBuf::from).unwrap_or_else(|| home.join(".codex")); vec![root.join("sessions"), root.join("archived_sessions")] }
        "qwen" => vec![home.join(".qwen/projects")],
        "gemini" => vec![environment("GEMINI_CLI_HOME").filter(|v| !v.trim().is_empty()).map(|v| {
            let v = v.trim(); if v == "~" { home.to_path_buf() } else if let Some(rest) = v.strip_prefix("~/").or_else(|| v.strip_prefix("~\\")) { home.join(rest) } else { PathBuf::from(v) }
        }).unwrap_or_else(|| home.join(".gemini")).join("tmp")],
        "cursor" | "antigravity" | "hindsight" | "mcode" => measured_readers::roots(home, agent, &environment),
        "opencode" | "kilo" | "micode" => opencode_store::roots(home,agent,&environment),
        _ => Vec::new(),
    }
}
fn enumerate(root: &Path, agent: &str, files: &mut Vec<PathBuf>, cancel: &AtomicBool, notes: &mut Vec<String>) -> Result<(), String> {
    check(cancel)?;
    if root.is_file() {
        if !fs::symlink_metadata(root).map(|m|m.file_type().is_symlink()).unwrap_or(true) && opencode_store::candidate(root,agent) { files.push(root.into()); }
        return Ok(());
    }
    let entries = match fs::read_dir(root) { Ok(v) => v, Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()), Err(e) => { notes.push(format!("无法读取目录 {}：{e}", root.display())); return Ok(()); } };
    for entry in entries {
        check(cancel)?;
        if files.len() >= MAX_FILES { notes.push(format!("文件数超过 {MAX_FILES}，计数可能不完整")); return Ok(()); }
        let entry = match entry { Ok(e) => e, Err(e) => { notes.push(format!("目录条目读取失败：{e}")); continue; } };
        let kind = entry.file_type().map_err(|e| e.to_string())?;
        if kind.is_symlink() { continue; }
        let path = entry.path();
        if kind.is_dir() { enumerate(&path, agent, files, cancel, notes)?; }
        else if kind.is_file() && (if opencode_store::agent(agent) {opencode_store::candidate(&path,agent)}else{measured_readers::candidate(&path, agent)}) { files.push(path); }
    }
    Ok(())
}
fn scan_sources(home: &Path, cache: &Path, prices: &Prices, _force: bool, cancel: &AtomicBool, progress: impl Fn(&str, usize)) -> Result<SpendSnapshot, String> {
    scan_roots(cache, prices, cancel, progress, |agent| roots(home, agent, |key| std::env::var(key).ok()))
}
// Explicit roots are the test seam: fixtures never discover the machine's stores.
fn scan_roots(cache: &Path, prices: &Prices, cancel: &AtomicBool, progress: impl Fn(&str, usize), inputs_for: impl Fn(&str) -> Vec<PathBuf>) -> Result<SpendSnapshot, String> {
    let mut identities = HashSet::new();
    let mut snapshot = SpendSnapshot { scanned_at: Utc::now().to_rfc3339(), records: Vec::new(), sources: Vec::new(), notes: Vec::new(), prices_at: None, pricing_status: "unavailable".into() };
    for (index, (agent, name)) in AGENTS.iter().enumerate() {
        check(cancel)?; progress(name, index + 1);
        let inputs = inputs_for(agent);
        let detected = inputs.iter().any(|p| p.exists());
        let mut files = Vec::new(); for root in &inputs { enumerate(root, agent, &mut files, cancel, &mut snapshot.notes)?; }
        files.sort(); files.dedup();
        let mut source = SpendSource { id: (*agent).into(), name: (*name).into(), status: if detected { "no-data" } else { "not-detected" }.into(), files: files.len(), cached_files: 0, records: 0,
            origin: if measured_readers::captured(agent) { "export" } else { "native" }.into(), roots: inputs.iter().map(|p| p.to_string_lossy().into_owned()).collect() };
        let mut per_file = Vec::new(); let mut raw_count = 0;
        for file in files {
            check(cancel)?;
            if snapshot.records.len() + raw_count >= MAX_RECORDS { snapshot.notes.push("记录超过读取上限，计数可能不完整".into()); break; }
            let before = match stamp(&file) { Ok(s) => s, Err(e) => { snapshot.notes.push(format!("读取文件元信息失败 {}：{e}", file.display())); continue; } };
            let cache_name = format!("transcript-{TRANSCRIPT_VERSION}-{}-{:016x}.json", agent, hash(file.to_string_lossy().to_string()));
            let cached = fs::read(cache.join(&cache_name)).ok().and_then(|b| serde_json::from_slice::<FileCache>(&b).ok()).filter(|c|
                c.version == TRANSCRIPT_VERSION && c.agent == *agent && c.fingerprint == before && c.records.iter().all(|record|
                    record.agent == *agent && record.hour < 24 && !record.model.trim().is_empty()
                    && chrono::NaiveDate::parse_from_str(&record.day, "%Y-%m-%d").is_ok()
                    && (record.tally.total() > 0 || record.unclassified_tokens > 0)
                    && (*agent != "qwen" || record.deduplication_id.is_some())
                    && (*agent != "cursor" || (record.source_timestamp.is_some() && record.source_scope.is_some()))));
            let records = if let Some(c) = cached { source.cached_files += 1; c.records } else {
                let (records, notes) = match parse_file(&file, agent, cancel) {
                    Ok(value) => value,
                    Err(error) if check(cancel).is_err() => return Err(error),
                    Err(error) => { snapshot.notes.push(error); continue; }
                };
                if notes.is_empty() && stamp(&file).ok() == Some(before) { let _ = write_cache(cache, &cache_name, &FileCache { version: TRANSCRIPT_VERSION, agent: (*agent).into(), fingerprint: before, records: records.clone() }, cancel); }
                else if stamp(&file).ok() != Some(before) { snapshot.notes.push(format!("扫描期间文件发生变化：{}；本次结果未缓存", file.display())); }
                snapshot.notes.extend(notes); records
            };
            check(cancel)?;
            raw_count += records.len(); per_file.push(records);
        }
        let mut records = if *agent == "cursor" { measured_readers::reconcile_cursor(per_file, &mut snapshot.notes, cancel)? } else { per_file.into_iter().flatten().collect() };
        let remaining = MAX_RECORDS.saturating_sub(snapshot.records.len());
        if records.len() > remaining { snapshot.notes.push("记录超过读取上限，计数可能不完整".into()); records.truncate(remaining); }
        records.retain(|record| record.deduplication_id.as_ref().map(|id| identities.insert(id.clone())).unwrap_or(true));
        for record in &mut records { check(cancel)?; record.cost = None; record.cost_breakdown = None; record.model_name = None;
                if let Some(price) = price_for_agent(&record.model, agent, prices) {
                    let tally = &record.tally;
                    let cost = [tally.input as f64 * price.input / 1e6, tally.output as f64 * price.output / 1e6,
                        tally.cache_write as f64 * price.cache_write.unwrap_or(price.input) / 1e6, tally.cache_read as f64 * price.cache_read.unwrap_or(price.input) / 1e6];
                    if record.tally.total() > 0 { record.cost = Some(cost.iter().sum()); record.cost_breakdown = Some(cost); }
                    record.model_name = price.name.clone();
                }
        }
        source.records = records.len(); snapshot.records.extend(records);
        if source.records > 0 { source.status = "counted".into(); }
        snapshot.sources.push(source);
    }
    check(cancel)?; snapshot.notes.sort(); snapshot.notes.dedup(); Ok(snapshot)
}

fn counter(value: &Value) -> u64 { value.as_u64().or_else(|| value.as_f64().filter(|x| x.is_finite() && *x >= 0.).map(|x| x as u64)).unwrap_or(0) }
fn parse_file(path: &Path, agent: &str, cancel: &AtomicBool) -> Result<(Vec<SpendRecord>, Vec<String>), String> {
    if opencode_store::agent(agent) { return opencode_store::read(path, agent, cancel); }
    if matches!(agent, "gemini" | "cursor" | "antigravity" | "hindsight" | "mcode") { return measured_readers::parse_file(path, agent, cancel); }
    let file = File::open(path).map_err(|e| format!("无法读取 {}：{e}", path.display()))?;
    let mut reader = BufReader::with_capacity(64 * 1024, file);
    let segments: Vec<_> = path.components().map(|part| part.as_os_str().to_string_lossy().to_string()).collect();
    let label = segments.iter().rposition(|v| v == "projects").and_then(|i| segments.get(i + 1)).cloned();
    let session = if agent == "qwen" { format!("{}-{}", label.as_deref().unwrap_or(""), path.file_stem().unwrap_or_default().to_string_lossy()) } else { format!("{:016x}", hash(path.to_string_lossy().to_string())) };
    let (mut records, notes) = parse_reader(&mut reader, agent, &session, cancel)?;
    if agent == "qwen" {
        // Qwen's encoded project segment is a label, never decoded into cwd.
        for record in &mut records { if record.project.is_none() { record.project = label.clone(); } }
    }
    Ok((records, notes))
}
fn parse_reader(reader: &mut impl BufRead, agent: &str, session: &str, cancel: &AtomicBool) -> Result<(Vec<SpendRecord>, Vec<String>), String> {
    let mut notes = Vec::new(); let mut seen = HashSet::new(); let mut model = None::<String>; let mut previous = None::<[u64; 4]>;
    let mut project = None; let mut grouped: HashMap<(String, u32, String, i64), Tally> = HashMap::new();
    let mut normalized = Vec::new(); let mut digest = std::collections::hash_map::DefaultHasher::new(); let mut emitted = 0usize;
    loop {
        check(cancel)?;
        // fill_buf gives a bounded IO chunk. Oversize message bodies are skipped,
        // rather than letting read_until allocate an arbitrary transcript line.
        let mut line = Vec::new(); let mut over_limit = false; let mut eof = false;
        loop {
            check(cancel)?; let buffer = reader.fill_buf().map_err(|e| e.to_string())?;
            if buffer.is_empty() { eof = true; break; }
            let length = buffer.iter().position(|b| *b == b'\n').map(|p| p + 1).unwrap_or(buffer.len());
            let ended = buffer[length - 1] == b'\n';
            if !over_limit { if line.len() + length <= MAX_LINE { line.extend_from_slice(&buffer[..length]); } else { over_limit = true; line.clear(); } }
            reader.consume(length); if ended { break; }
        }
        if over_limit { notes.push(format!("{agent}：一条记录超过 8 MiB，计数可能不完整")); }
        if line.is_empty() { if eof { break; } continue; }
        // Incremental byte digest for Qwen mirror identities; no body is retained.
        if agent == "qwen" { digest.write(&line); }
        let relevant = line.windows(7).any(|w| w == b"\"usage\"") || line.windows(15).any(|w| w == b"\"usageMetadata\"") || line.windows(13).any(|w| w == b"\"token_count\"") || line.windows(7).any(|w| w == b"\"model\"") || line.windows(5).any(|w| w == b"\"cwd\"");
        if !relevant { if eof { break; } continue; }
        let root: Value = match serde_json::from_slice(&line) { Ok(v) => v, Err(_) => { notes.push(format!("{agent}：无法解码一条用量/元信息记录，计数可能不完整")); if eof { break; } continue; } };
        let payload = &root["payload"];
        if project.is_none() { project = root["cwd"].as_str().or_else(|| payload["cwd"].as_str()).filter(|v| !v.trim().is_empty()).map(str::to_owned); }
        let timestamp = root["timestamp"].as_str().and_then(|s| DateTime::parse_from_rfc3339(s).ok());
        if agent == "qwen" {
            if root["type"] != "assistant" || !root["usageMetadata"].is_object() { if eof { break; } continue; }
            let (tally, unclassified) = qwen_usage(&root["usageMetadata"]);
            if tally.total() == 0 && unclassified == 0 { if eof { break; } continue; }
            if let (Some(timestamp), Some(model)) = (timestamp, root["model"].as_str()) {
                let local = timestamp.with_timezone(&Local);
                let sid = root["sessionId"].as_str().unwrap_or(session).to_owned();
                let id = root["id"].as_str().or_else(|| root["messageId"].as_str()).map(|id| format!("qwen:{sid}:{id}"));
                if id.as_ref().is_some_and(|id| !seen.insert(id.clone())) { if eof { break; } continue; }
                // Missing ids use the complete fragment digest and ordinal below.
                normalized.push(SpendRecord { agent: agent.into(), model: model.into(), day: local.format("%Y-%m-%d").to_string(), hour: chrono::Timelike::hour(&local),
                    session: sid, project: project.clone(), tally, unclassified_tokens: unclassified, deduplication_id: id, aggregate_timing: false, source_timestamp: None, source_scope: None, cost: None, cost_breakdown: None, model_name: None });
                emitted += 1;
            } else { notes.push("Qwen Code：实测计数缺少有效模型或时间，计数可能不完整".into()); }
            if emitted >= MAX_RECORDS { notes.push("Qwen Code：达到记录上限，计数可能不完整".into()); break; }
            if eof { break; } continue;
        }
        let tally = if agent == "claude" {
            let message = &root["message"]; let usage = &message["usage"];
            if root["type"] != "assistant" || !usage.is_object() { if eof { break; } continue; }
            let Some(named) = message["model"].as_str().filter(|m| *m != "<synthetic>") else { if eof { break; } continue; };
            model = Some(named.to_owned());
            if timestamp.is_none() { notes.push("Claude Code：缺少有效用量时间，计数可能不完整".into()); if eof { break; } continue; }
            if let Some(id) = message["id"].as_str() { if !seen.insert(id.to_owned()) { if eof { break; } continue; } }
            Tally { input: counter(&usage["input_tokens"]), output: counter(&usage["output_tokens"]), cache_write: counter(&usage["cache_creation_input_tokens"]), cache_read: counter(&usage["cache_read_input_tokens"]) }
        } else {
            if let Some(named) = payload["model"].as_str() { model = Some(named.to_owned()); }
            let totals = &payload["info"]["total_token_usage"];
            if payload["type"] != "token_count" || !totals.is_object() { if eof { break; } continue; }
            let current = [counter(&totals["input_tokens"]), counter(&totals["cached_input_tokens"]), counter(&totals["cache_write_input_tokens"]), counter(&totals["output_tokens"])];
            if previous.is_some_and(|p| current.iter().zip(p).any(|(now, old)| *now < old)) { notes.push("Codex：累计计数出现回退，计数可能不完整".into()); }
            let mut delta = [0; 4]; for i in 0..4 { delta[i] = current[i].saturating_sub(previous.map(|p| p[i]).unwrap_or(0)); }
            previous = Some(current);
            if model.is_none() { notes.push("Codex：存在缺少模型的实测计数；使用未知模型且不计价".into()); model = Some("unknown".into()); }
            if delta[1] > delta[0] { notes.push("Codex：缓存计数超过输入差额，分类可能不完整".into()); }
            Tally { input: delta[0].saturating_sub(delta[1]), output: delta[3], cache_write: delta[2], cache_read: delta[1] }
        };
        if tally.total() == 0 { if eof { break; } continue; }
        if let (Some(timestamp), Some(model)) = (timestamp, model.as_ref()) {
            let local = timestamp.with_timezone(&Local);
            let key = (local.format("%Y-%m-%d").to_string(), chrono::Timelike::hour(&local), model.clone(), timestamp.timestamp().div_euclid(900)*900);
            grouped.entry(key).or_default().add(&tally);
        } else { notes.push(format!("{agent}：缺少有效用量时间，计数可能不完整")); }
        if grouped.len() >= MAX_RECORDS { notes.push(format!("{agent}：达到记录上限，计数可能不完整")); break; }
        if eof { break; }
    }
    check(cancel)?; notes.sort(); notes.dedup();
    let mut records: Vec<_> = grouped.into_iter().map(|((day, hour, model, timestamp), tally)| SpendRecord { agent: agent.into(), model, day, hour, session: session.into(), project: project.clone(), tally, unclassified_tokens: 0, deduplication_id: None, aggregate_timing: false, source_timestamp: Some(timestamp), source_scope: None, cost: None, cost_breakdown: None, model_name: None }).collect();
    let fragment = digest.finish();
    for (index, record) in normalized.iter_mut().enumerate() { if record.deduplication_id.is_none() { record.deduplication_id = Some(format!("qwen:{}:{fragment:016x}:{index}", record.session)); } }
    records.extend(normalized);
    records.sort_by(|a, b| (&a.day, a.hour, &a.model).cmp(&(&b.day, b.hour, &b.model)));
    Ok((records, notes))
}

/// Qwen normalizes every backend into Gemini-shaped measured counters. A total
/// settles cache overlap; an inconsistent/bare total is counted but never priced.
fn qwen_usage(metadata: &Value) -> (Tally, u64) {
    let prompt = counter(&metadata["promptTokenCount"]); let cached = counter(&metadata["cachedContentTokenCount"]);
    let output = counter(&metadata["candidatesTokenCount"]).saturating_add(counter(&metadata["thoughtsTokenCount"]));
    let total = ["totalTokenCount", "total", "total_tokens"].iter().find_map(|key| metadata[*key].as_u64());
    if let Some(total) = total {
        let included = prompt.saturating_add(output); let disjoint = included.saturating_add(cached);
        if total == included && cached <= prompt { return (Tally { input: prompt - cached, output, cache_read: cached, cache_write: 0 }, 0); }
        if total == disjoint { return (Tally { input: prompt, output, cache_read: cached, cache_write: 0 }, 0); }
        return (Tally::default(), total);
    }
    (Tally { input: prompt.saturating_sub(cached), output, cache_read: cached, cache_write: 0 }, 0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::io::Cursor;
    #[tokio::test] async fn detailed_card_guard_precedes_history_discovery_and_network() {
        // A deliberately unknown/unselected provider cannot reach roots/prices,
        // irrespective of the machine's actual Token Spend preference.
        let result=spend_card_history("pulsewin-unselected-fixture".into()).await.unwrap();
        assert_eq!(result.status,"disabled");assert!(result.snapshot.is_none());
    }
    #[test] fn detailed_card_cancellation_is_independent_of_the_settings_run() {
        let cancel=Arc::new(AtomicBool::new(false));
        CARD_READS.get_or_init(||Mutex::new(Vec::new())).lock().unwrap().push(Arc::downgrade(&cancel));
        cancel_card_reads();assert!(cancel.load(Ordering::Acquire));
    }
    fn parse(lines: Vec<Value>, agent: &str) -> (Vec<SpendRecord>, Vec<String>) {
        let data = lines.iter().map(Value::to_string).collect::<Vec<_>>().join("\r\n");
        parse_reader(&mut Cursor::new(data), agent, "synthetic-session", &AtomicBool::new(false)).unwrap()
    }
    fn claude(id: &str, time: &str, model: &str, input: u64, output: u64) -> Value {
        json!({"type":"assistant","timestamp":time,"message":{"id":id,"model":model,"usage":{"input_tokens":input,"output_tokens":output,"cache_creation_input_tokens":2,"cache_read_input_tokens":3},"content":[{"text":"SENSITIVE CONTENT MUST NEVER BE KEPT"}]}})
    }
    fn codex(input: u64, cached: u64, output: u64) -> Value { json!({"timestamp":"2026-10-02T12:00:00Z","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":input,"cached_input_tokens":cached,"output_tokens":output}}}}) }
    #[test] fn claude_deduplicates_replays_and_excludes_synthetic() {
        let a = claude("same", "2026-10-02T12:00:00Z", "model-a", 100, 20);
        let (records, notes) = parse(vec![a.clone(), a, claude("error", "2026-10-02T12:00:00Z", "<synthetic>", 999, 999)], "claude");
        assert!(notes.is_empty()); assert_eq!(records.len(), 1); assert_eq!(records[0].tally.total(), 125);
        assert!(!serde_json::to_string(&records).unwrap().contains("SENSITIVE"));
    }
    #[test] fn codex_uses_cumulative_deltas_and_cached_input_is_not_double_counted() {
        let (records, notes) = parse(vec![json!({"payload":{"model":"gpt-test"}}), codex(100, 30, 20), codex(100, 30, 20), codex(180, 50, 30)], "codex");
        assert!(notes.is_empty()); assert_eq!(records[0].tally, Tally { input: 130, cache_read: 50, cache_write: 0, output: 30 });
        assert_eq!(records[0].tally.total(), 210);
    }
    #[test] fn model_changes_receive_their_own_delta() {
        let (records, _) = parse(vec![json!({"payload":{"model":"one"}}), codex(100, 0, 20), json!({"payload":{"model":"two"}}), codex(150, 0, 30)], "codex");
        assert_eq!(records.len(), 2); assert_eq!(records[0].tally.total(), 120); assert_eq!(records[1].tally.total(), 60);
    }
    #[test] fn missing_model_is_real_unpriced_usage_instead_of_zero() {
        let (records, notes) = parse(vec![codex(12, 2, 3)], "codex");
        assert_eq!(records[0].model, "unknown"); assert_eq!(records[0].tally.total(), 15); assert!(records[0].cost.is_none()); assert!(!notes.is_empty());
    }
    #[test] fn known_free_price_is_zero_and_unknown_remains_missing() {
        let prices = parse_prices(&json!({"openai":{"models":{"free":{"cost":{"input":0,"output":0}},"paid":{"cost":{"input":2,"output":8}}}},"github-copilot":{"models":{"other":{"cost":{"input":1,"output":1}}}}}));
        assert_eq!(price_for("free", &prices).unwrap().input, 0.); assert!(price_for("other", &prices).is_none()); assert!(price_for("almost-paid", &prices).is_none());
    }
    #[test] fn exact_aliases_and_first_party_order_match_original() {
        let prices = parse_prices(&json!({"openai":{"models":{"gpt-5.6-sol":{"cost":{"input":2,"output":8}}}},"anthropic":{"models":{"shared":{"cost":{"input":1,"output":2}}}},"xai":{"models":{"shared":{"cost":{"input":9,"output":9}}}}}));
        assert_eq!(price_for("gpt-5-6-sol-high", &prices).unwrap().output, 8.); assert_eq!(price_for("SHARED", &prices).unwrap().input, 1.);
    }
    #[test] fn cancellation_is_checked_inside_stream_and_never_publishes_partial() {
        assert!(parse_reader(&mut Cursor::new("{}\n"), "claude", "test", &AtomicBool::new(true)).is_err());
    }
    #[test] fn home_overrides_and_archive_sources_are_explicit() {
        let home = Path::new("fixture-home");
        assert_eq!(roots(home, "claude", |_| Some("alternate".into())), vec![PathBuf::from("alternate/projects")]);
        assert_eq!(roots(home, "codex", |_| None).len(), 2); assert!(roots(home, "unimplemented", |_| None).is_empty());
    }
    #[test] fn malformed_count_lines_report_shortfall() {
        let (records, notes) = parse_reader(&mut Cursor::new("{\"usage\":bad}\n"), "claude", "test", &AtomicBool::new(false)).unwrap();
        assert!(records.is_empty()); assert_eq!(notes.len(), 1);
    }
    struct Fixture { dir: PathBuf }
    impl Fixture {
        fn new() -> Self {
            let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../test-results").join(format!("spend-fixture-{}-{}", std::process::id(), NEXT_ID.fetch_add(1, Ordering::Relaxed)));
            fs::create_dir_all(dir.join("logs")).unwrap(); Self { dir }
        }
        fn scan(&self, prices: &Prices, cancel: &AtomicBool) -> Result<SpendSnapshot, String> {
            scan_roots(&self.dir.join("cache"), prices, cancel, |_, _| {}, |agent| if agent == "claude" { vec![self.dir.join("logs")] } else { vec![self.dir.join("absent")] })
        }
    }
    impl Drop for Fixture { fn drop(&mut self) {
        let base = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../test-results").canonicalize();
        if let (Ok(base), Ok(target)) = (base, self.dir.canonicalize()) { if target.starts_with(base) { let _ = fs::remove_dir_all(target); } }
    } }
    #[test] fn file_cache_reuses_unchanged_and_reprices_without_preserving_text() {
        let fixture = Fixture::new(); let file = fixture.dir.join("logs/session.jsonl");
        fs::write(&file, claude("one", "2026-10-02T12:00:00Z", "measured", 100, 20).to_string()).unwrap();
        let cancel = AtomicBool::new(false); let first = fixture.scan(&Prices::new(), &cancel).unwrap();
        assert_eq!(first.sources[0].cached_files, 0); assert!(first.records[0].cost.is_none());
        let prices = parse_prices(&json!({"anthropic":{"models":{"measured":{"cost":{"input":2,"output":8,"cache_read":0.2,"cache_write":2.5}}}}}));
        let second = fixture.scan(&prices, &cancel).unwrap(); assert_eq!(second.sources[0].cached_files, 1);
        assert!((second.records[0].cost.unwrap() - 0.0003656).abs() < 1e-10);
        for entry in fs::read_dir(fixture.dir.join("cache")).unwrap() { assert!(!fs::read_to_string(entry.unwrap().path()).unwrap().contains("SENSITIVE")); }
        use std::io::Write;
        writeln!(fs::OpenOptions::new().append(true).open(&file).unwrap(), "\n{}", claude("two", "2026-10-03T12:00:00Z", "measured", 50, 10)).unwrap();
        let changed = fixture.scan(&prices, &cancel).unwrap(); assert_eq!(changed.sources[0].cached_files, 0);
        assert_eq!(changed.records.iter().map(|r|r.tally.total()).sum::<u64>(), 190);
    }
    #[test] fn partial_reads_are_visible_and_never_admitted_to_cache() {
        let fixture = Fixture::new(); fs::write(fixture.dir.join("logs/partial.jsonl"), format!("{}\n{{\"usage\":broken}}\n", claude("one", "2026-10-02T12:00:00Z", "measured", 100, 20))).unwrap();
        let result = fixture.scan(&Prices::new(), &AtomicBool::new(false)).unwrap();
        assert_eq!(result.records[0].tally.total(), 125); assert!(!result.notes.is_empty());
        assert!(!fixture.dir.join("cache").exists());
    }
    #[test] fn cancelled_scan_writes_no_complete_cache() {
        let fixture = Fixture::new(); fs::write(fixture.dir.join("logs/test.jsonl"), claude("one", "2026-10-02T12:00:00Z", "test", 1, 2).to_string()).unwrap();
        assert!(fixture.scan(&Prices::new(), &AtomicBool::new(true)).is_err()); assert!(!fixture.dir.join("cache").exists());
    }
    #[test] fn oversize_line_skips_with_explicit_note_then_continues() {
        let mut data = vec![b'x'; MAX_LINE + 1]; data.push(b'\n'); data.extend(claude("one", "2026-10-02T12:00:00Z", "test", 1, 2).to_string().as_bytes());
        let (records, notes) = parse_reader(&mut BufReader::with_capacity(64, Cursor::new(data)), "claude", "fixture", &AtomicBool::new(false)).unwrap();
        assert_eq!(records[0].tally.total(), 8); assert_eq!(notes.len(), 1);
    }
    #[tokio::test] async fn disabled_command_rejects_before_any_scan_or_discovery() {
        assert_eq!(spend_begin_scan(false, false).await.unwrap_err(), "Token Spend reading is disabled");
    }
    #[test] fn a_client_boolean_cannot_override_persisted_opt_out() {
        assert!(ensure_opt_in(true, false).is_err()); assert!(ensure_opt_in(false, true).is_err()); assert!(ensure_opt_in(true, true).is_ok());
    }
    #[test] fn qwen_cache_overlap_and_reasoning_follow_reported_total() {
        let (inside, remainder) = qwen_usage(&json!({"promptTokenCount":100,"candidatesTokenCount":20,"thoughtsTokenCount":5,"cachedContentTokenCount":30,"totalTokenCount":125}));
        assert_eq!(inside, Tally { input:70, output:25, cache_read:30, cache_write:0 }); assert_eq!(remainder, 0);
        let (beside, remainder) = qwen_usage(&json!({"promptTokenCount":100,"candidatesTokenCount":20,"thoughtsTokenCount":5,"cachedContentTokenCount":30,"totalTokenCount":155}));
        assert_eq!(beside, Tally { input:100, output:25, cache_read:30, cache_write:0 }); assert_eq!(remainder, 0);
    }
    #[test] fn qwen_bare_or_inconsistent_total_is_unclassified_never_fabricated_input() {
        for usage in [json!({"totalTokenCount":999}), json!({"promptTokenCount":100,"candidatesTokenCount":20,"totalTokenCount":999})] {
            let (tally, remainder) = qwen_usage(&usage); assert_eq!(tally.total(), 0); assert_eq!(remainder, 999);
        }
        assert_eq!(qwen_usage(&json!({})), (Tally::default(), 0));
    }
    fn qwen(id: Option<&str>, session: &str, metadata: Value) -> Value {
        json!({"type":"assistant","id":id,"sessionId":session,"model":"qwen-test","timestamp":"2026-10-03T12:00:00Z","usageMetadata":metadata,"content":"SECRET QWEN MESSAGE BODY"})
    }
    #[test] fn qwen_cross_file_replays_and_cached_mirrors_are_counted_once() {
        let fixture = Fixture::new(); let row = qwen(Some("message-1"), "qwen-session", json!({"promptTokenCount":100,"candidatesTokenCount":20,"totalTokenCount":120}));
        fs::write(fixture.dir.join("logs/original.jsonl"), row.to_string()).unwrap();
        fs::write(fixture.dir.join("logs/mirror.jsonl"), row.to_string()).unwrap();
        let scan = || scan_roots(&fixture.dir.join("cache"), &Prices::new(), &AtomicBool::new(false), |_, _| {}, |agent| if agent == "qwen" { vec![fixture.dir.join("logs")] } else { vec![fixture.dir.join("absent")] }).unwrap();
        let first = scan(); assert_eq!(first.records.len(),1); assert_eq!(first.records[0].tally.total(),120);
        let second = scan(); assert_eq!(second.records.len(),1); assert_eq!(second.sources[2].cached_files,2);
        for entry in fs::read_dir(fixture.dir.join("cache")).unwrap() { assert!(!fs::read_to_string(entry.unwrap().path()).unwrap().contains("SECRET")); }
    }
    #[test] fn qwen_idless_byte_identical_fragments_fold_by_digest_and_position() {
        let fixture = Fixture::new(); let row = qwen(None, "qwen-session", json!({"totalTokenCount":321}));
        fs::write(fixture.dir.join("logs/original.jsonl"), row.to_string()).unwrap(); fs::write(fixture.dir.join("logs/mirror.jsonl"), row.to_string()).unwrap();
        let prices = parse_prices(&json!({"alibaba":{"models":{"qwen-test":{"cost":{"input":1,"output":2}}}}}));
        let snapshot = scan_roots(&fixture.dir.join("cache"), &prices, &AtomicBool::new(false), |_, _| {}, |agent| if agent == "qwen" { vec![fixture.dir.join("logs")] } else { vec![fixture.dir.join("absent")] }).unwrap();
        assert_eq!(snapshot.records.len(),1); assert_eq!(snapshot.records[0].unclassified_tokens,321); assert!(snapshot.records[0].cost.is_none());
    }
    #[test] fn cooperative_cancellation_discards_already_read_counts() {
        struct CancelAfterLine<'a> { inner: Cursor<Vec<u8>>, lines: usize, cancel: &'a AtomicBool }
        impl std::io::Read for CancelAfterLine<'_> { fn read(&mut self, output: &mut [u8]) -> std::io::Result<usize> { std::io::Read::read(&mut self.inner, output) } }
        impl BufRead for CancelAfterLine<'_> {
            fn fill_buf(&mut self) -> std::io::Result<&[u8]> { self.inner.fill_buf() }
            fn consume(&mut self, amount: usize) { self.inner.consume(amount); self.lines += 1; if self.lines == 2 { self.cancel.store(true, Ordering::Release); } }
        }
        let cancel = AtomicBool::new(false);
        let data = (0..4).map(|i| claude(&format!("id-{i}"), "2026-10-02T12:00:00Z", "test", 100, 20).to_string()).collect::<Vec<_>>().join("\n");
        let mut reader = CancelAfterLine { inner: Cursor::new(data.into_bytes()), lines:0, cancel:&cancel };
        assert_eq!(parse_reader(&mut reader,"claude","fixture",&cancel).unwrap_err(), "cancelled");
        assert!(reader.lines >= 2);
    }
    #[test] fn invalid_cache_metadata_is_rebuilt_from_the_real_fixture() {
        let fixture = Fixture::new(); fs::write(fixture.dir.join("logs/session.jsonl"), claude("one", "2026-10-02T12:00:00Z", "test", 100, 20).to_string()).unwrap();
        fixture.scan(&Prices::new(),&AtomicBool::new(false)).unwrap();
        let file = fs::read_dir(fixture.dir.join("cache")).unwrap().next().unwrap().unwrap().path();
        let mut value: Value = serde_json::from_slice(&fs::read(&file).unwrap()).unwrap(); value["records"][0]["hour"] = json!(99);
        fs::write(&file,value.to_string()).unwrap();
        let rebuilt = fixture.scan(&Prices::new(),&AtomicBool::new(false)).unwrap(); assert_eq!(rebuilt.sources[0].cached_files,0); assert!(rebuilt.records[0].hour < 24); assert_eq!(rebuilt.records[0].tally.total(),125);
    }
}
