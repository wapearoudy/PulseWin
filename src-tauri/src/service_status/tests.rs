use super::*;

fn fixture(name: &str) -> Value {
    serde_json::from_str(match name {
        "openai" => {
            include_str!("../../tests/fixtures/service-status/openai-status-operational.json")
        }
        "incident" => include_str!("../../tests/fixtures/service-status/openai-status-incident.json"),
        "impacts" => include_str!("../../tests/fixtures/service-status/openai-status-impacts.json"),
        "claude" => include_str!("../../tests/fixtures/service-status/claude-status-summary.json"),
        _ => include_str!("../../tests/fixtures/service-status/claude-status-uptime.json"),
    })
    .unwrap()
}
fn deepseek() -> Value {
    feeds::flashcat_data(include_str!(
        "../../tests/fixtures/service-status/deepseek-status-page.html"
    ))
    .unwrap()
}
fn days(count: i64) -> Vec<DaySpan> {
    let offset = chrono::FixedOffset::east_opt(28800).unwrap();
    let date = chrono::NaiveDate::from_ymd_opt(2026, 10, 4).unwrap();
    (0..count)
        .rev()
        .map(|i| {
            let d = date - chrono::Duration::days(i);
            let a = offset
                .from_local_datetime(&d.and_hms_opt(0, 0, 0).unwrap())
                .unwrap()
                .timestamp();
            (d.to_string(), a, a + 86400)
        })
        .collect()
}
fn bars(c: &Component) -> String {
    c.days
        .iter()
        .map(|d| match d.state {
            None => '-',
            Some(Health::Operational) => '0',
            Some(Health::Degraded) => '1',
            Some(Health::PartialOutage) => '2',
            Some(Health::FullOutage) => '3',
            Some(Health::Maintenance) => 'm',
            _ => '?',
        })
        .collect()
}
/// The rows one provider's page keeps, as `read` would show them.
fn shown(provider: &str, rows: Vec<Component>) -> Vec<Component> {
    let page = page(provider).unwrap();
    rows.into_iter()
        .filter(|c| page.shows.matches(c))
        .collect()
}

#[test]
fn the_catalogue_covers_every_page_the_original_had() {
    // 44 providers + the second MiniMax site; every address is https and every
    // page names the company it belongs to, as the catalogue footnote insists.
    assert_eq!(catalogue().len(), 45);
    for p in catalogue() {
        assert!(p.address.starts_with("https://"), "{}", p.provider);
        assert!(!p.company.is_empty(), "{}", p.provider);
        assert!(matches!(p.day_count, 90 | 91), "{}", p.provider);
    }
    let ids = catalogue().iter().map(|p| p.provider).collect::<Vec<_>>();
    let unique = ids.iter().collect::<HashSet<_>>();
    assert_eq!(ids.len(), unique.len());
    // A company's page is one feed, however many providers read it.
    assert_eq!(page("cursor").unwrap().feed, page("grok-bot").unwrap().feed);
    assert_eq!(
        page("codex").unwrap().feed,
        page("openai-api").unwrap().feed
    );
    assert_eq!(
        page("kimi-code").unwrap().feed,
        page("moonshot").unwrap().feed
    );
    // Two MiniMax storefronts are two pages, not one switch.
    assert_ne!(
        page("minimax").unwrap().address,
        page("minimax-cn").unwrap().address
    );
}

#[test]
fn the_catalogue_has_no_stand_in_pages() {
    // xAI's and Mistral's status sit behind a bot check; a stand-in page would
    // say "all clear" about nothing, so the original has none and neither do we.
    for provider in [
        "xai-api",
        "antigravity",
        "grok",
        "mistral",
        "zai",
        "ollama-cloud",
        "unknown-provider",
    ] {
        assert!(page(provider).is_none(), "{provider}");
    }
    assert!(page("codex").is_some());
    assert!(page("gemini-cli").is_some());
}

