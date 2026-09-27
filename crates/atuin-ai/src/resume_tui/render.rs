//! Drawing the picker. The layout, header, tabs, input box and borders follow the history search
//! (`atuin search -i`) exactly, so the two feel like one tool.

use std::ops::Range;
use std::path::Path;

use atuin_client::ai_session::HarnessKind;
use atuin_client::settings::{
    AiSessionColumn, KeymapMode, PreviewStrategy, Settings, Style as UiStyle,
};
use atuin_client::theme::{Meaning, Theme};
use atuin_common::string::ellipsis::{Indicator, Pos};
use atuin_common::string::{Alignment as Align, EllipsizeExt as _, Measure};
use atuin_common::time::{DurationExt as _, OffsetDateTimeExt as _};
use ratatui::Frame;
use ratatui::backend::FromCrossterm;
use ratatui::buffer::Buffer;
use ratatui::layout::{Alignment, Constraint, Direction, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{
    Block, BorderType, Borders, Clear, Paragraph, StatefulWidget, Tabs, Widget,
};
use time::{OffsetDateTime, UtcOffset};
use unicode_width::UnicodeWidthStr;

use super::query::{TokenKind, TokenState};
use super::resumer::Resumer;
use super::source::{Relation, SessionRow, Snippet, harness_badge, harness_label};
use super::state::{ListState, State, TAB_TITLES};

const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Sessions updated this recently get a live dot.
const LIVE_SECS: u64 = 120;

#[derive(Clone, Copy, Eq, PartialEq, Debug)]
pub enum Compactness {
    Ultracompact,
    Compact,
    Full,
}

pub fn to_compactness(area: Rect, settings: &Settings) -> Compactness {
    if match settings.style {
        UiStyle::Auto => area.height < 14,
        UiStyle::Compact => true,
        UiStyle::Full => false,
    } {
        if settings.auto_hide_height != 0 && area.height <= settings.auto_hide_height {
            Compactness::Ultracompact
        } else {
            Compactness::Compact
        }
    } else {
        Compactness::Full
    }
}

fn style(theme: &Theme, meaning: Meaning) -> Style {
    Style::from_crossterm(theme.as_style(meaning))
}

/// Collapse whitespace and control characters to single spaces, remembering each output byte's
/// source byte so highlight ranges still apply.
fn flatten(text: &str) -> (String, Vec<usize>) {
    let mut out = String::with_capacity(text.len());
    let mut map = Vec::with_capacity(text.len());
    let mut space = false;
    for (i, c) in text.trim().char_indices() {
        let offset = i + (text.len() - text.trim_start().len());
        if c.is_whitespace() || c.is_control() {
            if !space {
                out.push(' ');
                map.push(offset);
            }
            space = true;
        } else {
            let mut buf = [0u8; 4];
            let s = c.encode_utf8(&mut buf);
            out.push_str(s);
            map.extend(std::iter::repeat_n(offset, s.len()));
            space = false;
        }
    }
    (out, map)
}

/// One line of `text` fitted to `width` columns, with the `highlights` (byte ranges into `text`)
/// drawn in `hl`.
fn highlighted_line(
    text: &str,
    highlights: &[Range<usize>],
    width: usize,
    base: Style,
    hl: Style,
) -> Vec<Span<'static>> {
    let (flat, map) = flatten(text);
    let ellipsized = flat.ellipsize(Measure::Columns(width), Pos::End, Indicator::UNICODE);
    let display = ellipsized.to_string();

    let mut spans: Vec<Span<'static>> = Vec::new();
    let mut run = String::new();
    let mut run_hl = false;
    for (i, ch) in display.char_indices() {
        let is_hl = ellipsized
            .source_index(i)
            .and_then(|b| map.get(b))
            .is_some_and(|src| highlights.iter().any(|r| r.contains(src)));
        if is_hl != run_hl && !run.is_empty() {
            spans.push(Span::styled(
                std::mem::take(&mut run),
                if run_hl {
                    hl
                } else {
                    base
                },
            ));
        }
        run_hl = is_hl;
        run.push(ch);
    }
    if !run.is_empty() {
        spans.push(Span::styled(
            run,
            if run_hl {
                hl
            } else {
                base
            },
        ));
    }
    spans
}

