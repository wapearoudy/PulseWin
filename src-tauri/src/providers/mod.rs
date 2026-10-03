//! Provider registry.
//!
//! Every data source implements [`Provider`]. The scheduler runs them
//! concurrently so one slow/hanging provider cannot stall the panel.

pub mod abacus;
pub mod aixy;
pub mod alibaba_coding_plan;
pub mod alibaba_token_plan;
pub mod amp;
pub mod antigravity;
pub mod atlas_cloud;
pub mod augment;
pub mod bifrost;
pub mod chutes;
pub mod claude_code;
pub mod clawrouter;
pub mod cline_pass;
pub mod codebuff;
pub mod codex;
pub mod command_code;
pub mod copilot;
pub mod cursor;
pub mod deepinfra;
pub mod deepseek;
pub mod dev_pass;
pub mod devin;
pub mod elevenlabs;
pub mod factory;
pub mod gemini;
pub mod git_kraken;
pub mod grok;
pub mod grok_bot;
pub mod huggingface;
pub mod hyper;
pub mod ibm_bob;
pub mod jetbrains_ai;
pub mod kimi_code;
pub mod kilo_code;
pub mod kiro;
pub mod lite_llm;
pub mod llm_proxy;
pub mod long_cat;
pub mod manus;
pub mod minimax;
pub mod mistral;
pub mod moonshot;
pub mod neuralwatt;
pub mod new_api;
pub mod notion_ai;
pub mod nous_portal;
pub mod ollama_cloud;
pub mod open_code_go;
pub mod openai_platform;
pub mod pasted;
pub mod perplexity;
pub mod poe;
pub mod qoder;
pub mod qwen_cloud;
pub mod raycast_ai;
pub mod replicate;
pub mod sakana;
pub mod step_fun;
pub mod sub2api;
pub mod synthetic;
pub mod t3_chat;
pub mod type_safe;
pub mod v0;
pub mod v2ex;
pub mod venice;
pub mod vercel_ai_gateway;
pub mod volcengine;
pub mod warp;
pub mod windsurf;
pub mod x_kiro;
pub mod xai_api;
pub mod xiaomi_mimo;
pub mod zai;
pub mod zed;
pub mod zen_mux;
pub mod zoom_mate;

use std::sync::Arc;
use std::time::Duration;

use futures::future::BoxFuture;

use crate::credentials;
use crate::model::{ProviderUsage, UsageWindow};

/// Shared state handed to every provider fetch: one pooled HTTP client with a
/// hard timeout, so a provider that never answers degrades to an error card
/// instead of freezing the refresh loop.
pub struct Ctx {
    pub client: reqwest::Client,
    /// The same client with redirects turned **off**, for the self-hosted
    /// gateways.
    ///
    /// The original builds these requests with `NoRedirects`, and the reason
    /// travels with the address: a key is put on these requests as a header by
    /// hand, and a hand-set header rides a redirect to whatever host it names.
    /// That is the whole trust boundary these providers are careful about — the
    /// address is checked before the key is attached to it, and it must not be
    /// possible for the server on the other end to move the key somewhere else
    /// afterwards. A redirect therefore arrives as a 3xx and is read as the
    /// credential being turned away, which is what `classify` does with it.
    pub gateway_client: reqwest::Client,
}

impl Ctx {
    pub fn new() -> anyhow::Result<Self> {
        let client = configured_builder().build()?;
        let gateway_client = configured_builder()
            .redirect(reqwest::redirect::Policy::none())
            .build()?;

        Ok(Self {
            client,
            gateway_client,
        })
    }
}

/// The client every request is made with, before the one thing the gateways
/// need differently is applied.
///
/// Built twice rather than cloned: `ClientBuilder` is not `Clone`, and the two
/// clients around this share a connection pool anyway because the settings are
/// identical apart from the redirect policy.
fn configured_builder() -> reqwest::ClientBuilder {
    let mut builder = reqwest::Client::builder()
        .timeout(Duration::from_secs(20))
        .connect_timeout(Duration::from_secs(8))
        .user_agent(concat!("PulseWin/", env!("CARGO_PKG_VERSION")));

    // Windows keeps its proxy configuration in the registry, not in the
    // environment. Without this, every provider call times out for users
    // behind a local proxy (Clash, v2ray, a corporate MITM) — which is the
    // common setup for anyone using these tools.
    if let Some(url) = crate::proxy::system_proxy() {
        if let Ok(mut proxy) = reqwest::Proxy::all(&url) {
            if let Some(no_proxy) = crate::proxy::system_no_proxy() {
                proxy = proxy.no_proxy(reqwest::NoProxy::from_string(&no_proxy));
            }
            builder = builder.proxy(proxy);
        }
    }

    builder
}

pub type FetchFuture = BoxFuture<'static, ProviderUsage>;

