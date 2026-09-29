//! Drawing the picker. The layout, header, tabs, input box and borders follow the history search
//! (`atuin search -i`) exactly, so the two feel like one tool.

use std::borrow::Cow;
use std::ops::Range;
use std::path::Path;

use atuin_client::ai_session::{HarnessKind, HarnessSession};
use atuin_client::settings::{
    AiSessionColumn, KeymapMode, PreviewStrategy, Settings, Style as UiStyle,
};
use atuin_client::theme::{Meaning, Theme};
use atuin_common::string::ellipsis::{Indicator, Pos};
use atuin_common::string::{Alignment as Align, EllipsizeExt as _, Measure};
use atuin_common::time::OffsetDateTimeExt as _;
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
use super::panel::{self, SPLIT_MIN_WIDTH};
use super::query::{TokenKind, TokenState};
use super::resumer::shell_line;
use super::source::{SessionRow, Snippet, harness_badge, harness_label};
use super::state::{LIVE_SECS, ListState, Pane, SEARCH_LIMIT, State, TAB_TITLES};
use super::{clock, markdown};

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

/// A host's name as rows show it: without its domain (`MacBook-Pro.local` is `MacBook-Pro`).
pub(super) fn short_host(name: &str) -> &str {
    if name.chars().all(|c| c.is_ascii_digit() || c == '.') {
        return name;
    }
    name.split('.').next().filter(|n| !n.is_empty()).unwrap_or(name)
}

/// The branch a row shows: none when detached (`HEAD`).
pub(super) fn shown_branch(row: &SessionRow) -> Option<&str> {
    row.branch.as_deref().filter(|b| !b.is_empty() && *b != "HEAD")
}

/// `1234`, or `12k` once it no longer fits the messages column.
fn message_count(n: u64) -> String {
    if n < 10_000 {
        n.to_string()
    } else {
        panel::human(n)
    }
}

// --- the row layout --------------------------------------------------------------------------

/// The title keeps at least this many columns while the message count can give them up.
pub const TITLE_MIN: u16 = 30;