fn spans_width(spans: &[Span<'_>]) -> usize {
    spans.iter().map(|s| s.content.width()).sum()
}

/// The repository (or directory) name for the repo column.
fn repo_name(row: &SessionRow) -> String {
    row.git_root
        .as_deref()
        .or(row.cwd.as_deref())
        .and_then(Path::file_name)
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default()
}

fn is_live(now: OffsetDateTime, row: &SessionRow) -> bool {
    now.saturating_duration_since(row.updated_at).as_secs() < LIVE_SECS
}

fn ago(now: OffsetDateTime, ts: OffsetDateTime) -> String {
    now.saturating_duration_since(ts).display().largest_unit().to_string()
}

fn harness_style(theme: &Theme, harness: HarnessKind) -> Style {
    let meaning = match harness {
        HarnessKind::ClaudeCode => Meaning::AlertWarn,
        HarnessKind::Codex => Meaning::AlertInfo,
        HarnessKind::Opencode => Meaning::Guidance,
        HarnessKind::Pi => Meaning::Important,
        HarnessKind::Copilot | HarnessKind::Unknown => Meaning::Annotation,
    };
    style(theme, meaning).add_modifier(Modifier::BOLD)
}

// --- the session list ------------------------------------------------------------------------

pub struct SessionList<'a> {
    rows: &'a [SessionRow],
    block: Option<Block<'a>>,
    inverted: bool,
    alternate_highlight: bool,
    now: OffsetDateTime,
    indicator: &'a str,
    theme: &'a Theme,
    columns: &'a [AiSessionColumn],
    host_id: &'a str,
}

impl SessionList<'_> {
    fn get_items_bounds(&self, selected: usize, offset: usize, height: usize) -> (usize, usize) {
        let offset = offset.min(self.rows.len().saturating_sub(1));
        let max_scroll_space = height.min(10).min(self.rows.len() - selected);
        if offset + height < selected + max_scroll_space {
            let end = selected + max_scroll_space;
            (end - height, end)
        } else if selected < offset {
            (selected, selected + height)
        } else {
            (offset, offset + height)
        }
    }
}

impl StatefulWidget for SessionList<'_> {
    type State = ListState;

    fn render(mut self, area: Rect, buf: &mut Buffer, state: &mut ListState) {
        let list_area = self.block.take().map_or(area, |b| {
            let inner = b.inner(area);
            b.render(area, buf);
            inner
        });
        if list_area.width < 1 || list_area.height < 1 || self.rows.is_empty() {
            state.max_entries = usize::from(list_area.height);
            return;
        }
        state.selected = state.selected.min(self.rows.len() - 1);
        let height = usize::from(list_area.height);
        let (start, end) = self.get_items_bounds(state.selected, state.offset, height);
        state.offset = start;
        state.max_entries = end - start;

        for (y, row) in self.rows.iter().enumerate().skip(start).take(end - start) {
            let screen_y = u16::try_from(y - start).unwrap_or(u16::MAX);
            let cy = if self.inverted {
                list_area.top() + screen_y
            } else {
                list_area.bottom() - screen_y - 1
            };
            let selected = y == state.selected;
            let mut line = RowWriter {
                buf,
                x: list_area.left(),
                right: list_area.right(),
                y: cy,
                row_modifier: {
                    let mut m = Modifier::empty();
                    if row.host_id != self.host_id {
                        m |= Modifier::DIM;
                    }
                    if self.alternate_highlight && selected {
                        m |= Modifier::REVERSED;
                    }
                    m
                },
            };
            self.render_row(&mut line, row, selected, list_area.width);
        }
    }
}

struct RowWriter<'b> {
    buf: &'b mut Buffer,
    x: u16,
    right: u16,
    y: u16,
    row_modifier: Modifier,
}

