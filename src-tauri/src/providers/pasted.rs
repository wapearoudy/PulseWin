//! The credential a signed-in browser would have supplied, pasted by hand.
//!
//! **Why this module exists.** In the original, a provider whose service signs
//! in with a browser session does not ask the reader for anything: it reads the
//! session out of the browser's own cookie store (`Auth/BrowserCookies.swift`),
//! which on macOS means the login keychain for a Chromium key and Full Disk
//! Access for Safari. PulseWin cannot read either. What it can read is a
//! `Cookie:` header the reader copies out of their browser's network tab and
//! pastes into Settings, which is exactly what `cursor.rs` and `type_safe.rs`
//! already do — so the providers whose route needs nothing but an authenticated
//! request take that header here and nothing else about them changes.
//!
//! **One credential per provider, in the shape the settings window writes.**
//! `PULSEWIN_<ID>_COOKIE` (then `_TOKEN`, then `_KEY`) first, because someone
//! who exported a value meant that one to be used; then
//! `%APPDATA%\PulseWin\<id>.json`, reading `cookie` before `apiKey` — the
//! first is what a person pasting a header will call it, the second is what the
//! settings window writes for every provider.
//!
//! **What travels is only what the route needs.** [`keep`] is the original's
//! `ProviderProfile.keep`: a browser store holds a host's analytics and
//! preference cookies beside its session, and a credential store that forwards
//! everything it found is one that leaks whatever the site adds next.

use std::collections::HashSet;

use super::settings_path;
use crate::credentials;

/// The credential the reader pasted, from the two places this port has.
///
/// Env wins, as it does for an API key: an exported value is the one somebody
/// chose on purpose.
pub fn credential(id: &str) -> Option<String> {
    for key in ["cookie", "token", "key"] {
        if let Some(value) = credentials::env_override(id, key) {
            return Some(value);
        }
    }

    let path = settings_path(id)?;
    let json = credentials::read_json(&path)?;
    credentials::dig_first_str(&json, &["cookie", "apiKey", "api_key", "key", "token"])
}

/// A pasted `Cookie:` header as the header's own value.
///
/// The name is taken off where it came along — people copy the whole line out
/// of a network tab, and `Cookie: a=1` sent as a value would authenticate as a
/// cookie named `Cookie:`.
///
/// **A control character is refused rather than passed on.** A `Cookie` header
/// set by hand is a header injection if a value carries a newline, and what
/// somebody pastes is a whole line out of a text editor. The original checks
/// the same thing in the normalisers that read a browser store; this is the one
/// place every provider here crosses that boundary.
pub fn header(pasted: &str) -> Option<String> {
    if pasted.unicode_controls() {
        return None;
    }
    let trimmed = pasted.trim();
    let value = match trimmed.get(.."cookie:".len()) {
        Some(head) if head.eq_ignore_ascii_case("cookie:") => trimmed["cookie:".len()..].trim(),
        _ => trimmed,
    };
    if value.is_empty() {
        None
    } else {
        Some(value.to_string())
    }
}

/// The pasted credential as a header value, or nothing usable.
pub fn cookie(id: &str) -> Option<String> {
    credential(id).and_then(|pasted| header(&pasted))
}

/// The named cookies out of a `name=value; …` header, and nothing else.
///
/// The original's `ProviderProfile.keep`, rules and all:
///
/// - A name ending in `*` is a **prefix**, for a service whose session cookie
///   carries a suffix of its own — Mistral's is `ory_session_` and then the
///   deployment's id. The prefix has to be followed by something; `*` alone
///   would keep every cookie the browser holds for the host.
/// - Names joined with `|` are **alternatives**, for a service whose session
///   can sit under any one of several names. In the first entry that means any
///   one of them is enough; every one of them is kept.
/// - Without the first entry's name there is no session at all, and the whole
///   header is refused rather than half of it sent.
pub fn keep(header: &str, names: &[&str]) -> Option<String> {
    let required: Vec<&str> = names.first()?.split('|').collect();
    let patterns: Vec<&str> = names.iter().flat_map(|name| name.split('|')).collect();

    let mut pairs: Vec<(String, String)> = Vec::new();
    for part in header.split(';') {
        let Some((name, value)) = part.trim().split_once('=') else {
            continue;
        };
        if value.is_empty() || !patterns.iter().any(|pattern| matches(name, pattern)) {
            continue;
        }
        pairs.push((name.to_string(), value.to_string()));
    }

    if !pairs
        .iter()
        .any(|(name, _)| required.iter().any(|pattern| matches(name, pattern)))
    {
        return None;
    }

    let mut seen: HashSet<&str> = HashSet::new();
    let kept: Vec<String> = pairs
        .iter()
        .filter(|(name, _)| seen.insert(name.as_str()))
        .map(|(name, value)| format!("{name}={value}"))
        .collect();
    Some(kept.join("; "))
}

