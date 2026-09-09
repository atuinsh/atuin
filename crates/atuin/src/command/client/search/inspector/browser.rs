use std::time::Duration;

use atuin_client::database::Sqlite;
use atuin_client::history::{History, HistoryId};
use atuin_client::settings::Settings;
use atuin_common::string::EscapeNonPrintablePosixExt as _;
use atuin_common::time::DurationExt as _;
use ratatui::Frame;
use ratatui::backend::FromCrossterm as _;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{
    Block, BorderType, Borders, Cell, Padding, Paragraph, Row, Table, TableState, Tabs, Wrap,
};
use time::macros::format_description;
use unicode_width::UnicodeWidthStr as _;

use super::bindings::Bindings;
use super::output::Capture;
use crate::command::client::search::keybindings::Action;
use crate::command::client::search::syntax;
use crate::command::client::theme::{Meaning, Theme};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum View {
    #[default]
    Runs,
    Session,
    Stats,
    Output,
}

/// Switching views pivots on the selected occurrence, not the original search result.
#[derive(Default)]
pub struct Browser {
    pub view: View,
    entries: Vec<History>,
    scope: Option<(View, String)>,
    window_for: Option<HistoryId>,
    table: TableState,

    output: Capture,
    output_scroll: usize,
    output_max_scroll: usize,
    output_page_size: usize,
    output_return: View,
}

impl Browser {
    pub fn select_view(&mut self, view: View) {
        if self.view != view {
            if view == View::Output {
                self.output_return = self.view;
                self.output.reopen();
            }
            self.view = view;
        }
    }

    pub fn back_from_output(&mut self) {
        self.view = self.output_return;
    }

    /// The window is re-centered before the next input event, so its edges describe
    /// real navigation boundaries, not an arbitrary database page boundary.
    pub fn list_position(&self) -> (usize, usize) {
        match self.view {
            View::Runs | View::Session => (self.table.selected().unwrap_or(0), self.entries.len()),
            View::Stats => (0, 1),
            View::Output => (self.output_scroll, self.output_max_scroll + 1),
        }
    }

    pub fn scroll_output_page(&mut self, next: bool, half: bool) {
        let amount = if half {
            self.output_page_size / 2
        } else {
            self.output_page_size.saturating_sub(1)
        }
        .max(1);

        self.output_scroll = if next {
            self.output_scroll.saturating_add(amount).min(self.output_max_scroll)
        } else {
            self.output_scroll.saturating_sub(amount)
        };
    }

    pub fn scroll_output_edge(&mut self, end: bool) {
        self.output_scroll = if end {
            self.output_max_scroll
        } else {
            0
        };
    }

    pub fn scroll_output(&mut self, next: bool) {
        self.output_scroll = if next {
            self.output_scroll.saturating_add(1).min(self.output_max_scroll)
        } else {
            self.output_scroll.saturating_sub(1)
        };
    }

    pub async fn refresh(
        &mut self,
        db: &Sqlite,
        selected: &History,
        settings: &Settings,
    ) -> eyre::Result<(Option<HistoryId>, Option<HistoryId>)> {
        if self.view == View::Output {
            if self.output.poll(selected.id, settings).await {
                self.output_scroll = 0;
                self.output_max_scroll = 0;
            }

            return Ok((None, None));
        }

        let scope = if self.view == View::Session {
            selected.session.clone()
        } else {
            selected.command.clone()
        };
        let position = self.entries.iter().position(|entry| entry.id == selected.id);
        let at_edge = position.is_none_or(|i| i == 0 || i + 1 == self.entries.len());

        if self.scope.as_ref() != Some(&(self.view, scope.clone()))
            || (at_edge && self.window_for != Some(selected.id))
        {
            self.entries = db.inspector_history(selected, self.view == View::Session).await?;
            self.entries.reverse();
            self.scope = Some((self.view, scope));
            self.window_for = Some(selected.id);
            self.table = TableState::default();
        }

        let position = self.entries.iter().position(|entry| entry.id == selected.id);
        self.table.select(position);
        Ok(position.map_or((None, None), |i| {
            (
                i.checked_sub(1).map(|i| self.entries[i].id),
                self.entries.get(i + 1).map(|entry| entry.id),
            )
        }))
    }

