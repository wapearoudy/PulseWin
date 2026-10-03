//! Windsurf's daily and weekly quota, each a percentage remaining that Windsurf
//! states, read from the endpoint its own profile page calls:
//! `POST https://windsurf.com/_backend/exa.seat_management_pb.SeatManagementService/GetPlanStatus`,
//! Connect over protobuf.
//!
//! **Not Devin's route, and deliberately so.** Windsurf now belongs to
//! Cognition, and `devin.rs` already reads both of the other ways to this plan:
//! the Devin/Windsurf app's saved plan in `state.vscdb`, and `app.devin.ai`'s
//! quota endpoint with a session read out of the browser. Reading either again
//! here would be the same account counted twice under two names. This one is
//! windsurf.com's own endpoint.
//!
//! **The credential is not a cookie.** windsurf.com keeps its session in the
//! browser's `localStorage` as four `devin_*` values rather than in a cookie,
//! so the original reads them out of a Chromium profile
//! (`Auth/ChromiumLocalStorage.swift`) and saves all four as one JSON object.
//! PulseWin cannot read a Chromium profile, so the same object is pasted:
//!
//! ```text
//! {"devin_session_token": "…", "devin_auth1_token": "…",
//!  "devin_account_id": "…", "devin_primary_org_id": "…"}
//! ```
//!
//! Any of the four missing and there is no session. They are sent only to
//! windsurf.com.
//!
//! The shape is second-hand — field numbers from the original and its tests,
//! which took them from Windsurf's bundled protobuf metadata — and the fixture
//! in the tests says so.

use std::sync::Arc;

use serde_json::{Map, Value};

use super::{describe_reqwest_error, percent_from_fraction, Ctx, FetchFuture, Provider};
use crate::model::{ProviderUsage, UsageWindow};

const ID: &str = "windsurf";
const NAME: &str = "Windsurf";

const ORIGIN: &str = "https://windsurf.com";
const ENDPOINT: &str =
    "https://windsurf.com/_backend/exa.seat_management_pb.SeatManagementService/GetPlanStatus";

/// The four values windsurf.com keeps in `localStorage`, all of which the
/// endpoint is sent.
const STORAGE_KEYS: [&str; 4] = [
    "devin_session_token",
    "devin_auth1_token",
    "devin_account_id",
    "devin_primary_org_id",
];

pub struct Windsurf;

impl Provider for Windsurf {
    fn id(&self) -> &'static str {
        ID
    }

    fn name(&self) -> &'static str {
        NAME
    }

    /// Whether a pasted session naming all four values is on the machine. No
    /// network call.
    fn is_configured(&self) -> bool {
        super::pasted::credential(ID)
            .and_then(|pasted| Session::parse(&pasted))
            .is_some()
    }

    fn fetch(&self, ctx: Arc<Ctx>) -> FetchFuture {
        Box::pin(async move { fetch_inner(ctx).await })
    }
}

/// The four values windsurf.com keeps in `localStorage`, all of which the
/// endpoint is sent.
struct Session {
    token: String,
    auth1: String,
    account_id: String,
    organization_id: String,
}

impl Session {
    /// The pasted object, with a value the site stored JSON-encoded — a quoted
    /// string, which is what `localStorage` hands back — unwrapped, as the
    /// original unwraps it before saving the four.
    fn parse(pasted: &str) -> Option<Self> {
        let object: Map<String, Value> = match serde_json::from_str(pasted) {
            Ok(Value::Object(map)) => map,
            _ => return None,
        };

        let value = |key: &str| -> Option<String> {
            let raw = object.get(key)?.as_str()?.trim();
            if raw.is_empty() {
                return None;
            }
            let unwrapped = unwrap_quoted(raw);
            let trimmed = unwrapped.trim();
            if trimmed.is_empty() {
                None
            } else {
                Some(trimmed.to_string())
            }
        };

        Some(Self {
            token: value(STORAGE_KEYS[0])?,
            auth1: value(STORAGE_KEYS[1])?,
            account_id: value(STORAGE_KEYS[2])?,
            organization_id: value(STORAGE_KEYS[3])?,
        })
    }
}

/// A value the site stored JSON-encoded — a quoted string — unwrapped. A value
/// that is not one is left as it is.
fn unwrap_quoted(raw: &str) -> String {
    if raw.starts_with('"') {
        if let Ok(text) = serde_json::from_str::<String>(raw) {
            return text;
        }
    }
    raw.to_string()
}

