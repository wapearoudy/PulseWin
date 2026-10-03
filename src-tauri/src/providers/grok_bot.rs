//! Grok Bot's weekly allowance.
//!
//! **A ring of its own, though the credential is Cursor's.** Grok Bot is xAI's,
//! sold through Cursor and billed against the Cursor account, so the original
//! reads it with the login the Cursor editor already stored. PulseWin cannot
//! read that store — it is a SQLite `state.vscdb` — so the `Cookie` header
//! copied out of a signed-in `cursor.com` request is the credential here, as it
//! is for `cursor.rs` beside this. It is not a share of Cursor's monthly bill
//! the way the two model pools are: it is a separate weekly allowance under a
//! name people know, so it gets its own place on the rail rather than a fourth
//! row on somebody else's card.
//!
//! **And it is the other Grok.** `grok.rs` reads the weekly pool a SuperGrok
//! plan spends across every xAI product; this reads an allowance that arrives
//! with a Cursor subscription. Two companies' bills, one brand — so the two
//! never share a credential, an endpoint, or a mark.
//!
//! `POST https://cursor.com/api/dashboard/get-sand-usage-status` — Cursor's own
//! protocol calls Grok Bot "Sand". Not public API, exactly like the usage
//! summary beside it, and it can change without notice.
//!
//! **Redirects are not followed.** The session goes out as a `Cookie` header,
//! which rides a redirect to whatever host it names — unlike `Authorization`,
//! which is stripped — so a signed-out session arrives as the 3xx rather than
//! being carried somewhere else.
//!
//! Every fixture the tests use is built by hand from the field names this
//! decodes: nobody here holds a Cursor plan that includes Grok Bot to capture a
//! reply from.

use std::sync::Arc;

use serde_json::Value;

use super::{describe_reqwest_error, percent_from_scale, Ctx, FetchFuture, Provider};
use crate::model::{parse_reset, ProviderUsage, UsageWindow};

const ID: &str = "grok-bot";
const NAME: &str = "Grok Bot";

const ENDPOINT: &str = "https://cursor.com/api/dashboard/get-sand-usage-status";

pub struct GrokBot;

impl Provider for GrokBot {
    fn id(&self) -> &'static str {
        ID
    }

    fn name(&self) -> &'static str {
        NAME
    }

    /// Whether a pasted Cursor cookie is on the machine. No network call.
    fn is_configured(&self) -> bool {
        super::pasted::cookie(ID).is_some()
    }

    fn fetch(&self, ctx: Arc<Ctx>) -> FetchFuture {
        Box::pin(async move { fetch_inner(ctx).await })
    }
}

async fn fetch_inner(ctx: Arc<Ctx>) -> ProviderUsage {
    let Some(cookie) = super::pasted::cookie(ID) else {
        return ProviderUsage::failed(ID, NAME, super::pasted::needed(ID, "Cursor session cookie"));
    };

    let response = ctx
        .gateway_client
        .post(ENDPOINT)
        .header("Cookie", cookie)
        .header("Accept", "application/json")
        .header("Content-Type", "application/json")
        // The dashboard's own call sends it, and an endpoint that checks the
        // origin refuses a request without one.
        .header("Origin", "https://cursor.com")
        .body("{}")
        .send()
        .await;

    let response = match response {
        Ok(response) => response,
        Err(e) => {
            return ProviderUsage::failed(ID, NAME, format!("request failed: {}", describe_reqwest_error(&e)))
        }
    };

    let status = response.status();
    let body = match response.text().await {
        Ok(body) => body,
        Err(e) => return ProviderUsage::failed(ID, NAME, format!("cannot read body: {e}")),
    };

    if status.is_redirection() {
        return ProviderUsage::failed(
            ID,
            NAME,
            "HTTP 3xx — the session has expired; copy a fresh cookie from cursor.com",
        );
    }
    if !status.is_success() {
        let hints: &[(u16, &str)] = &[
            (401, " — the Cursor session has expired; copy a fresh cookie from cursor.com"),
            (403, " — the Cursor session has expired; copy a fresh cookie from cursor.com"),
            (429, " — rate limited, try again shortly"),
        ];
        return ProviderUsage::failed(ID, NAME, super::http_failure(status, hints));
    }

    reading(&body)
}

// ---------------------------------------------------------------------------
// Reading the reply
// ---------------------------------------------------------------------------