    pub fn draw(
        &mut self,
        f: &mut Frame<'_>,
        chunk: Rect,
        selected: &History,
        settings: &Settings,
        theme: &Theme,
        bindings: &Bindings,
    ) {
        let styles = Styles::new(theme);
        if self.view == View::Output {
            self.draw_output(f, chunk, selected, settings, theme, bindings);
            return;
        }

        let areas = Layout::vertical([
            Constraint::Min(3),
            Constraint::Length(if chunk.height >= 10 {
                5
            } else {
                0
            }),
        ])
        .split(chunk);
        self.draw_list(f, areas[0], settings, styles);
        draw_details(f, areas[1], selected, settings, styles);
    }

    fn draw_list(&mut self, f: &mut Frame<'_>, area: Rect, settings: &Settings, styles: Styles) {
        let session = self.view == View::Session;
        let narrow = area.width < 90;
        let tiny = area.width < 65;

        let rows = self.entries.iter().map(|entry| {
            let mut cells = vec![
                Cell::from(if tiny {
                    clock(entry, settings)
                } else {
                    timestamp(entry, settings)
                })
                .style(styles.muted),
                Cell::from(exit_label(entry.exit)).style(if entry.exit > 0 {
                    styles.failure
                } else {
                    styles.muted
                }),
                Cell::from(
                    Duration::saturating_from_nanos_i64(entry.duration)
                        .display()
                        .largest_unit()
                        .to_string(),
                )
                .style(styles.muted),
            ];
            if !narrow {
                cells.push(Cell::from(origin(entry)).style(styles.muted));
            }
            cells.push(Cell::from(if session {
                entry.command.escape_non_printable()
            } else {
                entry.cwd.escape_non_printable()
            }));
            Row::new(cells)
        });

        let mut widths = vec![
            Constraint::Length(if tiny {
                8
            } else {
                19
            }),
            Constraint::Length(8),
            Constraint::Length(8),
        ];
        let mut headings = vec!["Time", "Exit", "Duration"];
        if !narrow {
            widths.push(Constraint::Length(22));
            headings.push("User@Host");
        }
        widths.push(Constraint::Min(1));
        headings.push(if session {
            "Command"
        } else {
            "Directory"
        });

        let title = if session {
            " Session "
        } else {
            " Runs "
        };
        let block = panel(title, styles);
        let table = Table::new(rows, widths)
            .header(
                Row::new(headings).style(styles.label).bottom_margin(u16::from(area.height >= 10)),
            )
            .block(block)
            .style(styles.base)
            .row_highlight_style(styles.command.add_modifier(Modifier::REVERSED))
            .highlight_symbol("› ");

        f.render_stateful_widget(table, area, &mut self.table);
    }