pub trait Provider: Send + Sync {
    /// Stable machine id, also used for the `PULSEWIN_*` env overrides.
    fn id(&self) -> &'static str;
    /// Human-readable name shown on the card.
    fn name(&self) -> &'static str;
    /// Whether this tool is present on the machine at all.
    ///
    /// **Not the same question as "did it answer".** A tool whose credential
    /// fails to fetch still belongs on the rail — it is the thing the reader is
    /// looking for when a ring has gone quiet — while a tool that is not
    /// installed here must not be drawn at all, because a ring at zero is a
    /// reading and "you do not have this" is not. [`ProviderUsage::configured`]
    /// carries that same verdict on the result; this is the same question asked
    /// *before* the request so an unused tool costs nothing.
    ///
    /// **This must make no network call.** It is asked on every refresh, ahead
    /// of the fetch, and it answers only from what is on the machine: an
    /// environment variable that is set, or a credential file that exists.
    /// "Disabled providers are not fetched" is the original's rule, and this is
    /// where the port gets to keep it.
    fn is_configured(&self) -> bool;
    /// Perform one fetch. Implementations must not panic; return
    /// `ProviderUsage::failed` instead.
    fn fetch(&self, ctx: Arc<Ctx>) -> FetchFuture;
}

/// The providers PulseWin ships with, in the original's declaration order.
///
/// `Provider.builtIn` in `UsageProvider.swift` is the authoritative list, and
/// the order matters beyond tidiness: an account the stored rail has not met
/// yet is appended to the panel in exactly this order, so a port that shuffled
/// its registry would draw the same accounts in a different order from the
/// original. Providers still to be ported simply leave their slot empty.
pub fn registry() -> Vec<Arc<dyn Provider>> {
    vec![
        Arc::new(claude_code::ClaudeCode),
        Arc::new(codex::Codex),
        Arc::new(kiro::Kiro),
        Arc::new(antigravity::Antigravity),
        Arc::new(cursor::Cursor),
        Arc::new(open_code_go::OpenCodeGo),
        Arc::new(kimi_code::KimiCode),
        Arc::new(ollama_cloud::OllamaCloud),
        Arc::new(zai::Zai),
        Arc::new(zai::Zhipu),
        Arc::new(minimax::MiniMax),
        Arc::new(minimax::MiniMaxCn),
        Arc::new(copilot::Copilot),
        Arc::new(grok::Grok),
        Arc::new(grok_bot::GrokBot),
        Arc::new(volcengine::Volcengine),
        Arc::new(command_code::CommandCode),
        Arc::new(deepseek::DeepSeek),
        Arc::new(devin::Devin),
        Arc::new(xiaomi_mimo::XiaomiMiMo),
        Arc::new(sub2api::Sub2Api),
        Arc::new(new_api::NewApi),
        Arc::new(v2ex::V2ex),
        Arc::new(qoder::Qoder),
        Arc::new(step_fun::StepFun),
        Arc::new(cline_pass::ClinePass),
        // qoder and stepFun sit between the two in the original's order and are
        // still to be ported; they leave their slots empty here, as every
        // unported provider does.
        Arc::new(alibaba_coding_plan::AlibabaCodingPlan),
        Arc::new(alibaba_token_plan::AlibabaTokenPlan),
        Arc::new(qwen_cloud::QwenCloud),
        Arc::new(factory::Factory),
        Arc::new(gemini::GeminiCli),
        Arc::new(kilo_code::KiloCode),
        Arc::new(augment::Augment),
        Arc::new(jetbrains_ai::JetBrainsAI),
        Arc::new(t3_chat::T3Chat),
        Arc::new(synthetic::Synthetic),
        Arc::new(elevenlabs::ElevenLabs),
        Arc::new(warp::Warp),
        Arc::new(windsurf::Windsurf),
        Arc::new(bifrost::Bifrost),
        Arc::new(chutes::Chutes),
        Arc::new(long_cat::LongCat),
        Arc::new(zoom_mate::ZoomMate),
        Arc::new(notion_ai::NotionAi),
        Arc::new(ibm_bob::IbmBob),
        Arc::new(nous_portal::NousPortal),
        Arc::new(raycast_ai::RaycastAi),
        Arc::new(git_kraken::GitKraken),
        Arc::new(x_kiro::XKiro),
        Arc::new(abacus::Abacus),
        Arc::new(moonshot::Moonshot),
        Arc::new(hyper::Hyper),
        Arc::new(atlas_cloud::AtlasCloud),
        Arc::new(poe::Poe),
        Arc::new(venice::Venice),
        Arc::new(openai_platform::OpenAiPlatform),
        // zed, sakana and mistral sit between the two in the original's order
        // and are still to be ported.
        Arc::new(amp::Amp),
        Arc::new(zed::Zed),
        Arc::new(sakana::Sakana),
        Arc::new(mistral::Mistral),
        Arc::new(codebuff::Codebuff),
        Arc::new(llm_proxy::LlmProxy),
        Arc::new(lite_llm::LiteLlm),
        Arc::new(aixy::Aixy),
        Arc::new(neuralwatt::Neuralwatt),
        Arc::new(clawrouter::ClawRouter),
        Arc::new(zen_mux::ZenMux),
        Arc::new(v0::V0),
        Arc::new(dev_pass::DevPass),
        Arc::new(perplexity::Perplexity),
        Arc::new(manus::Manus),
        Arc::new(huggingface::HuggingFace),
        Arc::new(deepinfra::DeepInfra),
        Arc::new(xai_api::XaiApi),
        Arc::new(replicate::Replicate),
        Arc::new(type_safe::TypeSafe),
        Arc::new(vercel_ai_gateway::VercelAiGateway),
    ]
}

