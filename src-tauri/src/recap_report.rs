//! `--recap [YYYY-MM-DD | week-YYYY-MM-DD | YYYY-MM | YYYY]`: one period's
//! real Token figures as JSON on stdout. Read-only: it never stores a setting
//! and never starts the GUI. A figure it cannot stand behind is absent, not zero.
use chrono::{Datelike, Duration, Local, NaiveDate};
use serde_json::{json, Value};

struct Period {
    key: String,
    start: NaiveDate,
    end: NaiveDate,
}

fn parse_day(text: &str) -> Option<NaiveDate> {
    NaiveDate::parse_from_str(text, "%Y-%m-%d").ok()
}

fn period(value: Option<&str>) -> Result<Period, String> {
    let now = Local::now().date_naive();
    let Some(text) = value else {
        let start = NaiveDate::from_ymd_opt(now.year(), now.month(), 1).unwrap();
        let end = if now.month() == 12 {
            NaiveDate::from_ymd_opt(now.year() + 1, 1, 1).unwrap()
        } else {
            NaiveDate::from_ymd_opt(now.year(), now.month() + 1, 1).unwrap()
        };
        return Ok(Period { key: format!("{:04}-{:02}", now.year(), now.month()), start, end });
    };
    if let Some(rest) = text.strip_prefix("week-") {
        let day = parse_day(rest).ok_or_else(|| usage())?;
        let start = day - Duration::days(i64::from(day.weekday().num_days_from_monday()));
        return Ok(Period { key: format!("week-{start}"), start, end: start + Duration::days(7) });
    }
    let pieces = text.split('-').collect::<Vec<_>>();
    match pieces.as_slice() {
        [year] if year.len() == 4 => {
            let year: i32 = year.parse().map_err(|_| usage())?;
            let start = NaiveDate::from_ymd_opt(year, 1, 1).ok_or_else(|| usage())?;
            let end = NaiveDate::from_ymd_opt(year + 1, 1, 1).ok_or_else(|| usage())?;
            Ok(Period { key: format!("{year:04}"), start, end })
        }
        [year, month] if year.len() == 4 && month.len() == 2 => {
            let year: i32 = year.parse().map_err(|_| usage())?;
            let month: u32 = month.parse().map_err(|_| usage())?;
            let start = NaiveDate::from_ymd_opt(year, month, 1).ok_or_else(|| usage())?;
            let end = if month == 12 {
                NaiveDate::from_ymd_opt(year + 1, 1, 1)
            } else {
                NaiveDate::from_ymd_opt(year, month + 1, 1)
            }
            .ok_or_else(|| usage())?;
            Ok(Period { key: format!("{year:04}-{month:02}"), start, end })
        }
        _ => {
            let start = parse_day(text).ok_or_else(|| usage())?;
            Ok(Period { key: start.to_string(), start, end: start + Duration::days(1) })
        }
    }
}

fn usage() -> String {
    "usage: PulseWin --recap [YYYY-MM-DD | week-YYYY-MM-DD | YYYY-MM | YYYY]".into()
}

fn value_argument(args: &[String]) -> Option<&str> {
    let at = args.iter().position(|arg| arg == "--recap")? + 1;
    args.get(at).filter(|value| !value.starts_with('-')).map(String::as_str)
}

pub fn print(args: &[String]) -> Result<(), String> {
    let period = period(value_argument(args))?;
    let snapshot = tauri::async_runtime::block_on(crate::token_spend::cli_snapshot())?;
    let mut days = std::collections::BTreeMap::<String, (u128, f64)>::new();
    let mut models = std::collections::BTreeMap::<String, u128>::new();
    let mut agents = std::collections::BTreeMap::<String, u128>::new();
    let mut tokens = 0u128;
    let mut cost = 0f64;
    let mut cost_known = true;
    for record in &snapshot.records {
        let Ok(day) = NaiveDate::parse_from_str(&record.day, "%Y-%m-%d") else { continue };
        if day < period.start || day >= period.end {
            continue;
        }
        let amount = u128::from(record.tally.total()) + u128::from(record.unclassified_tokens);
        if amount == 0 {
            continue;
        }
        tokens += amount;
        let day_entry = days.entry(record.day.clone()).or_default();
        day_entry.0 += amount;
        *models.entry(record.model.clone()).or_default() += amount;
        *agents.entry(record.agent.clone()).or_default() += amount;
        match record.cost {
            Some(value) => {
                cost += value;
                day_entry.1 += value;
            }
            None => cost_known = false,
        }
    }
    let value = json!({
        "period": period.key,
        "start": period.start.to_string(),
        "end": period.end.to_string(),
        "tokens": tokens.to_string(),
        "estimatedCost": if cost_known { Some(cost) } else { None },
        "days": days.iter().map(|(day,(tokens,cost))| json!({"day":day,"tokens":tokens.to_string(),"estimatedCost":cost})).collect::<Vec<_>>(),
        "models": models.iter().map(|(model,tokens)| json!({"model":model,"tokens":tokens.to_string()})).collect::<Vec<_>>(),
        "agents": agents.iter().map(|(agent,tokens)| json!({"agent":agent,"tokens":tokens.to_string()})).collect::<Vec<_>>(),
        "notes": snapshot.notes,
        "pricesAt": snapshot.prices_at,
        "pricingStatus": snapshot.pricing_status,
    });
    println!("{}", serde_json::to_string_pretty(&value).map_err(|error| error.to_string())?);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn periods_accept_day_week_month_year_and_default_to_this_month() {
        let day = period(Some("2026-10-18")).unwrap();
        assert_eq!((day.key.as_str(), day.start.to_string(), day.end.to_string()), ("2026-10-18", "2026-10-18".into(), "2026-10-19".into()));
        let week = period(Some("week-2026-10-14")).unwrap();
        assert_eq!(week.key, "week-2026-10-12");
        assert_eq!((week.start.to_string(), week.end.to_string()), ("2026-10-12".into(), "2026-10-19".into()));
        let month = period(Some("2026-02")).unwrap();
        assert_eq!((month.start.to_string(), month.end.to_string()), ("2026-02-01".into(), "2026-03-01".into()));
        let year = period(Some("2024")).unwrap();
        assert_eq!((year.start.to_string(), year.end.to_string()), ("2024-01-01".into(), "2025-01-01".into()));
        assert!(period(Some("2026-02-30")).is_err());
        assert!(period(Some("2026-13")).is_err());
    }
}
