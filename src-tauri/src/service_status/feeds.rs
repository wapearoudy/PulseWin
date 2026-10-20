//! The twelve feed shapes an official status page is drawn from.
//!
//! Ported from Pulse 1.9's `ServiceStatus+*.swift` (Apache-2.0, copyright 2026
//! qunqin24). Each feed is read from what its page is drawn from: Statuspage's
//! and incident.io's own feeds, Flashcat's page payload, Better Stack's
//! `index.json`, and so on — because no common summary carries the history,
//! and some carry nothing at all of what the page shows.
//!
//! **Nothing here is Pulse's figure.** An uptime percentage is the page's,
//! passed through and never computed; a missing one is not shown. A page that
//! publishes no days gets no bars, never a row of green. A page that can't be
//! read is no reading, never "all clear".

use chrono::{Local, TimeZone};
use serde_json::Value;
use std::collections::HashMap;

use super::{component, Component, Day, Health};

/// One calendar day: its `YYYY-MM-DD` label and its half-open local bounds.
pub type DaySpan = (String, i64, i64);

pub fn list<'a>(v: &'a Value, k: &str) -> &'a [Value] {
    v.get(k)
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or(&[])
}
pub fn text<'a>(v: &'a Value, k: &str) -> Option<&'a str> {
    v.get(k)?.as_str()
}
fn number(v: Option<&Value>) -> Option<f64> {
    let v = v?;
    v.as_f64().or_else(|| v.as_str()?.trim().parse().ok())
}
pub fn percent(v: Option<&Value>) -> Option<f64> {
    let n = number(v)?;
    (n.is_finite() && (0.0..=100.0).contains(&n)).then_some(n)
}
pub fn stamp(v: Option<&Value>, seconds: bool) -> Option<i64> {
    let v = v?;
    if seconds {
        let f = v.as_f64()?;
        (f.is_finite() && f.abs() < 253_402_300_800.0).then_some(f as i64)
    } else {
        chrono::DateTime::parse_from_rfc3339(v.as_str()?)
            .ok()
            .map(|d| d.timestamp())
    }
}
/// The page's `YYYY-MM-DD`, a calendar day without a zone.
pub fn day_date(text: &str) -> Option<String> {
    let head = text.get(..10)?;
    chrono::NaiveDate::parse_from_str(head, "%Y-%m-%d").ok()?;
    Some(head.to_string())
}
fn row(id: &str, name: &str, state: Health) -> Component {
    Component {
        group: None,
        ..component(id, name, state)
    }
}

// MARK: - Impacts: the model incident.io, Flashcat and Google Cloud share

/// One stretch of a component not being operational; no end while it lasts.
pub struct Impact {
    pub component_id: String,
    pub start: i64,
    pub end: Option<i64>,
    pub state: Health,
}

/// Each component's days and uptime from its impacts: a day takes the worst
/// impact that overlaps it, and a day that ended before the page had data for
/// the component has none.
pub fn applying_history(
    rows: &mut [Component],
    impacts: &[Impact],
    since: &HashMap<String, Option<i64>>,
    uptimes: &HashMap<String, f64>,
    days: &[DaySpan],
    now: i64,
) {
    for c in rows.iter_mut() {
        let spans = impacts
            .iter()
            .filter(|i| i.component_id == c.id)
            .filter_map(|i| {
                let end = i.end.unwrap_or(now);
                (end > i.start).then_some((i.start, end, i.state))
            })
            .collect::<Vec<_>>();
        let started = since.get(&c.id).copied().flatten();
        c.days = days
            .iter()
            .map(|(date, a, b)| Day {
                date: date.clone(),
                state: if started.is_some_and(|s| *b <= s) {
                    None
                } else {
                    Some(
                        spans
                            .iter()
                            .filter(|(s, e, _)| *s < *b && *e > *a)
                            .map(|i| i.2)
                            .max_by_key(|s| s.severity())
                            .unwrap_or(Health::Operational),
                    )
                },
                colour: None,
                hours: None,
            })
            .collect();
        c.uptime = uptimes.get(&c.id).copied();
    }
}