#[test]
fn one_shared_feed_separates_grok_bot_from_the_rest_of_cursor() {
    let hidden = component("sm5wkcnqkvr9", "Grok Bot", Health::FullOutage);
    let cursor = page("cursor").unwrap();
    let grok = page("grok-bot").unwrap();
    assert!(!cursor.shows.matches(&hidden));
    assert!(!cursor.notifies_about(&hidden));
    assert!(grok.shows.matches(&hidden));
    assert!(grok.notifies_about(&hidden));
    // Cursor's own API rows are news for Cursor and not for Grok Bot.
    let api = component("rflc60xp5jp2", "API", Health::FullOutage);
    assert!(cursor.notifies_about(&api));
    assert!(!grok.notifies_about(&api));
}

#[test]
fn feed_words_and_severities_match_the_original() {
    for (word, state) in [
        ("OPERATIONAL", Health::Operational),
        ("DEGRADEDPERFORMANCE", Health::Degraded),
        ("degraded_performance", Health::Degraded),
        ("degraded", Health::Degraded),
        ("PARTIALOUTAGE", Health::PartialOutage),
        ("MAJOROUTAGE", Health::FullOutage),
        ("OUTAGE", Health::FullOutage),
        ("UNDERMAINTENANCE", Health::Maintenance),
        ("maintenance", Health::Maintenance),
        ("unknown", Health::Unrecognised),
        ("", Health::Unrecognised),
    ] {
        assert_eq!(Health::parse(word), state, "{word}");
    }
    assert!(Health::Degraded.outage());
    assert!(Health::PartialOutage.outage());
    assert!(Health::FullOutage.outage());
    // Maintenance is planned and unrecognised was never witnessed: neither is
    // announced as an outage.
    assert!(!Health::Maintenance.outage());
    assert!(!Health::Unrecognised.outage());
    assert!(!Health::Operational.outage());
    let order = [
        Health::Operational,
        Health::Maintenance,
        Health::Unrecognised,
        Health::Degraded,
        Health::PartialOutage,
        Health::FullOutage,
    ];
    assert_eq!(
        order.map(Health::severity),
        [0, 1, 2, 3, 4, 5]
    );
}

#[test]
fn codex_visible_group_and_affected_states() {
    let (rows, _) = feeds::incidentio_summary(&fixture("openai")).unwrap();
    let c = shown("codex", rows);
    assert_eq!(
        c.iter().map(|c| c.name.as_str()).collect::<Vec<_>>(),
        ["Codex Web", "Codex API", "CLI", "VS Code extension"]
    );
    assert!(c.iter().all(|c| c.state == Health::Operational));
    let (rows, _) = feeds::incidentio_summary(&fixture("incident")).unwrap();
    let c = shown("codex", rows);
    assert_eq!(
        c.iter().map(|c| c.state).collect::<Vec<_>>(),
        [
            Health::Degraded,
            Health::Operational,
            Health::FullOutage,
            Health::Unrecognised
        ]
    );
    assert!(feeds::incidentio_summary(&serde_json::json!({})).is_none());
}

#[test]
fn openai_history_matches_every_original_bar_and_official_uptime() {
    let (rows, _) = feeds::incidentio_summary(&fixture("openai")).unwrap();
    let mut c = shown("codex", rows);
    feeds::impact_history(&mut c, &fixture("impacts"), &days(91), 1791096000, false).unwrap();
    assert_eq!(bars(&c[0]),"0000000000000010011100000000000000000000000000000000000000011000000000000000000000300010000");
    assert_eq!(bars(&c[1]),"0000000000000000011100000000000000000000000000100000000000011000000000000000000000300010000");
    assert_eq!(bars(&c[2]),"0000000000001010011100000000000000000000000000000000000000011000000000000000000000300010000");
    assert_eq!(bars(&c[3]), bars(&c[0]));
    assert!(c.iter().all(|c| c.uptime == Some(99.95)));
    assert_eq!(c[0].days.len(), 91);
}