/// The weekly window, when the account actually has an allowance.
///
/// **The reply states no length**, which was checked rather than assumed:
/// Cursor's dashboard call and the Connect RPC behind it answer byte for byte,
/// and neither carries the reset a reference implementation decodes. So the
/// field is read where a reply offers one, the row simply has no reset line
/// where it does not, and the seven days are a **sort key, never a length to
/// divide by** — as Kimi's rolling week and Cursor's own billing cycle are.
/// That it is *weekly* is xAI's own word, not an inference from a start stamp.
///
/// **An absent `usagePercent` is not a zero here**, the opposite of the rule
/// `grok.rs` follows: this field is declared with explicit presence in the
/// schema Cursor ships, so absent means unset rather than a zero the serialiser
/// dropped.
///
/// **Nothing included is not nothing used.** An account with no Grok Bot
/// allowance answers 0% too, with the rest of the reply given over to upgrade
/// marketing — drawn literally that is a full green ring for something the
/// account does not have. So the account has to say it has an allowance before
/// a figure of nothing is believed to mean nothing *used*.
fn window(reply: &Value) -> Option<UsageWindow> {
    let flag = |key: &str| reply.get(key).and_then(Value::as_bool);

    if flag("usesPooledEnterpriseAllowance") == Some(true)
        || flag("includedLimitZero") == Some(true)
        || flag("hasNonZeroIncludedLimit") != Some(true)
    {
        return None;
    }

    // The service reports the share **gone**, 0…100, and the original holds it
    // to a share of the ring: a figure past either end is clamped rather than
    // dropped.
    let percent = super::dig_number(reply.get("usagePercent"))
        .filter(|percent| percent.is_finite())?;

    let reset = reply
        .get("nextResetTimestampUtc")
        .and_then(parse_reset);

    Some(
        UsageWindow::new("7d", Some(percent_from_scale(percent))).with_reset(reset),
    )
}

/// Why there is no window — and the reasons are different advice.
///
/// A plan that does not include Grok Bot is not a failure to report anything:
/// it is a complete answer, and "no limits reported" would send someone looking
/// for a fault that is not there.
///
/// **A reply that said nothing is not a reply that said no.** Every field here
/// is optional so a shape change costs one row rather than the card — but read
/// carelessly that turns any rename into a confident claim about somebody's
/// subscription. So the plan is only reported as excluding Grok Bot when the
/// reply actually *says* so.
fn absence(reply: &Value) -> ProviderUsage {
    let flag = |key: &str| reply.get(key).and_then(Value::as_bool);

    let said_something = flag("usesPooledEnterpriseAllowance").is_some()
        || flag("includedLimitZero").is_some()
        || flag("hasNonZeroIncludedLimit").is_some();
    if !said_something {
        return ProviderUsage::failed(ID, NAME, "the reply could not be read");
    }

    let entitled = flag("usesPooledEnterpriseAllowance") != Some(true)
        && flag("includedLimitZero") != Some(true)
        && flag("hasNonZeroIncludedLimit") == Some(true);

    ProviderUsage::failed(
        ID,
        NAME,
        if entitled {
            "no limits reported in the reply"
        } else {
            "This Cursor plan doesn't include Grok Bot."
        },
    )
}

