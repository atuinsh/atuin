//! The search frame: `interactive.rs`'s `draw_inner` ported as a single
//! eye-declare element.
//!
//! This is a deliberately faithful transplant, not a redesign — it reuses the
//! ratatui `Layout` solver and widgets (`HistoryList`, `Block`, `Tabs`,
//! `Paragraph`) directly, which works because ratatui re-exports the same
//! `ratatui_core` buffer type eye-declare renders into. Decomposing into
//! finer-grained elements can happen once behavior parity is locked in.
//!
//! The frame reports a fixed height (`inline_height`): a tail taller than the
//! terminal would stream rows into scrollback irreversibly, so the layout
//! always fits itself into the fixed region instead of sizing to content.
//!
//! Not yet ported (later phases): the inspector tab, history count in the
//! header, the update-needed notice, prefix/search-mode indicator states.

use atuin_client::{
    history::History,
    settings::{KeymapMode, PreviewStrategy, RequestedSearchMode, Settings},
    theme::{Meaning, Theme},
};
use atuin_common::string::EscapeNonPrintablePosixExt as _;
use eye_declare::Element;
use ratatui::{
    backend::FromCrossterm,
    buffer::Buffer,
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Modifier, Style},
    text::{Line, Span, Text},
    widgets::{Block, BorderType, Borders, Paragraph, StatefulWidget, Tabs, Widget},
};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use super::super::history_list::{HistoryHighlighter, HistoryList};
use super::app::SearchApp;
use crate::VERSION;

const TAB_TITLES: [&str; 2] = ["Search", "Inspect"];

#[derive(Clone, Copy, Eq, PartialEq)]
pub(super) enum Compactness {
    Ultracompact,
    Compact,
    Full,
}

/// `to_compactness` from the ratatui path, with the viewport height passed
/// directly instead of read off a `Frame`.
pub(super) fn compactness_for(height: u16, settings: &Settings) -> Compactness {
    if match settings.style {
        atuin_client::settings::Style::Auto => height < 14,
        atuin_client::settings::Style::Compact => true,
        atuin_client::settings::Style::Full => false,
    } {
        if settings.auto_hide_height != 0 && height <= settings.auto_hide_height {
            Compactness::Ultracompact
        } else {
            Compactness::Compact
        }
    } else {
        Compactness::Full
    }
}

pub(super) struct SearchFrame<'f, 'a> {
    pub app: &'f SearchApp<'a>,
}

struct Chunks {
    input: Rect,
    results_list: Rect,
    preview: Rect,
    tabs: Rect,
    header: Rect,
    warning: Rect,
    compactness: Compactness,
    preview_width: u16,
}

impl SearchFrame<'_, '_> {
    #[allow(clippy::bool_to_int_with_if)]
    fn chunks(&self, area: Rect) -> Chunks {
        let settings = self.app.settings;
        let compactness = compactness_for(area.height, settings);
        let invert = settings.invert;
        let border_size = match compactness {
            Compactness::Full => 1,
            _ => 0,
        };
        let preview_width = area.width.saturating_sub(2);
        let preview_height = calc_preview_height(
            settings,
            &self.app.results,
            self.app.results_state.borrow().selected(),
            compactness,
            border_size,
            preview_width,
        );

        let show_help = settings.show_help && (compactness == Compactness::Full || area.height > 1);
        let warning_height =
            u16::try_from(build_warnings(settings, self.app.theme).height()).unwrap_or(u16::MAX);
        let show_tabs = settings.show_tabs && !matches!(compactness, Compactness::Ultracompact);

        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .margin(0)
            .horizontal_margin(1)
            .constraints::<&[Constraint]>(
                if invert {
                    [
                        Constraint::Length(1 + border_size),               // input
                        Constraint::Min(1),                                // results list
                        Constraint::Length(preview_height),                // preview
                        Constraint::Length(if show_tabs { 1 } else { 0 }), // tabs
                        Constraint::Length(if show_help { 1 } else { 0 }), // header (sic)
                        Constraint::Length(warning_height),                // skim warning
                    ]
                } else {
                    match compactness {
                        Compactness::Ultracompact => [
                            Constraint::Length(if show_help { 1 } else { 0 }), // header
                            Constraint::Length(0),                             // tabs
                            Constraint::Min(1),                                // results list
                            Constraint::Length(0),                             // no input
                            Constraint::Length(0),                             // no preview
                            Constraint::Length(warning_height),                // skim warning
                        ],
                        _ => [
                            Constraint::Length(if show_help { 1 } else { 0 }), // header
                            Constraint::Length(if show_tabs { 1 } else { 0 }), // tabs
                            Constraint::Min(1),                                // results list
                            Constraint::Length(1 + border_size),               // input
                            Constraint::Length(preview_height),                // preview
                            Constraint::Length(warning_height),                // skim warning
                        ],
                    }
                }
                .as_ref(),
            )
            .split(area);

        Chunks {
            input: if invert { chunks[0] } else { chunks[3] },
            results_list: if invert { chunks[1] } else { chunks[2] },
            preview: if invert { chunks[2] } else { chunks[4] },
            tabs: if invert { chunks[3] } else { chunks[1] },
            header: if invert { chunks[4] } else { chunks[0] },
            // Always last, so it is the bottom row whichever way the layout
            // is stacked.
            warning: chunks[5],
            compactness,
            preview_width,
        }
    }