    fn draw_output(
        &mut self,
        f: &mut Frame<'_>,
        area: Rect,
        selected: &History,
        settings: &Settings,
        theme: &Theme,
        bindings: &Bindings,
    ) {
        let styles = Styles::new(theme);
        let spacious = area.height >= 10;
        let areas = Layout::vertical([
            Constraint::Length(if spacious {
                2
            } else {
                1
            }),
            Constraint::Min(1),
            Constraint::Length(1),
        ])
        .split(area);

        let mut command = command_text(selected, settings.ui.syntax_highlight, theme)
            .lines
            .into_iter()
            .next()
            .unwrap_or_default();
        command.spans.insert(0, Span::styled("cmd: ", styles.muted));
        let heading = vec![
            command,
            Line::styled(
                format!(
                    " {}  ·  {}  ·  exit {}",
                    timestamp(selected, settings),
                    origin(selected),
                    exit_label(selected.exit)
                ),
                styles.muted,
            ),
        ];
        f.render_widget(Paragraph::new(heading), areas[0]);

        let status = self.output.status().to_owned();
        let block = panel(format!(" {status} "), styles);

        // At very small heights, spend the remaining space on output instead of borders.
        let inner = if spacious {
            block.inner(areas[1])
        } else {
            areas[1]
        };

        let rows = self.output.rows(inner.width);
        self.output_page_size = usize::from(inner.height);
        self.output_max_scroll = rows.len().saturating_sub(self.output_page_size);
        self.output_scroll = self.output_scroll.min(self.output_max_scroll);

        if spacious {
            let progress = format!(
                " {}–{} / {} rows ",
                self.output_scroll + usize::from(!rows.is_empty()),
                (self.output_scroll + self.output_page_size).min(rows.len()),
                rows.len()
            );
            f.render_widget(block.title_bottom(Line::from(progress).right_aligned()), areas[1]);
        }

        let text = if rows.is_empty() {
            vec![Line::styled(status, styles.muted)]
        } else {
            rows.iter().skip(self.output_scroll).take(self.output_page_size).cloned().collect()
        };
        f.render_widget(Paragraph::new(text).style(Style::reset()), inner);

        f.render_widget(
            Paragraph::new(guide(View::Output, areas[2].width, styles, bindings)),
            areas[2],
        );
    }
}

#[derive(Clone, Copy)]
struct Styles {
    base: Style,
    muted: Style,
    label: Style,
    command: Style,
    key: Style,
    failure: Style,
}

impl Styles {
    fn new(theme: &Theme) -> Self {
        let style = |meaning| Style::from_crossterm(theme.as_style(meaning));
        Self {
            base: style(Meaning::Base),
            muted: style(Meaning::Annotation),
            label: style(Meaning::Annotation).add_modifier(Modifier::BOLD),
            command: style(Meaning::Important).add_modifier(Modifier::BOLD),
            key: style(Meaning::Guidance).add_modifier(Modifier::BOLD),
            failure: style(Meaning::AlertError),
        }
    }
}

fn panel<'a>(title: impl Into<Line<'a>>, styles: Styles) -> Block<'a> {
    Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(styles.muted)
        .title(title)
        .title_style(styles.label)
        .padding(Padding::horizontal(1))
}

fn timestamp(entry: &History, settings: &Settings) -> String {
    entry
        .timestamp
        .to_offset(settings.timezone.0)
        .format(format_description!("[year]-[month]-[day] [hour]:[minute]:[second]"))
        .unwrap_or_default()
}

fn clock(entry: &History, settings: &Settings) -> String {
    entry
        .timestamp
        .to_offset(settings.timezone.0)
        .format(format_description!("[hour]:[minute]:[second]"))
        .unwrap_or_default()
}

fn origin(entry: &History) -> String {
    format!(
        "{}@{}",
        entry.cmd_origin.user().into_inner().escape_non_printable(),
        entry.cmd_origin.host().into_inner().escape_non_printable()
    )
}

fn exit_label(exit: i64) -> String {
    match exit {
        n if n < 0 => "Unknown".into(),
        n => n.to_string(),
    }
}

fn draw_details(
    f: &mut Frame<'_>,
    area: Rect,
    selected: &History,
    settings: &Settings,
    styles: Styles,
) {
    if area.height == 0 {
        return;
    }

    let lines = vec![
        Line::from(vec![
            Span::styled("When  ", styles.label),
            Span::raw(timestamp(selected, settings)),
            Span::styled("   Exit  ", styles.label),
            Span::styled(
                exit_label(selected.exit),
                if selected.exit > 0 {
                    styles.failure
                } else {
                    styles.base
                },
            ),
            Span::styled("   Took  ", styles.label),
            Span::raw(Duration::saturating_from_nanos_i64(selected.duration).display().to_string()),
        ]),
        Line::from(vec![
            Span::styled("Host  ", styles.label),
            Span::raw(origin(selected)),
            Span::styled("   Shell  ", styles.label),
            Span::raw(selected.shell.as_deref().unwrap_or("—").escape_non_printable().into_owned()),
        ]),
        Line::from(vec![
            Span::styled("Cwd   ", styles.label),
            Span::raw(selected.cwd.escape_non_printable()),
        ]),
    ];

    f.render_widget(
        Paragraph::new(lines)
            .style(styles.base)
            .wrap(Wrap { trim: false })
            .block(panel(" Selected run ", styles)),
        area,
    );
}