impl RowWriter<'_> {
    fn put(&mut self, s: &str, style: Style) {
        if self.x >= self.right {
            return;
        }
        let w = usize::from(self.right - self.x);
        let (x, _) =
            self.buf.set_stringn(self.x, self.y, s, w, style.add_modifier(self.row_modifier));
        self.x = x;
    }

    fn put_spans(&mut self, spans: &[Span<'_>]) {
        for span in spans {
            self.put(&span.content, span.style);
        }
    }

    /// Pad with spaces up to column `to`, so the next column lines up.
    fn pad_to(&mut self, to: u16) {
        while self.x < to.min(self.right) {
            self.put(" ", Style::default());
        }
    }
}

impl SessionList<'_> {
    fn render_row(&self, w: &mut RowWriter<'_>, row: &SessionRow, selected: bool, width: u16) {
        let theme = self.theme;
        w.put(
            if selected {
                self.indicator
            } else {
                "   "
            },
            Style::default(),
        );

        let fixed: u16 = self.columns.iter().filter(|c| !c.expands()).map(|c| c.width() + 1).sum();
        let expand = width.saturating_sub(3 + fixed);

        for (idx, column) in self.columns.iter().enumerate() {
            if idx != 0 {
                w.put(" ", Style::default());
            }
            let col_width = if column.expands() {
                expand
            } else {
                column.width()
            };
            let end = w.x.saturating_add(col_width);
            let cw = usize::from(col_width);
            match column {
                AiSessionColumn::Time => {
                    let (text, meaning) = if is_live(self.now, row) {
                        (format!("● {}", ago(self.now, row.updated_at)), Meaning::AlertInfo)
                    } else {
                        (format!("{} ago", ago(self.now, row.updated_at)), Meaning::Guidance)
                    };
                    let text = text.pad_ellipsize(
                        Measure::Columns(cw),
                        Pos::End,
                        Indicator::UNICODE,
                        Align::End,
                    );
                    w.put(&text, style(theme, meaning));
                }
                AiSessionColumn::Harness => {
                    w.put(
                        harness_badge(row.handle.harness),
                        harness_style(theme, row.handle.harness),
                    );
                }
                AiSessionColumn::Children => {
                    let text = if row.children > 0 {
                        format!("+{}", row.children)
                    } else {
                        String::new()
                    };
                    w.put(&format!("{text:>cw$}"), style(theme, Meaning::Annotation));
                }
                AiSessionColumn::Title => {
                    let (base, hl) = if selected && !self.alternate_highlight {
                        let base = style(theme, Meaning::AlertError).add_modifier(Modifier::BOLD);
                        (base, style(theme, Meaning::AlertWarn).add_modifier(Modifier::BOLD))
                    } else {
                        let base = style(theme, Meaning::Base);
                        (base, base.add_modifier(Modifier::BOLD))
                    };
                    let host = (row.host_id != self.host_id).then(|| format!(" @{}", row.hostname));
                    let host_w = host.as_deref().map_or(0, UnicodeWidthStr::width);
                    let title_w = cw.saturating_sub(host_w).max(cw.min(8));
                    let spans =
                        highlighted_line(&row.title.text, &row.title.highlights, title_w, base, hl);
                    w.put_spans(&spans);
                    if let Some(host) = host {
                        w.put(&host, style(theme, Meaning::Annotation));
                    }
                }
                AiSessionColumn::Repo => {
                    let text = repo_name(row)
                        .pad_ellipsize(
                            Measure::Columns(cw),
                            Pos::End,
                            Indicator::UNICODE,
                            Align::Start,
                        )
                        .into_owned();
                    w.put(&text, style(theme, Meaning::Annotation));
                }
                AiSessionColumn::Branch => {
                    let text = row
                        .branch
                        .as_deref()
                        .unwrap_or("")
                        .pad_ellipsize(
                            Measure::Columns(cw),
                            Pos::End,
                            Indicator::UNICODE,
                            Align::Start,
                        )
                        .into_owned();
                    w.put(&text, style(theme, Meaning::Guidance));
                }
                AiSessionColumn::Messages => {
                    w.put(
                        &format!("{:>cw$}", row.message_count),
                        style(theme, Meaning::Annotation),
                    );
                }
            }
            w.pad_to(end);
        }
    }
}