    /// Width of the `[ MODE ] ` block before the input text, kept in sync
    /// with the list's fixed columns so the input aligns with the commands.
    fn prefix_width(&self) -> u16 {
        #[allow(clippy::cast_possible_truncation)]
        let prefix_width = self
            .app
            .settings
            .ui
            .columns
            .iter()
            .take_while(|col| !col.expand)
            .map(|col| col.width + 1)
            .sum::<u16>()
            + " > ".len() as u16;
        #[allow(clippy::cast_possible_truncation)]
        let min_prefix_width = "[ SRCH: FULLTXT ] ".len() as u16;
        std::cmp::max(prefix_width, min_prefix_width)
    }
}

impl Element for SearchFrame<'_, '_> {
    fn height(&self, _width: u16) -> u16 {
        self.app.inline_height
    }

    #[allow(clippy::too_many_lines)]
    fn render(&self, area: Rect, buf: &mut Buffer) {
        if area.width < 4 || area.height < 1 {
            return;
        }
        let app = self.app;
        let settings = app.settings;
        let theme = app.theme;
        let chunks = self.chunks(area);
        let compactness = chunks.compactness;
        let invert = settings.invert;

        if chunks.tabs.height > 0 {
            let titles: Vec<_> = TAB_TITLES.iter().copied().map(Line::from).collect();
            let tabs = Tabs::new(titles)
                .block(Block::default().borders(Borders::NONE))
                .select(0)
                .style(Style::default())
                .highlight_style(Style::from_crossterm(theme.as_style(Meaning::Important)));
            tabs.render(chunks.tabs, buf);
        }

        if chunks.header.height > 0 {
            let header_chunks = Layout::default()
                .direction(Direction::Horizontal)
                .constraints::<&[Constraint]>(
                    [
                        Constraint::Ratio(1, 5),
                        Constraint::Ratio(3, 5),
                        Constraint::Ratio(1, 5),
                    ]
                    .as_ref(),
                )
                .split(chunks.header);

            build_title(theme).render(header_chunks[0], buf);
            build_help(settings, theme).render(header_chunks[1], buf);
        }

        if chunks.warning.height > 0 {
            Paragraph::new(build_warnings(settings, theme)).render(chunks.warning, buf);
        }

        let indicator: String = match compactness {
            Compactness::Ultracompact => {
                if app.switched_search_mode {
                    format!("S{}>", app.search_mode.as_str().chars().next().unwrap())
                } else if app.search.custom_context.is_some() {
                    format!(
                        "C{}>",
                        app.search.filter_mode.as_str().chars().next().unwrap()
                    )
                } else {
                    format!(
                        "{}> ",
                        app.search.filter_mode.as_str().chars().next().unwrap()
                    )
                }
            }
            _ => " > ".to_string(),
        };

        let history_highlighter = HistoryHighlighter {
            engine: &app.highlight_engine,
            search_input: app.search.input.as_str(),
        };
        let results_list = HistoryList::new(
            &app.results,
            invert,
            app.keymap_mode == KeymapMode::VimNormal,
            &*app.now,
            settings.timezone.0,
            indicator.as_str(),
            theme,
            history_highlighter,
            settings.show_numeric_shortcuts,
            settings.ui.syntax_highlight,
            &settings.ui.columns,
        );
        let results_list = match compactness {
            Compactness::Full => {
                if invert {
                    results_list.block(
                        Block::default()
                            .borders(Borders::LEFT | Borders::RIGHT)
                            .border_type(BorderType::Rounded)
                            .title(format!(
                                "{:─>width$}",
                                "",
                                width = (chunks.input.width as usize).saturating_sub(2)
                            )),
                    )
                } else {
                    results_list.block(
                        Block::default()
                            .borders(Borders::TOP | Borders::LEFT | Borders::RIGHT)
                            .border_type(BorderType::Rounded),
                    )
                }
            }
            _ => results_list,
        };
        StatefulWidget::render(
            results_list,
            chunks.results_list,
            buf,
            &mut app.results_state.borrow_mut(),
        );

        if !matches!(compactness, Compactness::Ultracompact) {
            build_input(
                app,
                compactness,
                invert,
                chunks.input.width,
                self.prefix_width(),
            )
            .render(chunks.input, buf);

            let preview_width = match compactness {
                Compactness::Full => chunks.preview_width - 2,
                _ => chunks.preview_width,
            };
            build_preview(
                app,
                compactness,
                preview_width,
                chunks.preview.width.into(),
                theme,
            )
            .render(chunks.preview, buf);
        }
    }