/// The columns of rows `width` columns wide (the selection indicator included), each with its
/// width. The title takes what's left, and the message count goes when that would leave the
/// title under [`TITLE_MIN`].
pub fn row_layout(columns: &[AiSessionColumn], width: u16) -> Vec<(AiSessionColumn, u16)> {
    let mut cells = columns.to_vec();
    // Past the indicator, and a space between cells.
    let title_width = |cells: &[AiSessionColumn]| {
        let others: u16 = cells.iter().map(AiSessionColumn::width).sum();
        let gaps = u16::try_from(cells.len().saturating_sub(1)).unwrap_or(u16::MAX);
        width.saturating_sub(3).saturating_sub(others).saturating_sub(gaps)
    };
    if cells.iter().any(AiSessionColumn::expands) && title_width(&cells) < TITLE_MIN {
        cells.retain(|c| *c != AiSessionColumn::Messages);
    }
    let title = title_width(&cells);
    cells
        .into_iter()
        .map(|c| {
            (
                c,
                if c.expands() {
                    title
                } else {
                    c.width()
                },
            )
        })
        .collect()
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
    tz: UtcOffset,
    indicator: &'a str,
    theme: &'a Theme,
    cells: &'a [(AiSessionColumn, u16)],
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
                // Another host's session looks like any other: it resumes by being restored
                // from sync, behind the scenes.
                row_modifier: if self.alternate_highlight && selected {
                    Modifier::REVERSED
                } else {
                    Modifier::empty()
                },
            };
            self.render_row(&mut line, row, selected);
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
    fn render_row(&self, w: &mut RowWriter<'_>, row: &SessionRow, selected: bool) {
        let theme = self.theme;
        w.put(
            if selected {
                self.indicator
            } else {
                "   "
            },
            Style::default(),
        );

        let pad = |text: &str, cw: usize, align: Align| {
            text.pad_ellipsize(Measure::Columns(cw), Pos::End, Indicator::UNICODE, align)
                .into_owned()
        };
        for (idx, &(cell, col_width)) in self.cells.iter().enumerate() {
            if idx != 0 {
                w.put(" ", Style::default());
            }
            let end = w.x.saturating_add(col_width);
            let cw = usize::from(col_width);
            match cell {
                AiSessionColumn::Time => {
                    let when = clock::When::of(self.now, row.updated_at, self.tz);
                    let (text, meaning) = if is_live(self.now, row) {
                        (format!("● {}", when.short()), Meaning::AlertInfo)
                    } else {
                        (when.short().to_owned(), Meaning::Guidance)
                    };
                    w.put(&pad(&text, cw, Align::End), style(theme, meaning));
                }
                AiSessionColumn::Harness => {
                    w.put(
                        harness_badge(row.handle.harness),
                        harness_style(theme, row.handle.harness),
                    );
                }
                AiSessionColumn::Title => {
                    let (base, hl) = if selected && !self.alternate_highlight {
                        let base = style(theme, Meaning::AlertError).add_modifier(Modifier::BOLD);
                        (base, style(theme, Meaning::AlertWarn).add_modifier(Modifier::BOLD))
                    } else {
                        let base = style(theme, Meaning::Base);
                        (base, base.add_modifier(Modifier::BOLD))
                    };
                    let spans =
                        highlighted_line(&row.title.text, &row.title.highlights, cw, base, hl);
                    w.put_spans(&spans);
                }
                AiSessionColumn::Messages => {
                    w.put(
                        &format!("{:>cw$}", message_count(row.messages)),
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

/// How many lines of a pane's text in full are rendered at a time as it scrolls: the rendering
/// grows by this much as the view nears its end, so a long reply is never rendered whole just to
/// show its first screen.
const WINDOW_CHUNK: usize = 128;

/// What a scrolling pane shows.
pub(super) struct Body {
    pub lines: Vec<Line<'static>>,
    /// The first line shown of the text in full; 0 shows the overview.
    pub offset: usize,
    /// The lines of the text in full rendered.
    pub len: usize,
    /// More lines than `len`, not rendered yet.
    pub more: bool,
}

impl Body {
    fn overflows(&self, height: usize) -> bool {
        self.more || self.len > height
    }
}

/// `height` lines of a pane's text, scrolled to `want`: the `overview` at the top, else the text
/// in full from there (`document`, asked for enough lines to fill the view and more), stopping at
/// its end.
pub(super) fn scroll_body(
    want: usize,
    height: usize,
    overview: impl FnOnce() -> Vec<Line<'static>>,
    document: impl FnOnce(usize) -> (Vec<Line<'static>>, bool),
) -> Body {
    let limit = (want + 2 * height).max(1).div_ceil(WINDOW_CHUNK) * WINDOW_CHUNK;
    let (doc, more) = document(limit);
    let len = doc.len();
    let offset = want.min(len.saturating_sub(height));
    let lines = if offset == 0 {
        overview()
    } else {
        doc.into_iter().skip(offset).take(height).collect()
    };
    Body {
        lines,
        offset,
        len,
        more,
    }
}

/// A scrollbar in `track` (a border, or a column kept for it) for `body` shown `height` lines at
/// a time, once it has more than fits.
fn draw_scrollbar(f: &mut Frame, track: Rect, body: &Body, height: usize, theme: &Theme) {
    if !body.overflows(height) || track.height == 0 {
        return;
    }
    // The text not rendered yet counts for a screen more, so the thumb never says it's at the end.
    let len = body.len
        + if body.more {
            height
        } else {
            0
        };
    let mut state = ScrollbarState::new(len.saturating_sub(height))
        .position(body.offset)
        .viewport_content_length(height);
    let bar = Scrollbar::new(ScrollbarOrientation::VerticalRight)
        .begin_symbol(None)
        .end_symbol(None)
        .track_symbol(Some("│"))
        .thumb_symbol("┃")
        .style(style(theme, Meaning::Annotation));
    f.render_stateful_widget(bar, track, &mut state);
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
        let row = self.preview_row()?;
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

    /// The preview's line saying where the session previewed ran, what forked off it and how
    /// many branches it went on in, in the text column: `atuin · feat/ai-sessions ·
    /// @MacBook-Pro-3 · 2 forks · 2 branches`. `None` when there is
    /// nothing to say, or no room beside the text (`height` under 2).
    fn preview_meta(&self, height: usize, theme: &Theme) -> Option<Line<'static>> {
        if height < 2 {
            return None;
        }
        let row = self.preview_row()?;
        let muted = style(theme, Meaning::Annotation);
        let mut spans = panel::place(row, &self.context.host_id, theme);
        let forks = panel::forks(self.children.get(&row.handle).map(Vec::as_slice));
        for part in forks.into_iter().chain(panel::branches(row)) {
            if !spans.is_empty() {
                spans.push(Span::styled(" · ", muted));
            }
            spans.push(Span::styled(part, muted));
        }
        if spans.is_empty() {
            return None;
        }
        spans.insert(0, Span::raw(" ".repeat(PREVIEW_LABEL_WIDTH)));
        Some(Line::from(spans))
    }

    /// The strip's overview of the parts, in `height` lines of `width` columns: they share the
    /// lines, and a part with only one gets its markdown run onto that line.
    fn preview_overview(
        sources: &[PreviewSource<'_>],
        width: usize,
        height: usize,
        theme: &Theme,
    ) -> Vec<Line<'static>> {
        let label = |s: &'static str| Span::styled(s, style(theme, Meaning::Annotation));
        let inner = width.saturating_sub(PREVIEW_LABEL_WIDTH);
        let rendered = Self::preview_rendered(sources, width, height);
        let wants: Vec<usize> = rendered.iter().map(Vec::len).collect();
        let budgets = markdown::allocate(&wants, height);
        let mut lines: Vec<Line<'static>> = Vec::new();

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
        lines
    }

    /// The strip's parts in full, one after another under their labels, as it scrolls: up to
    /// `limit` lines a part, stopping at the first with more (`true`).
    fn preview_document(
        sources: &[PreviewSource<'_>],
        width: usize,
        limit: usize,
        theme: &Theme,
    ) -> (Vec<Line<'static>>, bool) {
        let opts = markdown::Opts {
            width: width.saturating_sub(PREVIEW_LABEL_WIDTH),
            max_lines: limit,
            spacing: false,
            urls: false,
        };
        let mut lines = Vec::new();
        for source in sources {
            let (body, more) =
                markdown::render_window(&source.text, &source.highlights, opts, &source.styles);
            for (i, line) in body.into_iter().enumerate() {
                let mut spans = vec![if i == 0 {
                    Span::styled(source.label, style(theme, Meaning::Annotation))
                } else {
                    Span::raw(" ".repeat(PREVIEW_LABEL_WIDTH))
                }];
                spans.extend(line.spans);
                lines.push(Line::from(spans));
            }
            if more {
                return (lines, true);
            }
        }
        (lines, false)
    }

    /// The strip's lines, in `height` lines of `width` columns: the metadata line (see
    /// [`Self::preview_meta`]), then the parts, scrolled (see [`scroll_body`]). Also the session
    /// shown, where the parts start and what scrolling them shows, for the caller to note.
    fn strip_lines(
        &self,
        width: usize,
        height: usize,
        theme: &Theme,
    ) -> (Vec<Line<'static>>, Option<(HarnessSession, usize, Body)>) {
        let (Some(row), Some((loaded, sources))) =
            (self.preview_row(), self.preview_sources(theme))
        else {
            return (Vec::new(), None);
        };
        let meta = self.preview_meta(height, theme);
        let top = usize::from(meta.is_some());
        let height = height - top;
        let want = self.scrolls[Pane::Strip as usize].offset_for(&row.handle);
        let body = scroll_body(
            want,
            height,
            || Self::preview_overview(&sources, width, height, theme),
            |limit| Self::preview_document(&sources, width, limit, theme),
        );
        let mut lines: Vec<Line<'static>> = meta.into_iter().collect();
        lines.extend(body.lines.iter().cloned());
        if !loaded && lines.len() == top {
            lines.push(Line::from(Span::styled("…", style(theme, Meaning::Annotation))));
        }
        (lines, Some((row.handle.clone(), top, body)))
    }

    /// Note how `pane` was drawn: for `session`, in `area` (what the mouse wheel scrolls), with
    /// `height` lines of text showing `body`.
    fn drawn(
        &mut self,
        pane: Pane,
        session: HarnessSession,
        area: Rect,
        height: usize,
        body: &Body,
    ) {
        let scroll = &mut self.scrolls[pane as usize];
        scroll.session = Some(session);
        scroll.offset = body.offset;
        scroll.area = Some(area);
        scroll.height = height;
        scroll.len = body.len;
        scroll.more = body.more;
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
                    // The metadata line takes the place of one of the text's.
                    let meta = usize::from(self.preview_meta(max, theme).is_some());
                    let lines: usize = Self::preview_rendered(&sources, width, max - meta)
                        .iter()
                        .map(Vec::len)
                        .sum();
                    // The text gets a line even before it is read (for its `…`).
                    u16::try_from(lines.max(1) + meta).unwrap_or(u16::MAX)
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
    fn anchor(
        &self,
        area: Rect,
        cells: &[(AiSessionColumn, u16)],
        invert: bool,
    ) -> Option<ListAnchor> {
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
        let before: u16 = cells
            .iter()
            .take_while(|(c, _)| !matches!(c, AiSessionColumn::Harness | AiSessionColumn::Title))
            .map(|(_, w)| w + 1)
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
        if self.warning.is_some() {
            self.draw_warning(f, theme);
        }
    }

    fn draw_main(&mut self, f: &mut Frame, settings: &Settings, theme: &Theme) {
        let area = f.area();
        self.list_anchor = None;
        for scroll in &mut self.scrolls {
            scroll.area = None;
        }
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
            // Less a column for the scrollbar, without a border to draw it on.
            let width = area.width.saturating_sub(2 + 2 * border_size + 1 - border_size);
            let height =
                self.calc_preview_height(settings, compactness, border_size, width.into(), theme);
            // An automatic height only grows: were it to follow each session's text (or shrink
            // while one is read), the list would jump as the selection moves.
            if settings.show_preview
                && self.tab_index == 0
                && settings.preview.strategy == PreviewStrategy::Auto
            {
                self.strip_height = self.strip_height.max(height);
                self.strip_height
            } else {
                height
            }
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
            let bold = Style::default().add_modifier(Modifier::BOLD);
            let guide = if self.expanded_children().is_some() {
                Line::from(vec![
                    Span::styled("<↑/↓>", bold),
                    Span::raw(": move  "),
                    Span::styled("<c>", bold),
                    Span::raw("/"),
                    Span::styled("<esc>", bold),
                    Span::raw(": collapse"),
                ])
            } else {
                Line::from(vec![
                    Span::styled("<esc>", bold),
                    Span::raw(": back  "),
                    Span::styled("<enter>", bold),
                    Span::raw(if settings.enter_accept {
                        ": resume  "
                    } else {
                        ": edit  "
                    }),
                    Span::styled("<tab>", bold),
                    Span::raw(": edit  "),
                    Span::styled("<ctrl-y>", bold),
                    Span::raw(": copy"),
                ])
            };
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

        let cells = row_layout(&settings.ai.sessions.columns, list_area.width);
        let list = SessionList {
            rows: &self.results,
            block: None,
            inverted: invert,
            alternate_highlight: self.keymap_mode == KeymapMode::VimNormal,
            now: (self.now)(),
            tz: settings.timezone.0,
            indicator: &indicator,
            theme,
            cells: &cells,
        };
        f.render_stateful_widget(list, list_area, &mut self.list);
        self.list_anchor = self.anchor(list_area, &cells, invert);

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
            self.draw_side(f, pane, settings.timezone.0, theme);
        }

        if compactness == Compactness::Ultracompact {
            return;
        }

        // Line the query up with the title column, as the history search lines it up with the
        // command, while the widest mode and count fit.
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
            prefix_width.max(u16::try_from("[ WORKSPACE 500+ ] ".len()).unwrap_or(19));
        f.render_widget(self.build_input(st, prefix_width, theme), input_chunk);

        // The text, and a column for the scrollbar: the right border, or one kept for it.
        let strip = Rect {
            x: preview_chunk.x + border_size,
            y: preview_chunk.y + border_size,
            width: preview_chunk.width.saturating_sub(2 * border_size + 1 - border_size),
            height: preview_chunk.height.saturating_sub(2 * border_size),
        };
        // Beside the pane (or with no room), the strip is only the box's bottom border.
        let (lines, scrolled) = if split || strip.height == 0 {
            (Vec::new(), None)
        } else {
            self.strip_lines(usize::from(strip.width), usize::from(strip.height), theme)
        };
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
        if let Some((session, top, body)) = scrolled {
            let top = u16::try_from(top).unwrap_or(0);
            let height = strip.height.saturating_sub(top);
            let track = Rect {
                x: preview_chunk.right().saturating_sub(1),
                y: strip.y + top,
                width: 1,
                height,
            };
            draw_scrollbar(f, track, &body, usize::from(height), theme);
            self.drawn(Pane::Strip, session, preview_chunk, usize::from(height), &body);
        }

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
        let text = match self.result_count() {
            None => String::new(),
            Some(n) if n == "1" => "1 session".to_owned(),
            Some(n) => format!("{n} sessions"),
        };
        Paragraph::new(text).style(style(theme, Meaning::Annotation)).alignment(Alignment::Right)
    }

    /// What the list holds, for the header and the mode prefix: `105`, or `500+` when the search
    /// stopped at its limit. `None` before the first results.
    fn result_count(&self) -> Option<String> {
        let n = self.results.len();
        (self.applied != 0).then(|| {
            if n >= SEARCH_LIMIT {
                format!("{SEARCH_LIMIT}+")
            } else {
                n.to_string()
            }
        })
    }

    fn build_input(&self, st: StyleState, prefix_width: u16, theme: &Theme) -> Paragraph<'static> {
        let mode = match self.result_count() {
            Some(n) => format!("{} {n}", self.mode_label()),
            None => self.mode_label().to_owned(),
        };
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

        // Where ctrl-r goes next, at the right while the query leaves room.
        if let Some(next) = self.next_mode() {
            let hint = format!("ctrl-r: {}", next.as_str().to_lowercase());
            let borders = if st.compactness == Compactness::Full {
                2
            } else {
                0
            };
            let room = st.inner_width.saturating_sub(borders);
            let used = usize::from(prefix_width) + input.width();
            if let Some(gap) = room.checked_sub(used + hint.width()).filter(|g| *g >= 2) {
                spans.push(Span::raw(" ".repeat(gap)));
                spans.push(Span::styled(hint, style(theme, Meaning::Annotation)));
            }
        }

        input_block(Paragraph::new(Line::from(spans)), st)
    }

    #[allow(clippy::too_many_lines)]
    fn draw_inspect(
        &mut self,
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

        let Some(row) = self.selected().cloned() else {
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
                if is_live(now, &row) {
                    spans.push(Span::styled("  ● live", style(theme, Meaning::AlertInfo)));
                }
                spans
            }),
            // The token counts share the message count's line, for the conversation's room.
            field("Messages", {
                let mut spans = text(row.messages.to_string());
                if let Some(t) = panel::tokens(&row.usage, true) {
                    spans.push(Span::styled(format!("  ·  {t} tokens"), key));
                }
                spans
            }),
        ];
        // A session that went on separately on several machines: its branches, newest first.
        for (n, head) in row.branches().iter().enumerate() {
            let mut spans = text(self.describe_head(head));
            if n == 0 {
                spans.push(Span::styled("  (latest)", key));
            }
            lines.push(field(
                if n == 0 {
                    "Branches"
                } else {
                    ""
                },
                spans,
            ));
        }
        if let Some(p) = self.previews.get(&row.handle).filter(|p| !p.activity.is_empty()) {
            let mut spans = vec![Span::styled(format!(" {:<10}", "Activity"), key)];
            let line = panel::activity_line(
                &p.activity,
                row.started_at,
                row.updated_at,
                width.min(60),
                theme,
            );
            spans.extend(line.spans);
            lines.push(Line::from(spans));
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

        // The forks, once read (a row with only subagents grouped under it has none).
        if let Some(forks) = self.children.get(&row.handle).filter(|c| !c.is_empty()).cloned() {
            let left = usize::from(inner.height).saturating_sub(lines.len());
            lines.extend(self.children_lines(&row, &forks, left, inner.width, tz, theme));
        }

        // The conversation, in whatever room is left, scrolling. Its scrollbar goes on the right
        // border, or in a column kept for it.
        let top = u16::try_from(lines.len()).unwrap_or(u16::MAX).min(inner.height);
        let left = usize::from(inner.height - top);
        let border = u16::from(st.compactness == Compactness::Full);
        let width = usize::from((inner.width + border).saturating_sub(1));
        let want = self.scrolls[Pane::Inspect as usize].offset_for(&row.handle);
        let body = scroll_body(
            want,
            left,
            || self.conversation(&row, width, left, 1, theme),
            |limit| self.conversation_document(&row, width, 1, limit, theme),
        );
        lines.extend(body.lines.iter().cloned());
        f.render_widget(Paragraph::new(Text::from(lines)), inner);

        let area = Rect {
            y: inner.y + top,
            height: inner.height - top,
            ..inner
        };
        let track = Rect {
            x: (inner.right() + border).saturating_sub(1),
            width: 1,
            ..area
        };
        draw_scrollbar(f, track, &body, left, theme);
        self.drawn(Pane::Inspect, row.handle.clone(), area, left, &body);
    }

    /// The detail pane beside the list, in `pane`: the session previewed (see
    /// [`State::preview_row`]), its conversation scrolling under what it is, with a scrollbar in
    /// the pane's right margin.
    fn draw_side(&mut self, f: &mut Frame, pane: Rect, tz: UtcOffset, theme: &Theme) {
        let text = pane.inner(ratatui::layout::Margin::new(1, 0));
        let Some(row) = self.preview_row().cloned() else {
            return;
        };
        let width = usize::from(text.width);
        let mut lines = self.detail_header(&row, width, tz, theme);
        let top = u16::try_from(lines.len()).unwrap_or(u16::MAX).min(text.height);
        let left = usize::from(text.height - top);
        if !self.previews.contains_key(&row.handle) {
            lines.push(Line::default());
            lines.push(Line::from(Span::styled("…", style(theme, Meaning::Annotation))));
            // Already wrapped (markdown keeps its indents, which the paragraph's wrap would trim).
            f.render_widget(Paragraph::new(Text::from(lines)), text);
            return;
        }
        let want = self.scrolls[Pane::Side as usize].offset_for(&row.handle);
        let body = scroll_body(
            want,
            left,
            || self.conversation(&row, width, left, 0, theme),
            |limit| self.conversation_document(&row, width, 0, limit, theme),
        );
        lines.extend(body.lines.iter().cloned());
        f.render_widget(Paragraph::new(Text::from(lines)), text);

        let track = Rect {
            x: pane.right().saturating_sub(1),
            y: text.y + top,
            width: 1,
            height: text.height - top,
        };
        draw_scrollbar(f, track, &body, left, theme);
        self.drawn(Pane::Side, row.handle, pane, left, &body);
    }

    /// Inspect's list of the forks grouped under `row`, in at most `room` lines: a blank line, a
    /// heading saying how many, and the tree. Collapsed, it shows the first few with a line
    /// saying how many more (`c` expands it), leaving the rest of the room to the conversation;
    /// expanded, it takes most of the room, scrolls, and has a cursor.
    fn children_lines(
        &mut self,
        row: &SessionRow,
        children: &[SessionRow],
        room: usize,
        width: u16,
        tz: UtcOffset,
        theme: &Theme,
    ) -> Vec<Line<'static>> {
        let key = style(theme, Meaning::Annotation);
        let heading = panel::forks(Some(children)).unwrap_or_default();
        let mut lines = vec![Line::default()];
        let tree =
            panel::tree_lines(&row.handle, children, (self.now)(), tz, usize::from(width), theme);
        let room = room.saturating_sub(2);
        let expanded = self.expanded_children().is_some();
        let mut heading =
            vec![Span::styled(format!(" {heading}"), key.add_modifier(Modifier::BOLD))];

        let shown: Vec<Line<'static>> =
            if let Some(view) = self.children_view.as_mut().filter(|_| expanded) {
                // Most of the room, leaving the conversation a few lines.
                let height = tree
                    .len()
                    .min(room.saturating_sub(INSPECT_CONVERSATION_MIN).max(CHILDREN_COLLAPSED));
                view.cursor = view.cursor.min(tree.len().saturating_sub(1));
                view.height = height.max(1);
                if view.cursor < view.offset {
                    view.offset = view.cursor;
                } else if view.cursor >= view.offset + view.height {
                    view.offset = view.cursor + 1 - view.height;
                }
                view.offset = view.offset.min(tree.len().saturating_sub(view.height));
                if tree.len() > height {
                    heading.push(Span::styled(
                        format!("  {}–{} of {}", view.offset + 1, view.offset + height, tree.len()),
                        key,
                    ));
                }
                let cursor = view.cursor;
                tree.into_iter()
                    .enumerate()
                    .skip(view.offset)
                    .take(height)
                    .map(|(i, line)| {
                        if i == cursor {
                            line.patch_style(Style::default().add_modifier(Modifier::REVERSED))
                        } else {
                            line
                        }
                    })
                    .collect()
            } else {
                // A few, with a line for the rest, keeping at least half the room for the
                // conversation.
                let fit = (room / 2).saturating_sub(1).clamp(1, CHILDREN_COLLAPSED);
                let n = if tree.len() <= fit + 1 {
                    tree.len().min(room)
                } else {
                    fit
                };
                let more = tree.len() - n;
                let mut shown: Vec<Line<'static>> = tree.into_iter().take(n).collect();
                if more > 0 {
                    shown.push(Line::from(Span::styled(
                        format!("   … and {more} more (c to expand)"),
                        key,
                    )));
                }
                shown
            };
        lines.push(Line::from(heading));
        lines.extend(shown);
        lines
    }
}

/// How many children Inspect shows before `… and N more`.
const CHILDREN_COLLAPSED: usize = 3;
/// The lines Inspect keeps for the conversation while the children list is expanded.
const INSPECT_CONVERSATION_MIN: usize = 4;

/// A full date and time, with what it doesn't already say beside it: `(12m ago)`,
/// `(yesterday)`, `(Monday)`.
fn format_when(ts: OffsetDateTime, now: OffsetDateTime, tz: UtcOffset) -> String {
    let at = ts.to_offset(tz).display().ymd_hm();
    match clock::beside_date(now, ts, tz) {
        Some(when) => format!("{at}  ({when})"),
        None => at.to_string(),
    }
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

#[cfg(test)]
mod tests {
    use rstest::rstest;

    use super::*;

    fn default_columns() -> Vec<AiSessionColumn> {
        atuin_client::settings::AiSessions::default().columns
    }

    use AiSessionColumn::{Harness, Messages, Time, Title};

    /// The title takes the rest of the row, whatever the width: 80 columns is 76 inside the box,
    /// and a split 120 leaves the list 69.
    #[rstest]
    #[case::wide(196, 174)]
    #[case::full_80(76, 54)]
    #[case::split_120(69, 47)]
    fn the_title_takes_the_rest(#[case] width: u16, #[case] title: u16) {
        assert_eq!(row_layout(&default_columns(), width), [
            (Time, 10),
            (Harness, 2),
            (Title, title),
            (Messages, 4)
        ]);
    }

    /// At every width the row fills it exactly, and the message count goes only when the title
    /// would otherwise be under [`TITLE_MIN`].
    #[rstest]
    fn every_width_fills_the_row_title_first() {
        for width in 30..=200u16 {
            let layout = row_layout(&default_columns(), width);
            let used: u16 = layout.iter().map(|(_, w)| w).sum::<u16>()
                + u16::try_from(layout.len() - 1).unwrap()
                + 3;
            let (_, title) = layout.iter().find(|(c, _)| *c == Title).copied().unwrap();
            if title > 0 {
                assert_eq!(used, width, "{width}: {layout:?}");
            }
            let counted = layout.iter().any(|(c, _)| *c == Messages);
            let gain = Messages.width() + 1;
            if counted {
                assert!(title >= TITLE_MIN, "{width}: {layout:?}");
            } else {
                assert!(title < TITLE_MIN + gain, "{width}: {layout:?}");
            }
        }
    }

    #[rstest]
    #[case("MacBook-Pro-3.local", "MacBook-Pro-3")]
    #[case("buildbox", "buildbox")]
    #[case("10.0.0.7", "10.0.0.7")]
    #[case(".weird", ".weird")]
    fn hosts_show_without_their_domain(#[case] name: &str, #[case] want: &str) {
        assert_eq!(short_host(name), want);
    }

    #[rstest]
    #[case(9_999, "9999")]
    #[case(12_345, "12k")]
    fn message_counts_fit_their_column(#[case] n: u64, #[case] want: &str) {
        assert_eq!(message_count(n), want);
        assert!(want.len() <= usize::from(AiSessionColumn::Messages.width()));
    }
}