/// Fetch every provider concurrently, preserving registry order in the result.
///
/// A provider with no credential on this machine is **not fetched at all**: it
/// returns [`ProviderUsage::unconfigured`], which is what tells the panel not to
/// draw it. Everything else is fetched as before, and a fetch that fails still
/// yields a configured card, because a ring that has gone quiet is the one the
/// reader is looking for.
pub async fn collect_all(ctx: Arc<Ctx>) -> Vec<ProviderUsage> {
    let ids = registry().iter().map(|p| p.id().to_string()).collect::<Vec<_>>();
    collect_selected(ctx, &ids).await
}

/// Selection is checked before discovery, so disabled tools cannot read credentials
/// or start helper processes through the monitor's refresh routes.
pub async fn collect_selected(ctx: Arc<Ctx>, enabled: &[String]) -> Vec<ProviderUsage> {
    collect_from(ctx, enabled, registry()).await
}

async fn collect_from(ctx: Arc<Ctx>, enabled: &[String], providers: Vec<Arc<dyn Provider>>) -> Vec<ProviderUsage> {
    let futures: Vec<FetchFuture> = providers
        .iter()
        .filter(|p| enabled.iter().any(|id| id == p.id()))
        .map(|p| -> FetchFuture {
            let ctx = Arc::clone(&ctx);
            let p = Arc::clone(p);
            // Each provider owns its own error boundary: a failure yields a
            // `failed` snapshot rather than propagating out of the join.
            let id = p.id();
            let name = p.name();
            Box::pin(async move {
                let stored = match crate::settings::read_credential_checked(id) {
                    Ok(stored) => stored,
                    Err(error) => return ProviderUsage::failed(id, name, error.to_string()),
                };
                if !p.is_configured() && stored.api_key.is_none() {
                    return ProviderUsage::unconfigured(id, name, not_configured());
                }
                let usage = match tokio::time::timeout(Duration::from_secs(45), p.fetch(ctx)).await {
                    Ok(usage) => usage,
                    Err(_) => ProviderUsage::failed(id, name, "Usage check timed out. Try again."),
                };
                if usage.error.is_some() {
                    usage
                } else if usage.windows.is_empty() {
                    ProviderUsage::failed(id, name, "provider returned no usage windows")
                } else {
                    usage
                }
            })
        })
        .collect();

    futures::future::join_all(futures).await
}

/// The reason an unfetched provider carries, and the only one.
///
/// Deliberately short and deliberately provider-neutral. The card is not drawn
/// at all, so this is read in `--json` and the logs rather than on screen, and
/// the several ways in differ per provider — `missing_key` names the two for a
/// key-only tool, but six of these read a token or a cookie, and the gateways
/// need an address as well as a key, so no single sentence can be accurate for
/// all of them.
pub fn not_configured() -> &'static str {
    "not configured — no credential found on this machine"
}