/// Keep command context visible without spending a panel's borders and padding on it.
pub fn draw_command(
    f: &mut Frame<'_>,
    area: Rect,
    history: &History,
    settings: &Settings,
    theme: &Theme,
) -> Rect {
    let text = command_text(history, settings.ui.syntax_highlight, theme);
    let width = usize::from(area.width.saturating_sub(5).max(1));
    let rows = text.lines.iter().map(|line| line.width().max(1).div_ceil(width)).sum::<usize>();
    let height = if area.height >= 12 && rows > 1 {
        2
    } else {
        1
    };
    let areas = Layout::vertical([Constraint::Length(height), Constraint::Min(0)]).split(area);
    let columns = Layout::horizontal([Constraint::Length(5), Constraint::Min(0)]).split(areas[0]);

    f.render_widget(Paragraph::new("cmd: ").style(Styles::new(theme).muted), columns[0]);
    f.render_widget(Paragraph::new(text).wrap(Wrap { trim: false }), columns[1]);
    areas[1]
}

fn command_text(history: &History, highlight: bool, theme: &Theme) -> Text<'static> {
    // Classify the same escaped text we render, so byte offsets still agree for Unicode
    // and control-character replacements. Parse the whole command to retain multiline context.
    let text = history
        .command
        .lines()
        .map(|line| line.escape_non_printable().into_owned())
        .collect::<Vec<_>>()
        .join("\n");
    let meanings = if highlight {
        syntax::classify(&text, history.shell.as_deref())
    } else {
        Vec::new()
    };

    let fallback = Styles::new(theme).command;
    let mut lines = vec![Line::default()];
    for (index, ch) in text.char_indices() {
        if ch == '\n' {
            lines.push(Line::default());
            continue;
        }

        let style = meanings
            .get(index)
            .map_or(fallback, |meaning| Style::from_crossterm(theme.as_style(*meaning)));
        let line = lines.last_mut().expect("at least one line");
        if let Some(span) = line.spans.last_mut()
            && span.style == style
        {
            span.content.to_mut().push(ch);
        } else {
            line.spans.push(Span::styled(ch.to_string(), style));
        }
    }

    Text::from(lines)
}

pub fn input_guide(view: View, width: u16, theme: &Theme, bindings: &Bindings) -> Line<'static> {
    guide(view, width, Styles::new(theme), bindings)
}

fn guide(view: View, width: u16, styles: Styles, bindings: &Bindings) -> Line<'static> {
    use Action::{
        Delete, Exit, InspectNext, InspectOutput, InspectPrevious, InspectRuns, InspectSession,
        InspectStats, ReturnSelection, ScrollPageDown, ScrollPageUp, ScrollToBottom, ScrollToTop,
    };

    let actions: &[(&[Action], &str)] = match view {
        View::Runs | View::Session => &[
            (&[InspectOutput], "output"),
            (&[InspectPrevious, InspectNext], "select"),
            (&[ReturnSelection], "edit"),
            (&[Exit], "search"),
            (&[InspectRuns, InspectSession, InspectStats], "views"),
            (&[Delete], "delete"),
        ],
        View::Output => &[
            (&[Exit], "back"),
            (&[InspectPrevious, InspectNext], "scroll"),
            (&[ScrollPageUp, ScrollPageDown], "page"),
            (&[ScrollToTop, ScrollToBottom], "jump"),
            (&[ReturnSelection], "edit"),
        ],
        View::Stats => {
            &[(&[InspectRuns], "runs"), (&[InspectSession], "session"), (&[Exit], "search")]
        }
    };

    let mut spans = Vec::new();
    let mut used = 0;
    for (actions, action) in actions {
        let key = bindings.group(actions);
        if key.is_empty() {
            continue;
        }
        let key = format!("<{key}>");
        let label = format!(": {action}");
        let separator = if spans.is_empty() {
            ""
        } else {
            ", "
        };
        let needed = key.width() + label.width() + separator.len();

        // Avoid cutting a key or its action in half on narrow terminals.
        if used + needed > usize::from(width) {
            break;
        }

        spans.push(Span::styled(separator, styles.muted));
        spans.push(Span::styled(key, styles.key));
        spans.push(Span::styled(label, styles.muted));
        used += needed;
    }

    Line::from(spans)
}