/// The `component_impacts` / `component_uptimes` pair incident.io and Flashcat
/// both speak, with `seconds` selecting their timestamp shape.
pub fn impact_history(
    rows: &mut [Component],
    v: &Value,
    days: &[DaySpan],
    now: i64,
    seconds: bool,
) -> Result<(), String> {
    if !v["component_impacts"].is_array() || !v["component_uptimes"].is_array() {
        return Err("官方历史数据未能读取".into());
    }
    let impacts = list(v, "component_impacts")
        .iter()
        .filter_map(|i| {
            // A missing end is an outage still under way; an end that is
            // stated but unreadable is not — the row is dropped rather than
            // stretched to now, which would invent an outage that may never
            // have happened.
            let end = match i.get(if seconds { "end_at_seconds" } else { "end_at" }) {
                None | Some(Value::Null) => None,
                stated => Some(stamp(stated, seconds)?),
            };
            Some(Impact {
                component_id: text(i, "component_id")?.into(),
                start: stamp(
                    i.get(if seconds {
                        "start_at_seconds"
                    } else {
                        "start_at"
                    }),
                    seconds,
                )?,
                end,
                state: Health::parse(text(i, "status").unwrap_or("unknown")),
            })
        })
        .collect::<Vec<_>>();
    let mut since = HashMap::new();
    let mut uptimes = HashMap::new();
    for record in list(v, "component_uptimes") {
        let Some(id) = text(record, "component_id") else {
            continue;
        };
        since.insert(
            id.to_string(),
            stamp(
                record.get(if seconds {
                    "available_since_seconds"
                } else {
                    "data_available_since"
                }),
                seconds,
            ),
        );
        if let Some(u) = percent(record.get("uptime")) {
            uptimes.insert(id.to_string(), u);
        }
    }
    applying_history(rows, &impacts, &since, &uptimes, days, now);
    Ok(())
}

// MARK: - Atlassian Statuspage

/// Every component the page shows, in its order, each with its state now and
/// its group's id: groups' own rows left out, and one marked to show only when
/// degraded left out while it isn't. None when the page shows none.
pub fn statuspage_components(v: &Value) -> Option<Vec<Component>> {
    let all = list(v, "components");
    let mut rows = all
        .iter()
        .enumerate()
        .filter(|(_, c)| {
            c["group"] != true
                && !(c["only_show_if_degraded"] == true && text(c, "status") == Some("operational"))
        })
        .filter_map(|(index, c)| {
            let position = c["position"].as_i64().unwrap_or(i64::MAX);
            Some((
                (position, index as i64),
                Component {
                    group: text(c, "group_id").map(str::to_string),
                    ..component(
                        text(c, "id")?,
                        text(c, "name")?,
                        Health::parse(text(c, "status")?),
                    )
                },
            ))
        })
        .collect::<Vec<_>>();
    rows.sort_by_key(|(key, _)| *key);
    let out = rows.into_iter().map(|(_, c)| c).collect::<Vec<_>>();
    (!out.is_empty()).then_some(out)
}

