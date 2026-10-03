//! JetBrains AI: the AI Assistant quota a JetBrains IDE keeps on this machine.
//!
//! Nothing is asked of any server. Every JetBrains IDE with AI Assistant writes
//! the quota it last heard from JetBrains into its own settings folder —
//! `%APPDATA%\JetBrains\<IDE><version>\options\AIAssistantQuotaManager2.xml`,
//! and Android Studio's under `%APPDATA%\Google\` — as two XML attributes
//! holding JSON. PulseWin reads the most recently written one and never writes
//! to it. The shape is second-hand — taken from CodexBar's JetBrains provider
//! and its tests, not from a file on this machine — and the fixture below says
//! so.
//!
//! The original reads `~/Library/Application Support/JetBrains` and
//! `~/Library/Application Support/Google`; on Windows those two roots are the
//! same folders under `%APPDATA%`, which is what `credentials::config_relative`
//! resolves to on either platform.
//!
//! **As fresh as the IDE left it.** The file changes only while an IDE is
//! running and talking to JetBrains; with every IDE closed the figure is the
//! last one any of them saw. The pane says so rather than pretending to be live
//! in a way it is not.
//!
//! The quota is `current` of `maximum`, both stated. Its length is the refill
//! tariff's `duration` when that is an ISO 8601 length (`PT720H`), and is not
//! claimed when it isn't.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::SystemTime;

use chrono::{DateTime, Utc};
use serde_json::Value;

use super::{humanize_window_seconds, Ctx, FetchFuture, Provider};
use crate::credentials;
use crate::model::{ProviderUsage, UsageWindow};

const ID: &str = "jetbrains-ai";
const NAME: &str = "JetBrains AI";

/// The one component the quota lives in, and the file it is written to under an
/// IDE's own settings folder.
const COMPONENT: &str = "AIAssistantQuotaManager2";
const QUOTA_FILE: &str = "AIAssistantQuotaManager2.xml";

/// The settings folders that belong to an IDE with AI Assistant, by the name
/// each one starts with. Anything else under those roots — JetBrains Toolbox,
/// Chrome under `Google/` — is not looked in.
const IDE_FOLDERS: [&str; 15] = [
    "IntelliJIdea",
    "PyCharm",
    "WebStorm",
    "GoLand",
    "CLion",
    "DataGrip",
    "RubyMine",
    "Rider",
    "PhpStorm",
    "AppCode",
    "Fleet",
    "AndroidStudio",
    "RustRover",
    "Aqua",
    "DataSpell",
];

pub struct JetBrainsAI;

impl Provider for JetBrainsAI {
    fn id(&self) -> &'static str {
        ID
    }

    fn name(&self) -> &'static str {
        NAME
    }

    /// Whether an IDE here has written a quota at all.
    ///
    /// **The file is the credential.** Nothing is pasted and nothing is asked
    /// of a server, so the presence question is whether one of the IDEs has
    /// saved a quota — a machine with JetBrains IDEs but no AI Assistant has
    /// nothing this provider could show. Answered from the filesystem alone.
    fn is_configured(&self) -> bool {
        newest_quota_file(&settings_roots()).is_some()
    }

    fn fetch(&self, _ctx: Arc<Ctx>) -> FetchFuture {
        Box::pin(async move { fetch_inner().await })
    }
}

/// Where the IDEs keep their settings folders. On Windows both are under
/// `%APPDATA%`, which is what the credential helpers resolve.
fn settings_roots() -> Vec<PathBuf> {
    let mut roots = credentials::config_relative(&["JetBrains"]);
    roots.extend(credentials::config_relative(&["Google"]));
    roots
}

async fn fetch_inner() -> ProviderUsage {
    let Some(file) = newest_quota_file(&settings_roots()) else {
        return ProviderUsage::failed(ID, NAME, Reason::LocalAppMissing.message());
    };

    let Ok(xml) = std::fs::read_to_string(&file) else {
        return ProviderUsage::failed(ID, NAME, Reason::UnreadableReply.message());
    };

    match reading(&xml) {
        Ok(usage) => usage,
        Err(reason) => ProviderUsage::failed(ID, NAME, reason.message()),
    }
}

// ---------------------------------------------------------------------------
// Finding the file
// ---------------------------------------------------------------------------

