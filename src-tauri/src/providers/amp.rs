//! Amp, the coding agent: its free daily allowance, a paid tier's monthly
//! agent and Orb allowances, and the individual credit balance.
//!
//! Read with an access token the user pastes, from the read-only RPC Amp's own
//! CLI calls: `POST https://ampcode.com/api/internal?userDisplayBalanceInfo`
//! with a body that names the method and passes nothing.
//!
//! **The reply carries no fields — only the lines `amp usage` prints.** There
//! is no JSON object of figures to read: `result.displayText` is the text, and
//! a line this build does not know is skipped rather than guessed at. The
//! settings page only embeds the old free-tier object, so this is the route
//! that has the numbers.
//!
//! Every figure drawn is one the text states in both halves: "$18.57 of $20
//! remaining", "61% remaining today". Amp's "time to full" is not read — it
//! was an estimate from the replenishment rate, never a stated reset — and
//! neither are workspace balances, which have nowhere to sit beside the
//! account's own.

use std::sync::Arc;

use serde_json::Value;

use super::{
    by_window_length, describe_reqwest_error, find_ignoring_case, percent_from_fraction, Ctx,
    FetchFuture, Provider,
};
use crate::model::{ProviderUsage, UsageWindow};

const ENDPOINT: &str = "https://ampcode.com/api/internal?userDisplayBalanceInfo";

/// A paid allowance renews with the billing period, which is a month that is
/// not a fixed length: thirty days is a sort key only.
const MONTH: i64 = 30 * 86_400;

pub struct Amp;

impl Provider for Amp {
    fn id(&self) -> &'static str {
        "amp"
    }

    fn name(&self) -> &'static str {
        "Amp"
    }

    /// The token the fetch reads, and nothing else.
    fn is_configured(&self) -> bool {
        super::provider_key(self.id()).is_some()
    }

    fn fetch(&self, ctx: Arc<Ctx>) -> FetchFuture {
        Box::pin(async move { fetch_inner(ctx).await })
    }
}

