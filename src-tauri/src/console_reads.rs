//! Cancel all consumers of one optional console read without cancelling other providers.
use std::sync::Mutex;
use tokio_util::sync::CancellationToken;
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