/// What the search reads from disk.
///
/// Injected so a test can drive it without a real IDE on the machine: the rule
/// worth pinning down is which of several files wins, not how a folder is
/// walked. The original injects its existence check for the same reason.
struct Files<'a> {
    /// The names directly under a folder — empty when it cannot be read at all.
    names: &'a dyn Fn(&Path) -> Vec<String>,
    is_file: &'a dyn Fn(&Path) -> bool,
    /// When a file was written, or nothing when that cannot be read.
    modified: &'a dyn Fn(&Path) -> Option<SystemTime>,
}

/// The quota file written last, across every IDE that has one.
fn newest_quota_file(roots: &[PathBuf]) -> Option<PathBuf> {
    newest_of(
        roots,
        &Files {
            names: &|root| {
                std::fs::read_dir(root)
                    .map(|entries| {
                        entries
                            .flatten()
                            .filter_map(|entry| entry.file_name().into_string().ok())
                            .collect()
                    })
                    .unwrap_or_default()
            },
            is_file: &|path| path.is_file(),
            modified: &|path| {
                std::fs::metadata(path)
                    .and_then(|data| data.modified())
                    .ok()
            },
        },
    )
}

/// The rule itself: every IDE folder under every root, its quota file when it
/// has one, and the newest of those.
///
/// A file whose modification time cannot be read sorts below every one that
/// can, as `Date.distantPast` does in the original, and ties keep the first —
/// Swift's `max(by:)` only replaces on a strict increase.
fn newest_of(roots: &[PathBuf], files: &Files<'_>) -> Option<PathBuf> {
    let mut newest: Option<(Option<SystemTime>, PathBuf)> = None;

    for root in roots {
        for name in (files.names)(root) {
            if !IDE_FOLDERS
                .iter()
                .any(|ide| name.to_lowercase().starts_with(&ide.to_lowercase()))
            {
                continue;
            }

            let file = root.join(&name).join("options").join(QUOTA_FILE);
            if !(files.is_file)(&file) {
                continue;
            }
            let modified = (files.modified)(&file);

            let better = match &newest {
                Some((best, _)) => modified > *best,
                None => true,
            };
            if better {
                newest = Some((modified, file));
            }
        }
    }

    newest.map(|(_, file)| file)
}

// ---------------------------------------------------------------------------
// Reading the file
// ---------------------------------------------------------------------------

/// The reasons this provider can leave nothing to show.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Reason {
    /// No IDE has written a quota here yet.
    LocalAppMissing,
    UnreadableReply,
    NoLimitsReported,
}

impl Reason {
    fn message(self) -> &'static str {
        match self {
            Reason::LocalAppMissing => {
                "nothing saved by a JetBrains IDE yet — open one with AI Assistant, then retry"
            }
            Reason::UnreadableReply => "unreadable reply",
            Reason::NoLimitsReported => "no limits reported",
        }
    }
}

fn reading(xml: &str) -> Result<ProviderUsage, Reason> {
    // The IDE writes the component once AI Assistant has asked for a quota; a
    // file without it has nothing in it yet.
    let Some(quota) = option_value(xml, COMPONENT, "quotaInfo") else {
        return Err(Reason::LocalAppMissing);
    };
    let quota = json_object(&quota).ok_or(Reason::LocalAppMissing)?;

    let used = number(quota.get("current")).filter(|used| *used >= 0.0);
    let maximum = number(quota.get("maximum")).filter(|maximum| *maximum > 0.0);
    let (Some(used), Some(maximum)) = (used, maximum) else {
        return Err(Reason::NoLimitsReported);
    };

    let refill = option_value(xml, COMPONENT, "nextRefill").and_then(|text| json_object(&text));
    let duration = refill
        .as_ref()
        .and_then(|refill| {
            refill.get("duration").or_else(|| {
                refill
                    .get("tariff")
                    .and_then(|tariff| tariff.get("duration"))
            })
        })
        .and_then(Value::as_str);
    let stated = duration.and_then(seconds_from_iso_duration);

    let fraction = used / maximum;
    let window = UsageWindow::new(
        // A length the IDE stated is named by that length; a quota with no
        // stated length is an allowance in credits, and says so instead of
        // claiming a month.
        stated
            .map(humanize_window_seconds)
            .unwrap_or_else(|| "Credits".to_string()),
        Some(super::percent_from_fraction(fraction)),
    )
    .with_reset(
        refill
            .as_ref()
            .and_then(|refill| refill.get("next"))
            .and_then(Value::as_str)
            .and_then(iso_stamp),
    );

    Ok(ProviderUsage::ok(ID, NAME, vec![window]))
}