async fn fetch_inner(ctx: Arc<Ctx>) -> ProviderUsage {
    const ID: &str = "amp";
    const NAME: &str = "Amp";

    let token = match super::provider_key(ID) {
        Some(token) => token,
        None => return ProviderUsage::failed(ID, NAME, super::missing_key(ID, "")),
    };

    let response = ctx
        .client
        .post(ENDPOINT)
        .header("Authorization", format!("Bearer {token}"))
        .header("Content-Type", "application/json")
        .header("Accept", "application/json")
        .body(r#"{"method":"userDisplayBalanceInfo","params":{}}"#)
        .send()
        .await;

    let response = match response {
        Ok(r) => r,
        Err(e) => {
            return ProviderUsage::failed(
                ID,
                NAME,
                format!("request failed: {}", describe_reqwest_error(&e)),
            )
        }
    };

    let status = response.status();
    let body = match response.text().await {
        Ok(t) => t,
        Err(e) => return ProviderUsage::failed(ID, NAME, format!("cannot read body: {e}")),
    };

    if !status.is_success() {
        let hints: &[(u16, &str)] = &[
            (401, " — the access token was refused"),
            (403, " — the access token was refused"),
            (429, " — rate limited, try again shortly"),
        ];
        return ProviderUsage::failed(ID, NAME, super::http_failure(status, hints));
    }

    let json: Value = match serde_json::from_str(&body) {
        Ok(v) => v,
        Err(e) => return ProviderUsage::failed(ID, NAME, format!("bad JSON: {e}")),
    };

    super::claude_code::debug_dump(ID, &json);

    reading(&json)
}

/// The mapping, kept apart from the request so a fixture can drive it.
fn reading(json: &Value) -> ProviderUsage {
    const ID: &str = "amp";
    const NAME: &str = "Amp";

    // Amp answers a dead token with an envelope rather than a status.
    if json.get("ok").and_then(Value::as_bool) == Some(false) {
        let refused =
            json.get("error").and_then(|e| e.get("code")).and_then(Value::as_str)
                == Some("auth-required");
        return ProviderUsage::failed(
            ID,
            NAME,
            if refused {
                "the access token was refused"
            } else {
                "the service reported a failure"
            },
        );
    }

    let text = json
        .get("result")
        .and_then(|result| result.get("displayText"))
        .and_then(Value::as_str)
        .unwrap_or("");

    if json.get("ok").and_then(Value::as_bool) != Some(true) || text.is_empty() {
        return ProviderUsage::failed(ID, NAME, "bad reply: no balance text");
    }

    reading_text(text)
}

/// One window as the text states it, with the id the original gives it.
///
/// The port's row carries a label and no id, so the id is kept beside the
/// window here: the tier and the subscription lines describe the same two
/// allowances, and the first one Amp printed is the one that stands.
struct Row {
    id: &'static str,
    seconds: i64,
    window: UsageWindow,
}

fn reading_text(text: &str) -> ProviderUsage {
    const ID: &str = "amp";
    const NAME: &str = "Amp";

    let cleaned = clean(text);
    let lines: Vec<&str> = cleaned.lines().collect();

    let mut rows: Vec<Row> = Vec::new();
    let mut plan: Option<String> = None;
    let mut credits: Option<f64> = None;

    // The dollar form is exact; the percentage one is rounded, so it only
    // counts when the other is not there.
    if let Some(free) = lines
        .iter()
        .find_map(|line| free_dollars(line))
        .or_else(|| lines.iter().find_map(|line| free_percent(line)))
    {
        rows.push(Row {
            id: "amp.free",
            seconds: 86_400,
            window: free,
        });
    }

    for line in &lines {
        if let Some((tier_plan, windows)) = tier(line) {
            plan = plan.or(Some(tier_plan));
            rows.extend(windows);
        } else if let Some((legacy_plan, windows)) = subscription(line) {
            plan = plan.or(Some(legacy_plan));
            rows.extend(windows);
        } else if credits.is_none() {
            credits = individual_credits(line);
        }
    }

    if rows.is_empty() && credits.is_none() {
        return ProviderUsage::failed(
            ID,
            NAME,
            if looks_signed_out(text) {
                "the access token was refused"
            } else {
                "bad reply: no balance lines this build knows"
            },
        );
    }

    let mut seen: Vec<&'static str> = Vec::new();
    let mut kept: Vec<(i64, UsageWindow)> = Vec::new();
    for row in rows {
        if seen.contains(&row.id) {
            continue;
        }
        seen.push(row.id);
        kept.push((row.seconds, row.window));
    }

    let mut windows = by_window_length(kept);
    if let Some(credits) = credits {
        windows.push(super::balance_window("Balance", format!("{credits:.2} USD")));
    }

    let mut usage = ProviderUsage::ok(ID, NAME, windows).with_plan(plan);
    if let Some(credits) = credits { usage = usage.with_credit_remaining(credits, "USD"); }
    usage
}

// ---------------------------------------------------------------------------
// The lines
// ---------------------------------------------------------------------------

/// "Amp Free: $6/$10 remaining (replenishes +$0.5/hour)". It refills by the
/// hour and never turns over, so it has no reset and no length.
fn free_dollars(line: &str) -> Option<UsageWindow> {
    let rest = after_prefix(line, "Amp Free:")?;
    let rest = rest.trim_start();
    let rest = rest.strip_prefix('$').unwrap_or(rest);
    let (remaining, used) = take_number(rest)?;
    let rest = rest[used..].trim_start().strip_prefix('/')?;
    let rest = rest.trim_start();
    let rest = rest.strip_prefix('$').unwrap_or(rest);
    let (limit, used) = take_number(rest)?;
    let rest = required_space(&rest[used..])?;
    literal(rest, "remaining")?;

    let used = used_fraction(remaining, limit)?;
    Some(UsageWindow::new(
        "Credits · Amp Free",
        Some(percent_from_fraction(used)),
    ))
}

/// "Amp Free: 61% remaining today (resets daily)". Daily is stated; the hour it
/// turns over is not, so no reset is given.
fn free_percent(line: &str) -> Option<UsageWindow> {
    let rest = after_prefix(line, "Amp Free:")?;
    let rest = rest.trim_start();
    let (remaining, used) = take_number(rest)?;
    let rest = rest[used..].trim_start().strip_prefix('%')?;
    let rest = required_space(rest)?;
    literal(rest, "remaining")?;
    if !(remaining.is_finite() && remaining >= 0.0) {
        return None;
    }

    let used = (100.0 - remaining.min(100.0)).max(0.0) / 100.0;
    Some(UsageWindow::new(
        "Daily · Amp Free",
        Some(percent_from_fraction(used)),
    ))
}

/// "Amp Megawatt Tier: agent usage $18.57 of $20 remaining (93%), orb usage
/// 732.8h of 750h a1.small orb hours remaining (98%) - period 2026-09-13 to
/// 2026-10-13, resets upon renewal in 27 days". Dollars and hours, not the
/// rounded percentages beside them.
fn tier(line: &str) -> Option<(String, Vec<Row>)> {
    let rest = after_prefix(line, "Amp")?;
    let rest = required_space(rest)?;
    let marker = find_ignoring_case(rest, " tier:")?;
    let plan = rest[..marker].trim();
    if plan.is_empty() {
        return None;
    }
    let plan = plan.to_string();

    let rest = rest[marker + " tier:".len()..].trim_start();
    let rest = literal(rest, "agent usage")?;
    let rest = required_space(rest)?.strip_prefix('$')?;
    let (remaining, used) = take_number(rest)?;
    let rest = required_space(&rest[used..])?;
    let rest = literal(rest, "of")?;
    let rest = required_space(rest)?.strip_prefix('$')?;
    let (limit, used) = take_number(rest)?;
    let rest = required_space(&rest[used..])?;
    let rest = literal(rest, "remaining")?;
    if rest.starts_with(|c: char| c.is_ascii_alphanumeric() || c == '_') {
        return None;
    }

    let resets_at = period_end(rest);
    let mut rows = Vec::new();
    if let Some(used) = used_fraction(remaining, limit) {
        rows.push(monthly("amp.agent", None, used, resets_at.clone()));
    }
    // Only the unit Amp names its allowance in; another size of machine would
    // be another allowance.
    if let Some((orb_remaining, orb_limit)) = orb_hours(rest) {
        if let Some(used) = used_fraction(orb_remaining, orb_limit) {
            rows.push(monthly("amp.orb", Some("Orb"), used, resets_at));
        }
    }

    Some((plan, rows))
}

/// The older wording, in percentages remaining: "Amp Megawatt Subscription:
/// 68% other usage and 97% orb usage remaining - resets upon renewal in 5
/// days", or "Subscription Megawatt: …".
fn subscription(line: &str) -> Option<(String, Vec<Row>)> {
    let tail = if let Some(rest) = after_prefix(line, "Amp") {
        let rest = required_space(rest)?;
        let marker = find_ignoring_case(rest, " subscription:")?;
        let plan = rest[..marker].trim().to_string();
        if plan.is_empty() {
            return None;
        }
        (plan, &rest[marker + " subscription:".len()..])
    } else if let Some(rest) = after_prefix(line, "Subscription") {
        let rest = required_space(rest)?;
        let marker = rest.find(':')?;
        let plan = rest[..marker].trim().to_string();
        if plan.is_empty() {
            return None;
        }
        (plan, &rest[marker + 1..])
    } else {
        return None;
    };

    let (plan, rest) = tail;
    let rest = rest.trim_start();
    let (other, used) = take_number(rest)?;
    let rest = rest[used..].trim_start().strip_prefix('%')?;
    let rest = required_space(rest)?;
    let rest = literal(rest, "other usage and")?;
    let rest = required_space(rest)?;
    let (orb, used) = take_number(rest)?;
    let rest = rest[used..].trim_start().strip_prefix('%')?;
    let rest = required_space(rest)?;
    literal(rest, "orb usage remaining")?;

    let parts: [(&'static str, Option<&'static str>, f64); 2] =
        [("amp.agent", None, other), ("amp.orb", Some("Orb"), orb)];
    let mut rows = Vec::new();
    for (id, scope, remaining) in parts {
        if !(remaining.is_finite() && remaining >= 0.0) {
            continue;
        }
        let used = (100.0 - remaining.min(100.0)).max(0.0) / 100.0;
        rows.push(monthly(id, scope, used, None));
    }

    Some((plan, rows))
}

/// "Individual credits: $1,020.50 remaining (replenishes automatically)".
fn individual_credits(line: &str) -> Option<f64> {
    let rest = after_prefix(line, "Individual credits:")?;
    let rest = rest.trim_start();
    let rest = rest.strip_prefix('$').unwrap_or(rest);
    let (amount, used) = take_number(rest)?;
    let rest = required_space(&rest[used..])?;
    literal(rest, "remaining")?;
    if amount.is_finite() && amount >= 0.0 {
        Some(amount)
    } else {
        None
    }
}

/// A paid allowance renews with the billing period: a month whose length is a
/// sort key, never a claim, and which reports no reset unless the text named a
/// period.
fn monthly(id: &'static str, scope: Option<&str>, used: f64, resets_at: Option<String>) -> Row {
    let label = match scope {
        Some(scope) => format!("Monthly · {scope}"),
        None => "Monthly".to_string(),
    };
    Row {
        id,
        seconds: MONTH,
        window: UsageWindow::new(label, Some(percent_from_fraction(used))).with_reset(resets_at),
    }
}

/// `orb usage 732.8h of 750h a1.small orb hours remaining` — the two hours.
fn orb_hours(text: &str) -> Option<(f64, f64)> {
    let mut from = 0;
    while let Some(at) = find_ignoring_case(&text[from..], "orb usage") {
        let start = from + at + "orb usage".len();
        let rest = required_space(&text[start..])?;
        if let Some((remaining, used)) = take_number(rest) {
            if let Some(rest) = rest[used..].strip_prefix('h') {
                if let Some(rest) = required_space(rest).and_then(|r| literal(r, "of")) {
                    if let Some(rest) = required_space(rest) {
                        if let Some((limit, used)) = take_number(rest) {
                            if let Some(rest) = rest[used..].strip_prefix('h') {
                                if let Some(rest) = required_space(rest) {
                                    if literal(rest, "a1.small orb hours remaining").is_some() {
                                        return Some((remaining, limit));
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
        from = start;
    }
    None
}

/// "period 2026-09-13 to 2026-10-13": dates only, so the renewal is taken as
/// the start of that day in UTC. The countdown beside it ("in 27 days") is
/// rounded and moves every refresh, so it is not used.
fn period_end(text: &str) -> Option<String> {
    let mut from = 0;
    while let Some(at) = find_ignoring_case(&text[from..], "period") {
        let start = from + at + "period".len();
        // `\bperiod\s+`: the word, then whitespace, and the word may not run on.
        let before_ok = at == 0 || !word_char(text.as_bytes()[from + at - 1]);
        if before_ok {
            if let Some(rest) = required_space(&text[start..]) {
                if let Some((start_date, used)) = take_date(rest) {
                    if let Some(rest) = required_space(&rest[used..]).and_then(|r| literal(r, "to")) {
                        if let Some(rest) = required_space(rest) {
                            if let Some((end_date, _)) = take_date(rest) {
                                if end_date > start_date {
                                    return Some(format!("{end_date}T00:00:00Z"));
                                }
                            }
                        }
                    }
                }
            }
        }
        from = start;
    }
    None
}

// ---------------------------------------------------------------------------
// Reading a line, the way the original's regular expressions do
// ---------------------------------------------------------------------------

/// Terminal colour codes and Markdown bold, which the text may carry.
fn clean(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\u{1b}' {
            // `\u{1B}\[[0-9;]*[A-Za-z]`, the colour code Amp prints.
            if chars.peek() == Some(&'[') {
                chars.next();
                while matches!(chars.peek(), Some(c) if c.is_ascii_digit() || *c == ';') {
                    chars.next();
                }
                if matches!(chars.peek(), Some(c) if c.is_ascii_alphabetic()) {
                    chars.next();
                }
            }
            continue;
        }
        out.push(c);
    }
    out.replace("**", "")
}

fn looks_signed_out(text: &str) -> bool {
    let lower = text.to_lowercase();
    !lower.contains("signed in as")
        && (lower.contains("sign in") || lower.contains("log in") || lower.contains("login"))
}

/// Used over limit, from what is left of a stated limit. More left than the
/// limit is nothing used, not a negative.
fn used_fraction(remaining: f64, limit: f64) -> Option<f64> {
    if !remaining.is_finite() || !limit.is_finite() || remaining < 0.0 || limit <= 0.0 {
        return None;
    }
    Some((limit - remaining).max(0.0) / limit)
}

fn word_char(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'_'
}

/// `[0-9][0-9,]*(?:\.[0-9]+)?` at the head of `text`, with the bytes it used.
///
/// Spelling the original's `number` group out here rather than pulling in a
/// regular-expression engine: these lines are the whole grammar, and a figure
/// is the only thing in them that is not a fixed word.
fn take_number(text: &str) -> Option<(f64, usize)> {
    let bytes = text.as_bytes();
    if bytes.is_empty() || !bytes[0].is_ascii_digit() {
        return None;
    }
    let mut end = 0;
    while end < bytes.len() && (bytes[end].is_ascii_digit() || bytes[end] == b',') {
        end += 1;
    }
    if end < bytes.len() && bytes[end] == b'.' {
        let mut fraction = end + 1;
        while fraction < bytes.len() && bytes[fraction].is_ascii_digit() {
            fraction += 1;
        }
        if fraction > end + 1 {
            end = fraction;
        }
    }
    let raw: String = text[..end].chars().filter(|c| *c != ',').collect();
    let value = raw.parse::<f64>().ok()?;
    Some((value, end))
}

/// `\d{4}-\d{2}-\d{2}`, as a date and the day it names at midnight UTC.
fn take_date(text: &str) -> Option<(chrono::NaiveDate, usize)> {
    const SHAPE: usize = 10;
    let head = text.get(..SHAPE)?;
    let shape_ok = head
        .bytes()
        .enumerate()
        .all(|(index, byte)| match index {
            4 | 7 => byte == b'-',
            _ => byte.is_ascii_digit(),
        });
    if !shape_ok {
        return None;
    }
    let date = chrono::NaiveDate::parse_from_str(head, "%Y-%m-%d").ok()?;
    Some((date, SHAPE))
}

/// `\s*` — the whitespace may be absent.
fn skip_spaces(text: &str) -> &str {
    text.trim_start()
}

/// `\s+` — the whitespace must be there, so the word cannot run into the one
/// before it.
fn required_space(text: &str) -> Option<&str> {
    let rest = skip_spaces(text);
    if rest.len() == text.len() {
        None
    } else {
        Some(rest)
    }
}

fn literal<'a>(text: &'a str, word: &str) -> Option<&'a str> {
    let head = text.get(..word.len())?;
    if head.eq_ignore_ascii_case(word) {
        Some(&text[word.len()..])
    } else {
        None
    }
}

/// The line with its leading blanks gone, when it starts with `prefix`; every
/// pattern in the original is anchored at the start of a line.
fn after_prefix<'a>(line: &'a str, prefix: &str) -> Option<&'a str> {
    let head = skip_spaces(line);
    literal(head, prefix)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// The reply's shape: the lines `amp usage` prints, in `displayText`.
    fn reply(text: &str) -> Value {
        json!({ "ok": true, "result": { "displayText": text } })
    }

    /// A percentage as the arithmetic lands, not as the decimal is written:
    /// `$18.57 of $20` is 1.43/20 in binary either way but not bit for bit.
    fn percent(window: &UsageWindow, expected: f64) -> bool {
        matches!(window.percent_used, Some(value) if (value - expected).abs() < 1e-9)
    }

    /// Second-hand, from the original's fixture: a paid tier, Amp Free in
    /// percentages, and the individual credits.
    fn tier_fixture() -> Value {
        reply(
            "Signed in as user@example.com\n\
             **Amp Free:** 61% remaining today (resets daily) - https://ampcode.com/settings#amp-free\n\
             Amp Megawatt Tier: agent usage $18.57 of $20 remaining (93%), orb usage 732.8h of 750h a1.small orb hours remaining (98%) - period 2026-09-13 to 2026-10-13, resets upon renewal in 27 days\n\
             Individual credits: $1,020.50 remaining (replenishes automatically) - https://ampcode.com/settings",
        )
    }

    /// Second-hand, from the original's fixture: the older wording, colour
    /// codes and Markdown bold included.
    fn legacy_fixture() -> Value {
        reply(
            "\u{1b}[1mSigned in as cli@example.com (team)\u{1b}[0m\n\
             Amp Free: $6/$10 remaining (replenishes +$0.5/hour)\n\
             Amp Free: 61% remaining today (resets daily)\n\
             Subscription Megawatt: 97% other usage and 100% orb usage remaining - resets upon renewal in 29 days - https://ampcode.com/settings#subscription\n\
             Workspace Test Team: $7.25 remaining",
        )
    }

    #[test]
    fn reads_a_paid_tier_beside_amp_free_and_the_credits() {
        let usage = reading(&tier_fixture());
        let labels: Vec<&str> = usage.windows.iter().map(|w| w.label.as_str()).collect();
        assert_eq!(
            labels,
            vec!["Daily · Amp Free", "Monthly", "Monthly · Orb", "Balance"]
        );
        assert_eq!(usage.plan.as_deref(), Some("Megawatt"));

        assert!(percent(&usage.windows[0], 39.0));
        // Daily is stated; the hour it turns over is not.
        assert!(usage.windows[0].resets_at.is_none());
        assert!(percent(&usage.windows[1], 100.0 * 1.43 / 20.0));
        assert!(percent(&usage.windows[2], 100.0 * 17.2 / 750.0));
        // Both allowances renew with the period the tier line named.
        assert_eq!(
            usage.windows[1].resets_at.as_deref(),
            Some("2026-10-13T00:00:00Z")
        );
        assert_eq!(usage.windows[2].resets_at, usage.windows[1].resets_at);
    }

    #[test]
    fn the_credits_are_a_balance_and_not_a_ring() {
        let usage = reading(&tier_fixture());
        assert_eq!(usage.credit_remaining.as_ref().unwrap().amount, 1020.5);
        assert_eq!(usage.credit_remaining.as_ref().unwrap().currency, "USD");
        let balance = usage.windows.last().unwrap();
        assert_eq!(balance.detail.as_deref(), Some("1020.50 USD"));
        assert_eq!(balance.percent_used, None);
    }

    /// The older wording reads too, dollars win over the rounded free
    /// percentage, and a workspace balance has nowhere to sit.
    #[test]
    fn reads_the_older_wording_and_prefers_the_dollar_form() {
        let usage = reading(&legacy_fixture());
        assert!(usage.credit_remaining.is_none());
        assert_eq!(usage.plan.as_deref(), Some("Megawatt"));

        let free = &usage.windows[0];
        // "Amp Free: $6/$10" — the dollar line, not the 61% one.
        assert!(percent(free, 40.0));
        assert!(percent(&usage.windows[1], 3.0));
        assert!(percent(&usage.windows[2], 0.0));
        // The countdown alone is rounded and moves; it is not a reset.
        assert!(usage.windows.iter().all(|w| w.resets_at.is_none()));
        // No credits line, and the workspace balance is not one.
        assert_eq!(usage.windows.len(), 3);
    }

    /// A limit of nothing is left off; the credits beside it still show.
    #[test]
    fn a_limit_of_nothing_is_left_off_and_the_credits_remain() {
        let usage = reading(&reply(
            "Amp Megawatt Tier: agent usage $0 of $0 remaining - resets upon renewal in 27 days\n\
             Individual credits: $12 remaining",
        ));
        assert!(usage.error.is_none());
        assert_eq!(usage.windows.len(), 1);
        assert_eq!(usage.windows[0].detail.as_deref(), Some("12.00 USD"));
    }

    /// More left than the limit is nothing used, not a negative.
    #[test]
    fn more_left_than_the_limit_is_nothing_used() {
        let usage = reading(&reply(
            "Amp Example Tier: agent usage $1,100 of $1,000 remaining - resets upon renewal in 1 month",
        ));
        assert_eq!(usage.windows[0].percent_used, Some(0.0));
    }

    /// Only a1.small Orb hours are an allowance this reads.
    #[test]
    fn another_size_of_orb_machine_is_not_this_allowance() {
        let usage = reading(&reply(
            "Amp Example Tier: agent usage $3 of $20 remaining (15%), orb usage 12h of 50h a1.large orb hours remaining - resets upon renewal in 2 days",
        ));
        let labels: Vec<&str> = usage.windows.iter().map(|w| w.label.as_str()).collect();
        assert_eq!(labels, vec!["Monthly"]);
        assert!(percent(&usage.windows[0], 85.0));
    }

    /// A dead token is refused, whether Amp says so in the envelope or only in
    /// the text.
    #[test]
    fn a_dead_token_is_refused_in_the_envelope_and_in_the_text() {
        let envelope = json!({ "ok": false, "error": { "code": "auth-required", "message": "Sign in" } });
        let usage = reading(&envelope);
        assert!(usage.error.as_deref().unwrap().contains("refused"));

        let text = reading(&reply("Please log in to use Amp."));
        assert!(text.error.as_deref().unwrap().contains("refused"));
    }

    /// Any other failure in the envelope is the service's.
    #[test]
    fn another_failure_in_the_envelope_is_the_services() {
        let envelope = json!({ "ok": false, "error": { "code": "internal", "message": "oops" } });
        let usage = reading(&envelope);
        assert!(usage.error.as_deref().unwrap().contains("service"));
    }

    #[test]
    fn a_reply_this_build_cannot_read_is_unreadable() {
        for body in [
            json!({}),
            json!({ "ok": true }),
            json!("not json"),
            reply(""),
            reply("Signed in as a@b.c\nSomething new"),
            json!({ "result": { "displayText": "Amp Free: 61% remaining today" } }),
        ] {
            assert!(reading(&body).error.is_some(), "read {body}");
        }
    }

    /// Only a line that states both halves is a window; a percentage alone
    /// under a known name is still read, and a line this build does not know
    /// is skipped rather than guessed at.
    #[test]
    fn a_line_this_build_does_not_know_is_skipped() {
        let usage = reading(&reply(
            "Amp Free: 61% remaining today (resets daily)\n\
             Something new: 90% used",
        ));
        assert_eq!(usage.windows.len(), 1);
        assert!(percent(&usage.windows[0], 39.0));

        // The free percentage without its "remaining" is not that line.
        assert!(reading(&reply("Amp Free: 61% used today")).error.is_some());
    }

    /// The countdown beside the period is not a reset; the period's end is.
    #[test]
    fn only_a_period_states_a_reset() {
        let counted = reading(&reply(
            "Amp Example Tier: agent usage $3 of $20 remaining - resets upon renewal in 5 days",
        ));
        assert!(counted.windows[0].resets_at.is_none());

        // A period that ends before it starts names no reset either.
        let backwards = reading(&reply(
            "Amp Example Tier: agent usage $3 of $20 remaining - period 2026-10-13 to 2026-09-13",
        ));
        assert!(backwards.windows[0].resets_at.is_none());
    }

    #[test]
    fn terminal_colour_and_markdown_bold_are_stripped() {
        assert_eq!(clean("\u{1b}[1m**Amp**\u{1b}[0m"), "Amp");
        assert!(looks_signed_out("Please log in to use Amp."));
        assert!(!looks_signed_out("Signed in as a@b.c\nlogin now"));
    }
}