/// Each component's days, colours and uptime from the uptime showcase.
pub fn statuspage_history(v: &Value, rows: &mut [Component]) -> Result<(), String> {
    if !v["timelines"].is_object() {
        return Err("官方历史数据未能读取".into());
    }
    let mut read = 0;
    for c in rows.iter_mut() {
        let t = &v["timelines"][&c.id];
        if !t["days"].is_array() {
            continue;
        }
        let since = t.pointer("/component/startDate").and_then(Value::as_str);
        c.days = list(t, "days")
            .iter()
            .filter_map(|d| {
                let date = day_date(text(d, "date")?)?;
                Some(Day {
                    date: date.clone(),
                    state: if since.is_some_and(|s| date.as_str() < s) {
                        None
                    } else {
                        Some(
                            if d.pointer("/outages/m").and_then(Value::as_u64).unwrap_or(0) > 0 {
                                Health::FullOutage
                            } else if d.pointer("/outages/p").and_then(Value::as_u64).unwrap_or(0) > 0
                            {
                                Health::PartialOutage
                            } else {
                                Health::Operational
                            },
                        )
                    },
                    colour: None,
                    hours: None,
                })
            })
            .collect();
        if c.days.is_empty() {
            continue;
        }
        read += 1;
        // The page's own colours, only when there is one for every day — a
        // partial list can't be lined up with the days safely.
        let colours = v["components"][&c.id]
            .as_str()
            .map(bar_colours)
            .unwrap_or_default();
        if colours.len() == c.days.len() {
            for (d, col) in c.days.iter_mut().zip(colours) {
                d.colour = Some(col)
            }
        }
        c.uptime = list(v, "values")
            .iter()
            .find(|u| text(u, "component") == Some(c.id.as_str()))
            .and_then(|u| percent(u.get("ninety")));
    }
    if read == 0 {
        Err("官方历史没有可读取的日期".into())
    } else {
        Ok(())
    }
}

/// The `fill` of each `uptime-day` bar in the page's SVG, in order.
pub fn bar_colours(svg: &str) -> Vec<String> {
    svg.split("<rect")
        .skip(1)
        .filter_map(|s| {
            let tag = s.split_once('>')?.0;
            if !tag.contains("uptime-day") {
                return None;
            }
            let fill = tag.split_once("fill=\"#")?.1.get(..6)?;
            fill.bytes()
                .all(|b| b.is_ascii_hexdigit())
                .then(|| format!("#{fill}"))
        })
        .collect()
}

// MARK: - incident.io

/// Every visible component in the page's order — rows of their own and each
/// group's members, the group's name as theirs — with its state now; and
/// whether the page shows an uptime figure. None when the feed doesn't parse
/// or shows nothing.
pub fn incidentio_summary(v: &Value) -> Option<(Vec<Component>, bool)> {
    let summary = v.get("summary")?;
    let items = summary.pointer("/structure/items")?.as_array()?;
    let affected = list(summary, "affected_components")
        .iter()
        .filter_map(|a| Some((text(a, "component_id")?.to_string(), text(a, "status")?)))
        .collect::<HashMap<_, _>>();
    let member = |item: &Value, group: Option<String>| -> Option<Component> {
        if item.get("hidden") == Some(&Value::Bool(true)) {
            return None;
        }
        let id = text(item, "component_id")?;
        Some(Component {
            group,
            ..component(
                id,
                text(item, "name")?,
                affected
                    .get(id)
                    .map(|s| Health::parse(s))
                    .unwrap_or(Health::Operational),
            )
        })
    };
    let mut rows = Vec::new();
    for item in items {
        if let Some(group) = item.get("group") {
            if group.get("hidden") == Some(&Value::Bool(true)) {
                continue;
            }
            let Some(name) = text(group, "name") else {
                continue;
            };
            for c in list(group, "components") {
                rows.extend(member(c, Some(name.to_string())));
            }
        } else if let Some(c) = item.get("component") {
            rows.extend(member(c, None));
        }
    }
    (!rows.is_empty()).then_some((
        rows,
        text(summary, "display_uptime_mode") != Some("chart_only"),
    ))
}

// MARK: - Flashcat