#[test]
fn claude_colours_and_unknown_history_preserve_current() {
    let mut c = shown(
        "claude-code",
        feeds::statuspage_components(&fixture("claude")).unwrap(),
    );
    assert_eq!(c.len(), 6);
    feeds::statuspage_history(&fixture("uptime"), &mut c).unwrap();
    let code = c.iter().find(|c| c.id == "yyzkbfz2thpt").unwrap();
    assert_eq!(code.days.len(), 90);
    assert_eq!(code.days[0].state, Some(Health::PartialOutage));
    assert_eq!(code.days[0].colour.as_deref(), Some("#e75f36"));
    assert_eq!(code.days[1].colour.as_deref(), Some("#76ad2a"));
    for c in &c {
        for d in &c.days {
            assert_eq!(
                d.colour.as_deref() == Some("#76ad2a"),
                d.state == Some(Health::Operational)
            )
        }
    }
    let mut current = shown(
        "claude-code",
        feeds::statuspage_components(&fixture("claude")).unwrap(),
    );
    assert!(feeds::statuspage_history(&serde_json::json!({}), &mut current).is_err());
    assert!(current.iter().all(|c| c.days.is_empty()));
}

#[test]
fn deepseek_flight_data_component_order_and_every_original_bar() {
    let v = deepseek();
    let now = DateTime::parse_from_rfc3339("2026-10-04T16:00:00+08:00")
        .unwrap()
        .timestamp();
    let mut c = feeds::flashcat_summary(&v, now).unwrap();
    assert_eq!(c.len(), 5);
    let page = page("deepseek").unwrap();
    assert_eq!(c.iter().filter(|c| page.notifies_about(c)).count(), 2);
    feeds::impact_history(&mut c, &v, &days(90), now, true).unwrap();
    assert_eq!(c.iter().map(bars).collect::<Vec<_>>(),[
        "000000000000000001000101100120020000001000010000000000000300000000000010100000220000000100",
        "000000000000000001000100200020020010000000010120000000000000000000000020000100220000000100",
        "000000000000000200000000100000000000000000000000000000000000000000000020000100220000000100",
        "---------------000000000000000000000000000000000000000000000000000000000000000000000000000",
        "---------------000000020000000000000003000200000000000000000000000000000000000000000000000"]);
    assert_eq!(
        c.iter().map(|c| c.uptime).collect::<Vec<_>>(),
        [Some(99.92), Some(99.69), Some(99.68), Some(100.), Some(99.91)]
    );
    assert!(feeds::flashcat_data("<html>unknown redesign</html>").is_err());
}

#[test]
fn partial_colours_invalid_dates_and_before_data_are_not_fabricated() {
    let mut c = vec![component("x", "X", Health::Degraded)];
    let v = serde_json::json!({"timelines":{"x":{"component":{"startDate":"2026-10-02"},"days":[{"date":"2026-10-01"},{"date":"2026-10-02","outages":{"m":60}},{"date":"invalid"},{"date":"2026-10-03","outages":{"p":60}}]}},"components":{"x":"<rect fill=\"#000000\" class=\"uptime-day\"/>"},"values":[{"component":"x","ninety":101}]});
    feeds::statuspage_history(&v, &mut c).unwrap();
    assert_eq!(bars(&c[0]), "-32");
    // A colour list that doesn't line up with the days is not a colour list.
    assert!(c[0].days.iter().all(|d| d.colour.is_none()));
    // 101 is not a percentage a page may state.
    assert_eq!(c[0].uptime, None);
    assert_eq!(c[0].state, Health::Degraded);
}

#[test]
fn malformed_unrelated_components_do_not_drop_valid_rows() {
    let v = serde_json::json!({"components":[{"id":"bad","name":null},{"id":"group","name":"Group","status":"operational","group":true},{"id":"quiet","name":"Quiet","status":"operational","only_show_if_degraded":true},{"id":"a","name":"API","status":"new-state","position":2}]});
    let c = feeds::statuspage_components(&v).unwrap();
    assert_eq!(c.len(), 1);
    assert_eq!(c[0].state, Health::Unrecognised);
    // A page whose whole component list is unreadable is no reading, never an
    // empty "all clear".
    assert!(feeds::statuspage_components(&serde_json::json!({})).is_none());
    assert!(feeds::statuspage_components(&serde_json::json!({"components":[]})).is_none());
}

