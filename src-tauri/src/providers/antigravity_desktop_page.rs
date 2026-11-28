//! Read only the labeled percentages shown on Antigravity's model quota page.

use super::{by_window_length, length};
use crate::model::{UsageWindow, WindowKind};

pub(super) const MODELS_PAGE_HEADINGS: &[&str] =
    &["模型与用量", "Models & Usage", "Models and Usage"];

pub(super) fn is_models_page_heading(text: &str) -> bool {
    let heading = text.trim().to_ascii_lowercase();
    let heading = heading
        .strip_suffix(" refresh quota and credits data")
        .or_else(|| heading.strip_suffix(" 刷新额度与积分"))
        .unwrap_or(&heading);
    MODELS_PAGE_HEADINGS
        .iter()
        .any(|name| heading.eq_ignore_ascii_case(name))
}

fn group_name(text: &str) -> Option<String> {
    match text {
        "Claude 和 GPT 模型"
        | "Claude and GPT Models"
        | "Claude and GPT models"
        | "Claude & GPT Models"
        | "Claude & GPT models" => Some("Claude and GPT".into()),
        "Gemini 模型" | "Gemini Models" | "Gemini models" => Some("Gemini".into()),
        name if name.starts_with("Gemini 4 ") => {
            let model = name
                .strip_suffix(" 模型")
                .or_else(|| name.strip_suffix(" Models"))
                .or_else(|| name.strip_suffix(" models"))
                .unwrap_or(name);
            let family = model.strip_prefix("Gemini 4 ")?;
            // A model-group heading, rather than a sentence about a model.
            (!family.is_empty() && family.chars().all(|c| c.is_ascii_alphabetic()))
                .then(|| model.to_owned())
        }
        _ => None,
    }
}

fn quota_period(text: &str) -> Option<&'static str> {
    match text.to_ascii_lowercase().as_str() {
        "周剩余额度" | "weekly limit remaining" => Some("weekly"),
        "5 小时剩余额度" | "five hour limit remaining" | "5 hour limit remaining" => {
            Some("5h")
        }
        _ => None,
    }
}

fn reset_text(text: &str, period: &str) -> bool {
    let (chinese, english) = if period == "weekly" {
        ("周额度将在 ", "weekly limit resets in ")
    } else {
        ("5 小时额度将在 ", "five hour limit resets in ")
    };
    (text.starts_with(chinese) && text.ends_with("后重置。"))
        || text.to_ascii_lowercase().starts_with(english)
}

fn remaining_percent(text: &str) -> Option<f64> {
    let number = text.strip_suffix('%')?.trim_end();
    if !number.chars().all(|c| c.is_ascii_digit() || c == '.') {
        return None;
    }
    let value = number.parse().ok()?;
    (0.0..=100.0).contains(&value).then_some(value)
}

fn bucket<'a>(rows: &mut &'a [&str], scope: &str) -> Option<(i64, UsageWindow)> {
    let [label, rest @ ..] = *rows else {
        return None;
    };
    let period = quota_period(label)?;
    let (reset, rest) = match rest {
        [text, tail @ ..] if reset_text(text, period) => (Some((*text).to_owned()), tail),
        _ => (None, rest),
    };
    let [percent, tail @ ..] = rest else {
        return None;
    };
    let remaining = remaining_percent(percent)?;
    let (label, duration) = length(Some(period))?;
    *rows = tail;
    let window = UsageWindow::new(label, Some(100.0 - remaining))
        .with_id(Some(format!("desktop:{scope}:{period}")))
        .with_scope(Some(scope.to_owned()))
        .with_kind(WindowKind::Limit)
        .with_duration(Some(duration as f64))
        .with_exhausted(remaining == 0.0)
        // Rounded countdowns are display text, not authoritative reset times.
        .with_detail(reset);
    Some((duration, window))
}