/// Flatten a reqwest error into one line, including its source chain.
///
/// reqwest's top-level `Display` is usually just
/// "error sending request for url (...)", which tells the user nothing. The
/// useful cause — DNS failure, TLS handshake, timeout, connection reset — is in
/// the `source()` chain, so walk it.
pub fn describe_reqwest_error(e: &reqwest::Error) -> String {
    let mut parts = vec![e.to_string()];

    let mut source: Option<&(dyn std::error::Error + 'static)> = std::error::Error::source(e);
    while let Some(err) = source {
        let msg = err.to_string();
        // Guard against cycles in a badly-behaved source chain.
        if !parts.iter().any(|p| p == &msg) {
            parts.push(msg);
        }
        source = err.source();
    }

    parts.join(" <- ")
}

/// Normalize a percentage that may arrive as 0..1 or 0..100.
pub fn normalize_percent(raw: f64) -> f64 {
    let pct = if raw <= 1.0 && raw > 0.0 { raw * 100.0 } else { raw };
    pct.clamp(0.0, 100.0)
}

/// The original's `usedFraction` (0..1) as this port's `percent_used` (0..100).
///
/// Every service in `Pulse-original` carries a used *fraction*; the port's
/// model is 0..100 because the panel draws a ring per cent. Converting at the
/// boundary keeps each provider's arithmetic — `used / limit`, `100 - left`,
/// `min(max(…, 0), 1)` — identical to the original rather than rewritten in
/// another scale, which is where a port quietly loses an off-by-one.
pub fn percent_from_fraction(fraction: f64) -> f64 {
    (fraction * 100.0).clamp(0.0, 100.0)
}

/// The same conversion for a figure the original already holds as 0..100.
pub fn percent_from_scale(percent: f64) -> f64 {
    percent.clamp(0.0, 100.0)
}

/// Epoch seconds as the RFC3339 stamp the model carries.
///
/// The original's services all end at `Date(timeIntervalSince1970:)`, and this
/// port's model carries the stamp rather than the date, so every provider that
/// reads a Unix time goes through here. `None` for a stamp that is not a time
/// — the zero a service writes when it has no date, and anything past the
/// range `chrono` can hold.
pub fn stamp_from_epoch_seconds(seconds: f64) -> Option<String> {
    if !seconds.is_finite() || seconds <= 0.0 {
        return None;
    }
    let at = chrono::DateTime::from_timestamp(seconds as i64, 0)?;
    Some(at.to_rfc3339_opts(chrono::SecondsFormat::Secs, true))
}

/// The same for a figure that may be seconds or milliseconds.
///
/// The threshold is `model::parse_reset`'s own: anything past the year 2286
/// read as seconds is really milliseconds. Providers whose service draws the
/// line somewhere else say so and pick for themselves.
pub fn stamp_from_epoch(raw: f64) -> Option<String> {
    if !raw.is_finite() || raw <= 0.0 {
        return None;
    }
    stamp_from_epoch_seconds(if raw > 1e11 { raw / 1000.0 } else { raw })
}

/// Sort windows shortest-first, the order the original sorts every provider's
/// limits in, and drop the sort key.
///
/// The original sorts on `windowSeconds`, and so does this: a provider that
/// states a five-hour limit beside a weekly one must lead with the five-hour
/// one whatever the reply's own order was.
pub fn by_window_length(rows: Vec<(i64, crate::model::UsageWindow)>) -> Vec<crate::model::UsageWindow> {
    let mut rows = rows;
    rows.sort_by_key(|(seconds, _)| *seconds);
    rows.into_iter().map(|(_, window)| window).collect()
}

/// The API key a key-only provider reads, from the two places this port has.
///
/// The original keeps a pasted key in the Mac keychain (`keys.dat`). PulseWin
/// has no settings window yet, so the same key arrives from
/// `PULSEWIN_<ID>_KEY` (or the generic `PULSEWIN_KEY`) or from
/// `%APPDATA%\PulseWin\<id>.json` — the file `cursor.rs` already established
/// for a credential the tool itself does not store. Env wins, because someone
/// who exported a key meant that one to be used.
pub fn provider_key(id: &str) -> Option<String> {
    if let Some(value) = credentials::env_override(id, "key") {
        return Some(value);
    }

    let path = settings_path(id)?;
    let json = credentials::read_json(&path)?;
    credentials::dig_first_str(&json, &["apiKey", "api_key", "key", "token"])
}

/// `%APPDATA%\PulseWin\<id>.json`, where a key with no on-disk home of its own
/// is kept. Exposed so a provider can name the exact file in its error and in
/// its own fallback order.
pub fn settings_path(id: &str) -> Option<std::path::PathBuf> {
    crate::settings::credential_file(id)
}

/// The failure every key-only provider reports when neither source has a key.
///
/// It names both ways in, because "no API key" on its own leaves the user
/// guessing at a file name — and `extra` carries whatever else this provider
/// can read, which differs per provider.
pub fn missing_key(id: &str, extra: &str) -> String {
    let env = format!("PULSEWIN_{}_KEY", id.to_uppercase().replace('-', "_"));
    let file = settings_path(id)
        .map(|p| p.display().to_string())
        .unwrap_or_else(|| format!("%APPDATA%\\PulseWin\\{id}.json"));
    format!("no API key (set {env}, or create {file} with {{\"apiKey\": \"…\"}}{extra})")
}

/// One status line, one hint — the shape every provider in the port uses.
pub fn http_failure(status: reqwest::StatusCode, hints: &[(u16, &str)]) -> String {
    let hint = hints
        .iter()
        .find(|(code, _)| *code == status.as_u16())
        .map(|(_, hint)| *hint)
        .unwrap_or("");
    format!("HTTP {status}{hint}")
}

// ---------------------------------------------------------------------------
// Self-hosted gateways: a key and an address the reader names
// ---------------------------------------------------------------------------
//
// The original keeps the address beside the key in its own settings
// (`ProviderProfile.Credential.keyAndAddress`, checked by `GatewayAddress`).
// PulseWin has no settings window, so the same pair arrives from the two places
// this port has: `PULSEWIN_<ID>_KEY` plus `PULSEWIN_<ID>_BASE_URL`, or one
// `%APPDATA%\PulseWin\<id>.json` holding both `apiKey` and `baseUrl`.
//
// **The address is a trust boundary, not a convenience.** Every other provider
// in this directory knows where to go; these are *told*, which means PulseWin
// can be pointed at any host on the internet with a credential attached — and
// whatever is typed here gets a bearer token put on it, so the checks below are
// about where that token may go rather than about whether a string parses.
// They are the original's `GatewayAddress`, kept in one place for the same
// reason it is there: a second copy of a rule like this is a second copy to
// forget to tighten.

/// The address a self-hosted gateway is reached at.
///
/// Env wins, as it does for a key: someone who exported an address meant that
/// one to be used. The file field is `baseUrl`, with the spellings a person
/// pasting a client's config might use beside it.
pub fn provider_base_url(id: &str) -> Option<String> {
    if let Some(value) = credentials::env_override(id, "base_url") {
        return Some(value);
    }

    let path = settings_path(id)?;
    let json = credentials::read_json(&path)?;
    credentials::dig_first_str(&json, &["baseUrl", "base_url", "serverAddress", "address"])
}

/// Whether a self-hosted gateway has everything the fetch needs: a key, **and**
/// the address that key is sent to.
///
/// Both, because neither is a credential on its own. A key with nowhere to go
/// is a string, and an address with no key is somebody else's server. This is
/// the check every key-and-address provider's `is_configured` makes, and it
/// makes no network call — it asks only whether the two are on the machine.
///
/// Whether the address can actually be *used* is not asked here: `gateway_url`
/// settles that in the fetch, where a refusal can say so on the card rather
/// than leaving the reader with no ring and no reason. This mirrors the
/// original, whose fetch has a `serverAddressRefused` outcome of its own.
pub fn gateway_configured(id: &str) -> bool {
    provider_key(id).is_some() && provider_base_url(id).is_some()
}

/// The failure a gateway reports when the address half of its credential is
/// missing. Names the env var and the file, for the same reason `missing_key`
/// names both ways in.
pub fn missing_address(id: &str) -> String {
    let env = format!(
        "PULSEWIN_{}_BASE_URL",
        id.to_uppercase().replace('-', "_")
    );
    let file = settings_path(id)
        .map(|p| p.display().to_string())
        .unwrap_or_else(|| format!("%APPDATA%\\PulseWin\\{id}.json"));
    format!("no server address (set {env}, or create {file} with {{\"baseUrl\": \"…\"}})")
}

/// The failure a gateway reports when the address it was given may not be used.
///
/// One sentence, because the reader can act on all of it at once: the rules are
/// not a list to work through, they are what an address has to be.
pub fn refused_address() -> &'static str {
    "the server address cannot be used — it must be https (http only on a local or private host), with no user info, no query and no fragment"
}