#[test]
fn outages_deduplicate_escalate_deescalate_recover_and_survive_serialization() {
    let mut m = OutageMemory::default();
    let c = |s| vec![component("a", "API", s)];
    assert!(m.changes("codex", &c(Health::Operational)).worse.is_empty());
    assert_eq!(m.changes("codex", &c(Health::Degraded)).worse.len(), 1);
    let serialized = serde_json::to_string(&m).unwrap();
    let mut m: OutageMemory = serde_json::from_str(&serialized).unwrap();
    assert!(m.changes("codex", &c(Health::Degraded)).worse.is_empty());
    assert_eq!(m.changes("codex", &c(Health::FullOutage)).worse.len(), 1);
    assert!(m.changes("codex", &c(Health::PartialOutage)).worse.is_empty());
    assert_eq!(m.changes("codex", &c(Health::FullOutage)).worse.len(), 1);
    assert!(m.changes("codex", &c(Health::Maintenance)).worse.is_empty());
    assert!(m.changes("codex", &c(Health::Unrecognised)).worse.is_empty());
    assert_eq!(m.changes("codex", &c(Health::Operational)).recovered.len(), 1);
    assert!(m.changes("codex", &c(Health::Operational)).recovered.is_empty());
}

#[test]
fn disappearing_disabled_and_unrelated_components_never_claim_recovery() {
    let mut m = OutageMemory::default();
    let claude = page("claude-code").unwrap();
    // `changes` is handed the rows the page notifies about, as the check does.
    let told = |rows: &[Component]| {
        rows.iter()
            .filter(|c| claude.notifies_about(c))
            .cloned()
            .collect::<Vec<_>>()
    };
    let api = component("yyzkbfz2thpt", "Claude Code", Health::Degraded);
    let chat = component("chat", "claude.ai", Health::FullOutage);
    assert_eq!(m.changes("claude-code", &told(&[api.clone(), chat])).worse.len(), 1);
    assert!(m.changes("claude-code", &[]).recovered.is_empty());
    // Still down but no worse: recorded without a word, so getting worse again
    // is news again.
    assert_eq!(m.changes("claude-code", &told(&[api.clone()])).worse.len(), 1);
    assert!(m.changes("claude-code", &told(&[api.clone()])).worse.is_empty());
    let mut healthy = api.clone();
    healthy.state = Health::Operational;
    assert_eq!(m.changes("claude-code", &told(&[healthy])).recovered.len(), 1);
    // A provider no longer watched forgets everything it was told.
    m.keep(&HashSet::new());
    assert_eq!(m.changes("claude-code", &told(&[api])).worse.len(), 1);
}

#[test]
fn a_recovery_is_never_news_about_an_outage_nobody_heard_of() {
    let mut m = OutageMemory::default();
    // The first thing seen is a service already down: said at once.
    assert_eq!(m.changes("zed", &[component("x", "X", Health::FullOutage)]).worse.len(), 1);
    let mut quiet = OutageMemory::default();
    // A service that was never down and is now operational says nothing.
    assert!(quiet.changes("zed", &[component("x", "X", Health::Operational)]).recovered.is_empty());
    // An outage already under way when the setting goes on is said at once.
    assert_eq!(quiet.changes("zed", &[component("x", "X", Health::Degraded)]).worse.len(), 1);
}

#[test]
fn history_at_exact_boundaries_and_since_are_calendar_intersections() {
    let mut c = vec![component("x", "X", Health::Operational)];
    let v = serde_json::json!({"component_impacts":[{"component_id":"x","status":"full_outage","start_at_seconds":10,"end_at_seconds":20},{"component_id":"x","status":"partial_outage","start_at_seconds":30,"end_at_seconds":null},{"component_id":"x","status":"full_outage","start_at_seconds":15,"end_at_seconds":"broken"}],"component_uptimes":[{"component_id":"x","available_since_seconds":10,"uptime":99.95}]});
    feeds::impact_history(
        &mut c,
        &v,
        &[
            ("a".into(), 0, 10),
            ("b".into(), 10, 20),
            ("c".into(), 20, 30),
            ("d".into(), 30, 40),
        ],
        35,
        true,
    )
    .unwrap();
    assert_eq!(bars(&c[0]), "-302");
    assert_eq!(c[0].uptime, Some(99.95));
}

