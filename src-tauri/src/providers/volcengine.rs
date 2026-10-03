//! The Volcengine Coding Plan, sold on Volcengine Ark.
//!
//! Read with an access key pair the user pastes as **one string holding two
//! secrets**: `AccessKeyID:SecretAccessKey`, split on the *first* colon so a
//! secret containing a colon survives. It is signed against Volcengine's Top
//! OpenAPI, whose signature is their spelling of AWS SigV4:
//!
//! - `GET https://open.volcengineapi.com/?Action=GetCodingPlanUsage&Version=2024-01-01`
//!   answers with the levels the plan reports as used percentages.
//! - `GET …?Action=GetAFPUsage&Version=2024-01-01` ("Agent Flow Points")
//!   answers with a quota and a used figure per named window.
//!
//! An account can hold both plans and the two actions are independent, so a
//! refusal on one plan the account simply does not hold must not take the
//! other's figures down with it. When **both** fail, the more authoritative
//! refusal wins: a wrong key must be answered with "refused" rather than with
//! whichever request happened to lose the race.
//!
//! **The `arkcli` route is deliberately not ported.** Volcengine's own CLI
//! answers with the same plans, but it is a macOS/Linux binary reached through
//! a bounded-subprocess runner this port has no equivalent of, and its ambient
//! SSO session can be signed in to a different account than the keys — which is
//! exactly why the original prefers configured keys over it. With the CLI
//! absent there is one route and no ambiguity about which account is drawn.
//!
//! **A third route was left out by the original and stays out.** Ark returns
//! `x-ratelimit-remaining-requests` on a chat completion, and reading it means
//! *sending a completion*, so every refresh would spend a piece of the quota it
//! is measuring. A request-rate throttle is not the coding plan's quota either.
//!
//! **Not verified against a live account.** The reply shapes are second-hand
//! from the reference implementation's parser and its tests rather than
//! measured, which is why the parsing is covered by fixtures and the failure
//! copy is specific.

use std::sync::Arc;
use std::time::Duration;

use chrono::{DateTime, SecondsFormat, Utc};
use serde_json::Value;

use super::{
    by_window_length, describe_reqwest_error, percent_from_fraction, Ctx, FetchFuture, Provider,
};
use crate::model::{ProviderUsage, UsageWindow};

const CODING_PLAN_URL: &str =
    "https://open.volcengineapi.com/?Action=GetCodingPlanUsage&Version=2024-01-01";
const AGENT_PLAN_URL: &str = "https://open.volcengineapi.com/?Action=GetAFPUsage&Version=2024-01-01";

pub struct Volcengine;

impl Provider for Volcengine {
    fn id(&self) -> &'static str {
        "volcengine"
    }

    fn name(&self) -> &'static str {
        "Volcengine"
    }

    /// The pasted key pair, and nothing else. No network call.
    fn is_configured(&self) -> bool {
        super::provider_key(self.id()).is_some()
    }

    fn fetch(&self, ctx: Arc<Ctx>) -> FetchFuture {
        Box::pin(async move { fetch_inner(ctx).await })
    }
}

async fn fetch_inner(ctx: Arc<Ctx>) -> ProviderUsage {
    const ID: &str = "volcengine";
    const NAME: &str = "Volcengine";

    let entered = match super::provider_key(ID) {
        Some(entered) => entered,
        None => return ProviderUsage::failed(ID, NAME, super::missing_key(ID, "")),
    };

    let Some(credentials) = Credentials::parse(&entered) else {
        return ProviderUsage::failed(
            ID,
            NAME,
            "the pasted key is not an AccessKeyID:SecretAccessKey pair",
        );
    };

    // Both actions are asked for at once: neither depends on the other, and
    // the panel is waiting on the slower of the two either way.
    let (coding, agent) = futures::future::join(
        ask(&ctx, CODING_PLAN_URL, &credentials, coding_windows),
        ask(&ctx, AGENT_PLAN_URL, &credentials, agent_windows),
    )
    .await;

    if let (Err(coding_refusal), Err(agent_refusal)) = (&coding, &agent) {
        let reason = if coding_refusal.rank() >= agent_refusal.rank() {
            coding_refusal
        } else {
            agent_refusal
        };
        return ProviderUsage::failed(ID, NAME, reason.message());
    }

    let mut windows = coding.unwrap_or_default();
    windows.extend(agent.unwrap_or_default());
    if windows.is_empty() {
        return ProviderUsage::failed(ID, NAME, "no limits reported");
    }

    ProviderUsage::ok(ID, NAME, windows)
}