/// A JSON object out of an XML attribute, or nothing when it is not one.
fn json_object(text: &str) -> Option<Value> {
    match serde_json::from_str::<Value>(text).ok()? {
        value @ Value::Object(_) => Some(value),
        _ => None,
    }
}

/// The IDE writes its figures as strings; a number is taken as well.
///
/// **A boolean is not a figure.** Foundation's `NSNumber` would hand one over
/// as 0 or 1 here, which is not something the IDE ever means.
fn number(value: Option<&Value>) -> Option<f64> {
    let parsed = match value? {
        Value::String(text) => text.parse::<f64>().ok()?,
        Value::Number(number) => number.as_f64()?,
        _ => return None,
    };
    parsed.is_finite().then_some(parsed)
}

/// ISO 8601 with or without fractional seconds, the two shapes a reset arrives
/// in — and nothing else, which is what the original's own parser takes.
fn iso_stamp(text: &str) -> Option<String> {
    DateTime::parse_from_rfc3339(text.trim())
        .ok()
        .map(|at| {
            at.with_timezone(&Utc)
                .to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
        })
}

/// `PT720H`, `P30D`, `P1DT12H` — the lengths an ISO 8601 duration can state
/// exactly. Months and years are not a fixed number of seconds, and anything
/// that is not a duration at all (`monthly`) is not guessed at.
fn seconds_from_iso_duration(text: &str) -> Option<i64> {
    let rest = text.strip_prefix('P')?;
    let (date, time) = match rest.split_once('T') {
        Some((date, time)) => (date, Some(time)),
        None => (rest, None),
    };

    let mut total: i64 = 0;
    let mut remaining = date;
    for (marker, seconds) in [("W", 7 * 86_400), ("D", 86_400)] {
        if let Some((value, rest)) = leading_digits(remaining) {
            if let Some(after) = rest.strip_prefix(marker) {
                total = total.checked_add(value.checked_mul(seconds)?)?;
                remaining = after;
            }
        }
    }
    if !remaining.is_empty() {
        return None;
    }

    if let Some(mut remaining) = time {
        for (marker, seconds) in [("H", 3_600), ("M", 60), ("S", 1)] {
            if let Some((value, rest)) = leading_digits(remaining) {
                if let Some(after) = rest.strip_prefix(marker) {
                    total = total.checked_add(value.checked_mul(seconds)?)?;
                    remaining = after;
                }
            }
        }
        if !remaining.is_empty() {
            return None;
        }
    }

    (total > 0).then_some(total)
}

/// The digits at the front of `text`, and what follows them.
fn leading_digits(text: &str) -> Option<(i64, &str)> {
    let end = text
        .find(|character: char| !character.is_ascii_digit())
        .unwrap_or(text.len());
    if end == 0 {
        return None;
    }
    text[..end].parse::<i64>().ok().map(|value| (value, &text[end..]))
}

// ---------------------------------------------------------------------------
// The XML itself
// ---------------------------------------------------------------------------

/// The value of `option[@name='…']` inside `component[@name='…']`, which is
/// what the original asks for in one XPath.
///
/// Hand-written rather than a dependency: one attribute of one element of one
/// file is the whole job, and the crate carries no XML parser. Both the option
/// and the component have to be the ones asked for — an option of the same name
/// in another component is another provider's figure.
fn option_value(xml: &str, component: &str, option: &str) -> Option<String> {
    let mut depth = 0usize;

    for tag in tags(xml) {
        if tag.name == "component" {
            if tag.closing {
                if depth > 0 {
                    depth -= 1;
                }
                continue;
            }
            if depth > 0 {
                // A component inside the one being read is not the one asked
                // for, and neither are its options.
                if !tag.self_closing {
                    depth += 1;
                }
                continue;
            }
            if tag.attribute("name").as_deref() == Some(component) && !tag.self_closing {
                depth = 1;
            }
            continue;
        }

        // Only a direct child of the component, as the XPath states.
        if depth == 1 && tag.name == "option" && !tag.closing {
            if tag.attribute("name").as_deref() == Some(option) {
                return tag.attribute("value");
            }
        }
    }

    None
}