/// A checked URL for `path` on the gateway the reader named, or `None` when the
/// address may not be used.
///
/// - A scheme is assumed when none is typed, because `gateway.example.com` is
///   what people paste. It is assumed **https**, never http.
/// - Plain http is allowed only where there is nothing between this machine and
///   the server to intercept it: loopback, a private network, or `.local`. On a
///   public host it is **refused rather than upgraded**, because silently
///   rewriting somebody's address is how a credential ends up somewhere they
///   never looked.
/// - No user info and no fragment: both are ways of writing a URL whose host is
///   not the part a reader's eye lands on.
/// - No query either: the path built here is the whole request, and a query
///   pasted out of a dashboard link would be forwarded with the key.
/// - `trimming` holds suffixes to drop off what was typed before the route is
///   appended. People paste a gateway's root and people paste the base URL out
///   of their client's config, which usually ends in `/v1`; appending blindly
///   makes `/v1/v1/…`.
pub fn gateway_url(typed: &str, path: &str, trimming: &[&str]) -> Option<String> {
    let trimmed = typed.trim();
    if trimmed.is_empty() {
        return None;
    }

    let text = if trimmed.contains("://") {
        trimmed.to_string()
    } else {
        format!("https://{trimmed}")
    };
    let mut url = reqwest::Url::parse(&text).ok()?;

    let scheme = url.scheme().to_lowercase();
    if scheme != "https" && scheme != "http" {
        return None;
    }
    let host = url.host_str()?.to_string();
    if host.is_empty() {
        return None;
    }
    if !url.username().is_empty() || url.password().is_some() {
        return None;
    }
    if url.fragment().is_some() || url.query().is_some() {
        return None;
    }
    if scheme == "http" && !allows_plain_http(&host) {
        return None;
    }

    let root = gateway_root(url.path(), trimming);
    url.set_path(&format!("{root}{path}"));
    Some(url.to_string())
}