pub(super) fn windows_from_models_page(rows: &[String]) -> Vec<UsageWindow> {
    let rows: Vec<_> = rows
        .iter()
        .map(|row| row.trim())
        .filter(|row| !row.is_empty())
        .collect();
    let Some((title, mut rows)) = rows.split_first() else {
        return Vec::new();
    };
    if !is_models_page_heading(title) {
        return Vec::new();
    }

    let mut windows: Vec<UsageWindow> = Vec::new();
    while let [text, tail @ ..] = rows {
        rows = tail;
        let Some(scope) = group_name(text) else {
            continue;
        };
        let mut group = Vec::new();
        while let Some(window) = bucket(&mut rows, &scope) {
            if group
                .iter()
                .any(|previous: &(i64, UsageWindow)| previous.1.id == window.1.id)
                || windows.iter().any(|previous| previous.id == window.1.id)
            {
                return Vec::new();
            }
            group.push(window);
        }
        windows.extend(by_window_length(group));
    }
    windows
}

#[derive(Debug)]
pub(super) struct OcrLine {
    pub text: String,
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

impl OcrLine {
    fn middle(&self) -> f32 {
        self.y + self.height / 2.0
    }
    fn right(&self) -> f32 {
        self.x + self.width
    }
}

pub(super) struct VisualRows {
    pub left: usize,
    pub top: usize,
    pub right: usize,
    pub bottom: usize,
    centers: [f32; 4],
    tolerance: f32,
    broad_values: [Option<f64>; 4],
}

fn compact(text: &str) -> String {
    text.chars()
        .filter(|c| !c.is_whitespace())
        .map(|c| match c {
            '％' => '%',
            c @ '０'..='９' => char::from_u32(c as u32 - '０' as u32 + '0' as u32).unwrap_or(c),
            c => c.to_ascii_lowercase(),
        })
        .collect()
}

fn visual_group(text: &str) -> Option<usize> {
    // The small information icon is sometimes recognized as a trailing O.
    match compact(text).trim_end_matches(['o', '○', 'ⓘ']) {
        "gemini模型" | "geminimodels" => Some(0),
        "claude和gpt模型" | "claudeandgptmodels" | "claude&gptmodels" => Some(1),
        _ => None,
    }
}

fn visual_period(text: &str) -> Option<&'static str> {
    match compact(text).as_str() {
        "周剩余额度" | "weeklylimitremaining" => Some("weekly"),
        "5小时剩余额度" | "fivehourlimitremaining" | "5hourlimitremaining" => Some("5h"),
        _ => None,
    }
}

fn row_at(y: f32, centers: &[f32; 4], tolerance: f32) -> Option<usize> {
    let mut matches = centers
        .iter()
        .enumerate()
        .filter(|(_, center)| (y - **center).abs() <= tolerance);
    let (index, _) = matches.next()?;
    matches.next().is_none().then_some(index)
}