/// Every component the page shows, in its order, with its state now.
///
/// **Read out of the page**, because Flashcat's API needs an account key. The
/// page is a Next.js app: its data travels in `self.__next_f.push([1,"…"])`
/// chunks, which joined are lines of `id:JSON`. Two carry an `initialData`
/// object — the layout (`page`), and the impacts and uptimes — and nothing
/// else is read. A redesign that moves them is no reading, never "all clear".
pub fn flashcat_data(html: &str) -> Result<Value, String> {
    let mut flight = String::new();
    for part in html.split("self.__next_f.push([1,").skip(1) {
        let part = part.trim_start();
        if !part.starts_with('"') {
            continue;
        }
        let mut escaped = false;
        let mut end = None;
        for (i, ch) in part.char_indices().skip(1) {
            if escaped {
                escaped = false;
                continue;
            }
            if ch == '\\' {
                escaped = true
            } else if ch == '"' {
                end = Some(i + 1);
                break;
            }
        }
        if let Some(end) = end {
            if let Ok(s) = serde_json::from_str::<String>(&part[..end]) {
                flight.push_str(&s)
            }
        }
    }
    fn collect(v: &Value, out: &mut serde_json::Map<String, Value>, depth: u8) {
        if depth > 64 {
            return;
        }
        if let Some(obj) = v.as_object() {
            if let Some(data) = obj.get("initialData").and_then(Value::as_object) {
                for (k, v) in data {
                    out.entry(k.clone()).or_insert_with(|| v.clone());
                }
            }
            for v in obj.values() {
                collect(v, out, depth + 1)
            }
        } else if let Some(a) = v.as_array() {
            for v in a {
                collect(v, out, depth + 1)
            }
        }
    }
    let mut merged = serde_json::Map::new();
    for line in flight.lines().filter(|l| l.contains("\"initialData\"")) {
        if let Some((_, json)) = line.split_once(':') {
            if let Ok(v) = serde_json::from_str::<Value>(json) {
                collect(&v, &mut merged, 0)
            }
        }
    }
    if !merged
        .get("page")
        .is_some_and(|v| v["components"].is_array())
    {
        return Err("无法识别官方状态页数据".into());
    }
    Ok(Value::Object(merged))
}

/// The page's own word when it gives one; otherwise whatever impact is still
/// open; otherwise operational. Days come from `impact_history`.
pub fn flashcat_summary(v: &Value, now: i64) -> Result<Vec<Component>, String> {
    let page = &v["page"];
    if !page["components"].is_array() {
        return Err("无法读取官方状态组件".into());
    }
    let mut groups: Vec<(i64, Vec<&Value>)> = vec![];
    let visible = list(page, "components")
        .iter()
        .filter(|v| v["hide_all"] != true)
        .collect::<Vec<_>>();
    let order = |v: &Value| v["order_id"].as_i64().unwrap_or(i64::MAX);
    for c in &visible {
        if text(c, "section_id").unwrap_or("").is_empty() {
            groups.push((order(c), vec![*c]))
        }
    }
    for section in list(page, "sections")
        .iter()
        .filter(|v| v["hide_all"] != true)
    {
        if let Some(id) = text(section, "section_id") {
            let mut members = visible
                .iter()
                .copied()
                .filter(|c| text(c, "section_id") == Some(id))
                .collect::<Vec<_>>();
            members.sort_by_key(|c| order(c));
            groups.push((order(section), members));
        }
    }
    groups.sort_by_key(|g| g.0);
    let rows = groups
        .into_iter()
        .flat_map(|g| g.1)
        .filter_map(|c| {
            let id = text(c, "component_id")?;
            let name = text(c, "name")?;
            let open = list(v, "component_impacts")
                .iter()
                .filter(|i| {
                    text(i, "component_id") == Some(id)
                        && stamp(i.get("start_at_seconds"), true).is_some_and(|s| s <= now)
                        && stamp(i.get("end_at_seconds"), true).unwrap_or(i64::MAX) > now
                })
                .map(|i| Health::parse(text(i, "status").unwrap_or("unknown")))
                .max_by_key(|s| s.severity());
            let stated = text(c, "status")
                .filter(|s| !s.is_empty())
                .map(Health::parse);
            Some(Component {
                group: text(c, "section_id")
                    .filter(|s| !s.is_empty())
                    .map(str::to_string),
                ..component(id, name, stated.or(open).unwrap_or(Health::Operational))
            })
        })
        .collect::<Vec<_>>();
    if rows.is_empty() {
        Err("官方状态页没有可读取的组件".into())
    } else {
        Ok(rows)
    }
}