/// Why one signed action did not answer.
///
/// Kept as meaning rather than as text because one rule needs to tell them
/// apart: of two failures, a refusal or a throttle is the one to report, since
/// it is the service having answered the key at all.
#[derive(Clone, PartialEq, Eq, Debug)]
enum Refusal {
    ApiKeyRefused,
    RateLimited,
    /// 404 is how an account that does not hold this plan answers.
    NoPlan,
    /// The request never arrived, with reqwest's own cause chain: a DNS
    /// failure and a TLS handshake are the same verdict and different problems.
    Unreachable(String),
    ServerError,
    Unreadable,
}

impl Refusal {
    /// The higher, the more it says: a refusal means the keys were read and
    /// turned away, and an unreachable service says nothing about them.
    fn rank(&self) -> u8 {
        match self {
            Refusal::ApiKeyRefused | Refusal::RateLimited => 2,
            Refusal::NoPlan | Refusal::ServerError | Refusal::Unreadable => 1,
            Refusal::Unreachable(_) => 0,
        }
    }

    fn message(&self) -> String {
        match self {
            Refusal::ApiKeyRefused => "the access key pair was refused".to_string(),
            Refusal::RateLimited => "rate limited, try again shortly".to_string(),
            Refusal::NoPlan => "no limits reported".to_string(),
            Refusal::Unreachable(detail) => format!("cannot reach the service: {detail}"),
            Refusal::ServerError => "the service reported a failure".to_string(),
            Refusal::Unreadable => "bad reply: this build cannot read the usage".to_string(),
        }
    }
}

/// One signed action, and the windows its reply names.
async fn ask(
    ctx: &Ctx,
    url: &str,
    credentials: &Credentials,
    decode: fn(&[u8]) -> Option<Vec<UsageWindow>>,
) -> Result<Vec<UsageWindow>, Refusal> {
    let Some(signed) = headers("GET", url, &[], CONTENT_TYPE, credentials, Utc::now()) else {
        return Err(Refusal::Unreachable(format!("`{url}` is not an address")));
    };

    let mut request = ctx.client.get(url).timeout(Duration::from_secs(12));
    for (name, value) in signed {
        request = request.header(name, value);
    }

    let response = request
        .send()
        .await
        .map_err(|e| Refusal::Unreachable(describe_reqwest_error(&e)))?;
    let status = response.status();
    let body = response
        .bytes()
        .await
        .map_err(|e| Refusal::Unreachable(e.to_string()))?;

    match status.as_u16() {
        200 => {}
        401 | 403 => return Err(Refusal::ApiKeyRefused),
        429 => return Err(Refusal::RateLimited),
        404 => return Err(Refusal::NoPlan),
        _ => return Err(Refusal::ServerError),
    }

    decode(&body).ok_or(Refusal::Unreadable)
}

// ---------------------------------------------------------------------------
// The pasted pair
// ---------------------------------------------------------------------------

/// Where the account lives. Beijing is where the Ark coding plan is sold, and
/// it is a constant rather than a setting because a wrong region fails as a
/// signature mismatch — the least helpful error a settings field could produce.
const REGION: &str = "cn-beijing";

/// The two secrets one pasted string holds.
struct Credentials {
    access_key_id: String,
    secret_access_key: String,
}

impl Credentials {
    /// `AccessKeyID:SecretAccessKey`, split on the first colon and trimmed.
    /// Half a pair is no pair.
    fn parse(entered: &str) -> Option<Credentials> {
        let entered = entered.trim();
        let (id, secret) = entered.split_once(':')?;
        let id = id.trim();
        let secret = secret.trim();
        if id.is_empty() || secret.is_empty() {
            return None;
        }
        Some(Credentials {
            access_key_id: id.to_string(),
            secret_access_key: secret.to_string(),
        })
    }
}

// ---------------------------------------------------------------------------
// Reading the replies
// ---------------------------------------------------------------------------