    fn cursor(&self, area: Rect) -> Option<(u16, u16)> {
        let chunks = self.chunks(area);
        if matches!(chunks.compactness, Compactness::Ultracompact) {
            return None;
        }
        #[allow(clippy::cast_possible_truncation)]
        let extra_width = UnicodeWidthStr::width(self.app.search.input.substring()) as u16;
        let cursor_offset = match chunks.compactness {
            Compactness::Full => 1,
            _ => 0,
        };
        Some((
            chunks.input.x + extra_width + self.prefix_width() + cursor_offset,
            chunks.input.y + cursor_offset,
        ))
    }
}

/// `State::calc_preview_height`, minus the inspector tab (always tab 0 here).
#[allow(clippy::cast_possible_truncation, clippy::bool_to_int_with_if)]
fn calc_preview_height(
    settings: &Settings,
    results: &[History],
    selected: usize,
    compactness: Compactness,
    border_size: u16,
    preview_width: u16,
) -> u16 {
    if settings.show_preview
        && settings.preview.strategy == PreviewStrategy::Auto
        && !results.is_empty()
    {
        let length_current_cmd = results[selected].command.width() as u16;
        // calculate the number of newlines in the command
        let num_newlines = results[selected]
            .command
            .chars()
            .filter(|&c| c == '\n')
            .count() as u16;
        if num_newlines > 0 {
            std::cmp::min(
                settings.max_preview_height,
                results[selected]
                    .command
                    .split('\n')
                    .map(|line| {
                        (line.len() as u16 + preview_width - 1 - border_size)
                            / (preview_width - border_size)
                    })
                    .sum(),
            ) + border_size * 2
        }
        // The '- 19' takes the characters before the command (duration and time) into account
        else if length_current_cmd > preview_width - 19 {
            std::cmp::min(
                settings.max_preview_height,
                (length_current_cmd + preview_width - 1 - border_size)
                    / (preview_width - border_size),
            ) + border_size * 2
        } else {
            1
        }
    } else if settings.show_preview && settings.preview.strategy == PreviewStrategy::Static {
        let longest_command = results
            .iter()
            .max_by(|h1, h2| h1.command.len().cmp(&h2.command.len()));
        longest_command.map_or(0, |v| {
            std::cmp::min(
                settings.max_preview_height,
                v.command
                    .split('\n')
                    .map(|line| {
                        (line.len() as u16 + preview_width - 1 - border_size)
                            / (preview_width - border_size)
                    })
                    .sum(),
            )
        }) + border_size * 2
    } else if settings.show_preview && settings.preview.strategy == PreviewStrategy::Fixed {
        settings.max_preview_height + border_size * 2
    } else if !matches!(compactness, Compactness::Full) {
        0
    } else {
        1
    }
}

fn build_title(theme: &Theme) -> Paragraph<'static> {
    let style: Style = Style::from_crossterm(theme.as_style(Meaning::Base));
    Paragraph::new(Text::from(Span::styled(
        format!("Atuin v{VERSION}"),
        style.add_modifier(Modifier::BOLD),
    )))
    .alignment(Alignment::Left)
}