// MARK: - Better Stack

/// Every monitor the page lists, its sections in order and each one's monitors
/// in order, the section's name as its group. One request carries the state,
/// the days and the page's own availability figure. JSON:API: the monitors and
/// their sections are in `included`, told apart by `type`.
pub fn better_stack(v: &Value) -> Option<Vec<Component>> {
    let included = v.get("included")?.as_array()?;
    let sections = included
        .iter()
        .filter(|r| text(r, "type") == Some("status_page_section"))
        .filter_map(|r| {
            let id = text(r, "id")?;
            Some((
                id.to_string(),
                (
                    text(&r["attributes"], "name").map(str::to_string),
                    r["attributes"]["position"].as_i64(),
                ),
            ))
        })
        .collect::<HashMap<_, _>>();
    // A number in the feed; the section's own id is a string.
    let section_of = |a: &Value| -> Option<String> {
        a.get("status_page_section_id").and_then(|v| {
            v.as_i64()
                .map(|n| n.to_string())
                .or_else(|| v.as_str().map(str::to_string))
        })
    };
    let mut monitors = included
        .iter()
        .filter(|r| text(r, "type") == Some("status_page_resource"))
        .filter(|r| r["attributes"]["public_name"].is_string())
        .filter_map(|r| {
            let a = &r["attributes"];
            let section = section_of(a);
            let (group, position) = section
                .as_ref()
                .and_then(|id| sections.get(id))
                .map(|(name, position)| (name.clone(), *position))
                .unwrap_or((None, None));
            Some((
                (position.unwrap_or(-1), a["position"].as_i64().unwrap_or(i64::MAX)),
                Component {
                    group,
                    ..component(
                        text(r, "id")?,
                        text(a, "public_name")?,
                        text(a, "status")
                            .map(better_stack_state)
                            .unwrap_or(Health::Unrecognised),
                    )
                },
                a,
            ))
        })
        .collect::<Vec<_>>();
    monitors.sort_by_key(|(k, _, _)| *k);
    let rows = monitors
        .into_iter()
        .map(|(_, mut c, a)| {
            c.days = list(a, "status_history")
                .iter()
                .filter_map(|d| {
                    let date = day_date(text(d, "day")?)?;
                    // Not monitored that day: no record, not a clear day.
                    let state = match text(d, "status") {
                        Some("not_monitored") => None,
                        Some(s) => Some(better_stack_state(s)),
                        None => None,
                    };
                    Some(Day {
                        date,
                        state,
                        colour: None,
                        hours: None,
                    })
                })
                .collect();
            // A fraction, "0.999898".
            c.uptime = a["availability"]
                .as_f64()
                .or_else(|| {
                    a["availability"]
                        .as_str()
                        .and_then(|s| s.parse::<f64>().ok())
                })
                .filter(|f| f.is_finite())
                .map(|f| f * 100.0);
            c
        })
        .collect::<Vec<_>>();
    (!rows.is_empty()).then_some(rows)
}

/// Better Stack's words. `downtime` is its red — the monitor failing, which is
/// what an outage is — and `not_monitored` is not a reading of the service.
pub fn better_stack_state(value: &str) -> Health {
    match value {
        "operational" => Health::Operational,
        "degraded" | "recovered_degraded" => Health::Degraded,
        "downtime" => Health::FullOutage,
        "maintenance" => Health::Maintenance,
        _ => Health::Unrecognised,
    }
}

// MARK: - Instatus

/// `/v2/components.json`: every component with its state, in the page's order,
/// a group's name as its members'. A group's own row is in the list too, and is
/// left out like Statuspage's.
pub fn instatus(v: &Value) -> Option<Vec<Component>> {
    let all = v.get("components")?.as_array()?;
    let headers = all
        .iter()
        .filter_map(|c| text(&c["group"], "id"))
        .collect::<std::collections::HashSet<_>>();
    let rows = all
        .iter()
        .filter(|c| !headers.contains(text(c, "id").unwrap_or("")))
        .filter_map(|c| {
            Some(Component {
                group: text(&c["group"], "name").map(str::to_string),
                ..component(
                    text(c, "id")?,
                    text(c, "name")?,
                    Health::parse(text(c, "status")?),
                )
            })
        })
        .collect::<Vec<_>>();
    (!rows.is_empty()).then_some(rows)
}

