//! Cancel all consumers of one optional console read without cancelling other providers.
//!
//! The failure classes here are the shared half of the consoles' renewals. The
//! original records the same distinction in `renewalFailure`: only a credential
//! that was *rejected* may send somebody back through a login. A dropped
//! connection, a rate limit or a server fault keeps the reading the card
//! already has and says which class it was — saying "logged out" for those is
//! what made a network blip look like a removed session.
use std::sync::Mutex;
use tokio_util::sync::CancellationToken;
/// What a console read's failure was, in the classes the app treats differently.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Failure {
    /// The credential itself was turned away (401/403, or a console answering
    /// that its login expired). The only class that asks for a login.
    CredentialRejected,
    /// Another client error (400/404/422…): the request was wrong, but the
    /// credential was not what was refused.
    RequestRefused,
    RateLimited,
    ServerError,
    /// The request never completed: DNS, TLS, proxy or a timeout.
    Network,
    /// A reply arrived but could not be read as this console's JSON.
    UnreadableReply,
    /// The reply was larger than the console's reading limit.
    TooLarge,
    /// The site answered with a redirect. A request carrying a cookie or a token
    /// is not followed to another site, so this is a stop, not a detour.
    Redirected,
}
impl Failure {
    /// The class for an HTTP status that was not a success.
    pub fn of_status(status: u16) -> Self {
        match status {
            401 | 403 => Failure::CredentialRejected,
            429 => Failure::RateLimited,
            300..=399 => Failure::Redirected,
            400..=499 => Failure::RequestRefused,
            _ => Failure::ServerError,
        }
    }
    /// The stable id the frontend keys its wording off, so no label is parsed
    /// back out of prose.
    pub fn class(self) -> &'static str {
        match self {
            Failure::CredentialRejected => "credential-rejected",
            Failure::RequestRefused => "request-refused",
            Failure::RateLimited => "rate-limited",
            Failure::ServerError => "server-error",
            Failure::Network => "network",
            Failure::UnreadableReply => "unreadable-reply",
            Failure::TooLarge => "too-large",
            Failure::Redirected => "redirected",
        }
    }
    /// Whether the reading already held stays on screen. Everything but a
    /// rejected credential does.
    pub fn keeps_reading(self) -> bool {
        self != Failure::CredentialRejected
    }
    /// The sentence shown to the user for this class.
    pub fn text(self) -> String {
        match self {
            Failure::CredentialRejected => "控制台登录已失效，请重新登录。",
            Failure::RequestRefused => "控制台拒绝了这次请求，请稍后重试。",
            Failure::RateLimited => "控制台请求过于频繁，请稍后重试。",
            Failure::ServerError => "控制台服务返回错误，请稍后重试。",
            Failure::Network => "控制台请求失败，请检查网络或代理。",
            Failure::UnreadableReply => "控制台未能返回可读数据。",
            Failure::TooLarge => "控制台数据超过读取上限。",
            Failure::Redirected => "控制台请求被其他站点接手，已停止读取。",
        }
        .to_string()
    }
    /// The line recorded beside a reading that was kept, so the card says why
    /// it is still the previous one.
    pub fn kept_note(self) -> String {
        format!("本次读取失败（{}），以下仍是上次成功读取的读数。", self.label())
    }
    fn label(self) -> &'static str {
        match self {
            Failure::CredentialRejected => "登录被拒绝",
            Failure::RequestRefused => "请求被拒绝",
            Failure::RateLimited => "请求被限流",
            Failure::ServerError => "服务端错误",
            Failure::Network => "网络中断",
            Failure::UnreadableReply => "回包无法读取",
            Failure::TooLarge => "数据超过上限",
            Failure::Redirected => "被跳转到其他站点",
        }
    }
}
/// Whether `target` (a final URL or a `Location` header) leaves `origin`'s site.
///
/// A console request carries a cookie or a bearer token set by hand, and a
/// hand-set header rides a redirect to whatever host it names — which is the
/// trust boundary the consoles are careful about. Used two ways: the clients
/// they share do not follow redirects, and a 3xx is read as a stop; and any
/// final URL is checked before its body is trusted.
pub fn foreign_site(target: &str, origin: &str) -> bool {
    let Ok(base) = reqwest::Url::parse(origin) else {
        return true;
    };
    // A location that is not a URI reference at all is a stop too: `join` would
    // percent-encode it into a same-origin path and let an unusable redirect
    // look like a detour we can follow.
    let target = target.trim();
    if target.is_empty() || target.chars().any(|c| c.is_whitespace() || c.is_control()) {
        return true;
    }
    let Ok(target) = base.join(target) else {
        return true;
    };
    target.origin() != base.origin()
}
pub struct ReadGate(Mutex<Option<CancellationToken>>);
impl ReadGate {
    pub const fn new() -> Self {
        Self(Mutex::new(None))
    }
    pub fn request(&self) -> CancellationToken {
        let mut slot = self.0.lock().unwrap();
        if slot.as_ref().is_none_or(CancellationToken::is_cancelled) {
            *slot = Some(CancellationToken::new())
        }
        slot.as_ref().unwrap().clone()
    }
    pub fn cancel(&self) {
        if let Some(token) = self.0.lock().unwrap().as_ref() {
            token.cancel()
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    /// Only a rejected credential asks for a login again; every other class
    /// keeps the reading the card already has.
    #[test]
    fn only_a_rejected_credential_sends_somebody_back_to_the_login() {
        assert_eq!(Failure::of_status(401), Failure::CredentialRejected);
        assert_eq!(Failure::of_status(403), Failure::CredentialRejected);
        assert_eq!(Failure::of_status(429), Failure::RateLimited);
        assert_eq!(Failure::of_status(500), Failure::ServerError);
        assert_eq!(Failure::of_status(503), Failure::ServerError);
        assert_eq!(Failure::of_status(302), Failure::Redirected);
        assert_eq!(Failure::of_status(400), Failure::RequestRefused);
        assert_eq!(Failure::of_status(418), Failure::RequestRefused);
        assert!(!Failure::CredentialRejected.keeps_reading());
        for kept in [
            Failure::RequestRefused,
            Failure::RateLimited,
            Failure::ServerError,
            Failure::Network,
            Failure::UnreadableReply,
            Failure::TooLarge,
            Failure::Redirected,
        ] {
            assert!(kept.keeps_reading(), "{kept:?} must keep the last reading");
            assert_ne!(kept.class(), Failure::CredentialRejected.class());
            assert!(kept.kept_note().contains("上次成功读取"));
            assert!(!kept.text().contains("请重新登录"));
        }
        // Each class has its own id, so the frontend can word itself.
        let classes: Vec<&str> = [
            Failure::CredentialRejected,
            Failure::RequestRefused,
            Failure::RateLimited,
            Failure::ServerError,
            Failure::Network,
            Failure::UnreadableReply,
            Failure::TooLarge,
            Failure::Redirected,
        ]
        .iter()
        .map(|f| f.class())
        .collect();
        let mut unique = classes.clone();
        unique.sort();
        unique.dedup();
        assert_eq!(unique.len(), classes.len());
    }
    /// A redirect away from the console's site is a stop; a relative one stays
    /// on the same site.
    #[test]
    fn a_redirect_to_another_site_is_not_followed() {
        let origin = "https://opencode.ai";
        assert!(!foreign_site("https://opencode.ai/console/api/orgs", origin));
        assert!(!foreign_site("/console/api/orgs", origin));
        assert!(!foreign_site("https://opencode.ai:443/console", origin));
        assert!(foreign_site("https://elsewhere.example/console", origin));
        assert!(foreign_site("http://opencode.ai/console", origin));
        assert!(foreign_site("not a url", origin));
        assert!(foreign_site("https://opencode.ai/x", "not a url"));
        let origin = "https://platform.deepseek.com";
        assert!(!foreign_site("https://platform.deepseek.com/api/v0/x", origin));
        assert!(foreign_site("https://platform.deepseek.com.evil.example/x", origin));
    }
    #[tokio::test]
    async fn opt_out_cancels_all_waiters_and_reenable_gets_a_new_scope() {
        let a = ReadGate::new();
        let b = ReadGate::new();
        let one = a.request();
        let two = a.request();
        let independent = b.request();
        a.cancel();
        tokio::time::timeout(std::time::Duration::from_millis(50), async {
            one.cancelled().await;
            two.cancelled().await
        })
        .await
        .unwrap();
        assert!(!independent.is_cancelled());
        assert!(!a.request().is_cancelled());
    }
}