fn build_help(settings: &Settings, theme: &Theme) -> Paragraph<'static> {
    Paragraph::new(Text::from(Line::from(vec![
        Span::styled("<esc>", Style::default().add_modifier(Modifier::BOLD)),
        Span::raw(": exit"),
        Span::raw(", "),
        Span::styled("<tab>", Style::default().add_modifier(Modifier::BOLD)),
        Span::raw(": edit"),
        Span::raw(", "),
        Span::styled("<enter>", Style::default().add_modifier(Modifier::BOLD)),
        Span::raw(if settings.enter_accept {
            ": run"
        } else {
            ": edit"
        }),
        Span::raw(", "),
        Span::styled("<ctrl-o>", Style::default().add_modifier(Modifier::BOLD)),
        Span::raw(": inspect"),
    ])))
    .style(Style::from_crossterm(theme.as_style(Meaning::Annotation)))
    .alignment(Alignment::Center)
}

fn build_warnings(settings: &Settings, theme: &Theme) -> Text<'static> {
    if settings.requested_search_mode != RequestedSearchMode::Skim {
        return Text::default();
    }

    let style =
        Style::from_crossterm(theme.as_style(Meaning::AlertWarn)).add_modifier(Modifier::BOLD);
    let code_style =
        Style::from_crossterm(theme.as_style(Meaning::SyntaxCommand)).add_modifier(Modifier::BOLD);

    Text::from(vec![
        Span::styled(
            "Warning: \"skim\" mode was removed; falling back to \"fuzzy\"",
            style,
        )
        .into(),
        vec![
            Span::styled("Set ", style),
            Span::styled("search_mode = \"daemon-fuzzy\"", code_style),
            Span::styled(" for a similar experience", style),
        ]
        .into(),
    ])
    .left_aligned()
}

fn build_input<'a>(
    app: &'a SearchApp<'_>,
    compactness: Compactness,
    invert: bool,
    inner_width: u16,
    prefix_width: u16,
) -> Paragraph<'a> {
    let (pref, mode) = if app.prefix {
        ("", "PREFIX")
    } else if app.switched_search_mode {
        (" SRCH:", app.search_mode.as_str())
    } else if app.search.custom_context.is_some() {
        (" CTX:", app.search.filter_mode.as_str())
    } else {
        ("", app.search.filter_mode.as_str())
    };
    // 3: surrounding "[" "] "
    let mode_width = usize::from(prefix_width) - pref.len() - 3;
    // sanity check to ensure we don't exceed the layout limits
    debug_assert!(mode_width >= mode.len(), "mode name '{mode}' is too long!");
    let input = format!("[{pref}{mode:^mode_width$}] {}", app.search.input.as_str());
    let input = Paragraph::new(input);
    match compactness {
        Compactness::Full => {
            if invert {
                input.block(
                    Block::default()
                        .borders(Borders::LEFT | Borders::RIGHT | Borders::TOP)
                        .border_type(BorderType::Rounded),
                )
            } else {
                input.block(
                    Block::default()
                        .borders(Borders::LEFT | Borders::RIGHT)
                        .border_type(BorderType::Rounded)
                        .title(format!(
                            "{:─>width$}",
                            "",
                            width = (inner_width as usize).saturating_sub(2)
                        )),
                )
            }
        }
        _ => input,
    }
}

fn build_preview(
    app: &SearchApp<'_>,
    compactness: Compactness,
    preview_width: u16,
    chunk_width: usize,
    theme: &Theme,
) -> Paragraph<'static> {
    let selected = app.results_state.borrow().selected();
    let command = if app.results.is_empty() {
        String::new()
    } else {
        let s = &app.results[selected].command;
        let mut lines = Vec::new();
        for line in s.split('\n') {
            let line = line.escape_non_printable();
            let mut width = 0;
            let mut start = 0;
            for (idx, ch) in line.char_indices() {
                let w = ch.width().unwrap_or(0); // None for control chars which should not happen
                if width + w > preview_width.into() {
                    lines.push(line[start..idx].to_owned());
                    start = idx;
                    width = w;
                } else {
                    width += w;
                }
            }
            if width != 0 {
                lines.push(line[start..].to_owned());
            }
        }
        lines.join("\n")
    };

    match compactness {
        Compactness::Full => Paragraph::new(command).block(
            Block::default()
                .borders(Borders::BOTTOM | Borders::LEFT | Borders::RIGHT)
                .border_type(BorderType::Rounded)
                .title(format!(
                    "{:─>width$}",
                    "",
                    width = chunk_width.saturating_sub(2)
                )),
        ),
        _ => Paragraph::new(command)
            .style(Style::from_crossterm(theme.as_style(Meaning::Annotation))),
    }
}