pub fn draw_views(
    f: &mut Frame<'_>,
    chunk: Rect,
    view: View,
    theme: &Theme,
    bindings: &Bindings,
) -> Rect {
    if view == View::Output {
        return chunk;
    }
    let areas = Layout::vertical([Constraint::Length(1), Constraint::Min(0)]).split(chunk);
    let index = match view {
        View::Runs => 0,
        View::Session => 1,
        View::Stats => 2,
        View::Output => 3,
    };
    f.render_widget(
        Tabs::new([
            bindings.title("Runs", &Action::InspectRuns),
            bindings.title("Session", &Action::InspectSession),
            bindings.title("Stats", &Action::InspectStats),
        ])
        .select(index)
        .style(Style::from_crossterm(theme.as_style(Meaning::Base)))
        .highlight_style(
            Style::from_crossterm(theme.as_style(Meaning::Important))
                .add_modifier(Modifier::BOLD | Modifier::REVERSED),
        ),
        areas[0],
    );
    areas[1]
}

#[cfg(test)]
mod tests {
    use atuin_client::theme::ThemeManager;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use rstest::{fixture, rstest};
    use time::OffsetDateTime;

    use super::*;
    use crate::command::client::search::keybindings::EvalContext;
    use crate::command::client::search::keybindings::defaults::default_inspector_keymap;

    #[fixture]
    fn bindings() -> Bindings {
        Bindings::new(&default_inspector_keymap(&Settings::utc()), &EvalContext {
            cursor_position: 0,
            input_width: 0,
            input_byte_len: 0,
            selected_index: 0,
            results_len: 1,
            original_input_empty: false,
            has_context: false,
        })
    }

    #[fixture]
    fn history() -> History {
        History::capture()
            .timestamp(OffsetDateTime::UNIX_EPOCH)
            .command("echo hello")
            .cwd("/tmp")
            .build()
            .into()
    }

    #[rstest]
    #[tokio::test]
    async fn views_pivot_on_the_selected_occurrence(mut history: History) {
        let db = Sqlite::in_memory(Duration::from_secs(2)).await.unwrap();
        history.session = "first".into();
        db.save(&history).await.unwrap();
        let mut other_run = history.clone();
        other_run.id = HistoryId::new(atuin_common::utils::uuid_v7());
        other_run.timestamp += time::Duration::seconds(1);
        other_run.session = "second".into();
        db.save(&other_run).await.unwrap();
        let mut neighbor = other_run.clone();
        neighbor.id = HistoryId::new(atuin_common::utils::uuid_v7());
        neighbor.timestamp += time::Duration::seconds(1);
        neighbor.command = "pwd".into();
        db.save(&neighbor).await.unwrap();
        let mut browser = Browser::default();
        let settings = Settings::utc();
        assert_eq!(
            browser.refresh(&db, &history, &settings).await.unwrap(),
            (Some(other_run.id), None)
        );
        assert_eq!(
            browser.refresh(&db, &other_run, &settings).await.unwrap(),
            (None, Some(history.id))
        );
        browser.select_view(View::Session);
        assert_eq!(
            browser.refresh(&db, &other_run, &settings).await.unwrap(),
            (Some(neighbor.id), None)
        );
        assert_eq!(browser.entries[0].id, neighbor.id);
        assert_eq!(browser.table.selected(), Some(1));
        browser.select_view(View::Runs);
        assert_eq!(browser.refresh(&db, &neighbor, &settings).await.unwrap(), (None, None));
        assert_eq!(browser.entries[0].id, neighbor.id);
    }