#[test]
fn better_stack_reads_one_request_with_days_uptime_and_groups() {
    let v = serde_json::json!({"included":[
        {"id":"1","type":"status_page_section","attributes":{"name":"Website","position":0}},
        {"id":"2","type":"status_page_section","attributes":{"name":"API","position":1}},
        {"id":"10","type":"status_page_resource","attributes":{"public_name":"chutes.ai","position":0,"status_page_section_id":1,"status":"operational","availability":"0.9991","status_history":[{"day":"2026-10-13","status":"downtime"},{"day":"2026-10-14","status":"operational"}]}},
        {"id":"11","type":"status_page_resource","attributes":{"public_name":"API Heartbeat","position":0,"status_page_section_id":"2","status":"downtime","status_history":[{"day":"2026-10-13","status":"not_monitored"},{"day":"2026-10-14","status":"operational"}]}},
        {"id":"12","type":"status_page_resource","attributes":{"name":"hidden","status":"operational"}}]});
    let c = feeds::better_stack(&v).unwrap();
    assert_eq!(
        c.iter().map(|c| c.name.as_str()).collect::<Vec<_>>(),
        ["chutes.ai", "API Heartbeat"]
    );
    assert_eq!(
        c.iter().map(|c| c.group.as_deref()).collect::<Vec<_>>(),
        [Some("Website"), Some("API")]
    );
    assert_eq!(c[1].state, Health::FullOutage);
    // "not_monitored" is no record, not a clear day.
    assert_eq!(c[1].days[0].state, None);
    assert_eq!(c[1].days[1].state, Some(Health::Operational));
    // availability is a fraction, so 0.9991 is 99.91 %.
    assert_eq!(c[0].uptime, Some(99.91));
}

#[test]
fn instatus_drops_the_group_rows_and_keeps_their_members() {
    let v = serde_json::json!({"components":[
        {"id":"zed","name":"Zed","status":"operational","group":{"id":"zed","name":"Zed"}},
        {"id":"delta","name":"Delta","status":"operational","group":{"id":"delta","name":"Delta"}},
        {"id":"agent","name":"Agent","status":"operational","group":{"id":"zed","name":"Zed"}},
        {"id":"edit","name":"Edit Prediction","status":"partial_outage","group":{"id":"delta","name":"Delta"}}]});
    let c = feeds::instatus(&v).unwrap();
    // The group's own row is listed as a component too; it is an aggregate and
    // is not drawn.
    assert_eq!(
        c.iter().map(|c| c.name.as_str()).collect::<Vec<_>>(),
        ["Agent", "Edit Prediction"]
    );
    assert_eq!(c[1].state, Health::PartialOutage);
    assert_eq!(c[1].group.as_deref(), Some("Delta"));
}

#[test]
fn upptime_and_gatus_read_state_from_what_the_monitor_last_saw() {
    let v = serde_json::json!([{"name":"command-code","slug":"command-code","status":"down"}]);
    let c = feeds::upptime(&v).unwrap();
    assert_eq!(c[0].id, "command-code");
    assert_eq!(c[0].state, Health::FullOutage);
    assert!(c[0].days.is_empty());
    assert_eq!(feeds::upptime_state("up"), Health::Operational);
    assert_eq!(feeds::upptime_state("degraded"), Health::Degraded);

    let v = serde_json::json!([
        {"name":"API Health Check","group":"Availability","key":"api","results":[{"status":500,"success":false},{"status":200,"success":true}]},
        {"name":"Slow","key":"slow","results":[{"status":200,"success":false}]},
        {"name":"Down","key":"down","results":[{"status":0,"success":false}]},
        {"name":"New","key":"new","results":[]}]);
    let c = feeds::gatus(&v).unwrap();
    assert_eq!(
        c.iter().map(|c| c.state).collect::<Vec<_>>(),
        [
            Health::Operational,
            // Failed on a condition (response time) while still answering 200.
            Health::Degraded,
            Health::FullOutage,
            Health::Unrecognised
        ]
    );
    assert_eq!(c[0].group.as_deref(), Some("Availability"));
}