/// Locate the percentage column from the model labels and the percentages
/// already recognized in the broad pass. The tighter second pass reads from
/// the same frame without relying on fixed screen coordinates.
pub(super) fn locate_visual_rows(
    lines: &[OcrLine],
    width: usize,
    height: usize,
) -> Option<VisualRows> {
    let headings: Vec<_> = lines
        .iter()
        .filter_map(|line| visual_group(&line.text).map(|group| (group, line)))
        .collect();
    let [(0, gemini), (1, claude)] = headings.as_slice() else {
        return None;
    };
    if claude.y <= gemini.y || (claude.x - gemini.x).abs() > gemini.height * 2.0 {
        return None;
    }
    let mut quota_labels: Vec<_> = lines
        .iter()
        .filter_map(|line| {
            if line.y > gemini.y && (line.x - gemini.x).abs() < gemini.height * 2.5 {
                return Some((visual_period(&line.text)?, line));
            }
            None
        })
        .collect();
    quota_labels.sort_by(|a, b| a.1.y.total_cmp(&b.1.y));
    let [("weekly", gemini_week), ("5h", gemini_hours), ("weekly", claude_week), ("5h", claude_hours)] =
        quota_labels.as_slice()
    else {
        return None;
    };
    if gemini_hours.y >= claude.y || claude_week.y <= claude.y {
        return None;
    }
    let labels = [gemini_week, gemini_hours, claude_week, claude_hours];
    let centers = labels.map(|label| label.middle());
    let label_height = labels.iter().map(|label| label.height).fold(0.0, f32::max);
    let gap = centers
        .windows(2)
        .map(|pair| pair[1] - pair[0])
        .fold(f32::INFINITY, f32::min);
    let tolerance = (label_height * 1.6).min(gap * 0.44);

    let mut anchors = Vec::new();
    let mut broad_values = [None; 4];
    for line in lines {
        let Some(percent) = remaining_percent(&compact(&line.text)) else {
            continue;
        };
        let Some(index) = row_at(line.middle(), &centers, tolerance) else {
            continue;
        };
        if line.x <= gemini.x + width as f32 * 0.18 {
            continue;
        }
        if broad_values[index].replace(percent).is_some() {
            return None;
        }
        anchors.push(line);
    }
    if anchors.len() < 2 {
        return None;
    }
    let number_right = anchors.iter().map(|line| line.right()).fold(0.0, f32::max);
    let number_width = anchors.iter().map(|line| line.width).fold(0.0, f32::max);
    if anchors
        .iter()
        .any(|line| (number_right - line.right()).abs() > label_height * 1.1)
    {
        return None;
    }

    let left = (number_right - number_width - label_height * 1.6).floor() as usize;
    let right = ((number_right + label_height * 0.85).ceil() as usize).min(width);
    let top = (gemini_week.y - label_height * 1.2).floor() as usize;
    let bottom =
        ((claude_hours.y + claude_hours.height + label_height * 1.3).ceil() as usize).min(height);
    (left < right && top < bottom).then_some(VisualRows {
        left,
        top,
        right,
        bottom,
        centers,
        tolerance,
        broad_values,
    })
}