    #[rstest]
    #[case(View::Runs, "Directory")]
    #[case(View::Session, "Command")]
    fn list_shows_occurrence_metadata(
        mut history: History,
        #[case] view: View,
        #[case] heading: &str,
    ) {
        history.cwd = "/tmp/example".into();
        history.exit = 42;
        history.duration = 2_000_000_000;
        let mut browser = Browser {
            view,
            entries: vec![history.clone()],
            ..Browser::default()
        };
        browser.table.select(Some(0));
        let mut terminal = Terminal::new(TestBackend::new(80, 12)).unwrap();
        let mut themes = ThemeManager::new(Some(true), Some(String::new()));
        let theme = themes.load_theme("(none)", None);
        terminal
            .draw(|f| browser.draw(f, f.area(), &history, &Settings::utc(), theme, &bindings()))
            .unwrap();
        let rendered: String = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(ratatui::buffer::Cell::symbol)
            .collect();
        for text in [heading, "1970-01-01", "42", "2s", "/tmp/example"] {
            assert!(rendered.contains(text), "missing {text} in {rendered}");
        }
    }

    #[rstest]
    #[case("echo hello", 80, 24, 1)]
    #[case("echo first\necho second", 80, 24, 2)]
    #[case("a long command that wraps across lines", 12, 24, 2)]
    #[case("echo first\necho second", 80, 5, 1)]
    fn command_header_is_compact(
        mut history: History,
        #[case] command: &str,
        #[case] width: u16,
        #[case] height: u16,
        #[case] rows: u16,
    ) {
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        let mut themes = ThemeManager::new(Some(true), Some(String::new()));
        let theme = themes.load_theme("(none)", None);
        terminal
            .draw(|f| {
                history.command = command.into();
                let area = draw_views(f, f.area(), View::Runs, theme, &bindings());
                let rest = draw_command(f, area, &history, &Settings::utc(), theme);
                assert_eq!(rest.y, rows + 1);
                assert_eq!(rest.height, height - rows - 1);
            })
            .unwrap();
        let rendered: String = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(ratatui::buffer::Cell::symbol)
            .collect();
        assert!(!rendered.contains('╭'));
        assert!(rendered.contains("[r] Runs"));
        assert_eq!(terminal.backend().buffer()[(0, 1)].symbol(), "c");
        assert!(rendered.contains("cmd: "));
    }

    #[rstest]
    #[case(true)]
    #[case(false)]
    fn command_text_is_safe_and_respects_highlighting(
        mut history: History,
        #[case] highlight: bool,
    ) {
        history.command = "echo héllo\0\ncat '\x1b[31m世界'".into();
        let mut themes = ThemeManager::new(Some(true), Some(String::new()));
        let theme = themes.load_theme("(none)", None);
        let text = command_text(&history, highlight, theme);
        assert_eq!(text.to_string(), "echo héllo^@\ncat '^[[31m世界'");
        if highlight {
            let meaning = syntax::classify("echo", None)[0];
            assert_eq!(
                text.lines[0].spans[0].style,
                Style::from_crossterm(theme.as_style(meaning))
            );
        } else {
            assert!(
                text.lines
                    .iter()
                    .flat_map(|line| &line.spans)
                    .all(|span| span.style == Styles::new(theme).command)
            );
        }
    }

    #[rstest]
    fn output_back_preserves_list_position(history: History) {
        let mut browser = Browser {
            view: View::Session,
            entries: vec![history],
            ..Browser::default()
        };
        browser.table.select(Some(0));
        *browser.table.offset_mut() = 7;
        browser.select_view(View::Output);
        browser.back_from_output();
        assert_eq!(browser.view, View::Session);
        assert_eq!(browser.table.selected(), Some(0));
        assert_eq!(browser.table.offset(), 7);
    }