#[test]
fn uptime_robot_unescapes_names_and_grades_each_day_by_its_ratio() {
    let v = serde_json::json!({"data":[{"monitorId":803945156,"name":"Soft Serve &#40;git.charm.sh&#41;","statusClass":"danger","dailyRatios":[{"date":"2026-10-10","ratio":"0","color":"grey"},{"date":"2026-10-11","ratio":"99.5"},{"date":"2026-10-12","ratio":"100"}],"90dRatio":{"ratio":"99.95"}}]});
    let c = feeds::uptime_robot(&v).unwrap();
    assert_eq!(c[0].id, "803945156");
    assert_eq!(c[0].name, "Soft Serve (git.charm.sh)");
    assert_eq!(c[0].state, Health::FullOutage);
    assert_eq!(
        c[0].days.iter().map(|d| d.state).collect::<Vec<_>>(),
        [None, Some(Health::PartialOutage), Some(Health::Operational)]
    );
    assert_eq!(c[0].uptime, Some(99.95));
}

#[test]
fn open_code_draws_fifty_six_three_hour_bars_ending_now() {
    let now = 1792033200 + 3600;
    let v = serde_json::json!({"components":[
        {"service":"go","status":"degraded"},
        {"service":"payg","status":"unknown"},
        {"service":"new","status":"operational"}],
        "history":[
        {"start":1792033200000.0,"services":{"go":"degraded","payg":"operational"}},
        {"start":1792022400000.0,"services":{"go":"unknown","payg":"operational"}}]});
    let c = feeds::open_code(&v, now).unwrap();
    assert_eq!(
        c.iter().map(|c| c.name.as_str()).collect::<Vec<_>>(),
        ["Go", "Pay-as-you-go", "new"]
    );
    assert_eq!(
        c.iter().map(|c| c.state).collect::<Vec<_>>(),
        [Health::Degraded, Health::Unrecognised, Health::Operational]
    );
    assert_eq!(c[0].days.len(), 56);
    assert!(c.iter().all(|c| c.days.iter().all(|d| d.hours == Some(3))));
    // The last slot is the one under way, the one before it three hours back.
    assert_eq!(c[0].days[55].state, Some(Health::Degraded));
    // "unknown" is the page saying it doesn't know, and a slot the feed never
    // covered has no record either.
    assert_eq!(c[0].days[54].state, None);
    assert_eq!(c[0].days[53].state, None);
    assert_eq!(c[1].days[54].state, Some(Health::Operational));
}

#[test]
fn deep_infra_reads_the_services_and_ignores_the_models() {
    let v = serde_json::json!({"services":[{"publicId":"api","displayName":"API","status":"OUTAGE"}],"models":[]});
    let c = feeds::deep_infra(&v).unwrap();
    assert_eq!(c.iter().map(|c| c.id.as_str()).collect::<Vec<_>>(), ["api"]);
    assert_eq!(c[0].state, Health::FullOutage);
}

#[test]
fn ibm_bob_collapses_repeated_checks_to_the_worst_state_in_first_order() {
    let v = serde_json::json!({"regions":[{"services":[
        {"name":"Chat/Code Completion","status":"operational"},
        {"name":"Admin","status":"operational"},
        {"name":"Chat/Code Completion","status":"partial_outage"}]}]});
    let c = feeds::ibm_bob(&v).unwrap();
    assert_eq!(
        c.iter().map(|c| c.name.as_str()).collect::<Vec<_>>(),
        ["Chat/Code Completion", "Admin"]
    );
    assert_eq!(c[0].state, Health::PartialOutage);
    let page = page("ibm-bob").unwrap();
    assert!(page.notifies_about(&c[0]));
    // The admin console is not what a plan's token budget runs on.
    assert!(!page.notifies_about(&c[1]));
}