impl VisualRows {
    pub(super) fn finish(&self, lines: &[OcrLine]) -> Option<Vec<String>> {
        let mut values = [None; 4];
        for line in lines {
            let Some(percent) = remaining_percent(&compact(&line.text)) else {
                continue;
            };
            let Some(index) = row_at(line.middle(), &self.centers, self.tolerance) else {
                continue;
            };
            if values[index].replace(percent).is_some()
                || self.broad_values[index].is_some_and(|broad| broad != percent)
            {
                return None;
            }
        }
        let [Some(gemini_week), Some(gemini_hours), Some(claude_week), Some(claude_hours)] = values
        else {
            return None;
        };
        Some(vec![
            MODELS_PAGE_HEADINGS[0].into(),
            "Gemini 模型".into(),
            "周剩余额度".into(),
            format!("{gemini_week}%"),
            "5 小时剩余额度".into(),
            format!("{gemini_hours}%"),
            "Claude 和 GPT 模型".into(),
            "周剩余额度".into(),
            format!("{claude_week}%"),
            "5 小时剩余额度".into(),
            format!("{claude_hours}%"),
        ])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(rows: &[&str]) -> Vec<UsageWindow> {
        windows_from_models_page(&rows.iter().map(|row| (*row).into()).collect::<Vec<_>>())
    }

    #[test]
    fn chinese_236_quota_preserves_visible_percentages_and_group_order() {
        let windows = parse(&[
            "模型与用量 Refresh quota and credits data",
            "套餐",
            "Google AI Pro",
            "Gemini 模型",
            "周剩余额度",
            "周额度将在 6 天 1 小时后重置。",
            "77%",
            "5 小时剩余额度",
            "5 小时额度将在 3 小时 7 分钟后重置。",
            "85%",
            "Claude 和 GPT 模型",
            "周剩余额度",
            "100%",
            "5 小时剩余额度",
            "100%",
        ]);
        assert_eq!(windows.len(), 4);
        assert_eq!(windows[0].id.as_deref(), Some("desktop:Gemini:5h"));
        assert_eq!(windows[0].percent_used, Some(15.0));
        assert_eq!(
            windows[0].detail.as_deref(),
            Some("5 小时额度将在 3 小时 7 分钟后重置。")
        );
        assert_eq!(windows[0].window_seconds, Some(18_000.0));
        assert!(windows[0].resets_at.is_none());
        assert_eq!(windows[1].percent_used, Some(23.0));
        assert_eq!(windows[2].scope.as_deref(), Some("Claude and GPT"));
        assert_eq!(windows[2].percent_used, Some(0.0));
        assert!(!windows[3].is_exhausted);
    }

    #[test]
    fn optional_english_group_can_report_one_window_and_zero_remaining() {
        let windows = parse(&[
            "Models & Usage",
            "Gemini 4 Argon",
            "Weekly Limit Remaining",
            "0%",
        ]);
        assert_eq!(windows.len(), 1);
        assert_eq!(windows[0].scope.as_deref(), Some("Gemini 4 Argon"));
        assert_eq!(windows[0].percent_used, Some(100.0));
        assert!(windows[0].is_exhausted);
    }

    #[test]
    fn unrelated_percentages_or_ambiguous_ranges_are_not_read() {
        assert!(parse(&["Conversation", "Gemini 模型", "周剩余额度", "77%"]).is_empty());
        assert!(parse(&[
            "模型与用量",
            "Gemini 模型",
            "周剩余额度",
            "Unrelated",
            "77%"
        ])
        .is_empty());
        assert!(parse(&["模型与用量", "Gemini 模型", "周剩余额度", "112%"]).is_empty());
        assert!(parse(&[
            "模型与用量",
            "Gemini 模型",
            "周剩余额度",
            "77%",
            "周剩余额度",
            "85%"
        ])
        .is_empty());
    }

    #[test]
    fn visual_reader_binds_four_percentages_to_actual_quota_labels() {
        fn line(text: &str, x: f32, y: f32, width: f32) -> OcrLine {
            OcrLine {
                text: text.into(),
                x,
                y,
                width,
                height: 20.0,
            }
        }
        let broad = [
            line("Gemini 模 型 O", 778.0, 839.0, 143.0),
            line("周 剩 余 额 度", 796.0, 893.0, 105.0),
            line("5 小 时 剩 余 额 度", 797.0, 975.0, 141.0),
            line("Claude 和 GPT 模 型", 778.0, 1070.0, 185.0),
            line("周 剩 余 额 度", 796.0, 1133.0, 105.0),
            line("5 小 时 剩 余 额 度", 797.0, 1206.0, 141.0),
            line("85 ％", 1587.0, 991.0, 41.0),
            line("100 ％", 1575.0, 1135.0, 53.0),
            line("100 ％", 1575.0, 1208.0, 53.0),
        ];
        let rows = locate_visual_rows(&broad, 2187, 1435).unwrap();
        assert_eq!(
            (rows.left, rows.right, rows.top, rows.bottom),
            (1543, 1645, 869, 1252)
        );
        let mut numbers = [
            line("77%", 1587.0, 906.0, 41.0),
            line("85%", 1587.0, 991.0, 41.0),
            line("100%", 1575.0, 1135.0, 53.0),
            line("100%", 1575.0, 1208.0, 53.0),
        ];
        let windows = windows_from_models_page(&rows.finish(&numbers).unwrap());
        assert_eq!(
            windows.iter().map(|w| w.percent_used).collect::<Vec<_>>(),
            [Some(15.0), Some(23.0), Some(0.0), Some(0.0)]
        );
        assert!(rows.finish(&numbers[1..]).is_none());
        numbers[1].text = "65%".into();
        assert!(rows.finish(&numbers).is_none());
        assert!(locate_visual_rows(&broad[1..], 2187, 1435).is_none());
    }
}