/// Whether `typed` is an address that could be used at all, without naming a
/// route — what the original's Settings pane checks when the address is saved.
pub fn gateway_address_usable(typed: &str) -> bool {
    gateway_url(typed, "/", &[]).is_some()
}

/// The gateway's root: trailing slashes gone, and any suffix the reader already
/// typed that the route is about to repeat.
///
/// Longest suffix first, so `/v1/usage` is not left as `/usage` by a rule meant
/// to strip `/v1`.
fn gateway_root(typed_path: &str, trimming: &[&str]) -> String {
    let mut path = typed_path.to_string();
    while path.ends_with('/') {
        path.pop();
    }

    let mut suffixes: Vec<&str> = trimming.to_vec();
    suffixes.sort_by_key(|suffix| std::cmp::Reverse(suffix.len()));
    for suffix in suffixes {
        if path.ends_with(suffix) {
            path.truncate(path.len() - suffix.len());
            while path.ends_with('/') {
                path.pop();
            }
            break;
        }
    }

    path
}

/// Whether http is safe for this host because nothing routable sits between
/// here and it.
///
/// Loopback, the three RFC 1918 ranges, IPv4 link-local, IPv6 loopback,
/// unique-local and link-local, and the `.local` names Bonjour hands out.
/// Demanding a certificate on those would rule out the ordinary way people run
/// a gateway at home.
fn allows_plain_http(host: &str) -> bool {
    let name = host
        .trim_matches(|c| c == '[' || c == ']')
        .to_lowercase();

    if name == "localhost" || name.ends_with(".localhost") {
        return true;
    }
    if name == "::1" {
        return true;
    }
    if name.ends_with(".local") {
        return true;
    }
    // Unique-local (fc00::/7) and link-local (fe80::/10). The colon test is
    // what keeps a *name* beginning "fd" out of this.
    if name.contains(':')
        && (name.starts_with("fc") || name.starts_with("fd") || name.starts_with("fe80:"))
    {
        return true;
    }

    let octets: Vec<&str> = name.split('.').collect();
    if octets.len() != 4 {
        return false;
    }
    let Ok(numbers) = octets
        .iter()
        .map(|octet| octet.parse::<u8>())
        .collect::<Result<Vec<u8>, _>>()
    else {
        return false;
    };

    match (numbers[0], numbers[1]) {
        (127, _) | (10, _) | (192, 168) | (169, 254) => true,
        (172, 16..=31) => true,
        _ => false,
    }
}

/// A reading that is money (or points) and nothing else.
///
/// Providers like DeepSeek's balance-only mode, Moonshot, Poe and Vercel report
/// a balance, no allowance and no period, so there is **no fraction to draw** —
/// the original puts the money on the rail in place of a ring, which is what
/// this port's detail line carries. `percent_used` stays `None`: a percentage
/// nobody reported is never invented, and a zero here would also be a full red
/// ring and a notification saying the account is spent.
pub fn balance_window(label: &str, text: String) -> UsageWindow {
    UsageWindow::new(label, None).with_kind(crate::model::WindowKind::Balance).with_detail(Some(text))
}

/// A window length as the short heading the panel draws: `7d`, `5h`, `30m`.
///
/// Used only for a length a provider stated that is none of the familiar ones.
/// The original builds the same headings from `windowSeconds` in each service;
/// the port keeps one copy so two providers cannot disagree about what a week
/// is called.
pub fn humanize_window_seconds(seconds: i64) -> String {
    if seconds % 86_400 == 0 {
        format!("{}d", seconds / 86_400)
    } else if seconds % 3_600 == 0 {
        format!("{}h", seconds / 3_600)
    } else {
        format!("{}m", seconds / 60)
    }
}

/// A figure that may arrive as a number or as a numeric string.
///
/// MiniMax's note is why this exists: **the same field is `"96"` in one reply
/// and `75` in another**, so nothing may assume which — reading only one shape
/// would blank a window for the accounts that happen to get the other.
pub fn dig_number(value: Option<&serde_json::Value>) -> Option<f64> {
    match value? {
        serde_json::Value::Number(n) => n.as_f64(),
        serde_json::Value::String(s) => s.trim().parse::<f64>().ok(),
        _ => None,
    }
}

/// `find` without regard to case, for the providers that read a service's own
/// prose rather than its JSON.
///
/// The original compiles every one of its patterns case-insensitively, so a
/// port that matched case-sensitively would lose a line the first time a
/// service changed its capitalisation. Compared a byte at a time rather than
/// through a lower-cased copy: callers slice the original text at the offset
/// this returns, and lower-casing is not length-preserving for every character.
pub fn find_ignoring_case(text: &str, needle: &str) -> Option<usize> {
    let haystack = text.as_bytes();
    let pin = needle.as_bytes();
    if pin.is_empty() {
        return Some(0);
    }
    if pin.len() > haystack.len() {
        return None;
    }
    (0..=haystack.len() - pin.len())
        .find(|start| haystack[*start..*start + pin.len()].eq_ignore_ascii_case(pin))
}