/// The failure when neither place has the four, saying which four and both ways
/// in.
fn needed() -> String {
    let env = format!("PULSEWIN_{}_COOKIE", ID.to_uppercase().replace('-', "_"));
    let file = super::settings_path(ID)
        .map(|path| path.display().to_string())
        .unwrap_or_else(|| format!("%APPDATA%\\PulseWin\\{ID}.json"));
    format!(
        "no Windsurf session (set {env}, or create {file} with {{\"cookie\": \"<the four devin_* \
         values windsurf.com keeps in localStorage, as one JSON object>\"}}). The original reads \
         them out of a Chromium browser's localStorage, which needs a browser profile PulseWin \
         does not read; the object it saves is {{\"devin_session_token\": \"…\", \
         \"devin_auth1_token\": \"…\", \"devin_account_id\": \"…\", \"devin_primary_org_id\": \
         \"…\"}}, and every one of the four is required."
    )
}

async fn fetch_inner(ctx: Arc<Ctx>) -> ProviderUsage {
    let Some(session) = super::pasted::credential(ID).and_then(|pasted| Session::parse(&pasted))
    else {
        return ProviderUsage::failed(ID, NAME, needed());
    };

    // The body of two fields: the session token (1) and "include top-up
    // status" (2), which the page always sets.
    let mut body: Vec<u8> = Vec::new();
    protobuf::append_key(1, 2, &mut body);
    protobuf::append_varint(session.token.len() as u64, &mut body);
    body.extend_from_slice(session.token.as_bytes());
    protobuf::append_key(2, 0, &mut body);
    protobuf::append_varint(1, &mut body);

    // The client that refuses redirects, so a session that no longer works is
    // seen as the redirect it is and never followed with the session attached.
    let response = ctx
        .gateway_client
        .post(ENDPOINT)
        .header("Content-Type", "application/proto")
        .header("Connect-Protocol-Version", "1")
        .header("Origin", ORIGIN)
        .header("Referer", format!("{ORIGIN}/profile"))
        .header("x-auth-token", &session.token)
        .header("x-devin-session-token", &session.token)
        .header("x-devin-auth1-token", &session.auth1)
        .header("x-devin-account-id", &session.account_id)
        .header("x-devin-primary-org-id", &session.organization_id)
        .body(body)
        .send()
        .await;

    let response = match response {
        Ok(response) => response,
        Err(e) => {
            return ProviderUsage::failed(ID, NAME, format!("request failed: {}", describe_reqwest_error(&e)))
        }
    };

    let status = response.status();
    let body = match response.bytes().await {
        Ok(body) => body,
        Err(e) => return ProviderUsage::failed(ID, NAME, format!("cannot read body: {e}")),
    };

    if status.is_redirection() {
        return ProviderUsage::failed(
            ID,
            NAME,
            "HTTP 3xx — the session has expired; copy the four devin_* values again",
        );
    }
    if !status.is_success() {
        let hints: &[(u16, &str)] = &[
            (401, " — the session has expired; copy the four devin_* values again"),
            (403, " — the session has expired; copy the four devin_* values again"),
            (429, " — rate limited, try again shortly"),
        ];
        return ProviderUsage::failed(ID, NAME, super::http_failure(status, hints));
    }

    reading(&body)
}

// ---------------------------------------------------------------------------
// Reading the reply
// ---------------------------------------------------------------------------

/// What is read out of `plan_status`, field 1 of the reply.
#[derive(Default)]
struct PlanStatus {
    plan_name: Option<String>,
    daily_remaining: Option<u64>,
    weekly_remaining: Option<u64>,
    daily_reset_at: Option<u64>,
    weekly_reset_at: Option<u64>,
}

