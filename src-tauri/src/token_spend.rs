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

const CACHE_VERSION: u32 = 3;
const TRANSCRIPT_VERSION: u32 = 9;
const MAX_LINE: usize = 8 * 1024 * 1024;
const MAX_FILES: usize = 50_000;
const MAX_RECORDS: usize = 1_000_000;
const AGENTS: [(&str, &str); 41] = [("claude", "Claude Code"), ("codex", "Codex"), ("qwen", "Qwen Code"),
    ("gemini", "Gemini CLI"), ("cursor", "Cursor"), ("antigravity", "Antigravity"), ("hindsight", "Hindsight"), ("mcode", "MCode"),
    ("opencode", "OpenCode"), ("kilo", "Kilo CLI"), ("micode", "MiMo Code"), ("dsh", "DeepSeek Harness"),("pi","Pi"),("omp","Oh My Pi"),("kimchi","Kimchi"),("amp","Amp"),("droid","Droid"),("openclaw","OpenClaw"),("goose","Goose"),("hermes","Hermes"),("zed","Zed"),("crush","Crush"),("kiro","Kiro"),("kimi","Kimi CLI"),
    // Ported from Pulse 1.9's Groups B–D in this batch. Every id here has a
    // reader below; a source without one stays hidden in the backlog instead.
    ("roocode","Roo Code"),("kilocode","Kilo Code"),("cline","Cline"),("cherrystudio","Cherry Studio"),("commandcode","Command Code"),("opencodereview","OpenCodeReview"),("zcode","ZCode"),("unsloth","Unsloth"),("mux","Mux"),("codebuff","Codebuff"),("jcode","JCode"),("augment","Augment"),("gjc","Gajae Code"),("junie","Junie"),("fx","Fx"),("lmstudio","LM Studio"),("reasonix","Reasonix")];
#[path = "token_spend/dsh_reader.rs"]
mod dsh_reader;
#[path = "token_spend/measured_readers.rs"]
mod measured_readers;
#[path = "token_spend/opencode_store.rs"]
mod opencode_store;
#[path = "token_spend/history.rs"]
mod history;
mod reply_timing;
mod session_readers;
mod droid_reader;
mod warmer;
mod openclaw_reader;
mod aggregate_databases;
mod zed_reader;
mod crush_reader;
mod kiro_reader;
mod kimi_reader;
// Pulse 1.9 Groups B–D. Each file owns one store format (or one family of
// sibling stores) and stays out of the main scan loop until registered below.
mod editor_task_reader;
mod cherrystudio_reader;
mod commandcode_reader;
mod opencodereview_reader;
mod zcode_reader;
mod unsloth_reader;
mod mux_reader;
mod codebuff_reader;
mod jcode_reader;
mod augment_reader;
mod gjc_reader;
mod junie_reader;
mod fx_reader;
mod lmstudio_reader;
mod reasonix_reader;
pub fn watch(app:tauri::AppHandle){warmer::watch(app)}
pub fn reading_changed(){warmer::changed()}
pub fn forget_kept(){warmer::forget()}
#[tauri::command]pub fn spend_kept_snapshot()->Option<SpendSnapshot>{if crate::preferences::load().token_spend_enabled{warmer::snapshot()}else{None}}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Tally {
    pub input:u64, pub output:u64, pub cache_write:u64, pub cache_read:u64,
    /// A subset of cache_write, never added to total tokens again.
    #[serde(default,skip_serializing_if="is_zero")] pub cache_write_1h:u64,
    #[serde(default,skip_serializing_if="is_zero")] pub replies_without_cache_fields:u64,
    /// Per-request context bands, retained when quarters/days are aggregated.
    #[serde(default,skip_serializing_if="BTreeMap::is_empty")] pub context_bands:BTreeMap<u64,Tally>,
}
fn is_zero(n:&u64)->bool{*n==0}
impl Tally {
    pub(crate) fn total(&self) -> u64 { self.input.saturating_add(self.output).saturating_add(self.cache_write).saturating_add(self.cache_read) }
    fn add(&mut self, other: &Self) {
        self.input = self.input.saturating_add(other.input); self.output = self.output.saturating_add(other.output);
        self.cache_write = self.cache_write.saturating_add(other.cache_write); self.cache_read = self.cache_read.saturating_add(other.cache_read);
        self.cache_write_1h=self.cache_write_1h.saturating_add(other.cache_write_1h);
        self.replies_without_cache_fields=self.replies_without_cache_fields.saturating_add(other.replies_without_cache_fields);
        for (band,tally) in &other.context_bands{self.context_bands.entry(*band).or_default().add(tally);}
    }
    fn request(mut self,context:u64)->Self{
        if let Some(band)=[128_000,200_000,256_000,272_000].into_iter().rev().find(|b|context>*b){let mut plain=self.clone();plain.context_bands.clear();self.context_bands.insert(band,plain);}self
    }
    fn removing(&self,other:&Self)->Self{Self{input:self.input.saturating_sub(other.input),output:self.output.saturating_sub(other.output),cache_write:self.cache_write.saturating_sub(other.cache_write),cache_read:self.cache_read.saturating_sub(other.cache_read),cache_write_1h:self.cache_write_1h.saturating_sub(other.cache_write_1h),..Default::default()}}
    fn flat_cost(&self,p:&Price)->[f64;4]{let hour=self.cache_write_1h.min(self.cache_write);[self.input as f64*p.input/1e6,self.output as f64*p.output/1e6,((self.cache_write-hour) as f64*p.cache_write.unwrap_or(p.input)+hour as f64*p.input*2.)/1e6,self.cache_read as f64*p.cache_read.unwrap_or(p.input)/1e6]}
    fn without_cache(&self)->Self{let mut tally=self.clone();tally.input=tally.input.saturating_add(tally.cache_write).saturating_add(tally.cache_read);tally.cache_write=0;tally.cache_read=0;tally.cache_write_1h=0;tally.context_bands=self.context_bands.iter().map(|(k,v)|(*k,v.without_cache())).collect();tally}
    fn valid(&self)->bool{
        if self.cache_write_1h>self.cache_write||self.context_bands.len()>4{return false}
        let mut sums=[0u128;5];for (band,t)in &self.context_bands{if ![128000,200000,256000,272000].contains(band)||!t.context_bands.is_empty()||t.cache_write_1h>t.cache_write{return false}for(i,n)in[t.input,t.output,t.cache_write,t.cache_read,t.cache_write_1h].iter().enumerate(){sums[i]+=*n as u128;}}
        sums.iter().zip([self.input,self.output,self.cache_write,self.cache_read,self.cache_write_1h]).all(|(n,total)|*n<=total as u128)
    }
    fn cost_breakdown(&self,p:&Price)->[f64;4]{
        let mut rest=self.clone();rest.context_bands.clear();let mut cost=[0.;4];
        for (band,tally) in &self.context_bands{if let Some(tier)=p.tiers.iter().filter(|t|t.threshold<=*band).max_by_key(|t|t.threshold){rest=rest.removing(tally);let part=tally.flat_cost(&tier.price);for i in 0..4{cost[i]+=part[i];}}}
        let base=rest.flat_cost(p);for i in 0..4{cost[i]+=base[i]}cost
    }
}