// MARK: - Upptime

/// The repository's `history/summary.json`: each monitored site, `up`,
/// `degraded` or `down` as of the last check. The state only.
pub fn upptime(v: &Value) -> Option<Vec<Component>> {
    let rows = v
        .as_array()?
        .iter()
        .filter_map(|site| {
            Some(row(
                text(site, "slug")?,
                text(site, "name")?,
                upptime_state(text(site, "status")?),
            ))
        })
        .collect::<Vec<_>>();
    (!rows.is_empty()).then_some(rows)
}
pub fn upptime_state(value: &str) -> Health {
    match value {
        "up" => Health::Operational,
        "degraded" => Health::Degraded,
        "down" => Health::FullOutage,
        _ => Health::Unrecognised,
    }
}

// MARK: - Gatus

/// `/api/v1/endpoints/statuses`: each endpoint with its last checks, newest
/// last. The state is the newest check's: passed, operational; a failed check
/// that still got a 2xx failed on a condition such as response time, degraded;
/// any other failure, an outage — what Gatus draws red. The state only.
pub fn gatus(v: &Value) -> Option<Vec<Component>> {
    let rows = v
        .as_array()?
        .iter()
        .filter_map(|endpoint| {
            let results = list(endpoint, "results");
            let state = match results.last() {
                None => Health::Unrecognised,
                Some(r) if r["success"] == true => Health::Operational,
                Some(r) if r["status"].as_i64().is_some_and(|s| (200..300).contains(&s)) => {
                    Health::Degraded
                }
                Some(_) => Health::FullOutage,
            };
            Some(Component {
                group: text(endpoint, "group").map(str::to_string),
                ..component(
                    text(endpoint, "key")?,
                    text(endpoint, "name")?,
                    state,
                )
            })
        })
        .collect::<Vec<_>>();
    (!rows.is_empty()).then_some(rows)
}

// MARK: - UptimeRobot

/// A public page's `getMonitorList`: each monitor's state, its ninety daily
/// uptime ratios, and the page's ninety-day figure. A day the page greys out
/// (before the monitor existed, or paused) has no record; a whole day up is
/// operational, a whole day down a full outage, and anything between a partial
/// one — as Statuspage grades its days.
pub fn uptime_robot(v: &Value) -> Option<Vec<Component>> {
    let rows = list(v, "data")
        .iter()
        .filter_map(|monitor| {
            let id = monitor["monitorId"]
                .as_i64()
                .or_else(|| monitor["monitorId"].as_str()?.parse().ok())?;
            Some(Component {
                days: list(monitor, "dailyRatios")
                    .iter()
                    .filter_map(|d| {
                        let date = day_date(text(d, "date")?)?;
                        let colour = text(d, "color");
                        let Some(ratio) = text(d, "ratio").and_then(|r| r.parse::<f64>().ok())
                        else {
                            return Some(Day {
                                date,
                                state: None,
                                colour: None,
                                hours: None,
                            });
                        };
                        if colour.is_some_and(|c| c == "grey" || c == "black") {
                            return Some(Day {
                                date,
                                state: None,
                                colour: None,
                                hours: None,
                            });
                        }
                        let state = if ratio >= 100.0 {
                            Health::Operational
                        } else if ratio <= 0.0 {
                            Health::FullOutage
                        } else {
                            Health::PartialOutage
                        };
                        Some(Day {
                            date,
                            state: Some(state),
                            colour: None,
                            hours: None,
                        })
                    })
                    .collect(),
                uptime: monitor["90dRatio"]["ratio"]
                    .as_str()
                    .and_then(|r| r.parse::<f64>().ok())
                    .filter(|f| f.is_finite()),
                ..row(
                    &id.to_string(),
                    &html_unescaped(text(monitor, "name").unwrap_or("")),
                    uptime_robot_state(text(monitor, "statusClass").unwrap_or("")),
                )
            })
        })
        .collect::<Vec<_>>();
    (!rows.is_empty()).then_some(rows)
}