/// `plan_status` fields: 1 `plan_info` (whose 2 is the plan's name), 14 and 15
/// the daily and weekly percentage **remaining**, 17 and 18 their resets in
/// Unix seconds. Everything else is skipped.
fn plan_status(data: &[u8]) -> Option<PlanStatus> {
    let mut reply = protobuf::Reader::new(data);
    let mut found: Option<Vec<u8>> = None;
    while let Some((number, value)) = reply.next() {
        if let (1, protobuf::Value::Bytes(bytes)) = (number, value) {
            found = Some(bytes);
        }
    }
    // A reader that stopped early has not read the message.
    if !reply.is_complete() {
        return None;
    }
    let found = found?;

    let mut status = PlanStatus::default();
    let mut reader = protobuf::Reader::new(&found);
    while let Some((number, value)) = reader.next() {
        match (number, value) {
            (1, protobuf::Value::Bytes(info)) => {
                let mut plan = protobuf::Reader::new(&info);
                while let Some((number, value)) = plan.next() {
                    if number == 2 {
                        if let protobuf::Value::Bytes(name) = value {
                            status.plan_name = String::from_utf8(name).ok();
                        }
                    }
                }
            }
            (14, protobuf::Value::Varint(value)) => status.daily_remaining = Some(value),
            (15, protobuf::Value::Varint(value)) => status.weekly_remaining = Some(value),
            (17, protobuf::Value::Varint(value)) => status.daily_reset_at = Some(value),
            (18, protobuf::Value::Varint(value)) => status.weekly_reset_at = Some(value),
            _ => {}
        }
    }
    if !reader.is_complete() {
        return None;
    }

    Some(status)
}

/// The mapping, kept apart from the request so a fixture can drive it.
fn reading(body: &[u8]) -> ProviderUsage {
    let Some(status) = plan_status(body) else {
        return ProviderUsage::failed(ID, NAME, "the reply could not be read");
    };

    // Protobuf does not write a zero, so a quota at 0% remaining and a quota
    // the plan does not have look the same: absent. An absent one is left off —
    // a spent quota is not drawn rather than drawn as a guess.
    let rows: [(i64, &str, Option<u64>, Option<u64>); 2] = [
        (86_400, "Daily", status.daily_remaining, status.daily_reset_at),
        (
            7 * 86_400,
            "7d",
            status.weekly_remaining,
            status.weekly_reset_at,
        ),
    ];
    let windows: Vec<UsageWindow> = rows
        .into_iter()
        .filter_map(|(_, label, remaining, reset)| {
            let remaining = remaining.filter(|remaining| *remaining <= 100)?;
            let used = (100 - remaining) as f64 / 100.0;
            Some(
                UsageWindow::new(label, Some(percent_from_fraction(used))).with_reset(
                    reset.and_then(|seconds| super::stamp_from_epoch_seconds(seconds as f64)),
                ),
            )
        })
        .collect();

    if windows.is_empty() {
        return ProviderUsage::failed(ID, NAME, "no limits reported in the reply");
    }

    let plan = status
        .plan_name
        .map(|name| name.trim().to_string())
        .filter(|name| !name.is_empty());

    ProviderUsage::ok(ID, NAME, windows).with_plan(plan)
}

// ---------------------------------------------------------------------------
// Just enough protobuf
// ---------------------------------------------------------------------------

/// Just enough protobuf to read a reply and write a request: varints and
/// length-delimited fields, with the fixed-width ones skipped.
mod protobuf {
    #[derive(Debug, PartialEq, Eq)]
    pub enum Value {
        Varint(u64),
        Bytes(Vec<u8>),
        Skipped,
    }