#[derive(Clone, Debug, Deserialize)]
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
    #[serde(default)]pub cache_savings:Option<f64>,
    #[serde(default)]pub reply_timing:Option<reply_timing::ReplyTiming>,
}
impl Serialize for SpendRecord {
    fn serialize<S:serde::Serializer>(&self,serializer:S)->Result<S::Ok,S::Error>{
        use serde::ser::SerializeStruct;
        let counts=[self.tally.input,self.tally.output,self.tally.cache_write,self.tally.cache_read,self.unclassified_tokens];
        let exact=counts.iter().any(|v|*v>9_007_199_254_740_991)||counts.iter().fold(0u128,|a,b|a+*b as u128)>9_007_199_254_740_991;
        let mut s=serializer.serialize_struct("SpendRecord",if exact{19}else{17})?;
        s.serialize_field("agent",&self.agent)?;s.serialize_field("model",&self.model)?;s.serialize_field("day",&self.day)?;s.serialize_field("hour",&self.hour)?;
        s.serialize_field("session",&self.session)?;s.serialize_field("project",&self.project)?;s.serialize_field("tally",&self.tally)?;
        s.serialize_field("unclassifiedTokens",&self.unclassified_tokens)?;s.serialize_field("deduplicationId",&self.deduplication_id)?;
        s.serialize_field("aggregateTiming",&self.aggregate_timing)?;s.serialize_field("sourceTimestamp",&self.source_timestamp)?;s.serialize_field("sourceScope",&self.source_scope)?;
        s.serialize_field("cost",&self.cost)?;s.serialize_field("costBreakdown",&self.cost_breakdown)?;s.serialize_field("modelName",&self.model_name)?;
        s.serialize_field("cacheSavings",&self.cache_savings)?;
        s.serialize_field("replyTiming",&self.reply_timing)?;
        if exact{s.serialize_field("exactTally",&serde_json::json!({"input":self.tally.input.to_string(),"output":self.tally.output.to_string(),"cacheWrite":self.tally.cache_write.to_string(),"cacheRead":self.tally.cache_read.to_string()}))?;s.serialize_field("exactUnclassifiedTokens",&self.unclassified_tokens.to_string())?;}s.end()
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SpendSource { pub id: String, pub name: String, pub status: String, pub files: usize, pub cached_files: usize, pub records: usize,
    #[serde(default)] pub retained_records:usize,
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
static SCAN_LOCK: Mutex<()> = Mutex::new(());
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
    warmer::cancel();
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
                if crate::preferences::load().token_spend_enabled{warmer::keep(&snapshot);}
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
struct Price { input:f64, output:f64, cache_read:Option<f64>, cache_write:Option<f64>, name:Option<String>, #[serde(default)] tiers:Vec<ContextTier> }
#[derive(Clone,Debug,Serialize,Deserialize)]
struct ContextTier { threshold:u64,price:Price }
type Prices = BTreeMap<String, Price>;
#[derive(Serialize, Deserialize)]
struct PriceCache { version: u32, fetched_at: i64, prices: Prices }
fn number(value: &Value) -> Option<f64> { value.as_f64().filter(|v| v.is_finite() && *v >= 0.) }
fn parse_tiers(cost:&Value)->Vec<ContextTier>{
    let mut tiers=cost["tiers"].as_array().map(|a|a.iter().filter_map(|t|{
        if t["tier"]["type"]!="context"{return None}let size=t["tier"]["size"].as_u64()?;if size<=1{return None}
        Some(ContextTier{threshold:if (size-1)%1000==0 {size-1}else{size},price:Price{input:number(&t["input"])?,output:number(&t["output"])?,cache_read:number(&t["cache_read"]),cache_write:number(&t["cache_write"]),name:None,tiers:vec![]}})
    }).collect::<Vec<_>>()).unwrap_or_default();
    if tiers.is_empty(){let t=&cost["context_over_200k"];if let (Some(input),Some(output))=(number(&t["input"]),number(&t["output"])){tiers.push(ContextTier{threshold:200_000,price:Price{input,output,cache_read:number(&t["cache_read"]),cache_write:number(&t["cache_write"]),name:None,tiers:vec![]}})}}
    tiers.sort_by_key(|t|t.threshold);tiers
}
fn parse_prices(root: &Value) -> Prices {
    let mut prices = Prices::new();
    for vendor in ["anthropic", "openai", "xai", "moonshotai", "zhipuai", "minimax", "deepseek", "google", "xiaomi", "alibaba", "mistral", "meta"] {
        if let Some(models) = root[vendor]["models"].as_object() { for (id, model) in models {
            if prices.contains_key(id) { continue; }
            if let (Some(input), Some(output)) = (number(&model["cost"]["input"]), number(&model["cost"]["output"])) {
                prices.insert(id.clone(), Price { input, output, cache_read: number(&model["cost"]["cache_read"]),
                    cache_write: number(&model["cost"]["cache_write"]), name: model["name"].as_str().map(str::to_owned), tiers:parse_tiers(&model["cost"]) });
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
                    name: model["name"].as_str().map(str::to_owned), tiers:parse_tiers(&model["cost"]) });
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
    if agent=="kimi" {return None;}
    price_for(model, prices).or_else(|| {
        let vendor=match agent { "opencode"=>"opencode-go", "kilo"=>"kilo", _=>return None };
        price_for(&format!("{vendor}|{model}"), prices)
    })
}
async fn fetch_prices(cache: &Path, cancel: &AtomicBool) -> (Prices, Option<String>, String) {
    if check(cancel).is_err() { return (Prices::new(), None, "cancelled".into()); }
    let saved = fs::read(cache.join("prices-1.json")).ok().and_then(|b| serde_json::from_slice::<PriceCache>(&b).ok()).filter(|c| (2..=CACHE_VERSION).contains(&c.version));
    let now = Utc::now().timestamp();
    if let Some(ref value) = saved { if value.version==CACHE_VERSION && value.fetched_at<=now && now.saturating_sub(value.fetched_at) < 86400 {
        return (value.prices.clone(), DateTime::from_timestamp(value.fetched_at, 0).map(|d| d.to_rfc3339()), "cached".into());
    } }
    let request = async {
        let network=crate::preferences::load().network_proxy;
        let builder=crate::proxy::configure(reqwest::Client::builder().timeout(Duration::from_secs(30)),&network).ok()?;
        let mut response = builder.build().ok()?.get("https://models.dev/api.json").send().await.ok()?.error_for_status().ok()?;
        // The public table has a bound too; no transcript, key or identity is sent.
        if response.content_length().is_some_and(|n| n > 32 * 1024 * 1024) { return None; }
        let mut bytes=Vec::new();while let Some(chunk)=response.chunk().await.ok()?{if bytes.len()+chunk.len()>32*1024*1024{return None}bytes.extend_from_slice(&chunk);}
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
fn stamp(path: &Path, agent: &str) -> std::io::Result<u64> {
    let meta = fs::metadata(path)?;
    let main=(meta.len(), meta.modified()?.duration_since(UNIX_EPOCH).unwrap_or_default().as_nanos());
    // A running SQLite writer can append records without changing the main
    // file. Fingerprint its WAL/journal too; never open with immutable=1.
    let sidecars=if opencode_store::is_database(path)||openclaw_reader::database(path)||aggregate_databases::candidate(path,"goose")||aggregate_databases::candidate(path,"hermes")||zed_reader::candidate(path)||zcode_reader::candidate(path,"zcode")||unsloth_reader::candidate(path,"unsloth") { ["-wal", "-journal"].into_iter().map(|suffix| {
        let mut name=path.as_os_str().to_os_string(); name.push(suffix);
        match fs::metadata(PathBuf::from(name)) {
            Ok(m)=>Ok(Some((m.len(),m.modified()?.duration_since(UNIX_EPOCH).unwrap_or_default().as_nanos()))),
            Err(e) if e.kind()==std::io::ErrorKind::NotFound=>Ok(None),Err(e)=>Err(e)
        }
    }).collect::<std::io::Result<Vec<_>>>()? } else { Vec::new() };
    let kiro_sidecar=if agent=="kiro" {match fs::metadata(path.with_extension("jsonl")){Ok(m)=>Some((m.len(),m.modified()?.duration_since(UNIX_EPOCH).unwrap_or_default().as_nanos())),Err(e)if e.kind()==std::io::ErrorKind::NotFound=>None,Err(e)=>return Err(e)}}else{None};
    let dependency=path.file_name().and_then(|n|n.to_str()).and_then(|n|n.strip_suffix(".settings.json")).map(|stem|{
        match fs::metadata(path.with_file_name(format!("{stem}.jsonl"))){Ok(m)=>Ok(Some((m.len(),m.modified()?.duration_since(UNIX_EPOCH).unwrap_or_default().as_nanos()))),Err(e)if e.kind()==std::io::ErrorKind::NotFound=>Ok(None),Err(e)=>Err(e)}
    }).transpose()?;
    let registry = if openclaw_reader::candidate(path) {
        match fs::metadata(path.with_file_name("sessions.json")) {
            Ok(m) => Some((m.len(),m.modified()?.duration_since(UNIX_EPOCH).unwrap_or_default().as_nanos())),
            Err(e) if e.kind()==std::io::ErrorKind::NotFound => None,
            Err(e) => return Err(e)
        }
    } else { None };
    // Sibling inputs the ported readers consult: JCode's append journal, Fx's
    // session sidecar, editor model fallbacks and Command Code's legacy config.
    // The candidate file alone cannot fingerprint them, so cache freshness would
    // otherwise survive a sibling-only change.
    let probe=|path:&Path|->std::io::Result<Option<(u64,u128)>>{match fs::metadata(path){Ok(m)=>Ok(Some((m.len(),m.modified()?.duration_since(UNIX_EPOCH).unwrap_or_default().as_nanos()))),Err(e)if e.kind()==std::io::ErrorKind::NotFound=>Ok(None),Err(e)=>Err(e)}};
    let mut siblings=Vec::new();
    let name=path.file_name().and_then(|name|name.to_str()).unwrap_or_default();
    if agent=="jcode" { if let Some(stem)=path.file_stem().and_then(|stem|stem.to_str()){ siblings.push(probe(&path.with_file_name(format!("{stem}.journal.jsonl")))?); } }
    if agent=="fx" { siblings.push(probe(&path.with_file_name("session.json"))?); }
    if matches!(agent,"roocode"|"kilocode"|"cline") && name.eq_ignore_ascii_case("ui_messages.json") { siblings.push(probe(&path.with_file_name("api_conversation_history.json"))?); }
    if agent=="cline" && name.ends_with(".messages.json") { let stem=name.trim_end_matches(".messages.json"); siblings.push(probe(&path.with_file_name(format!("{stem}.json")))?); }
    if agent=="commandcode" { if let Some(dir)=path.ancestors().find(|dir|dir.file_name().and_then(|name|name.to_str())==Some(".commandcode")){ siblings.push(probe(&dir.join("config.json"))?); } }
    Ok(hash((path.to_string_lossy().to_string(), main, sidecars,dependency,registry,kiro_sidecar,siblings)))
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
        "kiro" => vec![home.join(".kiro/sessions/cli")],
        "kimi" => vec![home.join(".kimi/sessions")],
        "gemini" => vec![environment("GEMINI_CLI_HOME").filter(|v| !v.trim().is_empty()).map(|v| {
            let v = v.trim(); if v == "~" { home.to_path_buf() } else if let Some(rest) = v.strip_prefix("~/").or_else(|| v.strip_prefix("~\\")) { home.join(rest) } else { PathBuf::from(v) }
        }).unwrap_or_else(|| home.join(".gemini")).join("tmp")],
        "cursor" | "antigravity" | "hindsight" | "mcode" => measured_readers::roots(home, agent, &environment),
        "opencode" | "kilo" | "micode" => opencode_store::roots(home,agent,&environment),
        "dsh" => dsh_reader::roots(home, &environment),
        "zed"=>zed_reader::roots(home,&environment),
        "crush"=>crush_reader::roots(home,&environment),
        "openclaw"=>[".openclaw/agents",".clawdbot",".moltbot",".moldbot"].into_iter().map(|p|home.join(p)).collect(),
        id if aggregate_databases::agent(id)=>aggregate_databases::roots(home,id,&environment),
        id if session_readers::agent(id)=>session_readers::roots(home,id,&environment),
        id if ported_source(id)=>ported_roots(home,id,&environment),
        _ => Vec::new(),
    }
}
/// Sources whose readers were ported from Pulse 1.9 Groups B–D. Ids outside
/// this set have no reader, stay out of AGENTS, and are never scanned.
fn ported_source(agent: &str) -> bool {
    matches!(agent, "roocode"|"kilocode"|"cline"|"cherrystudio"|"commandcode"|"opencodereview"|"zcode"|"unsloth"|"mux"|"codebuff"|"jcode"|"augment"|"gjc"|"junie"|"fx"|"lmstudio"|"reasonix")
}
/// File-based stores keep per-file history so a deleted task log keeps its last
/// complete read. An accumulated SQLite ledger instead goes through StoreHistory.
fn ported_history(agent: &str) -> bool {
    matches!(agent, "roocode"|"kilocode"|"cline"|"cherrystudio"|"commandcode"|"opencodereview"|"zcode"|"mux"|"codebuff"|"jcode"|"augment"|"gjc"|"junie"|"fx"|"lmstudio"|"reasonix")
}
fn ported_roots(home: &Path, agent: &str, env: &impl Fn(&str) -> Option<String>) -> Vec<PathBuf> {
    match agent {
        "roocode"|"kilocode"|"cline" => editor_task_reader::roots(home,agent,env),
        "cherrystudio" => cherrystudio_reader::roots(home,agent,env),
        "commandcode" => commandcode_reader::roots(home,agent,env),
        "opencodereview" => opencodereview_reader::roots(home,agent,env),
        "zcode" => zcode_reader::roots(home,agent,env),
        "unsloth" => unsloth_reader::roots(home,agent,env),
        "mux" => mux_reader::roots(home,agent,env),
        "codebuff" => codebuff_reader::roots(home,agent,env),
        "jcode" => jcode_reader::roots(home,agent,env),
        "augment" => augment_reader::roots(home,agent,env),
        "gjc" => gjc_reader::roots(home,agent,env),
        "junie" => junie_reader::roots(home,agent,env),
        "fx" => fx_reader::roots(home,agent,env),
        "lmstudio" => lmstudio_reader::roots(home,agent,env),
        "reasonix" => reasonix_reader::roots(home,agent,env),
        _ => Vec::new(),
    }
}
fn ported_candidate(path: &Path, agent: &str) -> bool {
    match agent {
        "roocode"|"kilocode"|"cline" => editor_task_reader::candidate(path,agent),
        "cherrystudio" => cherrystudio_reader::candidate(path,agent),
        "commandcode" => commandcode_reader::candidate(path,agent),
        "opencodereview" => opencodereview_reader::candidate(path,agent),
        "zcode" => zcode_reader::candidate(path,agent),
        "unsloth" => unsloth_reader::candidate(path,agent),
        "mux" => mux_reader::candidate(path,agent),
        "codebuff" => codebuff_reader::candidate(path,agent),
        "jcode" => jcode_reader::candidate(path,agent),
        "augment" => augment_reader::candidate(path,agent),
        "gjc" => gjc_reader::candidate(path,agent),
        "junie" => junie_reader::candidate(path,agent),
        "fx" => fx_reader::candidate(path,agent),
        "lmstudio" => lmstudio_reader::candidate(path,agent),
        "reasonix" => reasonix_reader::candidate(path,agent),
        _ => false,
    }
}
fn ported_read(path: &Path, agent: &str, cancel: &AtomicBool) -> Result<(Vec<SpendRecord>, Vec<String>), String> {
    match agent {
        "roocode"|"kilocode"|"cline" => editor_task_reader::read(path,agent,cancel),
        "cherrystudio" => cherrystudio_reader::read(path,agent,cancel),
        "commandcode" => commandcode_reader::read(path,agent,cancel),
        "opencodereview" => opencodereview_reader::read(path,agent,cancel),
        "zcode" => zcode_reader::read(path,agent,cancel),
        "unsloth" => unsloth_reader::read(path,agent,cancel),
        "mux" => mux_reader::read(path,agent,cancel),
        "codebuff" => codebuff_reader::read(path,agent,cancel),
        "jcode" => jcode_reader::read(path,agent,cancel),
        "augment" => augment_reader::read(path,agent,cancel),
        "gjc" => gjc_reader::read(path,agent,cancel),
        "junie" => junie_reader::read(path,agent,cancel),
        "fx" => fx_reader::read(path,agent,cancel),
        "lmstudio" => lmstudio_reader::read(path,agent,cancel),
        "reasonix" => reasonix_reader::read(path,agent,cancel),
        _ => Ok((Vec::new(), Vec::new())),
    }
}
fn candidate(path: &Path, agent: &str) -> bool {
    if agent=="kimi" { kimi_reader::candidate(path) }
    else if agent=="kiro" { kiro_reader::candidate(path) }
    else if agent=="crush" { false }
    else if agent=="zed" { zed_reader::candidate(path) }
    else if aggregate_databases::agent(agent) { aggregate_databases::candidate(path,agent) }
    else if agent == "openclaw" { openclaw_reader::candidate(path) }
    else if agent == "dsh" { dsh_reader::candidate(path) }
    else if opencode_store::agent(agent) { opencode_store::candidate(path, agent) }
    else if session_readers::agent(agent) { session_readers::candidate(path, agent) }
    else if ported_source(agent) { ported_candidate(path, agent) }
    else { measured_readers::candidate(path, agent) }
}
fn enumerate(root: &Path, agent: &str, files: &mut Vec<PathBuf>, cancel: &AtomicBool, notes: &mut Vec<String>) -> Result<(), String> {
    let mut pending = vec![(root.to_path_buf(), 0usize)];
    let mut visited = 0usize;
    while let Some((path, depth)) = pending.pop() {
        check(cancel)?;
        visited += 1;
        if visited > 100_000 || files.len() >= MAX_FILES {
            notes.push("目录或文件数超过读取上限，计数可能不完整".into()); break;
        }
        let meta = match fs::symlink_metadata(&path) {
            Ok(meta) => meta,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
            Err(e) => { notes.push(format!("无法读取路径 {}：{e}", path.display())); continue; }
        };
        if meta.file_type().is_symlink() { continue; }
        if meta.is_file() { if candidate(&path, agent) { files.push(path); } continue; }
        if !meta.is_dir() { continue; }
        if depth >= 32 { notes.push(format!("目录深度超过读取上限：{}", path.display())); continue; }
        match fs::read_dir(&path) {
            Ok(entries) => for entry in entries {
                check(cancel)?;
                if pending.len() + visited >= 100_000 { notes.push("目录条目超过读取上限，计数可能不完整".into()); break; }
                match entry { Ok(entry) => pending.push((entry.path(), depth+1)), Err(e) => notes.push(format!("目录条目读取失败：{e}")) }
            },
            Err(e) => notes.push(format!("无法读取目录 {}：{e}", path.display()))
        }
    }
    Ok(())
}
fn scan_sources(home: &Path, cache: &Path, prices: &Prices, _force: bool, cancel: &AtomicBool, progress: impl Fn(&str, usize)) -> Result<SpendSnapshot, String> {
    scan_roots(cache, prices, cancel, progress, |agent| roots(home, agent, |key| std::env::var(key).ok()))
}
// Explicit roots are the test seam: fixtures never discover the machine's stores.
fn scan_roots(cache: &Path, prices: &Prices, cancel: &AtomicBool, progress: impl Fn(&str, usize), inputs_for: impl Fn(&str) -> Vec<PathBuf>) -> Result<SpendSnapshot, String> {
    // Card/background/manual readers share one durable journal. A cancelled
    // predecessor cannot overwrite a successor's stable counters.
    let _guard=SCAN_LOCK.lock().map_err(|e|e.to_string())?;
    check(cancel)?;
    let mut journals=Vec::new();
    let mut store_journals=Vec::new();
    let mut identities = HashSet::new();
    let mut snapshot = SpendSnapshot { scanned_at: Utc::now().to_rfc3339(), records: Vec::new(), sources: Vec::new(), notes: Vec::new(), prices_at: None, pricing_status: "unavailable".into() };
    for (index, (agent, name)) in AGENTS.iter().enumerate() {
        check(cancel)?; progress(name, index + 1);
        let inputs = inputs_for(agent);
        let note_start=snapshot.notes.len();
        let detected = inputs.iter().any(|p| p.exists());
        let mut files = Vec::new(); for root in &inputs { enumerate(root, agent, &mut files, cancel, &mut snapshot.notes)?; }
        files.sort(); files.dedup();
        let mut source = SpendSource { id: (*agent).into(), name: (*name).into(), status: if detected { "no-data" } else { "not-detected" }.into(), files: files.len(), cached_files: 0, records: 0, retained_records:0,
            origin: if measured_readers::captured(agent) { "export" } else { "native" }.into(), roots: inputs.iter().map(|p| p.to_string_lossy().into_owned()).collect() };
        let mut per_file = Vec::new(); let mut raw_count = 0;
        for file in files {
            check(cancel)?;
            if snapshot.records.len() + raw_count >= MAX_RECORDS { snapshot.notes.push("记录超过读取上限，计数可能不完整".into()); break; }
            let before = match stamp(&file, agent) { Ok(s) => s, Err(e) => { snapshot.notes.push(format!("读取文件元信息失败 {}：{e}", file.display())); continue; } };
            let cache_name = format!("transcript-{TRANSCRIPT_VERSION}-{}-{:016x}.json", agent, hash(file.to_string_lossy().to_string()));
            let cached = fs::read(cache.join(&cache_name)).ok().and_then(|b| serde_json::from_slice::<FileCache>(&b).ok()).filter(|c|
                c.version == TRANSCRIPT_VERSION && c.agent == *agent && c.fingerprint == before && c.records.iter().all(|record|
                    record.tally.valid() && record.agent == *agent && record.hour < 24 && !record.model.trim().is_empty()
                    && chrono::NaiveDate::parse_from_str(&record.day, "%Y-%m-%d").is_ok()
                    && (record.tally.total() > 0 || record.unclassified_tokens > 0)
                    && (*agent != "qwen" || record.deduplication_id.is_some())
                    && (*agent != "dsh" || (record.deduplication_id.is_some() && record.source_timestamp.is_some()))
                    && (*agent != "cursor" || (record.source_timestamp.is_some() && record.source_scope.is_some()))));
            let records = if let Some(c) = cached { source.cached_files += 1; c.records } else {
                let (records, notes) = match parse_file(&file, agent, cancel) {
                    Ok(value) => value,
                    Err(error) if check(cancel).is_err() => return Err(error),
                    Err(error) => { snapshot.notes.push(error); continue; }
                };
                if notes.is_empty() && stamp(&file, agent).ok() == Some(before) { let _ = write_cache(cache, &cache_name, &FileCache { version: TRANSCRIPT_VERSION, agent: (*agent).into(), fingerprint: before, records: records.clone() }, cancel); }
                else if stamp(&file, agent).ok() != Some(before) { snapshot.notes.push(format!("扫描期间文件发生变化：{}；本次结果未缓存", file.display())); }
                snapshot.notes.extend(notes); records
            };
            check(cancel)?;
            raw_count += records.len(); per_file.push((history::file_key(&file),records));
        }
        if *agent=="cursor" {
            let records=measured_readers::reconcile_cursor(per_file.into_iter().map(|(_,r)|r).collect(),&mut snapshot.notes,cancel)?;
            per_file=vec![("reconciled-cursor-store".into(),records)];
        }
        let stable=snapshot.notes.len()==note_start && snapshot.records.len()+raw_count<=MAX_RECORDS;
        history::order_live(&mut per_file);
        let mut records=if !inputs.is_empty() && (matches!(*agent,"claude"|"codex"|"qwen"|"gemini"|"pi"|"omp"|"kimchi"|"amp"|"droid"|"kiro"|"kimi") || ported_history(agent)) {
            if let Some(mut journal)=history::History::load(cache,agent,&mut snapshot.notes) {
                let (records,retained)=journal.reconcile(per_file,stable,&mut identities,cancel)?;
                source.retained_records=retained;
                if stable {journals.push(journal)}
                records
            } else {per_file.into_iter().flat_map(|(_,r)|r).filter(|r|history::claim(r,&mut identities)).collect()}
        } else {per_file.into_iter().flat_map(|(_,r)|r).filter(|r|history::claim(r,&mut identities)).collect()};
        if !inputs.is_empty() && !(matches!(*agent,"claude"|"codex"|"qwen"|"gemini"|"pi"|"omp"|"kimchi"|"amp"|"droid"|"kiro"|"kimi") || ported_history(agent)) {
            if let Some(mut journal)=history::StoreHistory::load(cache,agent,&mut snapshot.notes){
                source.retained_records=journal.reconcile(&mut records,stable,cancel)?;
                if stable {store_journals.push(journal)}
            }
        }
        let remaining = MAX_RECORDS.saturating_sub(snapshot.records.len());
        if records.len() > remaining { snapshot.notes.push("记录超过读取上限，计数可能不完整".into()); records.truncate(remaining); }
        for record in &mut records { check(cancel)?; record.cost = None; record.cost_breakdown = None; record.model_name = None;record.cache_savings=None;
                if let Some(price) = price_for_agent(&record.model, agent, prices) {
                    let tally = &record.tally;
                    let cost = tally.cost_breakdown(price);
                    if tally.cache_read>0{record.cache_savings=Some(tally.without_cache().cost_breakdown(price).iter().sum::<f64>()-cost.iter().sum::<f64>());}
                    if record.tally.total() > 0 { record.cost = Some(cost.iter().sum()); record.cost_breakdown = Some(cost); }
                    record.model_name = price.name.clone();
                }
        }
        source.records = records.len(); snapshot.records.extend(records);
        if source.records > 0 { source.status = "counted".into(); }
        snapshot.sources.push(source);
    }
    check(cancel)?;
    for journal in journals {
        if let Err(error)=journal.save(cancel) {
            check(cancel)?;snapshot.notes.push(format!("历史记录保存失败，原文件保留：{error}"));
        }
    }
    for journal in store_journals {
        if let Err(error)=journal.save(cancel){check(cancel)?;snapshot.notes.push(format!("历史记录保存失败，原文件保留：{error}"));}
    }
    snapshot.notes.sort(); snapshot.notes.dedup(); Ok(snapshot)
}

fn counter(value: &Value) -> u64 { value.as_u64().or_else(|| value.as_f64().filter(|x| x.is_finite() && *x >= 0.).map(|x| x as u64)).unwrap_or(0) }
fn parse_file(path: &Path, agent: &str, cancel: &AtomicBool) -> Result<(Vec<SpendRecord>, Vec<String>), String> {
    if ported_source(agent){return ported_read(path,agent,cancel)}
    if agent=="kiro"{return kiro_reader::read(path,cancel)}
    if agent=="kimi"{return kimi_reader::read(path,cancel)}
    if agent=="zed"{return zed_reader::read(path,cancel)}
    if aggregate_databases::agent(agent){return aggregate_databases::read(path,agent,cancel)}
    if agent=="openclaw"{return openclaw_reader::parse(path,cancel)}
    if agent == "dsh" { return dsh_reader::read(path, cancel); }
    if opencode_store::agent(agent) { return opencode_store::read(path, agent, cancel); }
    if session_readers::agent(agent){return session_readers::parse(path,agent,cancel);}
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
    let mut project = None;
    let mut normalized = Vec::new(); let mut digest = std::collections::hash_map::DefaultHasher::new(); let mut emitted = 0usize;
    let mut clock=reply_timing::Clock::default();
    let mut claude_replies=HashMap::<String,usize>::new();
    let mut codex_session=session.to_owned();let mut headed=false;let mut forked=false;let mut replaying=false;let mut codex_epoch=0u32;
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
        let relevant = line.windows(7).any(|w| w == b"\"usage\"") || line.windows(15).any(|w| w == b"\"usageMetadata\"") || line.windows(13).any(|w| w == b"\"token_count\"") || line.windows(7).any(|w| w == b"\"model\"") || line.windows(5).any(|w| w == b"\"cwd\"") || (agent=="codex"&&line.windows(14).any(|w|w==b"\"task_started\""||w==b"\"session_meta\""));
        let timing_line=[b"\"user\"".as_slice(),b"\"response_item\"",b"\"task_complete\""].iter().any(|needle|line.windows(needle.len()).any(|w|w==*needle));
        if !relevant && !timing_line { if eof { break; } continue; }
        let root: Value = match serde_json::from_slice(&line) { Ok(v) => v, Err(_) => { notes.push(format!("{agent}：无法解码一条用量/元信息记录，计数可能不完整")); if eof { break; } continue; } };
        let payload = &root["payload"];
        if agent=="codex"{
            if !headed&&root["type"]=="session_meta"{headed=true;if let Some(id)=payload["id"].as_str().filter(|s|!s.is_empty()){codex_session=id.into();}forked=payload["forked_from_id"].as_str().is_some_and(|s|!s.is_empty());}
            if payload["type"]=="task_started"{replaying=forked&&payload["turn_id"].as_str().is_some_and(|s|s.starts_with("rollout-"));if eof{break}continue}
        }
        if project.is_none() { project = root["cwd"].as_str().or_else(|| payload["cwd"].as_str()).filter(|v| !v.trim().is_empty()).map(str::to_owned); }
        let timestamp = root["timestamp"].as_str().and_then(|s| DateTime::parse_from_rfc3339(s).ok());
        clock.observe(&root,agent,replaying,&mut normalized);
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
                    session: sid, project: project.clone(), tally, unclassified_tokens: unclassified, deduplication_id: id, aggregate_timing: false, source_timestamp: Some(timestamp.timestamp()), source_scope: None, cost: None, cost_breakdown: None, model_name: None, cache_savings:None,reply_timing:None });
                emitted += 1;
            } else { notes.push("Qwen Code：实测计数缺少有效模型或时间，计数可能不完整".into()); }
            if emitted >= MAX_RECORDS { notes.push("Qwen Code：达到记录上限，计数可能不完整".into()); break; }
            if eof { break; } continue;
        }
        let mut codex_identity=None;
        let tally = if agent == "claude" {
            let message = &root["message"]; let usage = &message["usage"];
            if root["type"] != "assistant" || !usage.is_object() { if eof { break; } continue; }
            let Some(named) = message["model"].as_str().filter(|m| *m != "<synthetic>") else { if eof { break; } continue; };
            model = Some(named.to_owned());
            if timestamp.is_none() { notes.push("Claude Code：缺少有效用量时间，计数可能不完整".into()); if eof { break; } continue; }
            let written=counter(&usage["cache_creation_input_tokens"]);
            let context=counter(&usage["input_tokens"]).saturating_add(written).saturating_add(counter(&usage["cache_read_input_tokens"]));
            Tally { input:counter(&usage["input_tokens"]),output:counter(&usage["output_tokens"]),cache_write:written,cache_read:counter(&usage["cache_read_input_tokens"]),cache_write_1h:counter(&usage["cache_creation"]["ephemeral_1h_input_tokens"]).min(written),replies_without_cache_fields:u64::from(usage.get("cache_read_input_tokens").is_none()&&usage.get("cache_creation_input_tokens").is_none()),..Default::default() }.request(context)
        } else {
            if let Some(named) = payload["model"].as_str() { model = Some(named.to_owned()); }
            let totals = &payload["info"]["total_token_usage"];
            if payload["type"] != "token_count" || !totals.is_object() { if eof { break; } continue; }
            let current = [counter(&totals["input_tokens"]), counter(&totals["cached_input_tokens"]), counter(&totals["cache_write_input_tokens"]), counter(&totals["output_tokens"])];
            if previous.is_none()&&forked{
                let last=&payload["info"]["last_token_usage"];
                if !last.is_object(){notes.push("Codex：分叉首条缺少本次请求计数，继承量不计入；计数可能不完整".into());}
                let own=[counter(&last["input_tokens"]),counter(&last["cached_input_tokens"]),counter(&last["cache_write_input_tokens"]),counter(&last["output_tokens"])];
                previous=Some(std::array::from_fn(|i|current[i].saturating_sub(own[i])));
            }
            if previous.is_some_and(|p| current.iter().zip(p).any(|(now, old)| *now < old)) { notes.push("Codex：累计计数出现回退，计数可能不完整".into());codex_epoch=codex_epoch.saturating_add(1); }
            let mut delta = [0; 4]; for i in 0..4 { delta[i] = current[i].saturating_sub(previous.map(|p| p[i]).unwrap_or(0)); }
            previous = Some(current);
            if replaying{if eof{break}continue}
            let total=format!("{}:{}:{}:{}",current[0],current[1],current[3],counter(&totals["total_tokens"]));
            let id=format!("codex:{codex_session}:{codex_epoch}:{total}");
            codex_identity=Some((id,format!("codex-{}:{total}",if forked{"fork"}else{"original"})));
            if model.is_none() { notes.push("Codex：存在缺少模型的实测计数；使用未知模型且不计价".into()); model = Some("unknown".into()); }
            if delta[1] > delta[0] { notes.push("Codex：缓存计数超过输入差额，分类可能不完整".into()); }
            Tally { input: delta[0].saturating_sub(delta[1]), output: delta[3], cache_write: delta[2], cache_read: delta[1], ..Default::default() }.request(counter(&payload["info"]["last_token_usage"]["input_tokens"]))
        };
        if tally.total() == 0 { if eof { break; } continue; }
        if let (Some(timestamp), Some(model)) = (timestamp, model.as_ref()) {
            let local = timestamp.with_timezone(&Local);
            if agent=="claude" {
                let id=root["message"]["id"].as_str().map(|id|format!("claude:{id}"));
                // Streaming transcript fragments repeat a reply ID while its
                // output counter grows. Keep the fullest reported counters;
                // a first-fragment-only dedupe silently loses output tokens.
                if let Some(index)=id.as_ref().and_then(|id|claude_replies.get(id)).copied() {
                    let old:&mut SpendRecord=&mut normalized[index];
                    old.tally.input=old.tally.input.max(tally.input);old.tally.output=old.tally.output.max(tally.output);
                    old.tally.cache_write=old.tally.cache_write.max(tally.cache_write);old.tally.cache_read=old.tally.cache_read.max(tally.cache_read);
                    old.tally.cache_write_1h=old.tally.cache_write_1h.max(tally.cache_write_1h);
                    // A later fragment can disclose cache fields that the first
                    // fragment omitted. The latest complete reply is evidence.
                    old.tally.replies_without_cache_fields=tally.replies_without_cache_fields;
                    let context=old.tally.input.saturating_add(old.tally.cache_write).saturating_add(old.tally.cache_read);
                    old.tally.context_bands.clear();old.tally=old.tally.clone().request(context);
                } else {
                    if let Some(id)=&id {claude_replies.insert(id.clone(),normalized.len());}
                    normalized.push(SpendRecord{agent:agent.into(),model:model.clone(),day:local.format("%Y-%m-%d").to_string(),hour:chrono::Timelike::hour(&local),session:session.into(),project:project.clone(),tally,unclassified_tokens:0,deduplication_id:id,aggregate_timing:false,source_timestamp:Some(timestamp.timestamp()),source_scope:None,cost:None,cost_breakdown:None,model_name:None,cache_savings:None,reply_timing:None});
                }
            } else {
                let (id,scope)=codex_identity.unwrap();
                if !seen.insert(id.clone()){if eof{break}continue}
                normalized.push(SpendRecord{agent:agent.into(),model:model.clone(),day:local.format("%Y-%m-%d").to_string(),hour:chrono::Timelike::hour(&local),session:codex_session.clone(),project:project.clone(),tally,unclassified_tokens:0,deduplication_id:Some(id),aggregate_timing:false,source_timestamp:Some(timestamp.timestamp()),source_scope:Some(scope),cost:None,cost_breakdown:None,model_name:None,cache_savings:None,reply_timing:None});
                let index=normalized.len()-1;clock.counted(index,&mut normalized[index]);
            }
        } else { notes.push(format!("{agent}：缺少有效用量时间，计数可能不完整")); }
        if normalized.len() >= MAX_RECORDS { notes.push(format!("{agent}：达到记录上限，计数可能不完整")); break; }
        if eof { break; }
    }
    clock.finish();if agent=="claude"{for record in &mut normalized{record.reply_timing=record.deduplication_id.as_ref().and_then(|id|clock.timings.get(id).cloned());}}
    check(cancel)?; notes.sort(); notes.dedup();
    let mut records=Vec::new();
    let fragment = digest.finish();
    if agent=="qwen" {for (index, record) in normalized.iter_mut().enumerate() { if record.deduplication_id.is_none() { record.deduplication_id = Some(format!("qwen:{}:{fragment:016x}:{index}", record.session)); } }}
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
        if total == included && cached <= prompt { return (Tally { input: prompt - cached, output, cache_read: cached, cache_write: 0, ..Default::default() }, 0); }
        if total == disjoint { return (Tally { input: prompt, output, cache_read: cached, cache_write: 0, ..Default::default() }, 0); }
        return (Tally::default(), total);
    }
    (Tally { input: prompt.saturating_sub(cached), output, cache_read: cached, cache_write: 0, ..Default::default() }, 0)
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
        assert!(notes.is_empty());let mut tally=Tally::default();for r in &records{tally.add(&r.tally)}
        assert_eq!(tally,Tally { input:130,cache_read:50,cache_write:0,output:30,..Default::default() });assert_eq!(tally.total(),210);
    }
    #[test] fn codex_fork_first_read_counts_only_own_request_and_replay_turns_only_set_baseline(){
        let header=json!({"type":"session_meta","payload":{"id":"child","forked_from_id":"parent"}});
        let mut first=codex(1000,400,150);first["payload"]["info"]["last_token_usage"]=json!({"input_tokens":100,"cached_input_tokens":30,"output_tokens":20});
        let (records,notes)=parse(vec![header.clone(),json!({"payload":{"model":"m"}}),first.clone(),codex(1100,450,180)],"codex");assert!(notes.is_empty());assert_eq!(records.iter().map(|r|r.tally.total()).sum::<u64>(),250);assert!(records.iter().all(|r|r.session=="child"));
        let replay=json!({"payload":{"type":"task_started","turn_id":"rollout-1"}});let own=json!({"payload":{"type":"task_started","turn_id":"real-turn"}});
        let (records,notes)=parse(vec![header,json!({"payload":{"model":"m"}}),replay,first,own,codex(1100,450,180)],"codex");assert!(notes.is_empty());assert_eq!(records.len(),1);assert_eq!(records[0].tally.total(),130);
    }
    #[test] fn codex_copies_and_forks_deduplicate_with_cached_and_deleted_parents(){
        let f=Fixture::new();let original=vec![json!({"type":"session_meta","payload":{"id":"parent"}}),json!({"payload":{"model":"m"}}),codex(100,30,20)];
        let mut copied=codex(100,30,20);copied["payload"]["info"]["last_token_usage"]=json!({"input_tokens":100,"cached_input_tokens":30,"output_tokens":20});
        let fork=vec![json!({"type":"session_meta","payload":{"id":"child","forked_from_id":"parent"}}),json!({"payload":{"model":"m"}}),copied,codex(150,40,30)];
        let file=|rows:Vec<Value>|rows.into_iter().map(|v|v.to_string()).collect::<Vec<_>>().join("\n");
        let parent=f.dir.join("logs/z-parent.jsonl");fs::write(&parent,file(original.clone())).unwrap();fs::write(f.dir.join("logs/a-child.jsonl"),file(fork)).unwrap();fs::write(f.dir.join("logs/mirror.jsonl"),file(original)).unwrap();
        let scan=||scan_roots(&f.dir.join("cache"),&Prices::new(),&AtomicBool::new(false),|_,_|{},|a|if a=="codex"{vec![f.dir.join("logs")]}else{vec![]}).unwrap();
        for _ in 0..2{let s=scan();assert!(s.notes.is_empty());assert_eq!(s.records.iter().map(|r|r.tally.total()).sum::<u64>(),180);}
        fs::remove_file(parent).unwrap();let s=scan();assert_eq!(s.records.iter().map(|r|r.tally.total()).sum::<u64>(),180);
        fs::remove_file(f.dir.join("logs/mirror.jsonl")).unwrap();let s=scan();assert_eq!(s.records.iter().map(|r|r.tally.total()).sum::<u64>(),180);
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
    #[test] fn kimi_wire_counts_fresh_input_separately_without_model_or_price_guess(){
        let f=Fixture::new();let file=f.dir.join("logs/wire.jsonl");let row=json!({"timestamp":1791705600000i64,"message":{"payload":{"token_usage":{"input_other":100,"output":20,"input_cache_read":30,"input_cache_creation":5},"body":"KIMI_PRIVATE_SENTINEL"}}});
        fs::write(&file,format!("{}\n{}",row,json!({"timestamp":0,"message":{"payload":{"token_usage":{"input_other":999}}}}))).unwrap();
        let mut prices=Prices::new();prices.insert("kimi (unnamed)".into(),Price{input:1.,output:1.,cache_read:None,cache_write:None,name:None,tiers:vec![]});
        let scan=||scan_roots(&f.dir.join("cache"),&prices,&AtomicBool::new(false),|_,_|{},|id|if id=="kimi"{vec![f.dir.join("logs")]}else{vec![]}).unwrap();
        let first=scan();assert!(first.notes.is_empty());assert_eq!(first.records.len(),1);let r=&first.records[0];assert_eq!(r.tally.total(),155);assert_eq!(r.tally.input,100);assert_eq!(r.source_timestamp,Some(1791705600));assert!(r.cost.is_none());assert!(r.project.is_none());assert_eq!(r.model,"kimi (unnamed)");
        assert_eq!(scan().sources.iter().find(|s|s.id=="kimi").unwrap().cached_files,1);fs::remove_file(&file).unwrap();assert_eq!(scan().records[0].tally.total(),155);
        for entry in fs::read_dir(f.dir.join("cache")).unwrap(){assert!(!fs::read_to_string(entry.unwrap().path()).unwrap().contains("KIMI_PRIVATE_SENTINEL"));}
        assert_eq!(roots(&f.dir,"kimi",|_|None),vec![f.dir.join(".kimi/sessions")]);assert!(!candidate(&file.with_file_name("state.json"),"kimi"));
        assert_eq!(parse_file(&file,"kimi",&AtomicBool::new(true)).unwrap_err(),"cancelled");
    }
    #[test] fn kiro_explicit_turns_sidecar_changes_and_deleted_history(){
        let f=Fixture::new();let file=f.dir.join("logs/session.json");let sidecar=file.with_extension("jsonl");
        let header=json!({"session_id":"ks","cwd":"C:/project","private_body":"KIRO_PRIVATE_SENTINEL","session_state":{"rts_model_state":{"model_info":{"model_id":"claude-test"}},"conversation_metadata":{"user_turn_metadatas":[{"input_token_count":100,"output_token_count":20,"message_ids":["p"],"end_timestamp":1791705600},{"context_percentage":80,"character_count":999,"end_timestamp":1791705600}]}}});
        fs::write(&file,header.to_string()).unwrap();
        let scan=||scan_roots(&f.dir.join("cache"),&Prices::new(),&AtomicBool::new(false),|_,_|{},|id|if id=="kiro"{vec![f.dir.join("logs")]}else{vec![]}).unwrap();
        let first=scan();assert!(first.notes.is_empty());assert_eq!(first.records.len(),1);assert_eq!(first.records[0].tally.total(),120);assert_eq!(first.records[0].source_timestamp,Some(1791705600));assert_eq!(scan().sources.iter().find(|s|s.id=="kiro").unwrap().cached_files,1);
        fs::write(&sidecar,json!({"kind":"Prompt","data":{"message_id":"p","meta":{"timestamp":1791698400000i64},"body":"KIRO_PRIVATE_SENTINEL"}}).to_string()).unwrap();
        let moved=scan();assert_eq!(moved.records.len(),1);assert_eq!(moved.records[0].source_timestamp,Some(1791698400));assert_eq!(moved.sources.iter().find(|s|s.id=="kiro").unwrap().cached_files,0);
        fs::remove_file(&file).unwrap();let kept=scan();assert_eq!(kept.records.len(),1);assert_eq!(kept.records[0].tally.total(),120);assert_eq!(kept.records[0].source_timestamp,Some(1791698400));
        for entry in fs::read_dir(f.dir.join("cache")).unwrap(){assert!(!fs::read_to_string(entry.unwrap().path()).unwrap().contains("KIRO_PRIVATE_SENTINEL"));}
        assert_eq!(parse_file(&sidecar,"kiro",&AtomicBool::new(true)).unwrap_err(),"cancelled");
    }
    #[test] fn kiro_missing_count_time_and_malformed_header_are_not_zero_estimates(){
        let f=Fixture::new();let file=f.dir.join("logs/session.json");
        fs::write(&file,json!({"session_state":{"conversation_metadata":{"user_turn_metadatas":[{"input_token_count":3},{"character_count":999,"end_timestamp":1791705600}]}}}).to_string()).unwrap();
        let (records,notes)=parse_file(&file,"kiro",&AtomicBool::new(false)).unwrap();assert!(records.is_empty());assert_eq!(notes.len(),1);
        fs::write(&file,"{").unwrap();assert!(parse_file(&file,"kiro",&AtomicBool::new(false)).is_err());
        assert_eq!(roots(&f.dir,"kiro",|_|None),vec![f.dir.join(".kiro/sessions/cli")]);assert!(!candidate(&file.with_extension("jsonl"),"kiro"));
    }
    #[test] fn aggregate_databases_track_growth_wal_and_pruning_without_reading_bodies(){
        for agent in ["goose","hermes"] {
            let f=Fixture::new();let database=f.dir.join("logs").join(if agent=="goose"{"sessions.db"}else{"state.db"});
            let db=rusqlite::Connection::open(&database).unwrap();db.execute_batch("PRAGMA journal_mode=WAL;").unwrap();
            if agent=="goose"{db.execute_batch("CREATE TABLE sessions(id TEXT,model_config_json TEXT,created_at TEXT,input_tokens INTEGER,output_tokens INTEGER,private_body TEXT); INSERT INTO sessions VALUES('s','{\"model_name\":\"m\"}','2026-10-11 08:00:00',100,20,'PRIVATE_AGGREGATE_SENTINEL');").unwrap();}
            else{db.execute_batch("CREATE TABLE sessions(id TEXT,model TEXT,started_at INTEGER,input_tokens INTEGER,output_tokens INTEGER,private_body TEXT); INSERT INTO sessions VALUES('s','m',1791705600,100,20,'PRIVATE_AGGREGATE_SENTINEL');").unwrap();}
            let scan=||scan_roots(&f.dir.join("cache"),&Prices::new(),&AtomicBool::new(false),|_,_|{},|id|if id==agent{vec![database.clone()]}else{vec![]}).unwrap();
            let first=scan();assert_eq!(first.records.len(),1,"{agent}");assert_eq!(first.records[0].tally.total(),120);
            assert_eq!(scan().sources.iter().find(|s|s.id==agent).unwrap().cached_files,1);
            db.execute("UPDATE sessions SET input_tokens=150",[]).unwrap();let grew=scan();assert_eq!(grew.records.len(),1);assert_eq!(grew.records[0].tally.total(),170);assert_eq!(grew.sources.iter().find(|s|s.id==agent).unwrap().cached_files,0);
            db.execute("DELETE FROM sessions",[]).unwrap();let pruned=scan();assert_eq!(pruned.records.len(),1);assert_eq!(pruned.records[0].tally.total(),170);
            for file in fs::read_dir(f.dir.join("cache")).unwrap(){assert!(!fs::read_to_string(file.unwrap().path()).unwrap().contains("PRIVATE_AGGREGATE_SENTINEL"));}
            assert!(parse_file(&database,agent,&AtomicBool::new(true)).is_err());
        }
    }
    #[test] fn zed_compressed_sqlite_keeps_only_native_counters_and_reports_corruption(){
        let f=Fixture::new();let database=f.dir.join("logs/threads.db");let db=rusqlite::Connection::open(&database).unwrap();db.execute_batch("PRAGMA journal_mode=WAL;CREATE TABLE threads(id TEXT,updated_at TEXT,data_type TEXT,data BLOB,folder_paths TEXT);").unwrap();
        let value=json!({"model":{"provider":"zed.dev","model":"m"},"request_token_usage":[{"input_tokens":100,"output_tokens":20,"cache_read_input_tokens":30}],"private_body":"ZED_PRIVATE_SENTINEL"});let raw=serde_json::to_vec(&value).unwrap();let zipped=zstd::stream::encode_all(&raw[..],1).unwrap();db.execute("INSERT INTO threads VALUES('native','1791705600000','json',?1,'project')",[zipped]).unwrap();let mut imported=value.clone();imported["imported"]=json!(true);db.execute("INSERT INTO threads VALUES('imported','1791705600','json',?1,NULL)",[serde_json::to_vec(&imported).unwrap()]).unwrap();
        let scan=||scan_roots(&f.dir.join("cache"),&Prices::new(),&AtomicBool::new(false),|_,_|{},|id|if id=="zed"{vec![database.clone()]}else{vec![]}).unwrap();let first=scan();assert_eq!(first.records.len(),1);assert_eq!(first.records[0].tally.total(),150);assert!(first.records[0].aggregate_timing);assert!(first.notes.is_empty());assert_eq!(scan().sources.iter().find(|s|s.id=="zed").unwrap().cached_files,1);
        db.execute("INSERT INTO threads VALUES('corrupt','1791705600','zstd',?1,NULL)",[b"bad".to_vec()]).unwrap();let partial=scan();assert_eq!(partial.records.len(),1);assert!(partial.notes.iter().any(|n|n.contains("Zed")));db.execute("DELETE FROM threads",[]).unwrap();let pruned=scan();assert_eq!(pruned.records.len(),1);assert_eq!(pruned.records[0].tally.total(),150);
        for file in fs::read_dir(f.dir.join("cache")).unwrap(){assert!(!fs::read_to_string(file.unwrap().path()).unwrap().contains("ZED_PRIVATE_SENTINEL"));}assert!(parse_file(&database,"zed",&AtomicBool::new(true)).is_err());
    }
    #[test] fn crush_is_recognized_without_turning_money_into_tokens(){let f=Fixture::new();let registry=f.dir.join("logs/projects.json");fs::write(&registry,json!({"projects":[],"cost":123,"input_tokens":1000}).to_string()).unwrap();let result=scan_roots(&f.dir.join("cache"),&Prices::new(),&AtomicBool::new(false),|_,_|{},|id|if id=="crush"{vec![registry.clone()]}else{vec![]}).unwrap();assert!(result.records.is_empty());let source=result.sources.iter().find(|s|s.id=="crush").unwrap();assert_eq!(source.status,"no-data");assert_eq!(source.files,0);}
    #[test] fn enumeration_caps_depth_and_accepts_explicit_native_files(){
        let f=Fixture::new();let file=f.dir.join("logs/state.db");fs::write(&file,"fixture").unwrap();let mut files=vec![];let mut notes=vec![];enumerate(&file,"hermes",&mut files,&AtomicBool::new(false),&mut notes).unwrap();assert_eq!(files,vec![file]);
        let mut root=f.dir.join("logs");for _ in 0..34{root=root.join("d");fs::create_dir_all(&root).unwrap();}fs::write(root.join("state.db"),"deep").unwrap();files.clear();enumerate(&f.dir.join("logs"),"hermes",&mut files,&AtomicBool::new(false),&mut notes).unwrap();assert_eq!(files.len(),1);assert!(notes.iter().any(|n|n.contains("深度")));
    }
    #[test] fn pi_family_forks_cache_and_deleted_history_keep_one_counter_without_message_text(){
        for agent in ["pi","omp","kimchi"] {
            let f=Fixture::new(); let root=f.dir.join("logs");let a=root.join("a.jsonl");let b=root.join("b.jsonl");
            let header=json!({"type":"session","id":"same-session","cwd":"project"});
            let mut reply=json!({"type":"message","id":"reply","timestamp":"2026-10-02T12:00:00Z","message":{"role":"assistant","model":"m","content":"SENSITIVE_TEST_MESSAGE","usage":{"input":100,"output":20,"cacheRead":30,"cacheWrite":5,"totalTokens":155}}});
            if agent!="kimchi"{reply["message"]["responseId"]=json!("response-shared");}
            fs::write(&a,format!("{header}\n{reply}")).unwrap();fs::write(&b,format!("{header}\n{reply}")).unwrap();
            let scan=||scan_roots(&f.dir.join("cache"),&Prices::new(),&AtomicBool::new(false),|_,_|{},|id|if id==agent{vec![root.clone()]}else{vec![]}).unwrap();
            let first=scan();assert_eq!(first.records.len(),1,"{agent}");assert_eq!(first.records[0].tally.total(),155);
            let cached=scan();assert_eq!(cached.sources.iter().find(|s|s.id==agent).unwrap().cached_files,2);
            fs::remove_file(&a).unwrap();assert_eq!(scan().records.len(),1);fs::remove_file(&b).unwrap();let kept=scan();assert_eq!(kept.records.len(),1);assert_eq!(kept.records[0].tally,first.records[0].tally);
            for entry in fs::read_dir(f.dir.join("cache")).unwrap(){assert!(!fs::read_to_string(entry.unwrap().path()).unwrap().contains("SENSITIVE_TEST_MESSAGE"));}
        }
    }
    #[test] fn pi_header_validation_and_malformed_rows_do_not_create_complete_history(){
        let f=Fixture::new();let path=f.dir.join("logs/invalid.jsonl");let msg=json!({"type":"message","id":"r","timestamp":"2026-10-02T00:00:00Z","message":{"role":"assistant","model":"m","usage":{"input":10}}});
        fs::write(&path,msg.to_string()).unwrap();let cancel=AtomicBool::new(false);let (r,n)=parse_file(&path,"pi",&cancel).unwrap();assert!(r.is_empty());assert!(!n.is_empty());
        fs::write(&path,format!("{{\"type\":\"session\",\"id\":\"s\"}}\n{msg}\n{{broken" )).unwrap();
        let snapshot=scan_roots(&f.dir.join("cache"),&Prices::new(),&cancel,|_,_|{},|id|if id=="pi"{vec![f.dir.join("logs")]}else{vec![]}).unwrap();assert_eq!(snapshot.records.len(),1);assert!(!snapshot.notes.is_empty());assert!(!f.dir.join("cache").exists());
        assert!(parse_file(&path,"pi",&AtomicBool::new(true)).is_err());
    }
    #[test] fn amp_native_document_reconciles_ledger_and_retains_after_deletion(){
        let f=Fixture::new();let path=f.dir.join("logs/T-session.json");
        let root=json!({"id":"T-session","created":1790899200000i64,"messages":[{"role":"assistant","messageId":1,"content":"SENSITIVE_TEST_MESSAGE","usage":{"model":"m","inputTokens":100,"outputTokens":20}}],"usageLedger":{"events":[{"toMessageId":1,"timestamp":"2026-10-02T00:00:01Z","model":"m","tokens":{"input":100,"output":20}}]}});
        fs::write(&path,root.to_string()).unwrap();fs::write(f.dir.join("logs/not-a-thread.json"),root.to_string()).unwrap();
        let scan=||scan_roots(&f.dir.join("cache"),&Prices::new(),&AtomicBool::new(false),|_,_|{},|id|if id=="amp"{vec![f.dir.join("logs")]}else{vec![]}).unwrap();
        let first=scan();assert_eq!(first.records.len(),1);assert_eq!(first.records[0].tally.total(),120);assert_eq!(first.sources.iter().find(|s|s.id=="amp").unwrap().files,1);
        fs::remove_file(path).unwrap();let kept=scan();assert_eq!(kept.records.len(),1);assert_eq!(kept.records[0].tally,first.records[0].tally);assert!(!fs::read_to_string(f.dir.join("cache/history-amp.json")).unwrap().contains("SENSITIVE_TEST_MESSAGE"));
    }
    #[test] fn droid_session_total_replaces_prior_total_and_sibling_model_invalidates_cache(){
        let f=Fixture::new();let path=f.dir.join("logs/s.settings.json");let transcript=f.dir.join("logs/s.jsonl");
        let mut settings=json!({"providerLockTimestamp":"2026-10-02T12:00:00Z","tokenUsage":{"inputTokens":100,"outputTokens":20,"cacheReadTokens":30,"totalTokens":150}});
        fs::write(&path,settings.to_string()).unwrap();fs::write(&transcript,json!({"message":{"content":"Model: claude-opus-4.5 SENSITIVE_TEXT"}}).to_string()).unwrap();
        let scan=||scan_roots(&f.dir.join("cache"),&Prices::new(),&AtomicBool::new(false),|_,_|{},|id|if id=="droid"{vec![f.dir.join("logs")]}else{vec![]}).unwrap();
        let first=scan();assert!(first.notes.is_empty());assert_eq!(first.records.len(),1);assert_eq!(first.records[0].model,"claude-opus-4.5");assert_eq!(first.records[0].tally.total(),150);assert!(first.records[0].aggregate_timing);
        assert_eq!(scan().sources.iter().find(|s|s.id=="droid").unwrap().cached_files,1);
        fs::write(&transcript,json!({"message":{"content":"Model: GPT-5.6 SENSITIVE_TEXT"}}).to_string()).unwrap();let refreshed=scan();assert_eq!(refreshed.records[0].model,"GPT-5.6");assert_eq!(refreshed.sources.iter().find(|s|s.id=="droid").unwrap().cached_files,0);
        settings["tokenUsage"]["inputTokens"]=json!(200);settings["tokenUsage"]["totalTokens"]=json!(250);fs::write(&path,settings.to_string()).unwrap();assert_eq!(scan().records[0].tally.total(),250);
        fs::remove_file(path).unwrap();let kept=scan();assert_eq!(kept.records.len(),1);assert_eq!(kept.records[0].tally.total(),250);assert!(!fs::read_to_string(f.dir.join("cache/history-droid.json")).unwrap().contains("SENSITIVE_TEXT"));
    }
    #[test] fn reply_timing_delayed_codex_counter_and_ttft_survive_file_cache_and_retention(){
        let f=Fixture::new();let path=f.dir.join("logs/timing.jsonl");let lines=vec![
            json!({"type":"session_meta","timestamp":"2026-10-02T12:00:00Z","payload":{"id":"timing"}}),
            json!({"type":"turn_context","timestamp":"2026-10-02T12:00:00Z","payload":{"model":"gpt-5.6"}}),
            json!({"type":"response_item","timestamp":"2026-10-02T12:00:00Z","payload":{"type":"message","role":"user"}}),
            json!({"type":"response_item","timestamp":"2026-10-02T12:00:04Z","payload":{"type":"message","role":"assistant"}}),
            json!({"type":"response_item","timestamp":"2026-10-02T12:00:05Z","payload":{"type":"function_call_output"}}),
            json!({"type":"event_msg","timestamp":"2026-10-02T12:00:05Z","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":100,"output_tokens":200},"last_token_usage":{"input_tokens":100,"output_tokens":200}}}}),
            json!({"type":"event_msg","timestamp":"2026-10-02T12:00:06Z","payload":{"type":"task_complete","turn_id":"t","time_to_first_token_ms":1500}}),
            json!({"type":"event_msg","timestamp":"2026-10-02T12:00:07Z","payload":{"type":"task_complete","turn_id":"t","time_to_first_token_ms":1500}})
        ];fs::write(&path,lines.iter().map(Value::to_string).collect::<Vec<_>>().join("\n")).unwrap();
        let scan=||scan_roots(&f.dir.join("cache"),&Prices::new(),&AtomicBool::new(false),|_,_|{},|id|if id=="codex"{vec![f.dir.join("logs")]}else{vec![]}).unwrap();
        for snap in [scan(),scan()]{let t=snap.records[0].reply_timing.as_ref().unwrap();assert_eq!((t.output_tokens,t.seconds,t.replies),(200,4.,1));assert_eq!((t.first_token_seconds,t.first_token_turns),(1.5,1));}
        fs::remove_file(path).unwrap();let snap=scan();assert_eq!(snap.records[0].reply_timing.as_ref().unwrap().first_token_turns,1);
    }
    #[test]fn claude_reply_timing_uses_final_stream_fragment_and_keeps_ttft_unknown(){
        let (r,n)=parse(vec![json!({"type":"user","timestamp":"2026-10-02T12:00:00Z"}),claude("timed","2026-10-02T12:00:01Z","m",100,110),claude("timed","2026-10-02T12:00:04Z","m",100,200)],"claude");assert!(n.is_empty());assert_eq!(r.len(),1);let t=r[0].reply_timing.as_ref().unwrap();assert_eq!((t.output_tokens,t.seconds,t.first_token_turns),(200,4.,0));
    }
    #[test]fn damaged_context_metadata_preserves_existing_history_journal(){
        let f=Fixture::new();let path=f.dir.join("logs/a.jsonl");fs::write(&path,claude("test","2026-10-02T12:00:00Z","m",100,20).to_string()).unwrap();f.scan(&Prices::new(),&AtomicBool::new(false)).unwrap();
        let journal=f.dir.join("cache/history-claude.json");let mut v:Value=serde_json::from_slice(&fs::read(&journal).unwrap()).unwrap();let entry=v["files"].as_object_mut().unwrap().values_mut().next().unwrap();entry["records"][0]["tally"]["contextBands"]=json!({"200000":{"input":999,"output":0,"cacheRead":0,"cacheWrite":0}});fs::write(&journal,v.to_string()).unwrap();let original=fs::read(&journal).unwrap();
        let result=f.scan(&Prices::new(),&AtomicBool::new(false)).unwrap();assert_eq!(result.records.len(),1);assert!(result.notes.iter().any(|s|s.contains("历史文件损坏")));assert_eq!(fs::read(journal).unwrap(),original);
    }
    #[test]fn openclaw_sqlite_jsonl_wal_forks_and_pruning_are_one_counted_store(){
        use rusqlite::Connection;let f=Fixture::new();fs::create_dir_all(f.dir.join("logs/sessions")).unwrap();let database=f.dir.join("logs/openclaw-agent.sqlite");let conn=Connection::open(&database).unwrap();conn.execute_batch("PRAGMA journal_mode=WAL;PRAGMA wal_autocheckpoint=0;CREATE TABLE transcript_events(session_id TEXT,seq INTEGER,event_json TEXT);CREATE TABLE session_windows(session_id TEXT,model TEXT);INSERT INTO session_windows VALUES('s','gpt-5.6');").unwrap();
        let m=json!({"type":"message","id":"event-one","timestamp":"2026-10-11T00:00:00Z","message":{"role":"assistant","model":"gpt-5.6","content":"SENSITIVE_SENTINEL","usage":{"input":100,"output":20,"cacheRead":30,"cacheWrite":5}}});conn.execute("INSERT INTO transcript_events VALUES('s',1,?1)",[m.to_string()]).unwrap();let jsonl=f.dir.join("logs/sessions/s.jsonl");fs::write(&jsonl,m.to_string()).unwrap();fs::write(f.dir.join("logs/sessions/fork.jsonl.deleted.2026"),m.to_string()).unwrap();
        let scan=||scan_roots(&f.dir.join("cache"),&Prices::new(),&AtomicBool::new(false),|_,_|{},|id|if id=="openclaw"{vec![f.dir.join("logs")]}else{vec![]}).unwrap();let original=fs::read(&database).unwrap();let first=scan();assert!(first.notes.is_empty());assert_eq!(first.records.len(),1);assert_eq!(first.records[0].tally.total(),155);assert_eq!(scan().sources.iter().find(|s|s.id=="openclaw").unwrap().cached_files,3);
        let mut more=m.clone();more["id"]=json!("event-two");more["message"].as_object_mut().unwrap().remove("model");conn.execute("INSERT INTO transcript_events VALUES('s',2,?1)",[more.to_string()]).unwrap();assert_eq!(fs::read(&database).unwrap(),original);let appended=scan();assert_eq!(appended.records.len(),2);assert_eq!(appended.sources.iter().find(|s|s.id=="openclaw").unwrap().cached_files,2);
        conn.execute("DELETE FROM transcript_events WHERE seq=1",[]).unwrap();fs::remove_file(jsonl).unwrap();fs::remove_file(f.dir.join("logs/sessions/fork.jsonl.deleted.2026")).unwrap();let kept=scan();assert_eq!(kept.records.iter().map(|r|r.tally.total()).sum::<u64>(),310);assert_eq!(scan().records.iter().map(|r|r.tally.total()).sum::<u64>(),310);
        for file in fs::read_dir(f.dir.join("cache")).unwrap(){assert!(!fs::read_to_string(file.unwrap().path()).unwrap().contains("SENSITIVE_SENTINEL"));}assert!(parse_file(&database,"openclaw",&AtomicBool::new(true)).is_err());
    }
    #[test]fn openclaw_missing_model_and_time_keep_valid_rows_with_partial_note(){
        let f=Fixture::new();fs::create_dir_all(f.dir.join("logs/sessions")).unwrap();let file=f.dir.join("logs/sessions/s.jsonl");let good=json!({"type":"message","id":"good","timestamp":"2026-10-11T00:00:00Z","message":{"role":"assistant","model":"gpt-5.6","usage":{"input":10,"output":5}}});let mut bad=good.clone();bad["id"]=json!("bad");bad["timestamp"]=Value::Null;fs::write(&file,format!("{bad}\n{good}")).unwrap();let(r,n)=parse_file(&file,"openclaw",&AtomicBool::new(false)).unwrap();assert_eq!(r.len(),1);assert_eq!(n.len(),1);
    }
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
    #[test] fn one_hour_cache_is_a_subset_and_prices_at_twice_input(){
        let prices=parse_prices(&json!({"anthropic":{"models":{"m":{"cost":{"input":2,"output":8,"cache_write":2.5,"cache_read":0.2}}}}}));
        let tally=Tally{input:1_000_000,output:1_000_000,cache_write:1_000_000,cache_write_1h:600_000,cache_read:1_000_000,..Default::default()};
        assert_eq!(tally.total(),4_000_000);assert_eq!(tally.cost_breakdown(&prices["m"]),[2.,8.,3.4,0.2]);
        let over=Tally{cache_write:100,cache_write_1h:999,..Default::default()};assert!((over.cost_breakdown(&prices["m"])[2]-0.0004).abs()<1e-12);
    }
    #[test] fn context_tiers_price_whole_requests_and_do_not_apply_early(){
        let root=json!({"openai":{"models":{"m":{"cost":{"input":5,"output":30,"cache_read":0.5,"tiers":[{"input":10,"output":45,"cache_read":1,"tier":{"type":"context","size":272001}}]}}}}});
        let prices=parse_prices(&root);let p=&prices["m"];assert_eq!(p.tiers[0].threshold,272000);
        let short=Tally{input:1_000_000,..Default::default()}.request(272000);let long=Tally{input:1_000_000,..Default::default()}.request(300000);
        assert_eq!(short.cost_breakdown(p)[0],5.);assert_eq!(long.cost_breakdown(p)[0],10.);let mut sum=short;sum.add(&long);assert_eq!(sum.cost_breakdown(p)[0],15.);assert_eq!(sum.total(),2_000_000);
        let mut odd=p.clone();odd.tiers[0].threshold=262144;assert_eq!(Tally{input:1_000_000,..Default::default()}.request(260000).cost_breakdown(&odd)[0],5.);
        assert_eq!(parse_tiers(&json!({"tiers":[{"input":9,"output":9,"tier":{"type":"context","size":272000}}]}))[0].threshold,272000);
        assert_eq!(parse_tiers(&json!({"context_over_200k":{"input":6,"output":22.5}}))[0].threshold,200000);
    }
    #[test] fn parsed_hour_cache_and_context_bands_survive_cache_and_deleted_source(){
        let f=Fixture::new();let path=f.dir.join("logs/hour.jsonl");let mut line=claude("hour","2026-10-02T12:00:00Z","m",300000,20);
        line["message"]["usage"]["cache_creation_input_tokens"]=json!(100);line["message"]["usage"]["cache_creation"]=json!({"ephemeral_1h_input_tokens":80});fs::write(&path,line.to_string()).unwrap();
        let prices=parse_prices(&json!({"anthropic":{"models":{"m":{"cost":{"input":2,"output":8,"cache_write":2.5}}}}}));
        let first=f.scan(&prices,&AtomicBool::new(false)).unwrap();assert_eq!(first.records[0].tally.cache_write_1h,80);assert_eq!(first.records[0].tally.context_bands[&272000].cache_write_1h,80);
        let cached=f.scan(&prices,&AtomicBool::new(false)).unwrap();assert_eq!(cached.records[0].tally,first.records[0].tally);fs::remove_file(path).unwrap();let kept=f.scan(&prices,&AtomicBool::new(false)).unwrap();assert_eq!(kept.records[0].cost,first.records[0].cost);assert_eq!(kept.records[0].tally,first.records[0].tally);
    }
    #[test] fn missing_cache_fields_are_absence_and_later_fragments_can_disclose_them(){
        let f=Fixture::new();let mut line=claude("reply","2026-10-02T12:00:00Z","m",100,20);line["message"]["usage"].as_object_mut().unwrap().remove("cache_creation_input_tokens");line["message"]["usage"].as_object_mut().unwrap().remove("cache_read_input_tokens");
        let path=f.dir.join("logs/reply.jsonl");fs::write(&path,line.to_string()).unwrap();assert_eq!(f.scan(&Prices::new(),&AtomicBool::new(false)).unwrap().records[0].tally.replies_without_cache_fields,1);
        let mut final_line=line.clone();final_line["message"]["usage"]["cache_read_input_tokens"]=json!(0);fs::write(path,format!("{line}\n{final_line}")).unwrap();assert_eq!(f.scan(&Prices::new(),&AtomicBool::new(false)).unwrap().records[0].tally.replies_without_cache_fields,0);
    }
    #[test] fn serialized_huge_token_counters_keep_an_exact_native_to_js_representation(){
        let f=Fixture::new();let path=f.dir.join("logs/huge.jsonl");let line=claude("huge","2026-10-02T12:00:00Z","m",9_007_199_254_740_993,20);fs::write(path,line.to_string()).unwrap();
        let record=f.scan(&Prices::new(),&AtomicBool::new(false)).unwrap().records.remove(0);let wire=serde_json::to_value(&record).unwrap();assert_eq!(wire["exactTally"]["input"],"9007199254740993");let read:SpendRecord=serde_json::from_value(wire).unwrap();assert_eq!(read.tally.input,record.tally.input);
    }
    #[test] fn legacy_history_millisecond_timestamps_migrate_without_recounting(){
        let f=Fixture::new();let path=f.dir.join("logs/one.jsonl");fs::write(&path,claude("one","2026-10-02T12:00:00Z","m",100,20).to_string()).unwrap();let mut record=f.scan(&Prices::new(),&AtomicBool::new(false)).unwrap().records.remove(0);
        let at=record.source_timestamp.unwrap();record.agent="gemini".into();record.source_timestamp=Some(at*1000);let cache=f.dir.join("cache");
        fs::write(cache.join("history-gemini.json"),json!({"format":1,"files":{"old":{"readings":6,"records":[record.clone()]}}}).to_string()).unwrap();
        let mut notes=vec![];let mut journal=history::History::load(&cache,"gemini",&mut notes).unwrap();let (records,retained)=journal.reconcile(vec![],true,&mut HashSet::new(),&AtomicBool::new(false)).unwrap();assert!(notes.is_empty());assert_eq!(retained,1);assert_eq!(records[0].source_timestamp,Some(at));journal.save(&AtomicBool::new(false)).unwrap();
        let bytes:Value=serde_json::from_slice(&fs::read(cache.join("history-gemini.json")).unwrap()).unwrap();assert_eq!(bytes["format"],2);
        record.agent="cursor".into();fs::write(cache.join("history-store-cursor.json"),json!({"format":1,"readings":6,"offset":Local::now().offset().local_minus_utc(),"records":[record.clone()]}).to_string()).unwrap();
        let mut journal=history::StoreHistory::load(&cache,"cursor",&mut notes).unwrap();let mut records=vec![];assert_eq!(journal.reconcile(&mut records,true,&AtomicBool::new(false)).unwrap(),1);assert_eq!(records[0].source_timestamp,Some(at));assert_eq!(records[0].day,record.day);
    }
    #[test] fn deleted_transcript_survives_repeated_scans_and_is_repriced(){
        let f=Fixture::new();let path=f.dir.join("logs/session.jsonl");
        fs::write(&path,claude("history-one","2026-10-02T12:00:00Z","history-model",100,20).to_string()).unwrap();
        let first=f.scan(&Prices::new(),&AtomicBool::new(false)).unwrap();
        fs::remove_file(path).unwrap();
        for _ in 0..2 {
            let kept=f.scan(&Prices::new(),&AtomicBool::new(false)).unwrap();
            assert_eq!(kept.records.len(),1);assert_eq!(kept.records[0].tally,first.records[0].tally);
            assert!(kept.records[0].cost.is_none());assert_eq!(kept.sources[0].files,0);
        }
        let prices=parse_prices(&json!({"anthropic":{"models":{"history-model":{"cost":{"input":2,"output":8,"cache_read":0.2,"cache_write":2.5}}}}}));
        let kept=f.scan(&prices,&AtomicBool::new(false)).unwrap();
        assert!((kept.records[0].cost.unwrap()-0.0003656).abs()<1e-10);
        assert_eq!(kept.records[0].session,first.records[0].session);
        assert!(!fs::read_to_string(f.dir.join("cache/history-claude.json")).unwrap().contains("SENSITIVE"));
    }
    #[test]fn streaming_claude_replies_keep_final_output_and_do_not_add_fragments(){
        let lines=[claude("stream","2026-10-02T12:00:00Z","measured",100,1),claude("stream","2026-10-02T12:00:01Z","measured",100,75),claude("stream","2026-10-02T12:00:02Z","measured",100,70)].map(|v|v.to_string()).join("\n");
        let (records,notes)=parse_reader(&mut Cursor::new(lines),"claude","session",&AtomicBool::new(false)).unwrap();
        assert!(notes.is_empty());assert_eq!(records.len(),1);assert_eq!(records[0].tally.output,75);
        assert_eq!(records[0].tally.total(),180);assert_eq!(records[0].deduplication_id.as_deref(),Some("claude:stream"));
    }
    #[test]fn large_history_can_be_atomically_replaced_after_it_exceeds_one_mib(){
        let f=Fixture::new();let path=f.dir.join("logs/a.jsonl");
        let lines=(0..3000).map(|i|claude(&format!("large-{i}"),"2026-10-02T12:00:00Z","measured",100,20).to_string()).collect::<Vec<_>>().join("\n");
        fs::write(&path,&lines).unwrap();let first=f.scan(&Prices::new(),&AtomicBool::new(false)).unwrap();assert_eq!(first.records.len(),3000);
        let journal=f.dir.join("cache/history-claude.json");assert!(fs::metadata(&journal).unwrap().len()>1024*1024);
        fs::write(&path,format!("{}\n{}",lines,claude("added","2026-10-02T13:00:00Z","measured",10,20))).unwrap();
        let next=f.scan(&Prices::new(),&AtomicBool::new(false)).unwrap();assert!(next.notes.is_empty());
        fs::remove_file(path).unwrap();let kept=f.scan(&Prices::new(),&AtomicBool::new(false)).unwrap();assert_eq!(kept.records.len(),3001);
    }
    #[test] fn live_transcript_growth_replaces_its_previous_counters(){
        let f=Fixture::new();let path=f.dir.join("logs/session.jsonl");
        let one=claude("history-one","2026-10-02T12:00:00Z","measured",100,20);
        fs::write(&path,one.to_string()).unwrap();f.scan(&Prices::new(),&AtomicBool::new(false)).unwrap();
        fs::write(&path,format!("{}\n{}",one,claude("history-two","2026-10-02T13:00:00Z","measured",50,10))).unwrap();
        let grown=f.scan(&Prices::new(),&AtomicBool::new(false)).unwrap();
        assert_eq!(grown.records.iter().map(|r|r.tally.total()).sum::<u64>(),190);
        fs::remove_file(path).unwrap();let kept=f.scan(&Prices::new(),&AtomicBool::new(false)).unwrap();
        assert_eq!(kept.records.iter().map(|r|r.tally.total()).sum::<u64>(),190);
    }
    #[test] fn archived_reply_claims_prevent_resume_and_backup_double_count(){
        let f=Fixture::new();let original=f.dir.join("logs/a.jsonl");let copy=f.dir.join("logs/b.jsonl");
        let one=claude("copied-history","2026-10-02T12:00:00Z","measured",100,20);
        fs::write(&original,one.to_string()).unwrap();f.scan(&Prices::new(),&AtomicBool::new(false)).unwrap();
        fs::remove_file(&original).unwrap();
        fs::write(&copy,format!("{}\n{}",one,claude("new-reply","2026-10-02T13:00:00Z","measured",50,10))).unwrap();
        let resumed=f.scan(&Prices::new(),&AtomicBool::new(false)).unwrap();
        assert_eq!(resumed.records.iter().map(|r|r.tally.total()).sum::<u64>(),190);
        fs::write(&original,one.to_string()).unwrap();
        let restored=f.scan(&Prices::new(),&AtomicBool::new(false)).unwrap();
        assert_eq!(restored.records.iter().map(|r|r.tally.total()).sum::<u64>(),190);
        fs::remove_file(&original).unwrap();fs::remove_file(&copy).unwrap();
        let gone=f.scan(&Prices::new(),&AtomicBool::new(false)).unwrap();
        assert_eq!(gone.records.iter().map(|r|r.tally.total()).sum::<u64>(),190);
    }
    #[test] fn partial_and_cancelled_reads_cannot_replace_durable_history(){
        let f=Fixture::new();let path=f.dir.join("logs/a.jsonl");
        fs::write(&path,claude("one","2026-10-02T12:00:00Z","measured",100,20).to_string()).unwrap();
        f.scan(&Prices::new(),&AtomicBool::new(false)).unwrap();
        let journal=f.dir.join("cache/history-claude.json");let before=fs::read(&journal).unwrap();
        fs::write(&path,format!("{}\n{{\"usage\":broken}}",claude("two","2026-10-02T12:00:00Z","measured",500,20))).unwrap();
        let partial=f.scan(&Prices::new(),&AtomicBool::new(false)).unwrap();assert!(!partial.notes.is_empty());
        assert_eq!(fs::read(&journal).unwrap(),before);
        assert!(f.scan(&Prices::new(),&AtomicBool::new(true)).is_err());assert_eq!(fs::read(&journal).unwrap(),before);
        fs::remove_file(path).unwrap();let kept=f.scan(&Prices::new(),&AtomicBool::new(false)).unwrap();
        assert_eq!(kept.records[0].tally.total(),125);
    }
    #[test] fn damaged_or_newer_history_is_never_overwritten(){
        let f=Fixture::new();let path=f.dir.join("logs/a.jsonl");
        fs::write(path,claude("one","2026-10-02T12:00:00Z","measured",1,2).to_string()).unwrap();
        fs::create_dir_all(f.dir.join("cache")).unwrap();let journal=f.dir.join("cache/history-claude.json");
        for bytes in ["broken","{\"format\":999,\"files\":{}}"] {
            fs::write(&journal,bytes).unwrap();let live=f.scan(&Prices::new(),&AtomicBool::new(false)).unwrap();
            assert_eq!(live.records[0].tally.total(),8);assert!(live.notes.iter().any(|n|n.contains("历史文件损坏")));
            assert_eq!(fs::read_to_string(&journal).unwrap(),bytes);
        }
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
        assert_eq!(inside, Tally { input:70, output:25, cache_read:30, cache_write:0, ..Default::default() }); assert_eq!(remainder, 0);
        let (beside, remainder) = qwen_usage(&json!({"promptTokenCount":100,"candidatesTokenCount":20,"thoughtsTokenCount":5,"cachedContentTokenCount":30,"totalTokenCount":155}));
        assert_eq!(beside, Tally { input:100, output:25, cache_read:30, cache_write:0, ..Default::default() }); assert_eq!(remainder, 0);
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
        let file = fs::read_dir(fixture.dir.join("cache")).unwrap().map(|e|e.unwrap().path()).find(|p|p.file_name().unwrap().to_string_lossy().starts_with("transcript-")).unwrap();
        let mut value: Value = serde_json::from_slice(&fs::read(&file).unwrap()).unwrap(); value["records"][0]["hour"] = json!(99);
        fs::write(&file,value.to_string()).unwrap();
        let rebuilt = fixture.scan(&Prices::new(),&AtomicBool::new(false)).unwrap(); assert_eq!(rebuilt.sources[0].cached_files,0); assert!(rebuilt.records[0].hour < 24); assert_eq!(rebuilt.records[0].tally.total(),125);
    }
    #[test] fn ported_sidecar_inputs_invalidate_the_transcript_cache() {
        let fixture = Fixture::new(); let dir = fixture.dir.clone();
        let put = |relative: &str, content: &str| { let path = dir.join(relative); fs::create_dir_all(path.parent().unwrap()).unwrap(); fs::write(&path, content).unwrap(); path };
        // JCode appends most replies to a sibling journal, not to the session JSON.
        let jcode = put("logs/jcode/session_a.json", "{}"); let journal = put("logs/jcode/session_a.journal.jsonl", "");
        let before = stamp(&jcode, "jcode").unwrap(); fs::write(&journal, "{\"append\":1}\n").unwrap();
        assert_ne!(stamp(&jcode, "jcode").unwrap(), before, "journal-only append must invalidate");
        // Fx keeps its version and creation time in a sibling session sidecar.
        let fx = put("logs/fx/s1/usage-v2.json", "{}"); put("logs/fx/s1/session.json", "{}");
        let before = stamp(&fx, "fx").unwrap(); fs::write(dir.join("logs/fx/s1/session.json"), "{\"updated_at_ms\":1}").unwrap();
        assert_ne!(stamp(&fx, "fx").unwrap(), before, "fx sidecar must invalidate");
        // The editor task log's model fallback is a sibling document.
        let ui = put("logs/roocode/tasks/t1/ui_messages.json", "[]"); put("logs/roocode/tasks/t1/api_conversation_history.json", "");
        let before = stamp(&ui, "roocode").unwrap(); fs::write(dir.join("logs/roocode/tasks/t1/api_conversation_history.json"), "[{\"model\":\"m\"}]").unwrap();
        assert_ne!(stamp(&ui, "roocode").unwrap(), before, "editor model fallback must invalidate");
        // Cline CLI keeps its session manifest beside the message log.
        let cli = put("logs/cline/abc.messages.json", "[]"); put("logs/cline/abc.json", "{}");
        let before = stamp(&cli, "cline").unwrap(); fs::write(dir.join("logs/cline/abc.json"), "{\"model\":\"m\"}").unwrap();
        assert_ne!(stamp(&cli, "cline").unwrap(), before, "cline manifest must invalidate");
        // Command Code's legacy model default is the store-level config.
        let command = put("logs/store/.commandcode/projects/p/s.jsonl", ""); put("logs/store/.commandcode/config.json", "{}");
        let before = stamp(&command, "commandcode").unwrap(); fs::write(dir.join("logs/store/.commandcode/config.json"), "{\"model\":\"m\"}").unwrap();
        assert_ne!(stamp(&command, "commandcode").unwrap(), before, "command code config must invalidate");
        // A running SQLite writer changes only the WAL / rollback journal.
        let zcode = put("logs/zcode/cli/db/db.sqlite", "sqlite"); put("logs/zcode/cli/db/db.sqlite-wal", "");
        let before = stamp(&zcode, "zcode").unwrap(); fs::write(dir.join("logs/zcode/cli/db/db.sqlite-wal"), "wal").unwrap();
        assert_ne!(stamp(&zcode, "zcode").unwrap(), before, "zcode WAL must invalidate");
        let unsloth = put("logs/unsloth/studio.db", "sqlite"); put("logs/unsloth/studio.db-journal", "");
        let before = stamp(&unsloth, "unsloth").unwrap(); fs::write(dir.join("logs/unsloth/studio.db-journal"), "journal").unwrap();
        assert_ne!(stamp(&unsloth, "unsloth").unwrap(), before, "unsloth rollback journal must invalidate");
        let stable = stamp(&ui, "roocode").unwrap(); assert_eq!(stamp(&ui, "roocode").unwrap(), stable, "an untouched candidate keeps its fingerprint");
    }
    #[test] fn ported_sources_count_natively_and_survive_rescans() {
        let fixture = Fixture::new(); let home = fixture.dir.join("home");
        let put = |relative: &str, content: &str| { let path = home.join(relative); fs::create_dir_all(path.parent().unwrap()).unwrap(); fs::write(&path, content).unwrap(); };
        put(".augment/sessions/aug-1.json", &json!({"sessionId":"aug-1","chatHistory":[{"completed":true,"finishedAt":1791028800,
            "exchange":{"model_id":"claude-sonnet-4","request_id":"req-1","response_nodes":[{"token_usage":{"input_tokens":10,"output_tokens":2}}]}}]}).to_string());
        put(".mux/sessions/ws-1/session-usage.json", &json!({"version":1,"byModel":{"anthropic:claude":{"input":{"tokens":100},"output":{"tokens":50}}},
            "lastRequest":{"timestamp":1_700_000_000_000i64},"prompt":"SIDE-CHANNEL PROMPT"}).to_string());
        put(".config/Code/User/globalStorage/rooveterinaryinc.roo-cline/tasks/task-1/ui_messages.json", &json!([
            {"type":"say","say":"api_req_started","ts":1_700_000_000_000i64,"modelInfo":{"modelId":"claude-3"},
             "text":"{\"tokensIn\":100,\"tokensOut\":20,\"cacheWrites\":5,\"cacheReads\":10}"}]).to_string());
        let scan=||scan_roots(&fixture.dir.join("cache"),&Prices::new(),&AtomicBool::new(false),|_,_|{},|id|roots(&home,id,|_|None)).unwrap();
        let first=scan();
        assert_eq!(first.sources.len(), 41, "every registered agent reports one source");
        assert!(first.notes.is_empty(), "unexpected notes: {:?}", first.notes);
        let counted=|snapshot:&SpendSnapshot,id:&str|snapshot.sources.iter().find(|source|source.id==id).unwrap().records;
        assert_eq!(counted(&first,"augment"), 1); assert_eq!(counted(&first,"mux"), 1); assert_eq!(counted(&first,"roocode"), 1);
        let total=|snapshot:&SpendSnapshot,id:&str|snapshot.records.iter().filter(|record|record.agent==id).map(|record|record.tally.total()).sum::<u64>();
        assert_eq!(total(&first,"augment"), 12); assert_eq!(total(&first,"mux"), 150); assert_eq!(total(&first,"roocode"), 135);
        for id in ["augment","mux","roocode"] { let source=first.sources.iter().find(|source|source.id==id).unwrap();
            assert_eq!(source.origin, "native"); assert_eq!(source.status, "counted"); assert!(source.files >= 1); }
        assert!(!serde_json::to_string(&first.records).unwrap().contains("SIDE-CHANNEL"));
        // The second pass reads the transcript caches and reconciles the same
        // identities, so a rescan cannot duplicate a ported source's counters.
        let second=scan(); assert_eq!(second.records.len(), first.records.len());
        for id in ["augment","mux","roocode"] { let source=second.sources.iter().find(|source|source.id==id).unwrap();
            assert_eq!(source.records, 1); assert_eq!(source.cached_files, 1); }
        assert_eq!(total(&second,"augment"), 12); assert_eq!(total(&second,"mux"), 150); assert_eq!(total(&second,"roocode"), 135);
    }
}