/// `success` is up, `danger` down; a paused monitor (`black`) is not a reading.
pub fn uptime_robot_state(value: &str) -> Health {
    match value {
        "success" => Health::Operational,
        "warning" => Health::Degraded,
        "danger" => Health::FullOutage,
        _ => Health::Unrecognised,
    }
}

/// The page writes names with HTML entities: "Soft Serve &#40;git.charm.sh&#41;".
pub fn html_unescaped(text: &str) -> String {
    if !text.contains('&') {
        return text.to_string();
    }
    let mut result = text
        .replace("&amp;", "&")
        .replace("&quot;", "\"")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&apos;", "'");
    let mut out = String::with_capacity(result.len());
    let mut rest = result.as_str();
    while let Some(at) = rest.find("&#") {
        out.push_str(&rest[..at]);
        let after = &rest[at + 2..];
        let digits = after.chars().take_while(char::is_ascii_digit).count();
        if digits > 0 && after[digits..].starts_with(';') {
            match after[..digits].parse::<u32>().ok().and_then(char::from_u32) {
                Some(c) => {
                    out.push(c);
                    rest = &after[digits + 1..];
                    continue;
                }
                None => {}
            }
        }
        out.push_str("&#");
        rest = after;
    }
    out.push_str(rest);
    result = out;
    result
}

// MARK: - OpenCode

/// The page's bars: three hours each, seven days of them.
pub const OPEN_CODE_BAR_HOURS: i64 = 3;
pub const OPEN_CODE_BAR_COUNT: i64 = 56;

/// `/api/status`: one row per service, named as the page names them, with the
/// bars the page draws — the last seven days in three-hour slots, 56 of them,
/// aligned to UTC and ending with the slot now under way. The feed's `history`
/// fills the slots it covers (about two and a half days); a slot it doesn't, or
/// one it calls `unknown`, has no record, as the page draws it.
pub fn open_code(v: &Value, now: i64) -> Option<Vec<Component>> {
    let components = v.get("components")?.as_array()?;
    let slot = OPEN_CODE_BAR_HOURS * 3600;
    let current = now.div_euclid(slot) * slot;
    let starts = (0..OPEN_CODE_BAR_COUNT)
        .rev()
        .map(|back| current - back * slot)
        .collect::<Vec<_>>();
    let buckets = list(v, "history")
        .iter()
        .filter_map(|b| {
            let start = b["start"].as_f64()?;
            (start.is_finite()).then_some((start as i64 / 1000, &b["services"]))
        })
        .collect::<HashMap<_, _>>();
    let rows = components
        .iter()
        .filter_map(|service| {
            let id = text(service, "service")?;
            let at = |start: i64| -> Option<String> {
                Local
                    .timestamp_opt(start, 0)
                    .single()
                    .map(|d| d.format("%Y-%m-%d %H:%M").to_string())
            };
            Some(Component {
                days: starts
                    .iter()
                    .map(|start| Day {
                        date: at(*start).unwrap_or_default(),
                        state: buckets
                            .get(start)
                            .and_then(|services| services.get(id))
                            .and_then(Value::as_str)
                            .and_then(|word| {
                                (word != "unknown").then(|| Health::parse(word))
                            }),
                        colour: None,
                        hours: Some(OPEN_CODE_BAR_HOURS),
                    })
                    .collect(),
                ..row(
                    id,
                    &open_code_name(id),
                    Health::parse(text(service, "status").unwrap_or("unknown")),
                )
            })
        })
        .collect::<Vec<_>>();
    (!rows.is_empty()).then_some(rows)
}

