pub(super) mod bindings;
pub mod browser;
mod output;

use std::time::Duration;

use atuin_client::history::HistoryStats;
use atuin_common::time::DurationExt as _;
use ratatui::Frame;
use ratatui::backend::FromCrossterm as _;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Bar, BarChart, BarGroup, Block, BorderType, Borders, Padding, Paragraph};
use time::macros::format_description;

use super::super::theme::{Meaning, Theme};

#[derive(Clone, Copy)]
struct Styles {
    base: Style,
    muted: Style,
    important: Style,
}

fn panel(title: impl Into<Line<'static>>, styles: Styles) -> Block<'static> {
    Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(styles.muted)
        .title_style(styles.important)
        .title(title)
}

/// Unknown exit codes (negative sentinel values) are not failures or successes.
fn success_rate(stats: &HistoryStats) -> String {
    let (success, known) = stats.exits.iter().filter(|(exit, _)| *exit >= 0).fold(
        (0_u128, 0_u128),
        |(success, known), (exit, count)| {
            let count = u128::try_from(*count).unwrap_or(0);
            (
                success
                    + if *exit == 0 {
                        count
                    } else {
                        0
                    },
                known + count,
            )
        },
    );
    if known == 0 {
        return "—".into();
    }
    let tenths = (success * 1000 + known / 2) / known;
    format!("{}.{:01}%", tenths / 10, tenths % 10)
}

fn monthly_durations(durations: &[(String, i64)]) -> Vec<(time::Date, i64)> {
    let mut durations: Vec<_> = durations
        .iter()
        .filter_map(|(date, duration)| {
            time::Date::parse(date, format_description!("[day]-[month]-[year]"))
                .ok()
                .map(|date| (date, *duration))
        })
        .collect();
    durations.sort_by_key(|(date, _)| *date);
    durations
}

/// Aggregate-only: occurrence metadata and command context belong in Runs.
pub fn draw(f: &mut Frame<'_>, area: Rect, stats: &HistoryStats, theme: &Theme) {
    let styles = Styles {
        base: Style::from_crossterm(theme.as_style(Meaning::Base)),
        muted: Style::from_crossterm(theme.as_style(Meaning::Annotation)),
        important: Style::from_crossterm(theme.as_style(Meaning::Important))
            .add_modifier(Modifier::BOLD),
    };
    let compact = area.height < 16 || area.width < 50;
    let areas = Layout::vertical([
        Constraint::Length(if compact {
            3
        } else {
            4
        }),
        Constraint::Min(0),
    ])
    .split(area);
    let metrics = [
        ("Total runs", stats.total.to_string()),
        ("Success rate¹", success_rate(stats)),
        ("Avg runtime", Duration::from_nanos(stats.average_duration).display().to_string()),
    ];
    if compact {
        let lines: Vec<_> = metrics
            .iter()
            .map(|(label, value)| {
                Line::from(vec![
                    Span::styled(format!("{label}  "), styles.muted),
                    Span::styled(value.as_str(), styles.important),
                ])
            })
            .collect();
        f.render_widget(Paragraph::new(lines), areas[0]);
        f.render_widget(Paragraph::new("¹ Known exits only").style(styles.muted), areas[1]);
    } else {
        let cards = Layout::horizontal([Constraint::Fill(1); 3]).split(areas[0]);
        for ((label, value), card) in metrics.into_iter().zip(cards.iter()) {
            f.render_widget(
                Paragraph::new(value)
                    .style(styles.important)
                    .block(panel(format!(" {label} "), styles).padding(Padding::horizontal(1))),
                *card,
            );
        }
        draw_charts(f, areas[1], stats, styles);
    }
}

fn chart(
    f: &mut Frame<'_>,
    area: Rect,
    title: String,
    bars: &[Bar<'_>],
    width: u16,
    styles: Styles,
) {
    f.render_widget(
        BarChart::default()
            .block(panel(title, styles))
            .bar_width(width)
            .bar_gap(1)
            .bar_style(styles.important)
            .value_style(styles.base)
            .label_style(styles.muted)
            .data(BarGroup::default().bars(bars)),
        area,
    );
}

fn draw_charts(f: &mut Frame<'_>, area: Rect, stats: &HistoryStats, styles: Styles) {
    let areas = Layout::vertical([Constraint::Fill(1), Constraint::Fill(1), Constraint::Length(1)])
        .split(area);
    let top = Layout::horizontal([Constraint::Fill(1); 2]).split(areas[0]);
    let mut exits = stats.exits.clone();
    exits.sort_by_key(|(exit, _)| *exit);
    let exit_width = if exits.iter().any(|(exit, _)| *exit < 0) {
        7
    } else {
        4
    };
    let capacity = usize::from(top[0].width.saturating_sub(2) / (exit_width + 1));
    let exit_bars: Vec<_> = exits
        .iter()
        .take(capacity)
        .map(|(exit, count)| {
            Bar::default()
                .label(if *exit < 0 {
                    "Unknown".into()
                } else {
                    exit.to_string()
                })
                .value(u64::try_from(*count).unwrap_or(0))
        })
        .collect();
    let title = if exits.len() > capacity {
        format!(" Exit codes · {capacity}/{} shown ", exits.len())
    } else {
        " Exit codes ".into()
    };
    chart(f, top[0], title, &exit_bars, exit_width, styles);
    // Single-character weekday labels let all seven days fit in a narrow terminal.
    let day_width = if top[1].width >= 30 {
        3
    } else {
        1
    };
    let days = if day_width == 3 {
        ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"]
    } else {
        ["S", "M", "T", "W", "T", "F", "S"]
    };
    let day_bars: Vec<_> = days
        .iter()
        .enumerate()
        .map(|(i, day)| {
            let count = stats
                .day_of_week
                .iter()
                .find(|(d, _)| d == &i.to_string())
                .map_or(0, |(_, count)| *count);
            Bar::default().label(*day).value(u64::try_from(count).unwrap_or(0))
        })
        .collect();
    chart(f, top[1], " Runs by weekday (UTC) ".into(), &day_bars, day_width, styles);
    let months = monthly_durations(&stats.duration_over_time);
    let capacity = usize::from(areas[1].width.saturating_sub(2) / 8);
    let bars: Vec<_> = months
        .iter()
        .skip(months.len().saturating_sub(capacity))
        .map(|(date, duration)| {
            Bar::default()
                .label(date.format(format_description!("[month]/[year]")).unwrap_or_default())
                .value(u64::try_from(*duration).unwrap_or(0))
                .text_value(
                    Duration::saturating_from_nanos_i64(*duration)
                        .display()
                        .largest_unit()
                        .to_string(),
                )
        })
        .collect();
    let title = if months.len() > capacity {
        format!(" Mean runtime / month · latest {capacity} of {} ", months.len())
    } else {
        " Mean runtime / month ".into()
    };
    if bars.is_empty() {
        f.render_widget(
            Paragraph::new("No runtime samples yet")
                .style(styles.muted)
                .block(panel(title, styles)),
            areas[1],
        );
    } else {
        chart(f, areas[1], title, &bars, 7, styles);
    }
    f.render_widget(Paragraph::new("¹ Known exits only").style(styles.muted), areas[2]);
}

#[cfg(test)]
mod tests {
    use atuin_client::theme::ThemeManager;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use rstest::{fixture, rstest};

    use super::*;

    #[fixture]
    fn stats() -> HistoryStats {
        HistoryStats {
            total: 10,
            average_duration: 2_000_000_000,
            exits: vec![(-1, 2), (0, 6), (1, 2)],
            day_of_week: vec![("0".into(), 4), ("2".into(), 6)],
            duration_over_time: vec![
                ("01-02-2026".into(), 2_000_000_000),
                ("01-12-2025".into(), 1_000_000_000),
            ],
        }
    }

    #[rstest]
    #[case(vec![(0, 6), (1, 2), (-1, 2)], "75.0%")]
    #[case(vec![(-1, 10)], "—")]
    #[case(vec![], "—")]
    #[case(vec![(0, 1), (1, 2)], "33.3%")]
    fn success_uses_only_known_exits(
        mut stats: HistoryStats,
        #[case] exits: Vec<(i64, i64)>,
        #[case] expected: &str,
    ) {
        stats.exits = exits;
        assert_eq!(success_rate(&stats), expected);
    }

    #[rstest]
    fn months_are_chronological(stats: HistoryStats) {
        let months = monthly_durations(&stats.duration_over_time);
        assert!(months[0].0 < months[1].0);
        assert_eq!(months[0].1, 1_000_000_000);
    }

    #[rstest]
    #[case(100, 30)]
    #[case(80, 24)]
    #[case(40, 6)]
    fn stats_show_aggregates_not_occurrence_details(
        stats: HistoryStats,
        #[case] width: u16,
        #[case] height: u16,
    ) {
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        let mut themes = ThemeManager::new(Some(true), Some(String::new()));
        let theme = themes.load_theme("(none)", None);
        terminal.draw(|f| draw(f, f.area(), &stats, theme)).unwrap();
        let text: String = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(ratatui::buffer::Cell::symbol)
            .collect();
        for expected in ["Total runs", "Success rate", "75.0%", "Avg runtime"] {
            assert!(text.contains(expected), "missing {expected}: {text}");
        }
        for removed in
            ["Previous command", "Next command", "Directory", "Selected run", "Command stats"]
        {
            assert!(!text.contains(removed));
        }
        if height >= 16 {
            assert!(text.contains("Exit codes"));
            assert!(text.contains("Mean runtime / month"));
        }
        assert!(!text.contains('\0'));
    }

    #[rstest]
    #[case(1, 1)]
    #[case(20, 5)]
    fn empty_stats_fit_small_terminals(
        mut stats: HistoryStats,
        #[case] width: u16,
        #[case] height: u16,
    ) {
        stats.total = 0;
        stats.exits.clear();
        stats.duration_over_time.clear();
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        let mut themes = ThemeManager::new(Some(true), Some(String::new()));
        let theme = themes.load_theme("(none)", None);
        terminal.draw(|f| draw(f, f.area(), &stats, theme)).unwrap();
    }
}