struct Tag<'a> {
    name: &'a str,
    closing: bool,
    self_closing: bool,
    attributes: Vec<(&'a str, String)>,
}

impl Tag<'_> {
    fn attribute(&self, name: &str) -> Option<String> {
        self.attributes
            .iter()
            .find(|(key, _)| *key == name)
            .map(|(_, value)| value.clone())
    }
}

/// Every element tag in the document, in order.
fn tags(xml: &str) -> Vec<Tag<'_>> {
    let mut found = Vec::new();
    let mut rest = xml;

    while let Some(start) = rest.find('<') {
        let after = &rest[start..];

        // Comments, declarations and CDATA hold no elements.
        if let Some(comment) = after.strip_prefix("<!--") {
            match comment.find("-->") {
                Some(end) => rest = &comment[end + 3..],
                None => break,
            }
            continue;
        }
        if after.starts_with("<?") || after.starts_with("<!") {
            match after.find('>') {
                Some(end) => rest = &after[end + 1..],
                None => break,
            }
            continue;
        }

        let Some(end) = after.find('>') else {
            break;
        };
        let raw = &after[1..end];
        rest = &after[end + 1..];

        let closing = raw.starts_with('/');
        let body = raw.trim_start_matches('/').trim_end();
        let self_closing = body.ends_with('/');
        let body = body.strip_suffix('/').unwrap_or(body).trim_end();

        let name_end = body
            .find(|character: char| character.is_whitespace())
            .unwrap_or(body.len());
        let (name, attributes) = body.split_at(name_end);
        if name.is_empty() {
            continue;
        }

        found.push(Tag {
            name,
            closing,
            self_closing,
            attributes: attributes_of(attributes),
        });
    }

    found
}

/// The `name="value"` pairs of a tag's remainder, in the order they are written.
fn attributes_of(text: &str) -> Vec<(&str, String)> {
    let mut found = Vec::new();
    let mut rest = text.trim();

    while let Some(equals) = rest.find('=') {
        let name = rest[..equals].trim();
        let after = rest[equals + 1..].trim_start();

        let (value, remainder) = match after.chars().next() {
            Some(quote @ ('"' | '\'')) => {
                let text = &after[quote.len_utf8()..];
                match text.find(quote) {
                    Some(end) => (&text[..end], &text[end + quote.len_utf8()..]),
                    None => (text, ""),
                }
            }
            // An unquoted value runs to the next space.
            _ => {
                let end = after
                    .find(|character: char| character.is_whitespace())
                    .unwrap_or(after.len());
                (&after[..end], &after[end..])
            }
        };

        if let Some(name) = name.rsplit(|character: char| character.is_whitespace()).next() {
            if !name.is_empty() {
                found.push((name, unescape(value)));
            }
        }
        rest = remainder;
    }

    found
}