#[cfg(test)]
mod tests {
    struct SelectionProbe {
        id: &'static str,
        discovered: Arc<std::sync::atomic::AtomicUsize>,
        fetched: Arc<std::sync::atomic::AtomicUsize>,
    }
    impl super::Provider for SelectionProbe {
        fn id(&self) -> &'static str { self.id }
        fn name(&self) -> &'static str { self.id }
        fn is_configured(&self) -> bool { self.discovered.fetch_add(1, std::sync::atomic::Ordering::SeqCst); true }
        fn fetch(&self, _: Arc<super::Ctx>) -> super::FetchFuture {
            self.fetched.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            let id = self.id;
            Box::pin(async move { crate::model::ProviderUsage::ok(id, id, vec![crate::model::UsageWindow::new("test", Some(12.0))]) })
        }
    }
    #[tokio::test]
    async fn disabled_provider_never_discovers_credentials_or_fetches() {
        let discovered = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let fetched = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let provider = Arc::new(SelectionProbe { id: "test-only-disabled", discovered: discovered.clone(), fetched: fetched.clone() });
        let result = super::collect_from(Arc::new(super::Ctx::new().unwrap()), &[], vec![provider]).await;
        assert!(result.is_empty());
        assert_eq!(discovered.load(std::sync::atomic::Ordering::SeqCst), 0);
        assert_eq!(fetched.load(std::sync::atomic::Ordering::SeqCst), 0);
    }
    #[tokio::test]
    async fn targeted_collection_only_fetches_selected_provider() {
        let discovered = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let fetched = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let items = ["selected", "disabled"].iter().map(|id| Arc::new(SelectionProbe { id, discovered: discovered.clone(), fetched: fetched.clone() }) as Arc<dyn super::Provider>).collect();
        let result = super::collect_from(Arc::new(super::Ctx::new().unwrap()), &["selected".into()], items).await;
        assert_eq!(result.len(), 1); assert_eq!(result[0].id, "selected");
        assert_eq!(discovered.load(std::sync::atomic::Ordering::SeqCst), 1);
        assert_eq!(fetched.load(std::sync::atomic::Ordering::SeqCst), 1);
    }
    use super::*;

    #[test]
    fn registry_ids_are_unique() {
        let ids: Vec<&str> = registry().iter().map(|p| p.id()).collect();
        let mut sorted = ids.clone();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(ids.len(), sorted.len(), "duplicate provider id in registry");
    }

    /// Every provider answers the presence question, offline.
    ///
    /// This is what a refresh asks each of them **before** any request is made,
    /// so a provider that cannot answer it is a provider that takes the panel
    /// down — and an answer that needed the network would make this test hang
    /// rather than fail, which is the same statement from the other side.
    #[test]
    fn every_provider_answers_whether_it_is_configured() {
        let providers = registry();
        assert!(!providers.is_empty());
        for provider in providers {
            let _ = provider.is_configured();
        }
    }

    #[test]
    fn normalizes_fractions_and_percents() {
        assert_eq!(normalize_percent(0.5), 50.0);
        assert_eq!(normalize_percent(50.0), 50.0);
        assert_eq!(normalize_percent(0.0), 0.0);
        assert_eq!(normalize_percent(150.0), 100.0);
    }

    #[test]
    fn converts_used_fractions_to_percent() {
        assert_eq!(percent_from_fraction(0.25), 25.0);
        assert_eq!(percent_from_fraction(0.0), 0.0);
        assert_eq!(percent_from_fraction(1.0), 100.0);
        // A spend limit can run past its own limit; the ring cannot.
        assert_eq!(percent_from_fraction(1.5), 100.0);
        assert_eq!(percent_from_scale(96.0), 96.0);
        assert_eq!(percent_from_scale(-3.0), 0.0);
    }

    #[test]
    fn sorts_windows_shortest_first() {
        let rows = vec![
            (7 * 86_400, UsageWindow::new("7d", Some(10.0))),
            (5 * 3_600, UsageWindow::new("5h", Some(20.0))),
            (30 * 86_400, UsageWindow::new("Monthly", Some(30.0))),
        ];
        let labels: Vec<String> = by_window_length(rows).into_iter().map(|w| w.label).collect();
        assert_eq!(labels, vec!["5h", "7d", "Monthly"]);
    }

    #[test]
    fn missing_key_names_both_ways_in() {
        let message = missing_key("zai", "");
        assert!(message.contains("PULSEWIN_ZAI_KEY"));
        assert!(message.contains("zai.json"));
    }

    #[test]
    fn http_failure_picks_the_matching_hint() {
        let hints = &[(401u16, " — the key was refused"), (404, " — no plan")];
        assert_eq!(
            http_failure(reqwest::StatusCode::UNAUTHORIZED, hints),
            "HTTP 401 Unauthorized — the key was refused"
        );
        assert_eq!(
            http_failure(reqwest::StatusCode::BAD_GATEWAY, hints),
            "HTTP 502 Bad Gateway"
        );
    }

