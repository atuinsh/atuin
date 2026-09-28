//! Drawing the picker. The layout, header, tabs, input box and borders follow the history search
//! (`atuin search -i`) exactly, so the two feel like one tool.

use std::borrow::Cow;
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
    Block, BorderType, Borders, Clear, Paragraph, Scrollbar, ScrollbarOrientation, ScrollbarState,
    StatefulWidget, Tabs, Widget,
};
use time::{OffsetDateTime, UtcOffset};
use unicode_width::UnicodeWidthStr;

use super::chooser::ListAnchor;
use super::markdown;
use super::panel::{self, SPLIT_MIN_WIDTH};
use super::query::{TokenKind, TokenState};
use super::resumer::shell_line;
use super::source::{SessionRow, Snippet, harness_badge, harness_label};
use super::state::{LIVE_SECS, ListState, State, TAB_TITLES};

const VERSION: &str = env!("CARGO_PKG_VERSION");

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

pub(super) fn style(theme: &Theme, meaning: Meaning) -> Style {
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

/// Group characters into spans by whether their source byte is highlighted.
fn spans_from(
    chars: impl Iterator<Item = (char, Option<usize>)>,
    map: &[usize],
    highlights: &[Range<usize>],
    base: Style,
    hl: Style,
) -> Vec<Span<'static>> {
    let mut spans: Vec<Span<'static>> = Vec::new();
    let mut run = String::new();
    let mut run_hl = false;
    for (ch, source) in chars {
        let is_hl = source
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
    let chars = display.char_indices().map(|(i, c)| (c, ellipsized.source_index(i)));
    spans_from(chars, &map, highlights, base, hl)
}

/// The repository (or directory) name for the repo column.
pub(super) fn repo_name(row: &SessionRow) -> String {
    row.git_root
        .as_deref()
        .or(row.cwd.as_deref())
        .and_then(Path::file_name)
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default()
}

pub(super) fn is_live(now: OffsetDateTime, row: &SessionRow) -> bool {
    now.saturating_duration_since(row.updated_at).as_secs() < LIVE_SECS
}

pub(super) fn ago(now: OffsetDateTime, ts: OffsetDateTime) -> String {
    now.saturating_duration_since(ts).display().largest_unit().to_string()
}

pub(super) fn harness_style(theme: &Theme, harness: HarnessKind) -> Style {
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
                    // Another host's session resumes by restoring it from sync (or continuing it
                    // elsewhere), when atuin can resume its harness at all.
                    if row.host_id != self.host_id && row.handle.harness.harness().is_none() {
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

/// The width of the preview's `first  ` / `match  ` / `last   ` labels.
const PREVIEW_LABEL_WIDTH: usize = 7;

/// One preview part, ready to render as markdown.
struct PreviewSource<'a> {
    label: &'static str,
    text: Cow<'a, str>,
    highlights: Vec<Range<usize>>,
    styles: markdown::Styles,
}

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

    /// The selected session's preview parts as markdown sources: a label, the text, its
    /// highlights, and the style it runs in.
    fn preview_sources(&self, theme: &Theme) -> Option<(bool, Vec<PreviewSource<'_>>)> {
        let parts = self.preview_parts()?;
        let base = style(theme, Meaning::Base);
        let muted = style(theme, Meaning::Annotation);
        let mut sources = Vec::new();
        if let Some((text, highlights)) = parts.first {
            sources.push(PreviewSource {
                label: "first  ",
                text: Cow::Borrowed(text),
                highlights,
                styles: markdown::Styles::new(theme, base),
            });
        }
        if let Some(Snippet { text, highlights }) = parts.matched {
            sources.push(PreviewSource {
                label: "match  ",
                text: Cow::Owned(format!("…{text}")),
                highlights: shift(highlights, '…'.len_utf8()),
                styles: markdown::Styles::new(theme, base),
            });
        }
        if let Some((text, highlights)) = parts.last {
            sources.push(PreviewSource {
                label: "last   ",
                text: Cow::Borrowed(text),
                highlights,
                styles: markdown::Styles::new(theme, muted),
            });
        }
        Some((parts.loaded, sources))
    }

    /// Each preview part rendered at `width` in at most `height` lines.
    fn preview_rendered(
        sources: &[PreviewSource<'_>],
        width: usize,
        height: usize,
    ) -> Vec<Vec<Line<'static>>> {
        let opts = markdown::Opts {
            width: width.saturating_sub(PREVIEW_LABEL_WIDTH),
            max_lines: height,
            spacing: false,
            urls: false,
        };
        sources.iter().map(|s| markdown::render(&s.text, &s.highlights, opts, &s.styles)).collect()
    }

    /// The preview's lines for the selected session, in `height` lines of `width` columns: the
    /// parts share the lines, and a part with only one gets its markdown run onto that line.
    fn preview_lines(&self, width: usize, height: usize, theme: &Theme) -> Vec<Line<'static>> {
        let Some((loaded, sources)) = self.preview_sources(theme) else {
            return Vec::new();
        };
        let label = |s: &'static str| Span::styled(s, style(theme, Meaning::Annotation));
        let inner = width.saturating_sub(PREVIEW_LABEL_WIDTH);
        let rendered = Self::preview_rendered(&sources, width, height);
        let wants: Vec<usize> = rendered.iter().map(Vec::len).collect();
        let budgets = markdown::allocate(&wants, height);
        let mut lines = Vec::new();

        for ((source, rendered), n) in sources.iter().zip(&rendered).zip(budgets) {
            let body = match n {
                0 => continue,
                1 => vec![markdown::render_flat(
                    &source.text,
                    &source.highlights,
                    inner,
                    &source.styles,
                )],
                n => markdown::fit(rendered, n, inner, source.styles.muted),
            };
            for (i, line) in body.into_iter().enumerate() {
                let mut spans = vec![if i == 0 {
                    label(source.label)
                } else {
                    Span::raw(" ".repeat(PREVIEW_LABEL_WIDTH))
                }];
                spans.extend(line.spans);
                lines.push(Line::from(spans));
            }
        }
        if !loaded && lines.is_empty() {
            lines.push(Line::from(label("…")));
        }
        lines
    }

    fn calc_preview_height(
        &self,
        settings: &Settings,
        compactness: Compactness,
        border_size: u16,
        width: usize,
        theme: &Theme,
    ) -> u16 {
        if settings.show_preview && self.tab_index == 0 {
            let wanted = match settings.preview.strategy {
                PreviewStrategy::Fixed => settings.max_preview_height,
                PreviewStrategy::Static => 3,
                PreviewStrategy::Auto => self.preview_sources(theme).map_or(1, |(_, sources)| {
                    let max = usize::from(settings.max_preview_height);
                    let lines: usize =
                        Self::preview_rendered(&sources, width, max).iter().map(Vec::len).sum();
                    u16::try_from(lines).unwrap_or(u16::MAX)
                }),
            };
            wanted.min(settings.max_preview_height).max(1) + border_size * 2
        } else if compactness != Compactness::Full || self.tab_index == 1 {
            0
        } else {
            1
        }
    }

    /// Where the selected row of the list in `area` is, for the chooser to open against.
    fn anchor(&self, area: Rect, columns: &[AiSessionColumn], invert: bool) -> Option<ListAnchor> {
        if self.results.is_empty() || area.height == 0 {
            return None;
        }
        let from_top = u16::try_from(self.list.selected.checked_sub(self.list.offset)?).ok()?;
        let row = if invert {
            area.top() + from_top
        } else {
            area.bottom().checked_sub(from_top + 1)?
        };
        // Past the indicator, and the columns before the badge (or the title, without one).
        let before: u16 = columns
            .iter()
            .take_while(|c| !matches!(c, AiSessionColumn::Harness | AiSessionColumn::Title))
            .map(|c| c.width() + 1)
            .sum();
        Some(ListAnchor {
            list: area,
            row,
            badge_x: area.x + 3 + before,
        })
    }

    #[allow(clippy::too_many_lines)]
    pub fn draw(&mut self, f: &mut Frame, settings: &Settings, theme: &Theme) {
        self.draw_main(f, settings, theme);
        if self.chooser.is_some() {
            self.draw_chooser(f, settings, theme);
        }
    }

    fn draw_main(&mut self, f: &mut Frame, settings: &Settings, theme: &Theme) {
        let area = f.area();
        self.list_anchor = None;
        f.render_widget(Clear, area);
        let compactness = to_compactness(area, settings);
        let invert = settings.invert;
        let border_size = u16::from(compactness == Compactness::Full);
        // Wide terminals get the detail pane beside the list instead of the preview strip.
        let split = self.tab_index == 0
            && settings.show_preview
            && compactness != Compactness::Ultracompact
            && area.width >= SPLIT_MIN_WIDTH;
        let preview_height = if split {
            border_size
        } else {
            let width = area.width.saturating_sub(2 + 2 * border_size);
            self.calc_preview_height(settings, compactness, border_size, width.into(), theme)
        };

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
            self.draw_inspect(f, list_chunk, st, settings, theme);
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

        let block = match compactness {
            Compactness::Full if invert => Some(
                Block::default()
                    .borders(Borders::LEFT | Borders::RIGHT)
                    .border_type(BorderType::Rounded)
                    .title(format!("{:─>width$}", "", width = st.inner_width - 2)),
            ),
            Compactness::Full => Some(
                Block::default()
                    .borders(Borders::TOP | Borders::LEFT | Borders::RIGHT)
                    .border_type(BorderType::Rounded),
            ),
            _ => None,
        };
        let inner = block.as_ref().map_or(list_chunk, |b| b.inner(list_chunk));
        if let Some(block) = block {
            f.render_widget(block, list_chunk);
        }
        let (list_area, divider, pane) = if split {
            let [list_area, divider, pane] = Layout::horizontal([
                Constraint::Fill(3),
                Constraint::Length(1),
                Constraint::Fill(2),
            ])
            .areas(inner);
            (list_area, Some(divider), Some(pane))
        } else {
            (inner, None, None)
        };

        // The pane shows the repository and branch, so the split list gives their room to titles.
        let columns: Vec<AiSessionColumn> = settings
            .ai
            .sessions
            .columns
            .iter()
            .copied()
            .filter(|c| !split || !matches!(c, AiSessionColumn::Repo | AiSessionColumn::Branch))
            .collect();
        let list = SessionList {
            rows: &self.results,
            block: None,
            inverted: invert,
            alternate_highlight: self.keymap_mode == KeymapMode::VimNormal,
            now: (self.now)(),
            indicator: &indicator,
            theme,
            columns: &columns,
            host_id: &self.context.host_id,
        };
        f.render_stateful_widget(list, list_area, &mut self.list);
        self.list_anchor = self.anchor(list_area, &columns, invert);

        // A scrollbar on the right border (or the divider) once the list overflows.
        let visible = usize::from(list_area.height);
        let track = divider.unwrap_or(Rect {
            x: list_chunk.right().saturating_sub(1),
            y: list_area.y,
            width: 1,
            height: list_area.height,
        });
        if let Some(divider) = divider {
            let line = style(theme, Meaning::Annotation);
            for y in divider.top()..divider.bottom() {
                f.buffer_mut()[(divider.x, y)].set_symbol("│").set_style(line);
            }
        }
        if self.results.len() > visible && (divider.is_some() || compactness == Compactness::Full) {
            let top = if invert {
                self.list.offset
            } else {
                self.results.len().saturating_sub(self.list.offset + visible)
            };
            let mut state = ScrollbarState::new(self.results.len().saturating_sub(visible))
                .position(top)
                .viewport_content_length(visible);
            let bar = Scrollbar::new(ScrollbarOrientation::VerticalRight)
                .begin_symbol(None)
                .end_symbol(None)
                .track_symbol(Some("│"))
                .thumb_symbol("┃")
                .style(style(theme, Meaning::Annotation));
            f.render_stateful_widget(bar, track, &mut state);
        }

        if let Some(pane) = pane {
            let pane = pane.inner(ratatui::layout::Margin::new(1, 0));
            let lines = self.detail_lines(usize::from(pane.width), usize::from(pane.height), theme);
            // Already wrapped (markdown keeps its indents, which the paragraph's wrap would trim).
            f.render_widget(Paragraph::new(Text::from(lines)), pane);
        }

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
        let preview_lines = usize::from(preview_chunk.height.saturating_sub(2 * border_size));
        let lines = self.preview_lines(preview_width, preview_lines, theme);
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

        // Join the divider to the box's top and bottom borders.
        if let Some(divider) = divider
            && compactness == Compactness::Full
        {
            let border = f.buffer_mut()[(divider.x, list_chunk.y)].style();
            f.buffer_mut()[(divider.x, list_chunk.y)].set_symbol("┬").set_style(border);
            let below = list_chunk.bottom();
            if below < area.bottom() {
                f.buffer_mut()[(divider.x, below)].set_symbol("┴").set_style(border);
            }
        }

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
        if let Some(t) = panel::tokens(&row.usage) {
            lines.push(field("Tokens", text(t)));
        }
        if let Some(p) = self.previews.get(&row.handle).filter(|p| !p.activity.is_empty()) {
            let spark =
                panel::sparkline(&p.activity, row.started_at, row.updated_at, width.min(60));
            lines.push(field("Activity", vec![Span::styled(
                spark,
                style(theme, Meaning::Guidance),
            )]));
        }
        lines.push(match self.plans.get(&row.handle) {
            None => field("Resume", vec![Span::styled("…", key)]),
            Some(Ok(resume)) => field("Resume", vec![
                Span::styled(
                    shell_line(&resume.plan),
                    style(theme, Meaning::Important).add_modifier(Modifier::BOLD),
                ),
                // Its transcript is written from the synced messages first.
                Span::styled(
                    if resume.restore.is_some() {
                        "  from sync"
                    } else {
                        ""
                    },
                    key,
                ),
            ]),
            Some(Err(why)) => field("Resume", vec![Span::styled(
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
                Some(children) => lines.extend(panel::tree_lines(
                    &row.handle,
                    children,
                    now,
                    usize::from(inner.width),
                    theme,
                )),
            }
        }

        // The conversation, in whatever room is left.
        let left = usize::from(inner.height).saturating_sub(lines.len());
        lines.extend(self.conversation(row, usize::from(inner.width), left, 1, theme));

        f.render_widget(Paragraph::new(Text::from(lines)), inner);
    }
}

fn format_when(ts: OffsetDateTime, now: OffsetDateTime, tz: UtcOffset) -> String {
    format!("{}  ({} ago)", ts.to_offset(tz).display().ymd_hm(), ago(now, ts))
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