/// The entities an XML attribute may carry. The IDE writes its JSON with every
/// quote escaped, so a value is not JSON until this has run.
fn unescape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;

    while let Some(start) = rest.find('&') {
        out.push_str(&rest[..start]);
        let after = &rest[start..];
        let Some(end) = after.find(';').filter(|end| *end <= 12) else {
            out.push('&');
            rest = &after[1..];
            continue;
        };

        let entity = &after[1..end];
        let replacement = match entity {
            "lt" => Some('<'),
            "gt" => Some('>'),
            "quot" => Some('"'),
            "apos" => Some('\''),
            "amp" => Some('&'),
            _ => entity
                .strip_prefix("#x")
                .or_else(|| entity.strip_prefix("#X"))
                .and_then(|hex| u32::from_str_radix(hex, 16).ok())
                .or_else(|| {
                    entity
                        .strip_prefix('#')
                        .and_then(|digits| digits.parse::<u32>().ok())
                })
                .and_then(char::from_u32),
        };

        match replacement {
            Some(character) => out.push(character),
            None => out.push_str(&after[..end + 1]),
        }
        rest = &after[end + 1..];
    }

    out.push_str(rest);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// A quota file's shape, as CodexBar's tests describe it: two XML
    /// attributes, each holding JSON with its quotes escaped.
    fn fixture(refill: &str) -> String {
        format!(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<application>
  <component name="AIAssistantQuotaManager2">
    <option name="quotaInfo" value="{{&quot;current&quot;:&quot;42.5&quot;,&quot;maximum&quot;:&quot;100&quot;}}" />
    {refill}
  </component>
</application>"#
        )
    }

    fn refill(duration: &str) -> String {
        format!(
            r#"<option name="nextRefill" value="{{&quot;duration&quot;:&quot;{duration}&quot;,&quot;next&quot;:&quot;2026-11-01T00:00:00Z&quot;}}" />"#
        )
    }

    /// A component holding one `quotaInfo`, as the IDE writes it.
    fn quota(json: &str) -> String {
        format!(
            r#"<application><component name="AIAssistantQuotaManager2">
            <option name="quotaInfo" value="{json}" />
            </component></application>"#
        )
    }

    #[test]
    fn reads_the_quota_and_the_length_the_refill_states() {
        let usage = reading(&fixture(&refill("PT720H"))).unwrap();

        assert_eq!(usage.windows.len(), 1);
        // 42.5 of 100, both written as strings by the IDE.
        assert_eq!(usage.windows[0].percent_used, Some(42.5));
        // 720 hours is thirty days, which is the heading it is given.
        assert_eq!(usage.windows[0].label, "30d");
        assert_eq!(
            usage.windows[0].resets_at.as_deref(),
            Some("2026-11-01T00:00:00Z")
        );
        assert!(usage.error.is_none());
    }

    #[test]
    fn a_quota_with_no_stated_length_is_an_allowance_in_credits() {
        let usage = reading(&fixture("")).unwrap();
        assert_eq!(usage.windows[0].label, "Credits");
        assert!(usage.windows[0].resets_at.is_none());

        // A refill with a length that is not one states no length either —
        // and its reset is still read.
        let usage = reading(&fixture(&refill("monthly"))).unwrap();
        assert_eq!(usage.windows[0].label, "Credits");
        assert!(usage.windows[0].resets_at.is_some());
    }

    #[test]
    fn the_tariffs_own_duration_is_read_too() {
        let tariff = r#"<option name="nextRefill" value="{&quot;tariff&quot;:{&quot;duration&quot;:&quot;P30D&quot;}}" />"#;
        let usage = reading(&fixture(tariff)).unwrap();
        assert_eq!(usage.windows[0].label, "30d");

        // The refill's own duration wins over the tariff's.
        let both = r#"<option name="nextRefill" value="{&quot;duration&quot;:&quot;PT12H&quot;,&quot;tariff&quot;:{&quot;duration&quot;:&quot;P30D&quot;}}" />"#;
        let usage = reading(&fixture(both)).unwrap();
        assert_eq!(usage.windows[0].label, "12h");
    }

    #[test]
    fn only_the_component_and_the_option_asked_for_are_read() {
        // The right option in another component is another provider's figure.
        let elsewhere = r#"<?xml version="1.0"?>
<application>
  <component name="SomethingElse">
    <option name="quotaInfo" value="{&quot;current&quot;:&quot;1&quot;,&quot;maximum&quot;:&quot;2&quot;}" />
  </component>
</application>"#;
        assert_eq!(
            reading(elsewhere).unwrap_err(),
            Reason::LocalAppMissing
        );

        // A file whose component has not been written yet.
        let empty = r#"<?xml version="1.0"?><application></application>"#;
        assert_eq!(reading(empty).unwrap_err(), Reason::LocalAppMissing);

        // A component that is there and holds no quota.
        let bare = r#"<application><component name="AIAssistantQuotaManager2">
            <option name="nextRefill" value="{}" />
        </component></application>"#;
        assert_eq!(reading(bare).unwrap_err(), Reason::LocalAppMissing);

        // A component nested inside the one asked for holds its own options.
        let nested = format!(
            r#"<application><component name="AIAssistantQuotaManager2">
            <component name="Inner">{}</component>
            </component></application>"#,
            quota(r#"{&quot;current&quot;:&quot;1&quot;,&quot;maximum&quot;:&quot;2&quot;}"#)
                .replace("<application>", "")
                .replace("</application>", "")
        );
        assert_eq!(reading(&nested).unwrap_err(), Reason::LocalAppMissing);
    }

    #[test]
    fn a_quota_with_no_maximum_or_a_negative_figure_reports_no_limits() {
        for value in [
            r"{&quot;current&quot;:&quot;5&quot;,&quot;maximum&quot;:&quot;0&quot;}",
            r"{&quot;current&quot;:&quot;5&quot;}",
            r"{&quot;maximum&quot;:&quot;100&quot;}",
            r"{&quot;current&quot;:&quot;-1&quot;,&quot;maximum&quot;:&quot;100&quot;}",
            r"{&quot;current&quot;:&quot;5&quot;,&quot;maximum&quot;:&quot;lots&quot;}",
        ] {
            assert_eq!(
                reading(&quota(value)).unwrap_err(),
                Reason::NoLimitsReported,
                "{value}"
            );
        }

        // A figure that is not a figure at all is the same answer, and a
        // boolean is never a figure — Foundation's number type would have
        // handed `true` over as 1.
        assert_eq!(
            reading(&quota(r"{&quot;current&quot;:true,&quot;maximum&quot;:&quot;100&quot;}"))
                .unwrap_err(),
            Reason::NoLimitsReported
        );
        assert_eq!(number(Some(&json!(true))), None);
        assert_eq!(number(Some(&json!("42.5"))), Some(42.5));
        assert_eq!(number(Some(&json!(42))), Some(42.0));
    }

    /// A quota already spent is a hundred, not an error and not a negative.
    #[test]
    fn a_spent_quota_reads_as_a_full_ring() {
        let spent = quota(r"{&quot;current&quot;:&quot;100&quot;,&quot;maximum&quot;:&quot;100&quot;}");
        let usage = reading(&spent).unwrap();
        assert_eq!(usage.windows[0].percent_used, Some(100.0));

        // A figure past the maximum is still a hundred on the ring.
        let over = quota(r"{&quot;current&quot;:&quot;150&quot;,&quot;maximum&quot;:&quot;100&quot;}");
        let usage = reading(&over).unwrap();
        assert_eq!(usage.windows[0].percent_used, Some(100.0));
    }

    #[test]
    fn a_length_is_read_from_a_duration_and_from_nothing_else() {
        assert_eq!(seconds_from_iso_duration("PT720H"), Some(720 * 3_600));
        assert_eq!(seconds_from_iso_duration("P30D"), Some(30 * 86_400));
        assert_eq!(seconds_from_iso_duration("P1W"), Some(7 * 86_400));
        assert_eq!(seconds_from_iso_duration("P1W2D"), Some(9 * 86_400));
        assert_eq!(seconds_from_iso_duration("P1DT12H"), Some(36 * 3_600));
        assert_eq!(seconds_from_iso_duration("PT1H30M"), Some(5_400));
        assert_eq!(seconds_from_iso_duration("PT45S"), Some(45));

        // Months, years, fractions and words are not lengths this can state.
        for text in ["P1M", "P1Y", "P1.5D", "monthly", "", "P", "PT", "30D", "P1D2W"] {
            assert_eq!(seconds_from_iso_duration(text), None, "{text}");
        }
    }

    #[test]
    fn only_an_iso_stamp_is_a_reset() {
        assert_eq!(
            iso_stamp("2026-11-01T00:00:00Z").as_deref(),
            Some("2026-11-01T00:00:00Z")
        );
        assert_eq!(
            iso_stamp("2026-11-01T00:00:00.500Z").as_deref(),
            Some("2026-11-01T00:00:00Z")
        );
        // A stamp with no zone is not one, which is what the original's own
        // parser takes as well.
        assert!(iso_stamp("2026-11-01 00:00:00").is_none());
        assert!(iso_stamp("").is_none());
    }

    #[test]
    fn an_entity_escaped_value_is_read_back_as_json() {
        assert_eq!(unescape("&quot;a&quot;"), "\"a\"");
        assert_eq!(unescape("a &amp; b"), "a & b");
        assert_eq!(unescape("&lt;x&gt;"), "<x>");
        assert_eq!(unescape("&#65;&#x42;"), "AB");
        // An entity that is not one is left standing.
        assert_eq!(unescape("100% & {ok}"), "100% & {ok}");

        // The fixture's own value goes through both readers.
        let xml = fixture(&refill("PT720H"));
        let found = option_value(&xml, COMPONENT, "quotaInfo").unwrap();
        assert_eq!(number(json_object(&found).unwrap().get("current")), Some(42.5));
    }

    /// The rule, driven from a fixture tree rather than from a real IDE: the
    /// file written last wins, whatever order the folders are listed in, and
    /// one that belongs to no IDE is not looked in at all.
    #[test]
    fn the_newest_quota_file_across_the_ides_is_the_one_read() {
        let root = PathBuf::from("C:\\Users\\me\\AppData\\Roaming");
        let roots = vec![root.join("JetBrains"), root.join("Google")];
        let now = SystemTime::now();
        let older = now - std::time::Duration::from_secs(7_200);
        let old = now - std::time::Duration::from_secs(3_600);

        let file_of = |root: &PathBuf, ide: &str| {
            root.join(ide).join("options").join(QUOTA_FILE)
        };

        // What is under each root, in the order a folder happens to list it.
        let folders: Vec<(PathBuf, &str)> = vec![
            (roots[0].clone(), "Toolbox"),
            (roots[0].clone(), "PyCharm2024.1"),
            (roots[0].clone(), "IntelliJIdea2024.3"),
            // An IDE with AI Assistant that has never saved a quota.
            (roots[0].clone(), "WebStorm2024.2"),
            (roots[1].clone(), "AndroidStudio2024.1"),
        ];
        let written: Vec<(PathBuf, Option<SystemTime>)> = vec![
            (file_of(&roots[0], "Toolbox"), Some(now)),
            (file_of(&roots[0], "PyCharm2024.1"), Some(now)),
            (file_of(&roots[0], "IntelliJIdea2024.3"), Some(older)),
            (file_of(&roots[1], "AndroidStudio2024.1"), Some(old)),
        ];

        let listing: Files<'_> = Files {
            names: &|folder| {
                folders
                    .iter()
                    .filter(|(parent, _)| parent == folder)
                    .map(|(_, name)| (*name).to_string())
                    .collect()
            },
            is_file: &|path| written.iter().any(|(file, _)| file == path),
            modified: &|path| {
                written
                    .iter()
                    .find(|(file, _)| file == path)
                    .and_then(|(_, at)| *at)
            },
        };

        // PyCharm and Toolbox share a timestamp; Toolbox is not an IDE, and
        // PyCharm is listed first of the IDEs.
        assert_eq!(
            newest_of(&roots, &listing).unwrap(),
            file_of(&roots[0], "PyCharm2024.1")
        );

        // A file whose date cannot be read sorts below every one that can, so
        // the Android Studio quota is the reading rather than a guess at the
        // newest.
        let unreadable: Files<'_> = Files {
            names: listing.names,
            is_file: listing.is_file,
            modified: &|path| {
                if path == file_of(&roots[0], "PyCharm2024.1") {
                    None
                } else {
                    (listing.modified)(path)
                }
            },
        };
        assert_eq!(
            newest_of(&roots, &unreadable).unwrap(),
            file_of(&roots[1], "AndroidStudio2024.1")
        );

        // Two files with the same date keep the first, which is what Swift's
        // `max(by:)` does.
        let tied: Files<'_> = Files {
            names: listing.names,
            is_file: listing.is_file,
            modified: &|_| Some(now),
        };
        assert_eq!(
            newest_of(&roots, &tied).unwrap(),
            file_of(&roots[0], "PyCharm2024.1")
        );

        // A machine with no IDE at all, and one whose roots cannot be read.
        let nothing = Files {
            names: &|_| Vec::new(),
            is_file: &|_| false,
            modified: &|_| None,
        };
        assert!(newest_of(&roots, &nothing).is_none());

        let only_toolbox = Files {
            names: &|_| vec!["Toolbox".to_string()],
            is_file: &|_| true,
            modified: &|_| Some(now),
        };
        assert!(newest_of(&roots, &only_toolbox).is_none());
    }

    /// The two roots are the folders the original reads, in the spelling this
    /// platform uses.
    #[test]
    fn the_roots_are_the_ides_own_settings_folders() {
        let roots = settings_roots();
        assert!(roots.iter().any(|root| root.ends_with("JetBrains")));
        assert!(roots.iter().any(|root| root.ends_with("Google")));
        assert!(roots
            .iter()
            .all(|root| root.ends_with("JetBrains") || root.ends_with("Google")));

        let ide_folders: Vec<String> = IDE_FOLDERS.iter().map(|ide| ide.to_lowercase()).collect();
        assert!(ide_folders.contains(&"intellijidea".to_string()));
        assert!(ide_folders.contains(&"androidstudio".to_string()));
        assert!(!ide_folders.contains(&"toolbox".to_string()));
    }
}