// --- the frame -------------------------------------------------------------------------------

struct PreviewParts<'a> {
    loaded: bool,
    first: Option<(&'a str, Vec<Range<usize>>)>,
    matched: Option<&'a Snippet>,
    last: Option<(&'a str, Vec<Range<usize>>)>,
}

/// `text` with the match's highlights when the match lies inside it (taking the match, so it
/// isn't shown twice).
fn locate<'a>(
    text: Option<&'a str>,
    matched: &mut Option<&Snippet>,
) -> Option<(&'a str, Vec<Range<usize>>)> {
    let text = text?;
    let highlights = match *matched {
        Some(m) if !m.text.is_empty() => text.find(m.text.as_str()).map(|at| {
            *matched = None;
            shift(&m.highlights, at)
        }),
        _ => None,
    };
    Some((text, highlights.unwrap_or_default()))
}

fn shift(ranges: &[Range<usize>], by: usize) -> Vec<Range<usize>> {
    ranges.iter().map(|r| r.start + by..r.end + by).collect()
}

#[derive(Clone, Copy)]
struct StyleState {
    compactness: Compactness,
    invert: bool,
    inner_width: usize,
}

impl State {
    /// The selected session's preview parts: the first prompt, the match (unless it lies inside
    /// the first prompt or the last reply, which then carry its highlights), and the last reply.
    fn preview_parts(&self) -> Option<PreviewParts<'_>> {
        let row = self.selected()?;
        let preview = self.previews.get(&row.handle);
        let mut matched = row.matched.as_ref();
        let first = locate(preview.and_then(|p| p.first_prompt.as_deref()), &mut matched);
        let last = locate(preview.and_then(|p| p.last_assistant.as_deref()), &mut matched);
        Some(PreviewParts {
            loaded: preview.is_some(),
            first,
            matched,
            last,
        })
    }

    /// The preview's lines for the selected session.
    fn preview_lines(&self, width: usize, theme: &Theme) -> Vec<Line<'static>> {
        let Some(parts) = self.preview_parts() else {
            return Vec::new();
        };
        let label = |s: &'static str| Span::styled(s, style(theme, Meaning::Annotation));
        let width = width.saturating_sub(7);
        let base = style(theme, Meaning::Base);
        let muted = style(theme, Meaning::Annotation);
        let hl = style(theme, Meaning::AlertWarn).add_modifier(Modifier::BOLD);
        let mut lines = Vec::new();

        if let Some((text, highlights)) = &parts.first {
            let mut spans = vec![label("first  ")];
            spans.extend(highlighted_line(text, highlights, width, base, hl));
            lines.push(Line::from(spans));
        }
        if let Some(Snippet { text, highlights }) = parts.matched {
            let mut spans = vec![label("match  ")];
            spans.extend(highlighted_line(
                &format!("…{text}"),
                &shift(highlights, '…'.len_utf8()),
                width,
                base,
                hl,
            ));
            lines.push(Line::from(spans));
        }
        if let Some((text, highlights)) = &parts.last {
            let mut spans = vec![label("last   ")];
            spans.extend(highlighted_line(text, highlights, width, muted, hl));
            lines.push(Line::from(spans));
        }
        if !parts.loaded && lines.is_empty() {
            lines.push(Line::from(label("…")));
        }
        lines
    }

    fn calc_preview_height(
        &self,
        settings: &Settings,
        compactness: Compactness,
        border_size: u16,
    ) -> u16 {
        if settings.show_preview && self.tab_index == 0 {
            let wanted = match settings.preview.strategy {
                PreviewStrategy::Fixed => settings.max_preview_height,
                PreviewStrategy::Static => 3,
                PreviewStrategy::Auto => self.preview_parts().map_or(1, |p| {
                    u16::from(p.first.is_some())
                        + u16::from(p.matched.is_some())
                        + u16::from(p.last.is_some())
                }),
            };
            wanted.min(settings.max_preview_height).max(1) + border_size * 2
        } else if compactness != Compactness::Full || self.tab_index == 1 {
            0
        } else {
            1
        }
    }

    #[allow(clippy::too_many_lines)]
    pub fn draw(
        &mut self,
        f: &mut Frame,
        settings: &Settings,
        theme: &Theme,
        resumer: &dyn Resumer,
    ) {
        let area = f.area();
        f.render_widget(Clear, area);
        let compactness = to_compactness(area, settings);
        let invert = settings.invert;
        let border_size = u16::from(compactness == Compactness::Full);
        let preview_height = self.calc_preview_height(settings, compactness, border_size);

        let show_help = settings.show_help && (compactness == Compactness::Full || area.height > 1);
        let show_tabs = settings.show_tabs && compactness != Compactness::Ultracompact;
        let status_height = u16::from(self.status.is_some());
        let help_h = u16::from(show_help);
        let tabs_h = u16::from(show_tabs);

        let constraints: [Constraint; 6] = if invert {
            [
                Constraint::Length(1 + border_size),
                Constraint::Min(1),
                Constraint::Length(preview_height),
                Constraint::Length(tabs_h),
                Constraint::Length(help_h),
                Constraint::Length(status_height),
            ]
        } else if compactness == Compactness::Ultracompact {
            [
                Constraint::Length(help_h),
                Constraint::Length(0),
                Constraint::Min(1),
                Constraint::Length(0),
                Constraint::Length(0),
                Constraint::Length(status_height),
            ]
        } else {
            [
                Constraint::Length(help_h),
                Constraint::Length(tabs_h),
                Constraint::Min(1),
                Constraint::Length(1 + border_size),
                Constraint::Length(preview_height),
                Constraint::Length(status_height),
            ]
        };
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .margin(0)
            .horizontal_margin(1)
            .constraints(constraints)
            .split(area);

        let (input_chunk, list_chunk, preview_chunk, tabs_chunk, header_chunk) = if invert {
            (chunks[0], chunks[1], chunks[2], chunks[3], chunks[4])
        } else {
            (chunks[3], chunks[2], chunks[4], chunks[1], chunks[0])
        };
        let status_chunk = chunks[5];

        if show_tabs {
            let titles: Vec<Line> = TAB_TITLES.iter().copied().map(Line::from).collect();
            let tabs = Tabs::new(titles)
                .block(Block::default().borders(Borders::NONE))
                .select(self.tab_index)
                .style(Style::default())
                .highlight_style(style(theme, Meaning::Important));
            f.render_widget(tabs, tabs_chunk);
        }

        let st = StyleState {
            compactness,
            invert,
            inner_width: input_chunk.width.into(),
        };

        let header_chunks = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Length(16), Constraint::Min(0), Constraint::Length(16)])
            .split(header_chunk);
        f.render_widget(
            Paragraph::new(Span::styled(
                format!("Atuin v{VERSION}"),
                style(theme, Meaning::Base).add_modifier(Modifier::BOLD),
            )),
            header_chunks[0],
        );
        f.render_widget(self.build_help(settings, theme), header_chunks[1]);
        f.render_widget(self.build_stats(theme), header_chunks[2]);

        if let Some((message, meaning)) = &self.status {
            f.render_widget(
                Paragraph::new(Span::styled(
                    message.clone(),
                    style(theme, *meaning).add_modifier(Modifier::BOLD),
                )),
                status_chunk,
            );
        }

        let indicator = match compactness {
            Compactness::Ultracompact => {
                format!("{}> ", self.mode_label().chars().next().unwrap_or(' '))
            }
            _ => " > ".to_owned(),
        };

        if self.tab_index == 1 {
            self.draw_inspect(f, list_chunk, st, settings, theme, resumer);
            let guide = Line::from(vec![
                Span::styled("<esc>", Style::default().add_modifier(Modifier::BOLD)),
                Span::raw(": back  "),
                Span::styled("<enter>", Style::default().add_modifier(Modifier::BOLD)),
                Span::raw(if settings.enter_accept {
                    ": resume  "
                } else {
                    ": edit  "
                }),
                Span::styled("<tab>", Style::default().add_modifier(Modifier::BOLD)),
                Span::raw(": edit  "),
                Span::styled("<ctrl-y>", Style::default().add_modifier(Modifier::BOLD)),
                Span::raw(": copy"),
            ]);
            let guide = Paragraph::new(guide).style(style(theme, Meaning::Annotation));
            f.render_widget(input_block(guide, st), input_chunk);
            return;
        }

        let list = SessionList {
            rows: &self.results,
            block: None,
            inverted: invert,
            alternate_highlight: self.keymap_mode == KeymapMode::VimNormal,
            now: (self.now)(),
            indicator: &indicator,
            theme,
            columns: &settings.ai.sessions.columns,
            host_id: &self.context.host_id,
        };
        let list = match compactness {
            Compactness::Full if invert => SessionList {
                block: Some(
                    Block::default()
                        .borders(Borders::LEFT | Borders::RIGHT)
                        .border_type(BorderType::Rounded)
                        .title(format!("{:─>width$}", "", width = st.inner_width - 2)),
                ),
                ..list
            },
            Compactness::Full => SessionList {
                block: Some(
                    Block::default()
                        .borders(Borders::TOP | Borders::LEFT | Borders::RIGHT)
                        .border_type(BorderType::Rounded),
                ),
                ..list
            },
            _ => list,
        };
        f.render_stateful_widget(list, list_chunk, &mut self.list);

        if compactness == Compactness::Ultracompact {
            return;
        }

        // Line the query up with the title column, as the history search lines it up with the
        // command.
        let prefix_width = settings
            .ai
            .sessions
            .columns
            .iter()
            .take_while(|c| !c.expands())
            .map(|c| c.width() + 1)
            .sum::<u16>()
            + 3;
        let prefix_width =
            prefix_width.max(u16::try_from("[ SRCH: FULLTXT ] ".len()).unwrap_or(18));
        f.render_widget(self.build_input(st, prefix_width, theme), input_chunk);

        let preview_width = usize::from(preview_chunk.width.saturating_sub(2 * border_size));
        let lines = self.preview_lines(preview_width, theme);
        let preview = match compactness {
            Compactness::Full => Paragraph::new(Text::from(lines)).block(
                Block::default()
                    .borders(Borders::BOTTOM | Borders::LEFT | Borders::RIGHT)
                    .border_type(BorderType::Rounded)
                    .title(format!(
                        "{:─>width$}",
                        "",
                        width = usize::from(preview_chunk.width) - 2
                    )),
            ),
            _ => Paragraph::new(Text::from(lines)).style(style(theme, Meaning::Annotation)),
        };
        f.render_widget(preview, preview_chunk);

        let before_cursor = self.input.substring().width();
        let cursor_offset = border_size;
        f.set_cursor_position((
            input_chunk.x
                + u16::try_from(before_cursor).unwrap_or(u16::MAX)
                + prefix_width
                + cursor_offset,
            input_chunk.y + cursor_offset,
        ));
    }

    fn build_help(&self, settings: &Settings, theme: &Theme) -> Paragraph<'static> {
        let bold = Style::default().add_modifier(Modifier::BOLD);
        let line = if self.tab_index == 0 {
            Line::from(vec![
                Span::styled("<esc>", bold),
                Span::raw(": exit, "),
                Span::styled("<tab>", bold),
                Span::raw(": edit, "),
                Span::styled("<enter>", bold),
                Span::raw(if settings.enter_accept {
                    ": resume"
                } else {
                    ": edit"
                }),
                Span::raw(", "),
                Span::styled("<ctrl-o>", bold),
                Span::raw(": inspect"),
            ])
        } else {
            Line::default()
        };
        Paragraph::new(line).style(style(theme, Meaning::Annotation)).alignment(Alignment::Center)
    }

    fn build_stats(&self, theme: &Theme) -> Paragraph<'static> {
        let n = self.results.len();
        let text = if self.applied == 0 {
            String::new()
        } else if n == 1 {
            "1 session".to_owned()
        } else {
            format!("{n} sessions")
        };
        Paragraph::new(text).style(style(theme, Meaning::Annotation)).alignment(Alignment::Right)
    }

    fn build_input(&self, st: StyleState, prefix_width: u16, theme: &Theme) -> Paragraph<'static> {
        let mode = self.mode_label();
        // 3: the surrounding "[" and "] ".
        let mode_width = usize::from(prefix_width) - 3;
        let mut spans = vec![Span::raw(format!("[{mode:^mode_width$}] "))];

        // Filter tokens render as chips; the rest is plain text.
        let input = self.input.as_str();
        let parsed = self.parsed_query();
        let mut at = 0;
        for token in &parsed.tokens {
            if token.range.start > at {
                spans.push(Span::raw(input[at..token.range.start].to_owned()));
            }
            let chip = match (token.state, token.kind) {
                (TokenState::Invalid, _) => {
                    style(theme, Meaning::AlertError).add_modifier(Modifier::UNDERLINED)
                }
                (TokenState::Pending, _) => {
                    style(theme, Meaning::Annotation).add_modifier(Modifier::REVERSED)
                }
                (TokenState::Valid, TokenKind::Harness) => {
                    style(theme, Meaning::Important).add_modifier(Modifier::REVERSED)
                }
                (TokenState::Valid, _) => {
                    style(theme, Meaning::Guidance).add_modifier(Modifier::REVERSED)
                }
            };
            spans.push(Span::styled(input[token.range.clone()].to_owned(), chip));
            at = token.range.end;
        }
        if at < input.len() {
            spans.push(Span::raw(input[at..].to_owned()));
        }

        input_block(Paragraph::new(Line::from(spans)), st)
    }

    #[allow(clippy::too_many_lines)]
    fn draw_inspect(
        &self,
        f: &mut Frame,
        chunk: Rect,
        st: StyleState,
        settings: &Settings,
        theme: &Theme,
        resumer: &dyn Resumer,
    ) {
        let block = match st.compactness {
            Compactness::Full if st.invert => Block::default()
                .borders(Borders::LEFT | Borders::RIGHT)
                .border_type(BorderType::Rounded)
                .title(format!("{:─>width$}", "", width = st.inner_width - 2)),
            Compactness::Full => Block::default()
                .borders(Borders::TOP | Borders::LEFT | Borders::RIGHT)
                .border_type(BorderType::Rounded),
            _ => Block::default(),
        };
        let inner = block.inner(chunk);
        f.render_widget(block, chunk);

        let Some(row) = self.selected() else {
            f.render_widget(
                Paragraph::new("Nothing to inspect").alignment(Alignment::Center),
                inner,
            );
            return;
        };

        let now = (self.now)();
        let tz = settings.timezone.0;
        let key = style(theme, Meaning::Annotation);
        let base = style(theme, Meaning::Base);
        let width = usize::from(inner.width).saturating_sub(11);
        let field = |name: &'static str, spans: Vec<Span<'static>>| {
            let mut line = vec![Span::styled(format!(" {name:<10}"), key)];
            line.extend(spans);
            Line::from(line)
        };
        let text = |s: String| vec![Span::styled(s, base)];
        let when = |ts: OffsetDateTime| format_when(ts, now, tz);

        let this_host = row.host_id == self.context.host_id;
        let plan = resumer.plan(row);
        let mut lines = vec![
            field("Session", vec![
                Span::styled(row.handle.session.to_string(), base.add_modifier(Modifier::BOLD)),
                Span::raw("  "),
                Span::styled(
                    harness_label(row.handle.harness),
                    harness_style(theme, row.handle.harness),
                ),
            ]),
            field("Title", highlighted_line(&row.title.text, &[], width, base, base)),
            field("Host", vec![
                Span::styled(row.hostname.clone(), base),
                Span::styled(
                    if this_host {
                        "  (this host)"
                    } else {
                        "  (remote)"
                    },
                    key,
                ),
            ]),
            field(
                "Directory",
                text(row.cwd.as_deref().map(|p| p.display().to_string()).unwrap_or_default()),
            ),
            field("Branch", text(row.branch.clone().unwrap_or_default())),
            field("Model", text(row.model.clone().unwrap_or_default())),
            field("Started", text(when(row.started_at))),
            field("Updated", {
                let mut spans = text(when(row.updated_at));
                if is_live(now, row) {
                    spans.push(Span::styled("  ● live", style(theme, Meaning::AlertInfo)));
                }
                spans
            }),
            field("Messages", text(row.message_count.to_string())),
        ];
        lines.push(match &plan.blocked {
            None => field("Resume", vec![Span::styled(
                plan.shell_line(),
                style(theme, Meaning::Important).add_modifier(Modifier::BOLD),
            )]),
            Some(why) => field("Resume", vec![Span::styled(
                format!("not resumable: {why}"),
                style(theme, Meaning::AlertError),
            )]),
        });

        let children = self.children.get(&row.handle);
        if row.children > 0 || children.is_some_and(|c| !c.is_empty()) {
            lines.push(Line::default());
            let count = children.map_or(row.children as usize, Vec::len);
            lines.push(Line::from(Span::styled(
                format!(" Children ({count})"),
                key.add_modifier(Modifier::BOLD),
            )));
            match children {
                None => lines.push(Line::from(Span::styled("   …", key))),
                Some(children) => {
                    for child in children {
                        lines.push(child_line(child, now, usize::from(inner.width), theme));
                    }
                }
            }
        }

        f.render_widget(Paragraph::new(Text::from(lines)), inner);
    }
}