    #[rstest]
    fn output_pages_and_edges() {
        let mut browser = Browser {
            output_page_size: 20,
            output_max_scroll: 100,
            ..Browser::default()
        };
        browser.scroll_output_page(true, false);
        assert_eq!(browser.output_scroll, 19);
        browser.scroll_output_page(false, false);
        assert_eq!(browser.output_scroll, 0);
        browser.scroll_output_page(true, true);
        assert_eq!(browser.output_scroll, 10);
        browser.scroll_output_edge(true);
        assert_eq!(browser.output_scroll, 100);
        browser.scroll_output_page(true, false);
        assert_eq!(browser.output_scroll, 100);
        browser.scroll_output_edge(false);
        assert_eq!(browser.output_scroll, 0);
    }

    #[rstest]
    #[case(20, 5)]
    #[case(80, 24)]
    #[case(120, 40)]
    fn output_wraps_and_scrolls_to_the_end(
        history: History,
        #[case] width: u16,
        #[case] height: u16,
    ) {
        use ratatui::style::Color;
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        let mut themes = ThemeManager::new(Some(true), Some(String::new()));
        let theme = themes.load_theme("(none)", None);
        let mut browser = Browser {
            view: View::Output,
            output: Capture::from_text(
                &format!(
                    "\x1b[31;44;1m{}\x1b[0m\n\x1b[2J\x1b[H\x1b]52;c;AA==\x07THE END",
                    "x".repeat(5000)
                ),
                "Output · capture truncated",
            ),
            ..Browser::default()
        };
        let settings = Settings::utc();
        terminal
            .draw(|f| browser.draw(f, f.area(), &history, &settings, theme, &bindings()))
            .unwrap();
        assert!(browser.output_max_scroll > 0);
        let first_output = if height >= 10 {
            (2, 3)
        } else {
            (0, 1)
        };
        let cell = &terminal.backend().buffer()[first_output];
        assert_eq!(cell.symbol(), "x");
        assert_eq!(cell.fg, Color::Red);
        assert_eq!(cell.bg, Color::Blue);
        assert!(cell.modifier.contains(Modifier::BOLD));
        for cell in
            [&terminal.backend().buffer()[(0, 0)], &terminal.backend().buffer()[(0, height - 1)]]
        {
            assert_ne!(cell.bg, Color::Blue);
            assert_ne!(cell.fg, Color::Red);
        }
        if height >= 10 {
            assert!(
                browser.output_page_size >= usize::from(height - 5),
                "output should get almost the whole viewport"
            );
        }
        for _ in 0..6000 {
            browser.scroll_output(true);
        }
        terminal
            .draw(|f| browser.draw(f, f.area(), &history, &settings, theme, &bindings()))
            .unwrap();
        let rendered: String = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(ratatui::buffer::Cell::symbol)
            .collect();
        assert!(rendered.contains("THE END"));
        assert!(!rendered.contains('\x1b'));
        let end = terminal.backend().buffer().content().iter().find(|c| c.symbol() == "T").unwrap();
        assert_eq!(end.fg, Color::Reset);
        assert_eq!(end.bg, Color::Reset);
        assert!(end.modifier.is_empty());
        assert!(
            terminal
                .backend()
                .buffer()
                .content()
                .iter()
                .any(|c| c.symbol() == "x" && c.fg == ratatui::style::Color::Red)
        );
        assert_eq!(browser.output_scroll, browser.output_max_scroll);
        assert_eq!(
            browser.list_position(),
            (browser.output_max_scroll, browser.output_max_scroll + 1)
        );
        for _ in 0..6000 {
            browser.scroll_output(false);
        }
        assert_eq!(browser.output_scroll, 0);
        browser.back_from_output();
        terminal
            .draw(|f| browser.draw(f, f.area(), &history, &Settings::utc(), theme, &bindings()))
            .unwrap();
        assert!(terminal.backend().buffer().content().iter().all(|c| c.bg != Color::Blue));
    }
}