/// Which plan a window belongs to, as the name shown after its length —
/// "Weekly · Coding Plan". Product names, so never translated.
fn shape(label: &str, plan: &'static str) -> Option<(i64, String)> {
    let (name, seconds) = match label.to_lowercase().as_str() {
        "5h" | "5-hour" | "five_hour" | "session" => ("5h", 5 * 3_600),
        "weekly" | "week" => ("Weekly", 7 * 86_400),
        // A month is 28 to 31 days, so thirty is a sort key and not a
        // measurement: nothing divides by it.
        "monthly" | "month" => ("Monthly", 30 * 86_400),
        _ => return None,
    };
    Some((seconds, format!("{name} · {plan}")))
}

/// One window, from the label the reply states and a figure already as 0..100.
///
/// A label this build cannot name is **left out rather than guessed at**: its
/// length is stated nowhere, so neither its row nor its clock could be honest.
/// Ark reports no "you are blocked" flag of its own, so the only signal that a
/// window is spent is its own figure reaching its own ceiling.
fn window(
    label: &str,
    plan: &'static str,
    used_percent: f64,
    resets_at: Option<String>,
) -> Option<(i64, UsageWindow)> {
    let (seconds, name) = shape(label, plan)?;
    if !used_percent.is_finite() {
        return None;
    }
    let used = (used_percent / 100.0).clamp(0.0, 1.0);
    Some((
        seconds,
        UsageWindow::new(name, Some(percent_from_fraction(used))).with_reset(resets_at),
    ))
}

/// `GetCodingPlanUsage`: a level and a percentage per entry, already *used* —
/// no inversion here, unlike a "remaining" figure.
///
/// A reclaimed or inactive plan answers with a status and no `QuotaUsage` at
/// all. That is "nothing to report", not a malformed reply, and it must not
/// fail the Agent Plan beside it — which is why a missing list reads as empty
/// while a missing `Result` does not read at all.
fn coding_windows(body: &[u8]) -> Option<Vec<UsageWindow>> {
    let reply: Value = serde_json::from_slice(body).ok()?;
    let result = reply.get("Result")?;

    let mut rows: Vec<(i64, UsageWindow)> = Vec::new();
    for quota in result
        .get("QuotaUsage")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or_default()
    {
        let Some(level) = quota.get("Level").and_then(Value::as_str) else {
            continue;
        };
        let Some(percent) = number(quota.get("Percent")) else {
            continue;
        };
        if let Some(row) = window(
            level,
            "Coding Plan",
            percent,
            stamp(number(quota.get("ResetTimestamp"))),
        ) {
            rows.push(row);
        }
    }
    Some(by_window_length(rows))
}

/// `GetAFPUsage`: quota and used rather than a percentage, and its windows
/// named in the keys instead of in a list.
///
/// A quota of zero is not a full window, it is a plan that has no such window;
/// dividing by it would report 100% used of nothing. `AFPDaily` has no slot on
/// a ring and is deliberately not mapped.
fn agent_windows(body: &[u8]) -> Option<Vec<UsageWindow>> {
    let reply: Value = serde_json::from_slice(body).ok()?;
    let result = reply.get("Result")?;

    let named = [
        ("5h", "AFPFiveHour"),
        ("weekly", "AFPWeekly"),
        ("monthly", "AFPMonthly"),
    ];

    let mut rows: Vec<(i64, UsageWindow)> = Vec::new();
    for (label, key) in named {
        let Some(reported) = result.get(key) else {
            continue;
        };
        let Some(quota) = number(reported.get("Quota")).filter(|quota| *quota > 0.0) else {
            continue;
        };
        let Some(used) = number(reported.get("Used")) else {
            continue;
        };
        if let Some(row) = window(
            label,
            "Agent Plan",
            used / quota * 100.0,
            stamp(number(reported.get("ResetTime"))),
        ) {
            rows.push(row);
        }
    }
    Some(by_window_length(rows))
}

fn number(value: Option<&Value>) -> Option<f64> {
    value.and_then(Value::as_f64).filter(|value| value.is_finite())
}