fn matches(name: &str, pattern: &str) -> bool {
    match pattern.strip_suffix('*') {
        Some(prefix) => !prefix.is_empty() && name.len() > prefix.len() && name.starts_with(prefix),
        None => name == pattern,
    }
}

/// The failure when there is no usable cookie, saying which one and both ways
/// in — and telling the two cases apart, because "nothing was pasted" and "what
/// was pasted is not a header" want different things done about them.
pub fn needed(id: &str, wanted: &str) -> String {
    let env = format!(
        "PULSEWIN_{}_COOKIE",
        id.to_uppercase().replace('-', "_")
    );
    let file = settings_path(id)
        .map(|path| path.display().to_string())
        .unwrap_or_else(|| format!("%APPDATA%\\PulseWin\\{id}.json"));

    match credential(id) {
        Some(pasted) if header(&pasted).is_none() => format!(
            "the pasted cookie for {id} is not a header — paste one line, without a newline in it"
        ),
        _ => format!(
            "no {wanted} (set {env}, or create {file} with {{\"cookie\": \"<the Cookie header \
             from a signed-in browser>\"}}). The original reads this session out of a browser's \
             cookie store, which needs the macOS login keychain; PulseWin takes the pasted header."
        ),
    }
}

/// Whether a string carries a control character a header must not.
trait Controls {
    fn unicode_controls(&self) -> bool;
}

impl Controls for str {
    fn unicode_controls(&self) -> bool {
        self.chars().any(|c| c.is_control())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_pasted_header_may_carry_its_name() {
        assert_eq!(header("Cookie: a=1; b=2").as_deref(), Some("a=1; b=2"));
        assert_eq!(header("  a=1  ").as_deref(), Some("a=1"));
        // Only the ends are trimmed, as in the original: a space before a
        // separator is part of the value the browser held.
        assert_eq!(header("  a=1 ; ").as_deref(), Some("a=1 ;"));
        assert_eq!(header("cookie:a=1").as_deref(), Some("a=1"));
        // A cookie whose own name begins with "cookie" is left alone.
        assert_eq!(header("cookie_jar=1").as_deref(), Some("cookie_jar=1"));
    }

    #[test]
    fn a_header_with_a_newline_is_refused_rather_than_sent() {
        assert_eq!(header("a=1\r\nX-Injected: 1"), None);
        assert_eq!(header("a=1\n"), None);
        assert_eq!(header("   "), None);
    }

    #[test]
    fn keeps_only_the_named_cookies() {
        let header = "sessionid=abc; _ga=GA1.2; csrftoken=xyz; theme=dark";
        assert_eq!(
            keep(header, &["sessionid", "csrftoken"]).as_deref(),
            Some("sessionid=abc; csrftoken=xyz")
        );
    }

    #[test]
    fn without_the_first_name_there_is_no_session() {
        assert_eq!(keep("csrftoken=xyz", &["sessionid", "csrftoken"]), None);
        assert_eq!(keep("sessionid=", &["sessionid"]), None);
        assert_eq!(keep("", &["sessionid"]), None);
        // No names at all is nothing to keep, not everything.
        assert_eq!(keep("sessionid=abc", &[]), None);
    }

    #[test]
    fn a_star_is_a_prefix_that_has_to_be_followed_by_something() {
        let header = "ory_session_abc123=one; ory_session_=two; csrftoken=three; other=four";
        assert_eq!(
            keep(header, &["ory_session_*", "csrftoken"]).as_deref(),
            Some("ory_session_abc123=one; csrftoken=three")
        );
        assert_eq!(keep("other=1", &["ory_session_*"]), None);
    }

    #[test]
    fn alternatives_in_the_first_entry_are_any_one_of_them() {
        let names = &["login_qwencloud_ticket|login_aliyunid_ticket", "csrf"];
        assert_eq!(
            keep("login_aliyunid_ticket=t; csrf=c; nope=n", names).as_deref(),
            Some("login_aliyunid_ticket=t; csrf=c")
        );
        assert_eq!(keep("nope=n", names), None);
    }

    #[test]
    fn a_repeated_name_is_kept_once_as_in_any_cookie_header() {
        let header = "session_id=host-only; session_id=domain; other=1";
        assert_eq!(
            keep(header, &["session_id"]).as_deref(),
            Some("session_id=host-only")
        );
    }
}