/// The mapping, kept apart from the request so a fixture can drive it.
fn reading(body: &str) -> ProviderUsage {
    let reply: Value = match serde_json::from_str(body) {
        Ok(value @ Value::Object(_)) => value,
        _ => return ProviderUsage::failed(ID, NAME, "the reply could not be read"),
    };

    let Some(found) = window(&reply) else {
        return absence(&reply);
    };

    let plan = reply
        .get("grokPlanLabel")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|label| !label.is_empty())
        .map(str::to_string);

    ProviderUsage::ok(ID, NAME, vec![found]).with_plan(plan)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Built by hand from the field names this decodes, and from what the
    /// original records about the reply: nobody here holds a Cursor plan that
    /// includes Grok Bot to capture one from.
    fn reply(extra: &str) -> String {
        format!(
            r#"{{"currentPeriodStart":"2026-01-01T00:00:00Z",{extra}"grokPlanLabel":"Grok Bot Plan"}}"#
        )
    }

    fn included(extra: &str) -> String {
        reply(&format!(
            r#""hasNonZeroIncludedLimit":true,"includedLimitZero":false,"usesPooledEnterpriseAllowance":false,{extra}"#
        ))
    }

    #[test]
    fn a_normal_reply_becomes_one_weekly_window_with_a_stated_reset() {
        let usage = reading(&included(
            r#""nextResetTimestampUtc":"2026-01-08T00:00:00Z","usagePercent":30,"#,
        ));

        assert_eq!(usage.windows.len(), 1);
        assert_eq!(usage.windows[0].label, "7d");
        assert_eq!(usage.windows[0].percent_used, Some(30.0));
        assert_eq!(
            usage.windows[0].resets_at.as_deref(),
            Some("2026-01-08T00:00:00Z")
        );
        assert_eq!(usage.plan.as_deref(), Some("Grok Bot Plan"));
    }

    /// Neither Cursor's dashboard call nor the RPC behind it carries a reset,
    /// so the row simply has no reset line rather than falling back to
    /// `currentPeriodStart + 7 days`, which the vendor's own client does not do
    /// either.
    #[test]
    fn a_reply_that_never_states_a_reset_draws_a_window_with_no_reset_line() {
        let usage = reading(&included(r#""usagePercent":55,"#));
        assert_eq!(usage.windows[0].percent_used, Some(55.0));
        assert_eq!(usage.windows[0].resets_at, None);
    }

    /// Cursor's schema declares `usagePercent` with explicit presence: absent
    /// means unset, not a zero the serialiser dropped.
    #[test]
    fn an_absent_usage_percent_produces_no_window_even_on_an_included_plan() {
        let usage = reading(&included(""));
        assert!(usage.error.is_some());
    }

    /// Nothing included is not nothing used: a plan without Grok Bot answers 0%
    /// too, so the flags must say the plan includes it before a figure of
    /// nothing is believed to mean nothing used.
    #[test]
    fn a_plan_that_excludes_grok_bot_draws_no_window_despite_a_zero_percent_figure() {
        let excluded = reply(
            r#""usagePercent":0,"hasNonZeroIncludedLimit":false,"includedLimitZero":true,"usesPooledEnterpriseAllowance":false,"#,
        );
        let usage = reading(&excluded);
        assert!(usage.windows.is_empty());
        assert_eq!(
            usage.error.as_deref(),
            Some("This Cursor plan doesn't include Grok Bot.")
        );
    }

    /// A seat drawing on the organisation's pot has no personal allowance
    /// either, and is the same "not included" message as a plan that simply
    /// lacks the feature.
    #[test]
    fn a_pooled_enterprise_allowance_is_also_not_included() {
        let pooled = reading(&reply(
            r#""usagePercent":0,"usesPooledEnterpriseAllowance":true,"hasNonZeroIncludedLimit":true,"#,
        ));
        assert!(pooled.windows.is_empty());
        assert_eq!(
            pooled.error.as_deref(),
            Some("This Cursor plan doesn't include Grok Bot.")
        );
    }

    /// A reply carrying none of the three entitlement flags was one this could
    /// not read — a shape change, not a confident claim about the
    /// subscription.
    #[test]
    fn a_reply_saying_nothing_about_entitlement_at_all_is_unreadable() {
        let nothing = reading(r#"{"sandTrialExpiresAt":"2026-02-01T00:00:00Z","sandTrialCancelable":true}"#);
        assert!(nothing.windows.is_empty());
        assert_eq!(nothing.error.as_deref(), Some("the reply could not be read"));
    }

    #[test]
    fn a_usage_percent_of_100_reads_full() {
        let spent = reading(&included(
            r#""nextResetTimestampUtc":"2026-01-08T00:00:00Z","usagePercent":100,"#,
        ));
        assert_eq!(spent.windows[0].percent_used, Some(100.0));
    }

    /// The share is held to the ring at both ends, as the original holds it:
    /// over 100 is full, and a figure below zero is empty rather than dropped.
    #[test]
    fn a_share_outside_the_ring_is_held_to_it() {
        let over = reading(&included(r#""usagePercent":150,"#));
        assert_eq!(over.windows[0].percent_used, Some(100.0));

        let under = reading(&included(r#""usagePercent":-5,"#));
        assert_eq!(under.windows[0].percent_used, Some(0.0));
    }

    #[test]
    fn a_reply_that_is_not_an_object_cannot_be_read() {
        for body in ["not json", "[]", "{}"] {
            assert!(reading(body).error.is_some(), "read {body}");
        }
    }
}