/// A timestamp the reply states, as RFC 3339.
///
/// It has shipped as seconds and as milliseconds, and guessing by magnitude is
/// safe: 1e11 seconds is the year 5138 and 1e11 milliseconds is 1973, so
/// nothing real sits near the boundary.
fn stamp(value: Option<f64>) -> Option<String> {
    let value = value.filter(|value| value.is_finite() && *value > 0.0)?;
    let seconds = if value >= 1e11 { value / 1000.0 } else { value };
    let at = DateTime::from_timestamp(seconds.floor() as i64, 0)?;
    Some(at.to_rfc3339_opts(SecondsFormat::Secs, true))
}

// ---------------------------------------------------------------------------
// The signature
// ---------------------------------------------------------------------------
//
// Volcengine's published scheme, and nothing here is this port's opinion. It
// sits in this file rather than a module of its own because there is no second
// caller: a signature is either exactly right or a 403 with nothing to read.
//
// Two details cost more than they look:
//
// - The signed header list is **sorted, lower-cased**, and the canonical
//   request has to list them in that same order. The server re-sorts and
//   recomputes; anything else is a signature mismatch.
// - The credential scope is `<date>/<region>/ark/request`. `request`, not AWS's
//   `aws4_request`, and `ark` for this service.

const ALGORITHM: &str = "HMAC-SHA256";
const SERVICE: &str = "ark";
const TERMINATOR: &str = "request";
const SIGNED_HEADERS: &str = "content-type;host;x-content-sha256;x-date";
const CONTENT_TYPE: &str = "application/x-www-form-urlencoded; charset=utf-8";

/// The headers a signed request needs, `Authorization` among them, or `None`
/// when there is no address to sign for.
fn headers(
    method: &str,
    url: &str,
    body: &[u8],
    content_type: &str,
    credentials: &Credentials,
    date: DateTime<Utc>,
) -> Option<Vec<(&'static str, String)>> {
    let parsed = reqwest::Url::parse(url).ok()?;
    let host = parsed.host_str()?.to_string();

    let timestamp = date.format("%Y%m%dT%H%M%SZ").to_string();
    let day = date.format("%Y%m%d").to_string();
    let payload_hash = hex(&sha256(body));
    let canonical_request = [
        method.to_string(),
        canonical_path(&parsed),
        canonical_query(&parsed),
        format!("content-type:{content_type}"),
        format!("host:{host}"),
        format!("x-content-sha256:{payload_hash}"),
        format!("x-date:{timestamp}"),
        String::new(),
        SIGNED_HEADERS.to_string(),
        payload_hash.clone(),
    ]
    .join("\n");

    let scope = format!("{day}/{REGION}/{SERVICE}/{TERMINATOR}");
    let string_to_sign = [
        ALGORITHM.to_string(),
        timestamp.clone(),
        scope.clone(),
        hex(&sha256(canonical_request.as_bytes())),
    ]
    .join("\n");

    // The signing key is derived in four steps, each one keyed by the last.
    // Deriving it per request rather than caching is deliberate: it is scoped
    // to the day and the region, and a cache of it would be a secret with a
    // lifetime nobody is tracking.
    let mut key = hmac_sha256(credentials.secret_access_key.as_bytes(), day.as_bytes());
    key = hmac_sha256(&key, REGION.as_bytes());
    key = hmac_sha256(&key, SERVICE.as_bytes());
    key = hmac_sha256(&key, TERMINATOR.as_bytes());
    let signature = hex(&hmac_sha256(&key, string_to_sign.as_bytes()));

    Some(vec![
        ("Content-Type", content_type.to_string()),
        ("Host", host),
        ("X-Date", timestamp),
        ("X-Content-Sha256", payload_hash),
        (
            "Authorization",
            format!(
                "{ALGORITHM} Credential={}/{scope}, SignedHeaders={SIGNED_HEADERS}, Signature={signature}",
                credentials.access_key_id,
            ),
        ),
    ])
}

/// The path, decoded and then encoded by the signature's rules rather than the
/// URL's — the original re-encodes what `URL.path` hands it.
fn canonical_path(url: &reqwest::Url) -> String {
    let decoded = percent_decode(url.path());
    let path: &str = if decoded.is_empty() { "/" } else { &decoded };
    encode(path, true)
}