fn format_when(ts: OffsetDateTime, now: OffsetDateTime, tz: UtcOffset) -> String {
    format!("{}  ({} ago)", ts.to_offset(tz).display().ymd_hm(), ago(now, ts))
}

fn child_line(
    child: &SessionRow,
    now: OffsetDateTime,
    width: usize,
    theme: &Theme,
) -> Line<'static> {
    let (tag, meaning) = match child.relation {
        Relation::Fork => ("fork    ", Meaning::Guidance),
        Relation::Subagent => ("subagent", Meaning::Important),
        Relation::Root => ("session ", Meaning::Base),
    };
    let tail = format!(
        "{:>5} msgs  {:>9}",
        child.message_count,
        format!("{} ago", ago(now, child.updated_at))
    );
    let base = style(theme, Meaning::Base);
    let title_w = width.saturating_sub(3 + 8 + 2 + 2 + tail.width());
    let mut spans =
        vec![Span::raw("   "), Span::styled(tag, style(theme, meaning)), Span::raw("  ")];
    let title = highlighted_line(&child.title.text, &[], title_w, base, base);
    let pad = title_w.saturating_sub(spans_width(&title));
    spans.extend(title);
    spans.push(Span::raw(" ".repeat(pad + 2)));
    spans.push(Span::styled(tail, style(theme, Meaning::Annotation)));
    Line::from(spans)
}

/// The input box's borders, as in the history search.
fn input_block(p: Paragraph<'static>, st: StyleState) -> Paragraph<'static> {
    match st.compactness {
        Compactness::Full if st.invert => p.block(
            Block::default()
                .borders(Borders::LEFT | Borders::RIGHT | Borders::TOP)
                .border_type(BorderType::Rounded),
        ),
        Compactness::Full => p.block(
            Block::default()
                .borders(Borders::LEFT | Borders::RIGHT)
                .border_type(BorderType::Rounded)
                .title(format!("{:─>width$}", "", width = st.inner_width - 2)),
        ),
        _ => p,
    }
}