    pub struct Reader<'a> {
        bytes: &'a [u8],
        index: usize,
        /// False once anything could not be read; a reader that stopped early
        /// has not read the message.
        complete: bool,
    }

    impl<'a> Reader<'a> {
        pub fn new(bytes: &'a [u8]) -> Self {
            Self {
                bytes,
                index: 0,
                complete: true,
            }
        }

        pub fn is_complete(&self) -> bool {
            self.complete
        }

        pub fn next(&mut self) -> Option<(usize, Value)> {
            if !self.complete || self.index >= self.bytes.len() {
                return None;
            }

            let Some(key) = self.varint() else {
                return self.fail();
            };
            if key >> 3 == 0 {
                return self.fail();
            }
            let number = (key >> 3) as usize;

            match key & 0x07 {
                0 => {
                    let Some(value) = self.varint() else {
                        return self.fail();
                    };
                    Some((number, Value::Varint(value)))
                }
                1 => {
                    if !self.advance(8) {
                        return self.fail();
                    }
                    Some((number, Value::Skipped))
                }
                2 => {
                    let Some(length) = self.varint() else {
                        return self.fail();
                    };
                    if length > (self.bytes.len() - self.index) as u64 {
                        return self.fail();
                    }
                    let start = self.index;
                    self.index += length as usize;
                    Some((number, Value::Bytes(self.bytes[start..self.index].to_vec())))
                }
                5 => {
                    if !self.advance(4) {
                        return self.fail();
                    }
                    Some((number, Value::Skipped))
                }
                _ => self.fail(),
            }
        }

        fn fail(&mut self) -> Option<(usize, Value)> {
            self.complete = false;
            None
        }

        fn advance(&mut self, count: usize) -> bool {
            if self.bytes.len() - self.index < count {
                return false;
            }
            self.index += count;
            true
        }

        fn varint(&mut self) -> Option<u64> {
            let mut result: u64 = 0;
            let mut shift: u32 = 0;
            while self.index < self.bytes.len() && shift < 64 {
                let byte = self.bytes[self.index];
                self.index += 1;
                result |= u64::from(byte & 0x7F) << shift;
                if byte & 0x80 == 0 {
                    return Some(result);
                }
                shift += 7;
            }
            None
        }
    }

    pub fn append_key(number: usize, wire: u64, into: &mut Vec<u8>) {
        append_varint((number as u64) << 3 | wire, into);
    }

    pub fn append_varint(mut value: u64, into: &mut Vec<u8>) {
        while value >= 0x80 {
            into.push((value & 0x7F) as u8 | 0x80);
            value >>= 7;
        }
        into.push(value as u8);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Second-hand, from the original's fixture: a `GetPlanStatus` reply built
    /// from the field numbers in CodexBar's Windsurf provider and the values in
    /// its tests, base64 as the fixture file holds it. It pins the shape this
    /// reads; it does not prove the shape is right.
    const FIXTURE: &str = "CjcKBwgCEgNQcm8SBgiAvMPOBhoGCIDW4c8GUgIIAWABcER4VIgB4LPizwaQAYDB6M8GnQEAAIA/EAc=";

    const SESSION: &str = concat!(
        r#"{"devin_session_token":"devin-session-token$abc","devin_auth1_token":"auth1_xyz","#,
        r#""devin_account_id":"account-123","devin_primary_org_id":"org-456"}"#
    );

    /// The fixture is base64, as the original's fixture file is. This port has
    /// no base64 dependency, and a decode is a dozen lines.
    fn base64(text: &str) -> Vec<u8> {
        let mut out: Vec<u8> = Vec::new();
        let mut buffer: u32 = 0;
        let mut bits: u32 = 0;

        for byte in text.bytes() {
            let value = match byte {
                b'A'..=b'Z' => byte - b'A',
                b'a'..=b'z' => byte - b'a' + 26,
                b'0'..=b'9' => byte - b'0' + 52,
                b'+' => 62,
                b'/' => 63,
                // The padding, and anything that is not the alphabet.
                _ => continue,
            } as u32;
            buffer = (buffer << 6) | value;
            bits += 6;
            if bits >= 8 {
                bits -= 8;
                out.push((buffer >> bits) as u8);
            }
        }

        out
    }

    /// A reply with a `plan_status` of the given varint fields, for the cases
    /// the fixture does not cover.
    fn reply(fields: &[(usize, u64)]) -> Vec<u8> {
        let mut status: Vec<u8> = Vec::new();
        for (number, value) in fields {
            protobuf::append_key(*number, 0, &mut status);
            protobuf::append_varint(*value, &mut status);
        }

        let mut reply: Vec<u8> = Vec::new();
        protobuf::append_key(1, 2, &mut reply);
        protobuf::append_varint(status.len() as u64, &mut reply);
        reply.extend_from_slice(&status);
        reply
    }

    #[test]
    fn daily_and_weekly_are_read_as_used_100_minus_remaining_with_their_stated_resets() {
        let usage = reading(&base64(FIXTURE));

        assert_eq!(usage.plan.as_deref(), Some("Pro"));
        let labels: Vec<&str> = usage.windows.iter().map(|w| w.label.as_str()).collect();
        assert_eq!(labels, vec!["Daily", "7d"]);
        // 100 − 68 and 100 − 84.
        assert_eq!(usage.windows[0].percent_used, Some(32.0));
        assert_eq!(usage.windows[1].percent_used, Some(16.0));
        assert_eq!(
            usage.windows[0].resets_at.as_deref(),
            Some("2026-05-04T13:06:40Z")
        );
        assert_eq!(
            usage.windows[1].resets_at.as_deref(),
            Some("2026-05-05T16:53:20Z")
        );
    }

    /// Fields this does not read — a fixed-width one included — are skipped, not
    /// fatal.
    #[test]
    fn fields_this_does_not_read_are_skipped_not_fatal() {
        let status = plan_status(&base64(FIXTURE)).unwrap();
        assert_eq!(status.daily_remaining, Some(68));
        assert_eq!(status.weekly_remaining, Some(84));
        assert_eq!(status.daily_reset_at, Some(1_777_900_000));
        assert_eq!(status.weekly_reset_at, Some(1_778_000_000));
        assert_eq!(status.plan_name.as_deref(), Some("Pro"));
    }

    #[test]
    fn a_quota_left_out_is_left_off() {
        // It could be spent, or not on the plan: protobuf writes no zero.
        let usage = reading(&reply(&[(15, 40), (18, 1_778_000_000)]));
        let labels: Vec<&str> = usage.windows.iter().map(|w| w.label.as_str()).collect();
        assert_eq!(labels, vec!["7d"]);
        assert_eq!(usage.windows[0].percent_used, Some(60.0));
    }

    #[test]
    fn a_figure_over_100_is_not_a_percentage_and_nothing_left_is_no_limits() {
        let usage = reading(&reply(&[(14, 250)]));
        assert!(usage.error.is_some());
    }

    #[test]
    fn a_reply_that_cannot_be_read() {
        for reply in [
            Vec::new(),
            b"not protobuf at all".to_vec(),
            vec![0x0A, 0x05, 0x70],
            vec![0x08, 0x01],
        ] {
            assert!(reading(&reply).error.is_some(), "read {reply:?}");
        }
    }

    #[test]
    fn the_pasted_session_is_the_four_devin_values() {
        let session = Session::parse(SESSION).unwrap();
        assert_eq!(session.token, "devin-session-token$abc");
        assert_eq!(session.auth1, "auth1_xyz");
        assert_eq!(session.account_id, "account-123");
        assert_eq!(session.organization_id, "org-456");
    }

    #[test]
    fn anything_short_of_all_four_is_no_session() {
        for pasted in [
            r#"{"devin_session_token":"t","devin_auth1_token":"a","devin_account_id":"c"}"#,
            r#"{"devin_session_token":"t","devin_auth1_token":"a","devin_account_id":"c","devin_primary_org_id":" "}"#,
            "devin_session_token=t",
            "[]",
            "",
        ] {
            assert!(Session::parse(pasted).is_none(), "read {pasted}");
        }
    }

    /// What the browser stored is what the session reads, with the quoted values
    /// `localStorage` hands back unwrapped.
    #[test]
    fn a_value_the_site_stored_quoted_is_unwrapped() {
        let stored = concat!(
            r#"{"devin_session_token":"\"session-token-value\"","#,
            r#""devin_auth1_token":"auth1-token-value","#,
            r#""devin_account_id":"\"account-1\"","#,
            r#""devin_primary_org_id":"org-1","unrelated":"kept out"}"#
        );
        let session = Session::parse(stored).unwrap();
        assert_eq!(session.token, "session-token-value");
        assert_eq!(session.account_id, "account-1");
        assert_eq!(session.auth1, "auth1-token-value");
    }

    /// The request carries the session in the headers windsurf.com expects, and
    /// in the body.
    #[test]
    fn the_request_body_is_the_session_token_and_the_top_up_flag() {
        let session = Session::parse(SESSION).unwrap();

        let mut body: Vec<u8> = Vec::new();
        protobuf::append_key(1, 2, &mut body);
        protobuf::append_varint(session.token.len() as u64, &mut body);
        body.extend_from_slice(session.token.as_bytes());
        protobuf::append_key(2, 0, &mut body);
        protobuf::append_varint(1, &mut body);

        let mut reader = protobuf::Reader::new(&body);
        assert_eq!(
            reader.next().unwrap().1,
            protobuf::Value::Bytes(b"devin-session-token$abc".to_vec())
        );
        assert_eq!(reader.next().unwrap().1, protobuf::Value::Varint(1));
        assert!(reader.next().is_none());
        assert!(reader.is_complete());

        // The route is windsurf.com's own, and nothing else's.
        assert!(ENDPOINT.starts_with("https://windsurf.com/_backend/"));
        assert_eq!(STORAGE_KEYS.len(), 4);
    }

    /// A varint that never ends is not a message, and neither is a length that
    /// runs past the end.
    #[test]
    fn a_varint_that_never_ends_is_not_read() {
        let mut unfinished = protobuf::Reader::new(&[0x80, 0x80, 0x80]);
        assert!(unfinished.next().is_none());
        assert!(!unfinished.is_complete());

        assert!(protobuf::Reader::new(&[]).next().is_none());
        // A field number of zero is not one.
        assert!(protobuf::Reader::new(&[0x00, 0x01]).next().is_none());
    }
}