#[test]
fn google_cloud_grades_a_product_by_the_incidents_open_over_it() {
    let v = serde_json::json!([
        {"begin":"2026-10-14T10:00:00Z","end":null,"status_impact":"SERVICE_DISRUPTION","affected_products":[{"id":"gca"}]},
        {"begin":"2026-10-12T10:00:00Z","end":"2026-10-12T11:00:00Z","status_impact":"SERVICE_OUTAGE","affected_products":[{"id":"gca"}]},
        {"begin":"2026-10-15T01:00:00Z","end":null,"status_impact":"SERVICE_OUTAGE","affected_products":[{"id":"gcs"}]}]);
    let now = DateTime::parse_from_rfc3339("2026-10-15T12:00:00Z")
        .unwrap()
        .timestamp();
    // Four calendar days ending today, as the page counts them.
    let midnight = now - now.rem_euclid(86_400);
    let days = (0..4i64)
        .rev()
        .map(|i| {
            let a = midnight - i * 86_400;
            let date = DateTime::from_timestamp(a, 0).unwrap().date_naive().to_string();
            (date, a, a + 86_400)
        })
        .collect::<Vec<DaySpan>>();
    let c = feeds::google_cloud(&v, "gca", "Gemini Code Assist", now, Some(&days)).unwrap();
    assert_eq!(c[0].state, Health::PartialOutage);
    // [full on the 12th, clear on the 13th, the disruption open on the 14th
    // and still on the 15th] — the original's own bars for this feed.
    assert_eq!(bars(&c[0]), "3022");
    // A product with no incident is operational — that is how the dashboard says it.
    let c = feeds::google_cloud(&serde_json::json!([]), "gca", "GCA", now, None).unwrap();
    assert_eq!(c[0].state, Health::Operational);
    assert!(feeds::google_cloud(&serde_json::json!({}), "gca", "GCA", now, None).is_none());
}

#[test]
fn a_page_that_cannot_be_read_is_no_reading_never_an_empty_all_clear() {
    // Every feed parser returns None rather than an empty list.
    assert!(feeds::better_stack(&serde_json::json!({})).is_none());
    assert!(feeds::instatus(&serde_json::json!({})).is_none());
    assert!(feeds::upptime(&serde_json::json!({})).is_none());
    assert!(feeds::gatus(&serde_json::json!({})).is_none());
    assert!(feeds::uptime_robot(&serde_json::json!({})).is_none());
    assert!(feeds::open_code(&serde_json::json!({}), 0).is_none());
    assert!(feeds::deep_infra(&serde_json::json!({})).is_none());
    assert!(feeds::ibm_bob(&serde_json::json!({})).is_none());
    assert!(feeds::flashcat_data("<html></html>").is_err());
}

#[test]
fn every_page_picks_the_palette_its_own_status_page_uses() {
    assert_eq!(page("codex").unwrap().palette(), Palette::OpenAi);
    assert_eq!(page("claude-code").unwrap().palette(), Palette::Claude);
    assert_eq!(page("cursor").unwrap().palette(), Palette::Cursor);
    assert_eq!(page("grok-bot").unwrap().palette(), Palette::Cursor);
    assert_eq!(page("opencode-go").unwrap().palette(), Palette::OpenCode);
    assert_eq!(page("zed").unwrap().palette(), Palette::Default);
    assert_eq!(page("amp").unwrap().palette(), Palette::IncidentIo);
    assert_eq!(page("deepseek").unwrap().palette(), Palette::Default);
    // The address a person opens, without its scheme.
    assert_eq!(page("codex").unwrap().host(), "status.openai.com");
    // The page a person opens, not the feed behind it.
    assert_eq!(
        page("command-code").unwrap().host(),
        "status.commandcode.ai"
    );
}