/// The page's own names for the services the feed lists by id; one this build
/// doesn't know is shown by its id.
pub fn open_code_name(id: &str) -> String {
    match id {
        "console" => "Console API",
        "logs" => "Request logs",
        "inference" => "Inference API",
        "go" => "Go",
        "payg" => "Pay-as-you-go",
        "free" => "Free inference",
        _ => id,
    }
    .into()
}

// MARK: - DeepInfra

/// `/api/v1/status`: the services (the API, the website), not the hundreds of
/// models listed beside them.
pub fn deep_infra(v: &Value) -> Option<Vec<Component>> {
    let rows = list(v, "services")
        .iter()
        .filter_map(|s| {
            Some(row(
                text(s, "publicId")?,
                text(s, "displayName")?,
                Health::parse(text(s, "status")?),
            ))
        })
        .collect::<Vec<_>>();
    (!rows.is_empty()).then_some(rows)
}

// MARK: - IBM Bob

/// `/api/v1/status`: services by region, one per synthetic check — so a name
/// comes several times, once per check. One row per name, the worst of its
/// checks, in the order the page first lists it.
pub fn ibm_bob(v: &Value) -> Option<Vec<Component>> {
    let regions = v.get("regions")?.as_array()?;
    let mut order: Vec<String> = vec![];
    let mut worst: HashMap<String, Health> = HashMap::new();
    for service in regions
        .iter()
        .flat_map(|r| list(r, "services").iter())
    {
        let Some(name) = text(service, "name") else {
            continue;
        };
        let state = Health::parse(text(service, "status").unwrap_or("unknown"));
        match worst.get_mut(name) {
            Some(before) if state.severity() > before.severity() => *before = state,
            Some(_) => {}
            None => {
                order.push(name.to_string());
                worst.insert(name.to_string(), state);
            }
        }
    }
    let rows = order
        .iter()
        .map(|name| row(name, name, worst[name]))
        .collect::<Vec<_>>();
    (!rows.is_empty()).then_some(rows)
}

// MARK: - Google Cloud

/// `incidents.json`: the dashboard's incidents, each with when it began and
/// ended, how bad (`status_impact`) and the products it touched. One row, the
/// product's: down while an incident touching it is open, and — given `days` —
/// a day taking the worst incident that overlapped it. None when the feed can't
/// be read; a product with no incident is operational, which is how the
/// dashboard says it.
pub fn google_cloud(
    v: &Value,
    product: &str,
    name: &str,
    now: i64,
    days: Option<&[DaySpan]>,
) -> Option<Vec<Component>> {
    let incidents = v.as_array()?;
    let impacts = incidents
        .iter()
        .filter(|i| {
            list(i, "affected_products")
                .iter()
                .any(|p| text(p, "id") == Some(product))
        })
        .filter_map(|i| {
            Some(Impact {
                component_id: product.to_string(),
                start: stamp(i.get("begin"), false)?,
                end: stamp(i.get("end"), false),
                state: google_cloud_state(text(i, "status_impact")),
            })
        })
        .collect::<Vec<_>>();
    let open = impacts
        .iter()
        .filter(|i| i.start <= now && i.end.unwrap_or(i64::MAX) > now)
        .map(|i| i.state)
        .max_by_key(|s| s.severity());
    let mut rows = vec![row(product, name, open.unwrap_or(Health::Operational))];
    if let Some(days) = days {
        applying_history(&mut rows, &impacts, &HashMap::new(), &HashMap::new(), days, now);
    }
    Some(rows)
}

/// The dashboard's three kinds: an outage, a disruption, and "service
/// information" — an incident it lists for a lesser problem, which is still the
/// service not working as it should.
pub fn google_cloud_state(value: Option<&str>) -> Health {
    match value {
        Some("SERVICE_OUTAGE") => Health::FullOutage,
        Some("SERVICE_DISRUPTION") => Health::PartialOutage,
        Some("SERVICE_INFORMATION") => Health::Degraded,
        _ => Health::Unrecognised,
    }
}