    #[test]
    fn reads_a_figure_that_may_be_a_string() {
        assert_eq!(dig_number(Some(&serde_json::json!(96.0))), Some(96.0));
        assert_eq!(dig_number(Some(&serde_json::json!("96"))), Some(96.0));
        assert_eq!(dig_number(Some(&serde_json::json!(" 75 "))), Some(75.0));
        assert_eq!(dig_number(Some(&serde_json::json!(true))), None);
        assert_eq!(dig_number(None), None);
    }

    /// The offset is into the text that was passed in, whatever the case of
    /// the characters around it.
    #[test]
    fn finds_without_regard_to_case() {
        assert_eq!(find_ignoring_case("Amp Free: 61%", "amp free:"), Some(0));
        assert_eq!(find_ignoring_case("x**Amp Free:** 61%", "amp free:"), Some(3));
        assert_eq!(find_ignoring_case("nothing here", "amp free:"), None);
        assert_eq!(find_ignoring_case("", "amp"), None);
        assert_eq!(find_ignoring_case("anything", ""), Some(0));
        // A needle longer than the text is not a match, and not a panic.
        assert_eq!(find_ignoring_case("ab", "abc"), None);
    }

    /// The verdict travels with the result: an unfetched provider is not
    /// configured, and a fetched one is — whether it answered or failed.
    #[test]
    fn only_a_fetched_provider_is_configured() {
        let unset = ProviderUsage::unconfigured("zai", "z.ai", not_configured());
        assert!(!unset.configured);
        assert!(!unset.is_configured());
        assert!(unset.windows.is_empty());
        // Still a reason, so the log says why the ring is not there.
        assert_eq!(unset.error.as_deref(), Some(not_configured()));

        let answered = ProviderUsage::ok("zai", "z.ai", vec![UsageWindow::new("5h", Some(12.0))]);
        assert!(answered.configured);
        assert!(answered.is_configured());

        // A refusal is still this tool's card: the credential is here, the
        // service is what said no.
        let refused = ProviderUsage::failed("zai", "z.ai", "HTTP 401 Unauthorized");
        assert!(refused.configured);
        assert!(refused.is_configured());
    }

    /// The address a gateway is pointed at carries a key, so it is checked
    /// rather than trusted.
    #[test]
    fn a_gateway_address_is_checked_rather_than_trusted() {
        // A scheme is assumed, and assumed https.
        assert_eq!(
            gateway_url("gateway.example.com", "/key/info", &[]).as_deref(),
            Some("https://gateway.example.com/key/info")
        );
        // A base URL pasted out of a client's config ends in `/v1`, and the
        // route sits beside it rather than under it.
        assert_eq!(
            gateway_url("https://gw.example.com/v1", "/key/info", &["/v1"]).as_deref(),
            Some("https://gw.example.com/key/info")
        );

        // http is refused on a public host rather than quietly upgraded…
        assert!(gateway_url("http://gw.example.com", "/", &[]).is_none());
        // …and allowed where nothing routable can sit in between.
        assert!(gateway_url("http://localhost:8080", "/key/info", &[]).is_some());
        assert_eq!(
            gateway_url("http://127.0.0.1:4000", "/key/info", &[]).as_deref(),
            Some("http://127.0.0.1:4000/key/info")
        );
        assert!(gateway_url("http://192.168.1.10", "/", &[]).is_some());
        assert!(gateway_url("http://172.16.5.4", "/", &[]).is_some());
        assert!(gateway_url("http://gw.local", "/", &[]).is_some());
        assert!(gateway_url("http://gw.example.com", "/", &[]).is_none());

        // The three ways of writing a URL whose host is not the part the eye
        // lands on, and a scheme that is not one.
        assert!(gateway_url("https://user:pw@gw.example.com", "/", &[]).is_none());
        assert!(gateway_url("https://gw.example.com/?next=evil", "/", &[]).is_none());
        assert!(gateway_url("https://gw.example.com/#x", "/", &[]).is_none());
        assert!(gateway_url("ftp://gw.example.com", "/", &[]).is_none());
        assert!(!gateway_address_usable(""));
    }

    #[test]
    fn the_longest_trimmed_suffix_wins() {
        let trimmed = gateway_url(
            "https://gw.example.com/api/v1/",
            "/usage",
            &["/v1", "/api/v1"],
        );
        assert_eq!(trimmed.as_deref(), Some("https://gw.example.com/usage"));
        // A suffix that is not there is not stripped.
        assert_eq!(
            gateway_url("https://gw.example.com/v1/usage", "/x", &["/v1"]).as_deref(),
            Some("https://gw.example.com/v1/usage/x")
        );
    }
}