/// Sorted by name, then by value, each half encoded separately — the server
/// rebuilds this string from its own parse of the query, so the order in the
/// URL is not the order that is signed.
fn canonical_query(url: &reqwest::Url) -> String {
    let mut pairs: Vec<(String, String)> = url
        .query_pairs()
        .map(|(name, value)| (encode(&name, false), encode(&value, false)))
        .collect();
    pairs.sort();
    pairs
        .into_iter()
        .map(|(name, value)| format!("{name}={value}"))
        .collect::<Vec<String>>()
        .join("&")
}

/// Percent-encoding by the signature's rules, not the URL's: everything but the
/// unreserved set, and `~` is **not** escaped.
///
/// **Built from ASCII by hand.** A Unicode-wide "alphanumeric" test would leave
/// a non-ASCII letter in a query value unescaped here and let the server escape
/// it before recomputing — an unexplainable 403 the first time anything but
/// `Action` and `Version` is signed.
fn encode(value: &str, keep_slashes: bool) -> String {
    let mut out = String::with_capacity(value.len());
    for byte in value.as_bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~') {
            out.push(*byte as char);
        } else if keep_slashes && *byte == b'/' {
            out.push('/');
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

/// What a URL's per-cent escapes stand for, so the path can be re-encoded by
/// the signature's rules.
fn percent_decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut at = 0;
    while at < bytes.len() {
        if bytes[at] == b'%' {
            if let Some(hex) = text.get(at + 1..at + 3) {
                if let Ok(byte) = u8::from_str_radix(hex, 16) {
                    out.push(byte);
                    at += 3;
                    continue;
                }
            }
        }
        out.push(bytes[at]);
        at += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn hex(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push_str(&format!("{byte:02x}"));
    }
    out
}

// ---------------------------------------------------------------------------
// SHA-256 and HMAC-SHA256
// ---------------------------------------------------------------------------
//
// Written out here because the crate has no cryptography dependency and this is
// the only thing in the port that needs one. FIPS 180-4, and nothing more than
// the one digest the signature uses — pinned by the published test vectors and
// by the cross-checked signature below.

const K: [u32; 64] = [
    0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4, 0xab1c5ed5,
    0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe, 0x9bdc06a7, 0xc19bf174,
    0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f, 0x4a7484aa, 0x5cb0a9dc, 0x76f988da,
    0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7, 0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967,
    0x27b70a85, 0x2e1b2138, 0x4d2c6dfc, 0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85,
    0xa2bfe8a1, 0xa81a664b, 0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070,
    0x19a4c116, 0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
    0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7, 0xc67178f2,
];

fn sha256(data: &[u8]) -> [u8; 32] {
    let mut state: [u32; 8] = [
        0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab,
        0x5be0cd19,
    ];

    let mut message = data.to_vec();
    let bits = (data.len() as u64).wrapping_mul(8);
    message.push(0x80);
    while message.len() % 64 != 56 {
        message.push(0);
    }
    message.extend_from_slice(&bits.to_be_bytes());

    for block in message.chunks_exact(64) {
        let mut w = [0u32; 64];
        for (index, word) in w.iter_mut().take(16).enumerate() {
            *word = u32::from_be_bytes([
                block[index * 4],
                block[index * 4 + 1],
                block[index * 4 + 2],
                block[index * 4 + 3],
            ]);
        }
        for index in 16..64 {
            let s0 = w[index - 15].rotate_right(7)
                ^ w[index - 15].rotate_right(18)
                ^ (w[index - 15] >> 3);
            let s1 = w[index - 2].rotate_right(17)
                ^ w[index - 2].rotate_right(19)
                ^ (w[index - 2] >> 10);
            w[index] = w[index - 16]
                .wrapping_add(s0)
                .wrapping_add(w[index - 7])
                .wrapping_add(s1);
        }

        let [mut a, mut b, mut c, mut d, mut e, mut f, mut g, mut h] = state;
        for index in 0..64 {
            let s1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
            let choose = (e & f) ^ ((!e) & g);
            let temp1 = h
                .wrapping_add(s1)
                .wrapping_add(choose)
                .wrapping_add(K[index])
                .wrapping_add(w[index]);
            let s0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
            let majority = (a & b) ^ (a & c) ^ (b & c);
            let temp2 = s0.wrapping_add(majority);

            h = g;
            g = f;
            f = e;
            e = d.wrapping_add(temp1);
            d = c;
            c = b;
            b = a;
            a = temp1.wrapping_add(temp2);
        }

        state[0] = state[0].wrapping_add(a);
        state[1] = state[1].wrapping_add(b);
        state[2] = state[2].wrapping_add(c);
        state[3] = state[3].wrapping_add(d);
        state[4] = state[4].wrapping_add(e);
        state[5] = state[5].wrapping_add(f);
        state[6] = state[6].wrapping_add(g);
        state[7] = state[7].wrapping_add(h);
    }

    let mut out = [0u8; 32];
    for (index, word) in state.iter().enumerate() {
        out[index * 4..index * 4 + 4].copy_from_slice(&word.to_be_bytes());
    }
    out
}

fn hmac_sha256(key: &[u8], message: &[u8]) -> [u8; 32] {
    const BLOCK: usize = 64;

    let mut padded = [0u8; BLOCK];
    if key.len() > BLOCK {
        padded[..32].copy_from_slice(&sha256(key));
    } else {
        padded[..key.len()].copy_from_slice(key);
    }

    let mut inner = Vec::with_capacity(BLOCK + message.len());
    inner.extend(padded.iter().map(|byte| byte ^ 0x36));
    inner.extend_from_slice(message);
    let digest = sha256(&inner);

    let mut outer = Vec::with_capacity(BLOCK + 32);
    outer.extend(padded.iter().map(|byte| byte ^ 0x5c));
    outer.extend_from_slice(&digest);
    sha256(&outer)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The original's fixed clock and key, so its cross-checked signature is
    /// this port's test vector too.
    fn credentials() -> Credentials {
        Credentials {
            access_key_id: "AKLTTestAccessKeyId".to_string(),
            secret_access_key: "dGVzdC1zZWNyZXQtYWNjZXNzLWtleQ==".to_string(),
        }
    }

    /// 2026-09-07T09:30:00Z, so the day stamp and the timestamp are both fixed.
    fn date() -> DateTime<Utc> {
        DateTime::from_timestamp(1_788_773_400, 0).unwrap()
    }

    fn signed(url: &str, body: &[u8]) -> Vec<(&'static str, String)> {
        headers("GET", url, body, CONTENT_TYPE, &credentials(), date()).unwrap()
    }

    fn value(headers: &[(&'static str, String)], name: &str) -> String {
        headers
            .iter()
            .find(|(field, _)| field.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.clone())
            .unwrap_or_else(|| panic!("no {name} header"))
    }

    /// A percentage as the arithmetic lands, not as the decimal is written:
    /// `41.5` under a divide-and-multiply is not bit for bit `41.5`.
    fn percent(window: &UsageWindow, expected: f64) -> bool {
        matches!(window.percent_used, Some(value) if (value - expected).abs() < 1e-9)
    }

    // ---- the signature -----------------------------------------------------

    #[test]
    fn the_digest_is_the_published_one() {
        // FIPS 180-4's own vectors, so a transcription slip in the constants
        // fails here rather than as a 403 in the field.
        assert_eq!(
            hex(&sha256(b"")),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
        assert_eq!(
            hex(&sha256(b"abc")),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        // RFC 4231's first HMAC case.
        assert_eq!(
            hex(&hmac_sha256(&[0x0b; 20], b"Hi There")),
            "b0344c61d8db38535ca8afceaf0bf12b881dc200c9833da726e9376c2e32cff7"
        );
    }

    #[test]
    fn the_stamps_are_utc_and_the_scope_is_volcengines() {
        let headers = signed(CODING_PLAN_URL, &[]);
        assert_eq!(value(&headers, "X-Date"), "20260907T093000Z");
        let authorization = value(&headers, "Authorization");
        assert!(authorization.starts_with("HMAC-SHA256 Credential="));
        assert!(authorization
            .contains("Credential=AKLTTestAccessKeyId/20260907/cn-beijing/ark/request"));
        // `request`, not AWS's `aws4_request`.
        assert!(!authorization.contains("aws4_request"));
        assert!(authorization.contains("SignedHeaders=content-type;host;x-content-sha256;x-date,"));
        assert_eq!(value(&headers, "Host"), "open.volcengineapi.com");
        // An empty body is hashed, not skipped.
        assert_eq!(
            value(&headers, "X-Content-Sha256"),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
    }

    /// **Not copied out of this implementation.** The reference
    /// implementation's suite records this signature as having been computed by
    /// a separate implementation written from the scheme in another language,
    /// so the two agreeing is evidence about the algorithm rather than a
    /// photograph of whatever this code happens to do. What it still cannot say
    /// is that Volcengine accepts it; only an account can.
    #[test]
    fn the_whole_header_agrees_with_a_second_implementation() {
        assert_eq!(
            value(&signed(CODING_PLAN_URL, &[]), "Authorization"),
            "HMAC-SHA256 Credential=AKLTTestAccessKeyId/20260907/cn-beijing/ark/request, \
             SignedHeaders=content-type;host;x-content-sha256;x-date, \
             Signature=3bc6ebb4fd6da065cae0c05dbfc35285cdced26090d7c0aea87b2f2330cd031d"
        );
    }

    /// The server rebuilds the query string from its own parse, so the order in
    /// the URL is not the order that is signed.
    #[test]
    fn the_query_is_canonicalised() {
        let forwards = "https://open.volcengineapi.com/?Action=A&Version=B";
        let backwards = "https://open.volcengineapi.com/?Version=B&Action=A";
        assert_eq!(
            value(&signed(forwards, &[]), "Authorization"),
            value(&signed(backwards, &[]), "Authorization")
        );
    }

    /// A tilde is left alone and everything outside the unreserved set is
    /// escaped; a stock character set gets both of those wrong.
    #[test]
    fn the_encoding_rules_are_the_signatures() {
        assert_eq!(encode("a~b", false), "a~b");
        assert_eq!(encode("a b", false), "a%20b");
        assert_eq!(encode("a/b", true), "a/b");
        assert_eq!(encode("a/b", false), "a%2Fb");
        assert_eq!(encode("ü", false), "%C3%BC");
        assert_eq!(percent_decode("a%20b"), "a b");
        assert_eq!(percent_decode("a%2"), "a%2");
        assert_eq!(percent_decode("a%zz"), "a%zz");

        // Different inputs, different signatures: the query reached the
        // canonical form rather than being dropped.
        let tilde = value(
            &signed("https://open.volcengineapi.com/?Action=a~b", &[]),
            "Authorization",
        );
        let space = value(
            &signed("https://open.volcengineapi.com/?Action=a%20b", &[]),
            "Authorization",
        );
        assert_ne!(tilde, space);
        assert_ne!(tilde, value(&signed(CODING_PLAN_URL, &[]), "Authorization"));
    }

    #[test]
    fn a_different_body_changes_the_signature() {
        let empty = value(&signed(CODING_PLAN_URL, &[]), "Authorization");
        let filled = value(&signed(CODING_PLAN_URL, b"{}"), "Authorization");
        assert_ne!(empty, filled);
    }

    // ---- the pasted pair ---------------------------------------------------

    #[test]
    fn the_key_field_is_split_on_the_first_colon() {
        let parsed = Credentials::parse(" AKLTabc : secret:with:colons ").unwrap();
        assert_eq!(parsed.access_key_id, "AKLTabc");
        // A secret containing colons survives, which splitting on the last —
        // or on all of them — would not.
        assert_eq!(parsed.secret_access_key, "secret:with:colons");
    }

    #[test]
    fn half_a_pair_is_no_pair() {
        for entered in ["", "   ", "AKLTabc", "AKLTabc:", ":secret", ":"] {
            assert!(Credentials::parse(entered).is_none(), "accepted {entered}");
        }
    }

    // ---- GetCodingPlanUsage ------------------------------------------------

    /// Second-hand, from the original's fixture.
    fn coding_plan() -> Vec<u8> {
        br#"{
          "ResponseMetadata": { "RequestId": "fixture" },
          "Result": {
            "Status": "Active",
            "UpdateTimestamp": 1788773400,
            "QuotaUsage": [
              { "Level": "weekly", "Percent": 41.5, "ResetTimestamp": 1789085506 },
              { "Level": "5h", "Percent": 12, "ResetTimestamp": 1788780000 },
              { "Level": "fortnightly", "Percent": 3, "ResetTimestamp": 1789085506 }
            ]
          }
        }"#
        .to_vec()
    }

    #[test]
    fn reads_the_coding_plans_levels_shortest_first() {
        let windows = coding_windows(&coding_plan()).unwrap();
        let labels: Vec<&str> = windows.iter().map(|w| w.label.as_str()).collect();
        // "fortnightly" is a length this build has no name for: left out
        // rather than guessed at.
        assert_eq!(
            labels,
            vec!["5h · Coding Plan", "Weekly · Coding Plan"]
        );
        // `Percent` is what is used — no inversion here.
        assert!(percent(&windows[0], 12.0));
        assert!(percent(&windows[1], 41.5));
        assert_eq!(
            windows[0].resets_at.as_deref(),
            Some("2026-09-07T11:20:00Z")
        );
        assert_eq!(
            windows[1].resets_at.as_deref(),
            Some("2026-09-11T00:11:46Z")
        );
    }

    /// A reclaimed plan answers with a status and no quota, which is not a
    /// failure — and must not fail the Agent Plan beside it.
    #[test]
    fn a_coding_plan_without_a_quota_reports_nothing_rather_than_failing() {
        assert_eq!(
            coding_windows(br#"{"Result":{"Status":"Reclaimed"}}"#),
            Some(Vec::new())
        );
    }

    // ---- GetAFPUsage -------------------------------------------------------

    /// Second-hand, from the original's fixture.
    fn agent_plan() -> Vec<u8> {
        br#"{
          "Result": {
            "AFPFiveHour": { "Quota": 1000, "Used": 250, "ResetTime": 1788780000 },
            "AFPWeekly": { "Quota": 20000, "Used": 0, "ResetTime": 1789085506 },
            "AFPMonthly": { "Quota": 0, "Used": 0, "ResetTime": 0 },
            "AFPDaily": { "Quota": 500, "Used": 100, "ResetTime": 1788800000 }
          }
        }"#
        .to_vec()
    }

    #[test]
    fn reads_the_agent_plans_quota_and_used_shortest_first() {
        let windows = agent_windows(&agent_plan()).unwrap();
        let labels: Vec<&str> = windows.iter().map(|w| w.label.as_str()).collect();
        assert_eq!(
            labels,
            vec!["5h · Agent Plan", "Weekly · Agent Plan"]
        );
        assert!(percent(&windows[0], 25.0));
        assert!(percent(&windows[1], 0.0));
        // A quota of zero is a window the plan has not got, not a full one:
        // dividing by it would report 100% used of nothing. `AFPDaily` has no
        // slot on a ring and is deliberately not mapped.
        assert!(!labels.iter().any(|label| label.contains("Monthly")));
        assert!(!labels.iter().any(|label| label.contains("Daily")));
        assert_eq!(
            windows[0].resets_at.as_deref(),
            Some("2026-09-07T11:20:00Z")
        );
    }

    #[test]
    fn nonsense_is_none_rather_than_an_empty_reading() {
        assert!(coding_windows(b"not json").is_none());
        assert!(agent_windows(b"not json").is_none());
        // No `Result` at all is not a readable reply.
        assert!(coding_windows(br#"{"ResponseMetadata":{}}"#).is_none());
        assert!(agent_windows(br#"{"ResponseMetadata":{}}"#).is_none());
    }

    #[test]
    fn a_stamp_is_read_in_seconds_and_in_milliseconds() {
        assert_eq!(
            stamp(Some(1_788_773_400.0)),
            stamp(Some(1_788_773_400_000.0))
        );
        assert_eq!(stamp(Some(1_788_773_400.0)).as_deref(), Some("2026-09-07T09:30:00Z"));
        // Zero and less is no reset, not the epoch.
        assert_eq!(stamp(Some(0.0)), None);
        assert_eq!(stamp(Some(-5.0)), None);
        assert_eq!(stamp(None), None);
    }

    #[test]
    fn an_unknown_label_is_left_off() {
        assert!(window("fortnightly", "Coding Plan", 10.0, None).is_none());
        assert!(window("daily", "Agent Plan", 10.0, None).is_none());
        // A figure that is not a figure is not a window either.
        assert!(window("weekly", "Coding Plan", f64::NAN, None).is_none());
        // And a percentage past its ceiling clamps to a full ring.
        let (_, window) = window("weekly", "Coding Plan", 130.0, None).unwrap();
        assert!(percent(&window, 100.0));
    }
}
