//! Markdown for the preview strip, the detail pane and the Inspect tab. Session text is mostly
//! markdown written by coding agents, so it is rendered (calmly, in the theme's colours) rather
//! than shown raw.
//!
//! [`render`] turns markdown into ratatui lines already wrapped to a width (callers must not wrap
//! them again) and cut to a line budget with a trailing `…`. [`render_flat`] squeezes the same
//! rendering into one line, for the one-line preview slots.
//!
//! Both carry the search highlights through. Highlights are byte ranges into the markdown
//! *source*, and every text event pulldown-cmark produces knows the source range it came from, so
//! a highlighted source byte stays highlighted wherever it lands in the rendering, whatever the
//! syntax around it did (see [`map_highlights`]).
//!
//! The chat UI renders its markdown with eye_declare's `Markdown` element, which draws straight
//! into a buffer and keeps its parser private, so there's nothing there to share.

use std::cell::RefCell;
use std::ops::Range;
use std::rc::Rc;

use atuin_client::theme::{Meaning, Theme};
use pulldown_cmark::{Alignment, Event, Options, Parser, Tag, TagEnd};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

use super::render::style;

/// The styles markdown renders in.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Styles {
    /// Running text; emphasis, headings and links are modifiers on it.
    pub base: Style,
    /// Bullets, gutters, rules, table borders, link targets, and the `…` of a cut.
    pub muted: Style,
    /// Inline code and code blocks.
    pub code: Style,
    /// Patched over the query's matches.
    pub highlight: Style,
}

impl Styles {
    pub fn new(theme: &Theme, base: Style) -> Self {
        Self {
            base,
            muted: style(theme, Meaning::Annotation),
            code: style(theme, Meaning::SyntaxCommand),
            highlight: style(theme, Meaning::AlertWarn).add_modifier(Modifier::BOLD),
        }
    }
}

/// How to lay a rendering out.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Opts {
    /// Columns per line. Nothing is wider.
    pub width: usize,
    /// At most this many lines, the last ending in `…` when there was more; 0 for no limit.
    pub max_lines: usize,
    /// A blank line between blocks. The preview strip packs them instead.
    pub spacing: bool,
    /// Link targets after their text, when they differ from it.
    pub urls: bool,
}

/// `source` rendered as markdown, wrapped to `opts.width` and cut to `opts.max_lines`, with the
/// `highlights` (byte ranges into `source`) drawn in [`Styles::highlight`]. Cached: rendering the
/// same text again (every frame, while it is selected) is a lookup.
pub fn render(
    source: &str,
    highlights: &[Range<usize>],
    opts: Opts,
    styles: &Styles,
) -> Vec<Line<'static>> {
    let lines = cached(source, highlights, opts, Mode::Block, styles, || {
        if opts.max_lines > 0
            && let Some(head) = head(source, opts.max_lines, opts.width)
        {
            let mut r = Renderer::new(head, highlights, opts, false, *styles);
            r.run();
            if r.full {
                return r.finish();
            }
        }
        let mut r = Renderer::new(source, highlights, opts, false, *styles);
        r.run();
        r.finish()
    });
    lines.as_ref().clone()
}

/// `source` rendered as markdown on a single line of at most `width` columns: blocks run on,
/// whitespace collapses, and a cut ends in `…`.
pub fn render_flat(
    source: &str,
    highlights: &[Range<usize>],
    width: usize,
    styles: &Styles,
) -> Line<'static> {
    let opts = Opts {
        width,
        max_lines: 1,
        spacing: false,
        urls: false,
    };
    let lines = cached(source, highlights, opts, Mode::Flat, styles, || {
        if let Some(head) = head(source, 3, width) {
            let mut r = Renderer::new(head, highlights, opts, true, *styles);
            r.run();
            if r.full {
                return vec![r.finish_flat()];
            }
        }
        let mut r = Renderer::new(source, highlights, opts, true, *styles);
        r.run();
        vec![r.finish_flat()]
    });
    lines.first().cloned().unwrap_or_default()
}

/// The first `opts.max_lines` lines of `source` rendered as markdown, for a view that scrolls
/// through it, and whether there are more. Unlike [`render`], the last line isn't cut short with
/// a `…`: rendering more lines later only adds lines after these, so a view can ask for more as
/// it scrolls without what it shows moving.
pub fn render_window(
    source: &str,
    highlights: &[Range<usize>],
    opts: Opts,
    styles: &Styles,
) -> (Vec<Line<'static>>, bool) {
    let limit = opts.max_lines;
    let lines = cached(source, highlights, opts, Mode::Window, styles, || {
        let finish = |mut r: Renderer<'_>| {
            // One line past the limit says there are more.
            r.out.truncate(limit.saturating_add(1));
            r.out
        };
        if limit > 0
            && let Some(head) = head(source, limit, opts.width)
        {
            let mut r = Renderer::new(head, highlights, opts, false, *styles);
            r.run();
            if r.full {
                return finish(r);
            }
        }
        let mut r = Renderer::new(source, highlights, opts, false, *styles);
        r.run();
        finish(r)
    });
    let more = limit > 0 && lines.len() > limit;
    let shown = if more {
        limit
    } else {
        lines.len()
    };
    (lines[..shown].to_vec(), more)
}

/// A prefix of `source` that very likely holds more than `lines` lines of `width` columns, cut
/// at a blank line so no paragraph, heading or table is split; `None` when that's all of it.
///
/// pulldown-cmark finds the block structure of the whole document before yielding its first
/// event, so rendering the top of a long reply costs as much as rendering all of it. A cut at a
/// blank line renders the same as the whole up to there (only a link to a reference defined
/// further down loses its target), and callers fall back to the whole text when the prefix
/// doesn't fill the lines.
fn head(source: &str, lines: usize, width: usize) -> Option<&str> {
    let min_lines = (lines + 1) * 4 + 16;
    let min_bytes = (lines + 1).saturating_mul(width).saturating_mul(2) + 1024;
    let (at, _) = source.match_indices('\n').nth(min_lines)?;
    let from = at.max(min_bytes);
    let blank = from + source.get(from..)?.find("\n\n")?;
    Some(&source[..=blank])
}

/// Plain (not markdown) `spans` word-wrapped to `width`, in at most `max_lines` lines (0 for no
/// limit), the last ending in `…` when cut.
pub fn wrap_plain(
    spans: &[Span<'_>],
    width: usize,
    max_lines: usize,
    muted: Style,
) -> Vec<Line<'static>> {
    let mut content: Vec<Span<'static>> = Vec::new();
    for span in spans {
        for g in span.content.graphemes(true) {
            let g = if g.chars().any(char::is_control) {
                " "
            } else {
                g
            };
            if g.width() > 0 {
                push_merged(&mut content, g, span.style);
            }
        }
    }
    let mut out = Vec::new();
    wrap(&[], &[], &content, width, true, muted, &mut |line| {
        out.push(line);
        max_lines == 0 || out.len() <= max_lines
    });
    fit(
        &out,
        if max_lines == 0 {
            out.len()
        } else {
            max_lines
        },
        width,
        muted,
    )
}

/// The first `n` of `lines`, the last of them ending in `…` when some were left out.
pub fn fit(lines: &[Line<'static>], n: usize, width: usize, muted: Style) -> Vec<Line<'static>> {
    let mut out: Vec<Line<'static>> = lines.iter().take(n).cloned().collect();
    if lines.len() > n {
        cut_off(&mut out, width, muted);
    }
    out
}

/// Mark `lines` as cut short: the last one ends in `…`, and one that would be a blank separator
/// (the start of whatever was left out) is dropped.
fn cut_off(lines: &mut Vec<Line<'static>>, width: usize, muted: Style) {
    let blank =
        |l: &Line<'_>| l.spans.iter().all(|s| s.content.trim_matches(['│', ' ']).is_empty());
    while lines.len() > 1 && lines.last().is_some_and(blank) {
        lines.pop();
    }
    if let Some(last) = lines.last_mut() {
        ellipsize(last, width, muted);
    }
}

/// Share `budget` lines between parts that want `wants` lines each: a line each in turn, so every
/// part gets its first line before any gets a second.
pub fn allocate(wants: &[usize], budget: usize) -> Vec<usize> {
    let mut got = vec![0; wants.len()];
    let mut left = budget;
    while left > 0 {
        let mut gave = false;
        for (g, want) in got.iter_mut().zip(wants) {
            if left > 0 && *g < *want {
                *g += 1;
                left -= 1;
                gave = true;
            }
        }
        if !gave {
            break;
        }
    }
    got
}

// --- the cache -------------------------------------------------------------------------------

/// Renderings kept, most recent first: the selected session's parts at the current width (a
/// block, a one-line form and a scrolling window for each), and a few recently selected ones for
/// moving back and forth.
const CACHE_SIZE: usize = 36;

/// Which rendering: [`render`], [`render_flat`] or [`render_window`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Mode {
    Block,
    Flat,
    Window,
}

/// What a rendering depends on. The source is kept whole and compared (a `memcmp`, cheaper than
/// hashing it), so a preview that reloads with new text can never be served a stale rendering.
struct Key {
    source: Box<str>,
    highlights: Box<[Range<usize>]>,
    opts: Opts,
    mode: Mode,
    styles: Styles,
}

impl Key {
    fn is(
        &self,
        source: &str,
        highlights: &[Range<usize>],
        opts: Opts,
        mode: Mode,
        styles: &Styles,
    ) -> bool {
        self.opts == opts
            && self.mode == mode
            && self.styles == *styles
            && *self.highlights == *highlights
            && *self.source == *source
    }
}

thread_local! {
    static CACHE: RefCell<Vec<(Key, Rc<Vec<Line<'static>>>)>> = const { RefCell::new(Vec::new()) };
}

fn cached(
    source: &str,
    highlights: &[Range<usize>],
    opts: Opts,
    mode: Mode,
    styles: &Styles,
    make: impl FnOnce() -> Vec<Line<'static>>,
) -> Rc<Vec<Line<'static>>> {
    CACHE.with_borrow_mut(|cache| {
        if let Some(at) =
            cache.iter().position(|(k, _)| k.is(source, highlights, opts, mode, styles))
        {
            let entry = cache.remove(at);
            let lines = Rc::clone(&entry.1);
            cache.insert(0, entry);
            return lines;
        }
        let lines = Rc::new(make());
        let key = Key {
            source: source.into(),
            highlights: highlights.into(),
            opts,
            mode,
            styles: *styles,
        };
        cache.insert(0, (key, Rc::clone(&lines)));
        cache.truncate(CACHE_SIZE);
        lines
    })
}

// --- rendering -------------------------------------------------------------------------------

/// Something the text is inside of, which puts a prefix on its lines.
enum Container {
    /// `│ ` on every line.
    Quote,
    /// A list item: its marker on its first line, then an indent as wide as the marker.
    Item {
        marker: Option<Span<'static>>,
        indent: usize,
    },
}

/// A table being collected, cell by cell.
struct Table {
    alignments: Vec<Alignment>,
    /// The header row first (when there is one).
    rows: Vec<Vec<Vec<Span<'static>>>>,
    header: bool,
}

struct Renderer<'a> {
    source: &'a str,
    highlights: &'a [Range<usize>],
    opts: Opts,
    /// Everything on one line (see [`render_flat`]).
    flat: bool,
    styles: Styles,

    containers: Vec<Container>,
    /// Each open list's next number, `None` for bullets.
    lists: Vec<Option<u64>>,
    /// The inline style stack; the top is the current style.
    inline: Vec<Style>,
    /// The block being built, one entry per hard line.
    block: Vec<Vec<Span<'static>>>,
    code: bool,
    /// The open link's target and text so far.
    link: Option<(String, String)>,
    table: Option<Table>,

    out: Vec<Line<'static>>,
    /// A blank line to put before the next line, if another comes.
    blank: Option<Line<'static>>,
    /// More lines than wanted have been made; stop.
    full: bool,
}

const BULLETS: [&str; 2] = ["•", "◦"];

impl<'a> Renderer<'a> {
    fn new(
        source: &'a str,
        highlights: &'a [Range<usize>],
        opts: Opts,
        flat: bool,
        styles: Styles,
    ) -> Self {
        Self {
            source,
            highlights,
            opts,
            flat,
            styles,
            containers: Vec::new(),
            lists: Vec::new(),
            inline: vec![styles.base],
            block: Vec::new(),
            code: false,
            link: None,
            table: None,
            out: Vec::new(),
            blank: None,
            full: false,
        }
    }

    fn run(&mut self) {
        if self.opts.width == 0 {
            return;
        }
        let options =
            Options::ENABLE_TABLES | Options::ENABLE_STRIKETHROUGH | Options::ENABLE_TASKLISTS;
        for (event, range) in Parser::new_ext(self.source, options).into_offset_iter() {
            if self.full {
                return;
            }
            match event {
                Event::Start(tag) => self.start(tag),
                Event::End(tag) => self.end(tag),
                Event::Text(text) => {
                    let style = if self.code {
                        self.current().patch(self.styles.code)
                    } else {
                        self.current()
                    };
                    self.text(&text, range, style);
                }
                Event::Code(text) => {
                    let style = self.current().patch(self.styles.code);
                    self.text(&text, range, style);
                }
                Event::Html(text)
                | Event::InlineHtml(text)
                | Event::InlineMath(text)
                | Event::DisplayMath(text)
                | Event::FootnoteReference(text) => self.text(&text, range, self.current()),
                Event::SoftBreak => self.push(" ", self.current()),
                Event::HardBreak => self.block.push(Vec::new()),
                Event::Rule => {
                    self.flush();
                    let width = self.opts.width.min(24);
                    self.push(&"─".repeat(width), self.styles.muted);
                    self.flush();
                    self.gap();
                }
                Event::TaskListMarker(done) => {
                    self.push(
                        if done {
                            "[x] "
                        } else {
                            "[ ] "
                        },
                        self.styles.muted,
                    );
                }
            }
        }
        self.flush();
    }

    fn current(&self) -> Style {
        self.inline.last().copied().unwrap_or(self.styles.base)
    }

    fn push_style(&mut self, f: impl FnOnce(Style) -> Style) {
        self.inline.push(f(self.current()));
    }

    fn pop_style(&mut self) {
        if self.inline.len() > 1 {
            self.inline.pop();
        }
    }

    fn start(&mut self, tag: Tag<'_>) {
        match tag {
            Tag::Heading { .. } => {
                self.flush();
                self.push_style(|s| s.add_modifier(Modifier::BOLD));
            }
            Tag::BlockQuote(_) => {
                self.flush();
                self.containers.push(Container::Quote);
            }
            Tag::CodeBlock(_) => {
                self.flush();
                self.code = true;
            }
            Tag::List(start) => {
                self.flush();
                self.lists.push(start);
            }
            Tag::Item => {
                self.flush();
                let depth = self.lists.len().saturating_sub(1);
                let marker = match self.lists.last_mut() {
                    Some(Some(n)) => {
                        let marker = format!("{n}. ");
                        *n = n.saturating_add(1);
                        marker
                    }
                    _ => format!("{} ", BULLETS[depth % BULLETS.len()]),
                };
                let indent = marker.width();
                self.containers.push(Container::Item {
                    marker: Some(Span::styled(marker, self.styles.muted)),
                    indent,
                });
            }
            Tag::Table(alignments) => {
                self.flush();
                self.table = Some(Table {
                    alignments,
                    rows: Vec::new(),
                    header: false,
                });
            }
            Tag::TableHead | Tag::TableRow => {
                if let Some(table) = &mut self.table {
                    table.header |= matches!(tag, Tag::TableHead) && table.rows.is_empty();
                    table.rows.push(Vec::new());
                }
            }
            Tag::TableCell => self.block.clear(),
            Tag::Emphasis => self.push_style(|s| s.add_modifier(Modifier::ITALIC)),
            Tag::Strong => self.push_style(|s| s.add_modifier(Modifier::BOLD)),
            Tag::Strikethrough => self.push_style(|s| s.add_modifier(Modifier::CROSSED_OUT)),
            Tag::Link { dest_url, .. } => {
                self.push_style(|s| s.add_modifier(Modifier::UNDERLINED));
                self.link = Some((dest_url.into_string(), String::new()));
            }
            Tag::Image { .. } => {
                self.push("[", self.styles.muted);
                let muted = self.styles.muted;
                self.push_style(|s| s.patch(muted));
            }
            Tag::Paragraph
            | Tag::HtmlBlock
            | Tag::FootnoteDefinition(_)
            | Tag::DefinitionList
            | Tag::DefinitionListTitle
            | Tag::DefinitionListDefinition
            | Tag::MetadataBlock(_) => self.flush(),
            Tag::Superscript | Tag::Subscript => self.push_style(|s| s),
        }
    }

    fn end(&mut self, tag: TagEnd) {
        match tag {
            TagEnd::Heading(_) => {
                self.flush();
                self.pop_style();
                self.gap();
            }
            TagEnd::BlockQuote(_) => {
                self.flush();
                self.containers.pop();
                self.gap();
            }
            TagEnd::CodeBlock => {
                self.flush();
                self.code = false;
                self.gap();
            }
            TagEnd::List(_) => {
                self.flush();
                self.lists.pop();
                self.gap();
            }
            TagEnd::Item => {
                self.flush();
                self.containers.pop();
            }
            TagEnd::TableCell => {
                let mut cell: Vec<Span<'static>> = Vec::new();
                for (i, line) in std::mem::take(&mut self.block).into_iter().enumerate() {
                    if i > 0 {
                        push_merged(&mut cell, " ", self.styles.base);
                    }
                    for span in line {
                        push_merged(&mut cell, &span.content, span.style);
                    }
                }
                if let Some(row) = self.table.as_mut().and_then(|t| t.rows.last_mut()) {
                    row.push(cell);
                }
            }
            TagEnd::Table => {
                if let Some(table) = self.table.take() {
                    self.table_lines(&table);
                }
                self.gap();
            }
            TagEnd::Emphasis
            | TagEnd::Strong
            | TagEnd::Strikethrough
            | TagEnd::Superscript
            | TagEnd::Subscript => self.pop_style(),
            TagEnd::Link => {
                self.pop_style();
                if let Some((url, text)) = self.link.take()
                    && self.opts.urls
                    && !url.is_empty()
                    && !url.starts_with('#')
                    && url.trim_start_matches("mailto:") != text
                {
                    self.push(&format!(" ({url})"), self.styles.muted);
                }
            }
            TagEnd::Image => {
                self.pop_style();
                self.push("]", self.styles.muted);
            }
            TagEnd::Paragraph | TagEnd::HtmlBlock | TagEnd::FootnoteDefinition => {
                self.flush();
                self.gap();
            }
            TagEnd::TableHead
            | TagEnd::TableRow
            | TagEnd::DefinitionList
            | TagEnd::DefinitionListTitle
            | TagEnd::DefinitionListDefinition
            | TagEnd::MetadataBlock(_) => self.flush(),
        }
    }

    /// Append `s` to the block without highlights (markers, link targets).
    fn push(&mut self, s: &str, style: Style) {
        if self.block.is_empty() {
            self.block.push(Vec::new());
        }
        if let Some(line) = self.block.last_mut() {
            push_merged(line, s, style);
        }
    }

    /// Append source text `text`, which came from `range` of the source, carrying highlights.
    /// Newlines become hard lines; tabs, control characters and zero-width leftovers are made
    /// safe to put in a terminal cell.
    fn text(&mut self, text: &str, range: Range<usize>, style: Style) {
        let marks = map_highlights(self.source, self.highlights, text, range);
        if let Some((_, link_text)) = &mut self.link {
            link_text.push_str(text);
        }
        if self.block.is_empty() {
            self.block.push(Vec::new());
        }
        let hl = style.patch(self.styles.highlight);
        for (at, g) in text.grapheme_indices(true) {
            if g == "\n" || g == "\r\n" {
                self.block.push(Vec::new());
                continue;
            }
            let style = if marks.iter().any(|r| r.contains(&at)) {
                hl
            } else {
                style
            };
            let g = if g == "\t" {
                "    "
            } else if g == "\r" {
                continue;
            } else if g.chars().any(char::is_control) {
                "\u{FFFD}"
            } else if g.width() == 0 {
                continue;
            } else {
                g
            };
            if let Some(line) = self.block.last_mut() {
                push_merged(line, g, style);
            }
        }
    }

    /// The prefixes for the lines of a block starting now: the first line's (which takes any
    /// pending list markers) and the following lines'.
    fn prefixes(&mut self) -> (Vec<Span<'static>>, Vec<Span<'static>>) {
        let mut first = Vec::new();
        let mut rest = Vec::new();
        for container in &mut self.containers {
            match container {
                Container::Quote if !self.flat => {
                    first.push(Span::styled("│ ", self.styles.muted));
                    rest.push(Span::styled("│ ", self.styles.muted));
                }
                Container::Quote => {}
                Container::Item { marker, indent } => {
                    if let Some(marker) = marker.take() {
                        first.push(marker);
                    } else if !self.flat {
                        first.push(Span::raw(" ".repeat(*indent)));
                    }
                    if !self.flat {
                        rest.push(Span::raw(" ".repeat(*indent)));
                    }
                }
            }
        }
        (first, rest)
    }

    /// Lay the block built so far out into lines.
    fn flush(&mut self) {
        let blank = |line: &Vec<Span<'static>>| line.iter().all(|s| s.content.trim().is_empty());
        let mut block = std::mem::take(&mut self.block);
        // No blank lines at either end, and never two in a row.
        block.dedup_by(|a, b| blank(a) && blank(b));
        while block.last().is_some_and(blank) {
            block.pop();
        }
        let lead = block.iter().take_while(|l| blank(l)).count();
        block.drain(..lead);
        if block.is_empty() {
            return;
        }
        let (mut first, mut rest) = self.prefixes();
        if self.code && !self.flat {
            first.push(Span::raw("  "));
            rest.push(Span::raw("  "));
        }
        self.emit_block(&first, &rest, &block, !self.code);
    }

    fn emit_block(
        &mut self,
        first: &[Span<'static>],
        rest: &[Span<'static>],
        block: &[Vec<Span<'static>>],
        wrapped: bool,
    ) {
        if self.flat {
            let mut line = Vec::new();
            line.extend(first.iter().cloned());
            for (i, hard) in block.iter().enumerate() {
                if i > 0 {
                    line.push(Span::raw(" "));
                }
                line.extend(hard.iter().cloned());
            }
            if !self.out.is_empty() {
                self.out.push(Line::raw(" "));
            }
            self.out.push(Line::from(line));
            // Enough to fill the line and show it was cut.
            let wide: usize = self.out.iter().map(Line::width).sum();
            self.full = wide > self.opts.width.saturating_mul(2).saturating_add(8);
            return;
        }
        let Self {
            out,
            blank,
            full,
            opts,
            styles,
            ..
        } = self;
        for (i, hard) in block.iter().enumerate() {
            if *full {
                return;
            }
            let lead = if i == 0 {
                first
            } else {
                rest
            };
            wrap(lead, rest, hard, opts.width, wrapped, styles.muted, &mut |line| {
                if let Some(blank) = blank.take() {
                    out.push(blank);
                }
                out.push(line);
                *full = opts.max_lines != 0 && out.len() > opts.max_lines;
                !*full
            });
        }
    }

    /// End of a block: put a blank line before the next one (in the roomy views, and not between
    /// list items).
    fn gap(&mut self) {
        if !self.opts.spacing
            || self.flat
            || self.out.is_empty()
            || self.containers.iter().any(|c| matches!(c, Container::Item { .. }))
        {
            return;
        }
        let quoted = self.containers.iter().filter(|c| matches!(c, Container::Quote)).count();
        let gutter = "│ ".repeat(quoted);
        self.blank =
            Some(Line::from(Span::styled(gutter.trim_end().to_owned(), self.styles.muted)));
    }

    /// A table as an aligned grid, or row by row when the columns don't fit.
    fn table_lines(&mut self, table: &Table) {
        let (first, rest) = self.prefixes();
        let cols = table.rows.iter().map(Vec::len).max().unwrap_or(0).max(table.alignments.len());
        if cols == 0 {
            return;
        }
        let bold = |cell: &[Span<'static>]| -> Vec<Span<'static>> {
            cell.iter()
                .map(|s| Span::styled(s.content.clone(), s.style.add_modifier(Modifier::BOLD)))
                .collect()
        };
        let sep = Span::styled(" │ ", self.styles.muted);
        let avail =
            self.opts.width.saturating_sub(spans_width(&fit_prefix(&rest, self.opts.width)));
        let seps = 3 * (cols - 1);
        let natural: Vec<usize> = (0..cols)
            .map(|c| {
                table
                    .rows
                    .iter()
                    .filter_map(|r| r.get(c))
                    .map(|s| spans_width(s))
                    .max()
                    .unwrap_or(0)
            })
            .collect();

        if self.flat || avail < seps + 3 * cols {
            // Too narrow for a grid: each row on its own, cells run together.
            let rows: Vec<Vec<Span<'static>>> = table
                .rows
                .iter()
                .enumerate()
                .map(|(i, row)| {
                    let mut line = Vec::new();
                    for (c, cell) in row.iter().enumerate() {
                        if c > 0 {
                            line.push(sep.clone());
                        }
                        if i == 0 && table.header {
                            line.extend(bold(cell));
                        } else {
                            line.extend(cell.iter().cloned());
                        }
                    }
                    line
                })
                .collect();
            self.emit_block(&first, &rest, &rows, true);
            return;
        }

        // The widest cap on column widths that fits; narrow columns keep their width.
        let budget = avail - seps;
        let fits = |cap: usize| natural.iter().map(|w| (*w).min(cap)).sum::<usize>() <= budget;
        let (mut lo, mut hi) = (3, natural.iter().copied().max().unwrap_or(3).max(3));
        while lo < hi {
            let mid = (lo + hi).div_ceil(2);
            if fits(mid) {
                lo = mid;
            } else {
                hi = mid - 1;
            }
        }
        let widths: Vec<usize> = natural.iter().map(|w| (*w).min(lo)).collect();

        let mut lines: Vec<Vec<Span<'static>>> = Vec::new();
        for (i, row) in table.rows.iter().enumerate() {
            let header = i == 0 && table.header;
            let mut line = Vec::new();
            for (c, width) in widths.iter().enumerate() {
                if c > 0 {
                    line.push(sep.clone());
                }
                let cell = row.get(c).map(Vec::as_slice).unwrap_or_default();
                let cell = if header {
                    bold(cell)
                } else {
                    cell.to_vec()
                };
                let cell = truncate(&cell, *width, self.styles.muted);
                let pad = width.saturating_sub(spans_width(&cell));
                let (left, right) = match table.alignments.get(c) {
                    Some(Alignment::Right) => (pad, 0),
                    Some(Alignment::Center) => (pad / 2, pad - pad / 2),
                    _ => (0, pad),
                };
                if left > 0 {
                    line.push(Span::raw(" ".repeat(left)));
                }
                line.extend(cell);
                if right > 0 && c + 1 < cols {
                    line.push(Span::raw(" ".repeat(right)));
                }
            }
            lines.push(line);
            if header {
                let rule: Vec<String> = widths.iter().map(|w| "─".repeat(*w)).collect();
                lines.push(vec![Span::styled(rule.join("─┼─"), self.styles.muted)]);
            }
        }
        self.emit_block(&first, &rest, &lines, false);
    }

    /// The lines, cut to the budget.
    fn finish(mut self) -> Vec<Line<'static>> {
        if self.full && self.opts.max_lines > 0 {
            self.out.truncate(self.opts.max_lines);
            cut_off(&mut self.out, self.opts.width, self.styles.muted);
        }
        self.out
    }

    /// Everything on one line, whitespace collapsed, cut to the width.
    fn finish_flat(self) -> Line<'static> {
        let mut spans: Vec<Span<'static>> = Vec::new();
        let mut space = true;
        for span in self.out.iter().flat_map(|l| l.spans.iter()) {
            for g in span.content.graphemes(true) {
                if g.chars().all(char::is_whitespace) {
                    if !space {
                        push_merged(&mut spans, " ", span.style);
                    }
                    space = true;
                } else {
                    push_merged(&mut spans, g, span.style);
                    space = false;
                }
            }
        }
        if let Some(last) = spans.last_mut() {
            let trimmed = last.content.trim_end().len();
            last.content.to_mut().truncate(trimmed);
        }
        Line::from(truncate(&spans, self.opts.width, self.styles.muted))
    }
}

/// Where `highlights` (byte ranges into `source`) fall in `text`, an event's text that came from
/// `range` of the source.
///
/// Usually the text *is* that slice of the source, or sits inside it (a code span's range includes
/// its backticks, an escape's its backslash), and the ranges carry over by offset. When the text
/// was transformed (entities, `&amp;` → `&`), the highlighted source text is looked for in it
/// instead, so a match is still marked whenever it is still there to see.
fn map_highlights(
    source: &str,
    highlights: &[Range<usize>],
    text: &str,
    range: Range<usize>,
) -> Vec<Range<usize>> {
    let overlapping: Vec<&Range<usize>> =
        highlights.iter().filter(|r| r.start < range.end && r.end > range.start).collect();
    if overlapping.is_empty() || text.is_empty() {
        return Vec::new();
    }
    let slice = source.get(range.clone()).unwrap_or_default();
    let offset = if slice == text {
        Some(0)
    } else {
        slice.find(text)
    };
    match offset {
        Some(at) => {
            let base = range.start + at;
            overlapping
                .into_iter()
                .filter_map(|r| {
                    let start = r.start.max(base) - base;
                    let end = r.end.min(base + text.len()).saturating_sub(base);
                    (start < end).then_some(start..end)
                })
                .collect()
        }
        None => overlapping
            .into_iter()
            .filter_map(|r| source.get(r.start.max(range.start)..r.end.min(range.end)))
            .filter(|needle| !needle.trim().is_empty())
            .flat_map(|needle| {
                text.match_indices(needle).map(|(at, m)| at..at + m.len()).collect::<Vec<_>>()
            })
            .collect(),
    }
}

// --- wrapping --------------------------------------------------------------------------------

fn spans_width(spans: &[Span<'_>]) -> usize {
    spans.iter().map(|s| s.content.width()).sum()
}

/// Append `s` in `style`, merging into the last span when the style matches.
fn push_merged(spans: &mut Vec<Span<'static>>, s: &str, style: Style) {
    if let Some(last) = spans.last_mut()
        && last.style == style
    {
        last.content.to_mut().push_str(s);
    } else {
        spans.push(Span::styled(s.to_owned(), style));
    }
}

/// `spans` cut to at most `width` columns.
fn cut(spans: &[Span<'static>], width: usize) -> Vec<Span<'static>> {
    let mut out = Vec::new();
    let mut used = 0;
    for span in spans {
        for g in span.content.graphemes(true) {
            let w = g.width();
            if used + w > width {
                return out;
            }
            used += w;
            push_merged(&mut out, g, span.style);
        }
    }
    out
}

/// `spans` cut to `width` columns, ending in `…` when anything was cut.
pub fn truncate(spans: &[Span<'static>], width: usize, muted: Style) -> Vec<Span<'static>> {
    if spans_width(spans) <= width {
        return spans.to_vec();
    }
    let mut out = cut(spans, width.saturating_sub(1));
    if width > 0 {
        out.push(Span::styled("…", muted));
    }
    out
}

/// End `line` (at most `width` wide) with `…`, making room for it if it's full.
fn ellipsize(line: &mut Line<'static>, width: usize, muted: Style) {
    if width == 0 || line.spans.last().is_some_and(|s| s.content.ends_with('…')) {
        return;
    }
    let mut spans = cut(&line.spans, width - 1);
    while let Some(last) = spans.last_mut() {
        let trimmed = last.content.trim_end().len();
        if trimmed > 0 {
            last.content.to_mut().truncate(trimmed);
            break;
        }
        spans.pop();
    }
    spans.push(Span::styled("…", muted));
    line.spans = spans;
}

/// `prefix`, cut so it takes at most half of `width`: deep nesting squeezes its indent rather
/// than the text.
fn fit_prefix(prefix: &[Span<'static>], width: usize) -> Vec<Span<'static>> {
    let max = width / 2;
    if spans_width(prefix) <= max {
        return prefix.to_vec();
    }
    let mut out = Vec::new();
    let mut used = 0;
    'outer: for span in prefix {
        for g in span.content.graphemes(true) {
            if used + g.width() > max {
                break 'outer;
            }
            used += g.width();
            push_merged(&mut out, g, span.style);
        }
    }
    out
}

/// Word-wrap one hard line of `content` to `width` columns, the first line after `first` and the
/// rest after `rest`, handing each line to `emit` until it returns false. Unwrapped content (code)
/// is cut with `…` instead, keeping its whitespace.
fn wrap(
    first: &[Span<'static>],
    rest: &[Span<'static>],
    content: &[Span<'static>],
    width: usize,
    wrapped: bool,
    muted: Style,
    emit: &mut dyn FnMut(Line<'static>) -> bool,
) {
    if width == 0 {
        return;
    }
    let first = fit_prefix(first, width);
    let rest = fit_prefix(rest, width);

    struct Cell<'c> {
        g: &'c str,
        style: Style,
        width: usize,
    }
    let is_space = |c: &Cell<'_>| c.g.chars().all(char::is_whitespace);
    let line_of = |prefix: &[Span<'static>], cells: &[Cell<'_>]| {
        let mut spans = prefix.to_vec();
        let mut body = Vec::new();
        for c in cells {
            push_merged(&mut body, c.g, c.style);
        }
        spans.extend(body);
        Line::from(spans)
    };

    let mut prefix = first.as_slice();
    let mut on_first = true;
    let mut emitted = false;
    let mut avail = width.saturating_sub(spans_width(prefix));
    let mut cur: Vec<Cell<'_>> = Vec::new();
    let mut cur_width = 0;
    // Index in `cur` of the last space, where the line can break.
    let mut space: Option<usize> = None;

    let cells = content.iter().flat_map(|s| {
        s.content.graphemes(true).map(move |g| Cell {
            g,
            style: s.style,
            width: g.width(),
        })
    });
    for cell in cells {
        if cur_width + cell.width <= avail {
            if cur.is_empty() && wrapped && is_space(&cell) && !on_first {
                // No leading spaces on a wrapped line.
                continue;
            }
            if is_space(&cell) {
                space = Some(cur.len());
            }
            cur_width += cell.width;
            cur.push(cell);
            continue;
        }

        if !wrapped {
            // Code keeps its lines: cut, and skip the rest of this one.
            let mut line = line_of(prefix, &cur);
            ellipsize(&mut line, width, muted);
            emit(line);
            return;
        }

        // Break at the last space if there is one, else mid-word.
        let carry = match space {
            Some(at) if !is_space(&cell) => cur.split_off(at + 1),
            _ => Vec::new(),
        };
        while cur.last().is_some_and(is_space) {
            cur.pop();
        }
        // A line of nothing but the spaces it broke at isn't worth showing.
        if !cur.is_empty() {
            if !emit(line_of(prefix, &cur)) {
                return;
            }
            emitted = true;
            prefix = rest.as_slice();
            on_first = false;
            avail = width.saturating_sub(spans_width(prefix));
        }
        cur = carry;
        cur_width = cur.iter().map(|c| c.width).sum();
        space = None;
        if is_space(&cell) {
            continue;
        }
        if cur_width + cell.width > avail {
            // A word as wide as the line: break it here.
            if !cur.is_empty() {
                if !emit(line_of(prefix, &cur)) {
                    return;
                }
                emitted = true;
                cur.clear();
                cur_width = 0;
            }
            if cell.width > avail {
                // Wider than a whole line (a wide char in one column): drop it.
                continue;
            }
        }
        cur_width += cell.width;
        cur.push(cell);
    }
    if wrapped {
        while cur.last().is_some_and(is_space) {
            cur.pop();
        }
    }
    // An empty line only when it's the whole of an (intentionally) empty hard line.
    if !cur.is_empty() || !emitted {
        emit(line_of(prefix, &cur));
    }
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;
    use ratatui::style::Color;
    use rstest::rstest;

    use super::*;

    fn styles() -> Styles {
        Styles {
            base: Style::default(),
            muted: Style::default().fg(Color::DarkGray),
            code: Style::default().fg(Color::Green),
            highlight: Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD),
        }
    }

    fn opts(width: usize, max_lines: usize) -> Opts {
        Opts {
            width,
            max_lines,
            spacing: true,
            urls: true,
        }
    }

    fn md(source: &str, width: usize, max_lines: usize) -> Vec<Line<'static>> {
        render(source, &[], opts(width, max_lines), &styles())
    }

    fn plain(lines: &[Line<'_>]) -> String {
        lines.iter().map(ToString::to_string).collect::<Vec<_>>().join("\n")
    }

    /// The text drawn in `style`, run by run.
    fn styled(lines: &[Line<'_>], style: Style) -> Vec<String> {
        lines
            .iter()
            .flat_map(|l| l.spans.iter())
            .filter(|s| s.style == style)
            .map(|s| s.content.to_string())
            .collect()
    }

    fn highlighted(lines: &[Line<'_>]) -> Vec<String> {
        lines
            .iter()
            .flat_map(|l| l.spans.iter())
            .filter(|s| s.style.fg == Some(Color::Yellow))
            .map(|s| s.content.to_string())
            .collect()
    }

    /// Byte ranges of every `needle` in `haystack`.
    fn ranges(haystack: &str, needle: &str) -> Vec<Range<usize>> {
        haystack.match_indices(needle).map(|(at, m)| at..at + m.len()).collect()
    }

    #[rstest]
    fn headings_emphasis_and_code_are_styled_not_marked_up() {
        let lines = md("## Plan\n\nUse **bold**, *italic* and `cargo test`.", 40, 0);
        assert_eq!(plain(&lines), "Plan\n\nUse bold, italic and cargo test.");
        let bold = Style::default().add_modifier(Modifier::BOLD);
        assert_eq!(styled(&lines, bold), ["Plan", "bold"]);
        assert_eq!(styled(&lines, Style::default().add_modifier(Modifier::ITALIC)), ["italic"]);
        assert_eq!(styled(&lines, styles().code), ["cargo test"]);
    }

    #[rstest]
    fn list_items_hang_under_their_markers() {
        let source = "- one short item\n- a much longer item that has to wrap onto more \
                      lines\n\n1. first\n2. second\n   - nested bullet";
        assert_eq!(
            plain(&md(source, 24, 0)),
            "• one short item\n• a much longer item\n  that has to wrap onto\n  more lines\n\n1. \
             first\n2. second\n   ◦ nested bullet"
        );
    }

    #[rstest]
    fn code_blocks_keep_their_whitespace_and_are_cut_not_wrapped() {
        let source = "Run:\n\n```rust\nfn main() {\n    println!(\"a very long line \
                      indeed\");\n}\n```\n\nDone.";
        let lines = md(source, 24, 0);
        assert_eq!(plain(&lines), "Run:\n\n  fn main() {\n      println!(\"a very…\n  }\n\nDone.");
        assert_eq!(styled(&lines, styles().code)[0], "fn main() {");
    }

    #[rstest]
    fn unterminated_fences_run_to_the_end() {
        assert_eq!(plain(&md("text\n```\ncode\n  more", 20, 0)), "text\n\n  code\n    more");
    }

    #[rstest]
    fn quotes_get_a_gutter() {
        assert_eq!(
            plain(&md("> quoted text that wraps around\n>\n> second", 16, 0)),
            "│ quoted text\n│ that wraps\n│ around\n│\n│ second"
        );
    }

    #[rstest]
    fn links_show_their_target_when_asked() {
        let source = "See [the docs](https://docs.atuin.sh) or <https://atuin.sh>.";
        assert_eq!(
            plain(&md(source, 80, 0)),
            "See the docs (https://docs.atuin.sh) or https://atuin.sh."
        );
        let no_urls = Opts {
            urls: false,
            ..opts(80, 0)
        };
        let lines = render(source, &[], no_urls, &styles());
        assert_eq!(plain(&lines), "See the docs or https://atuin.sh.");
        let link = Style::default().add_modifier(Modifier::UNDERLINED);
        assert_eq!(styled(&lines, link), ["the docs", "https://atuin.sh"]);
    }

    #[rstest]
    fn tables_align_and_shrink_to_fit() {
        let source = "| Crate | Tests | Notes |\n|:--|--:|:-:|\n| atuin-ai | 412 | ok |\n| \
                      atuin-client | 97 | a much longer note |";
        assert_eq!(
            plain(&md(source, 60, 0)),
            "Crate        │ Tests │       \
             Notes\n─────────────┼───────┼───────────────────\natuin-ai     │   412 │         \
             ok\natuin-client │    97 │ a much longer note"
        );
        assert_eq!(
            plain(&md(source, 30, 0)),
            "Crate     │ Tests │   Notes\n──────────┼───────┼──────────\natuin-ai  │   412 │    \
             ok\natuin-cl… │    97 │ a much l…"
        );
        // Too narrow for a grid: row by row.
        assert_eq!(
            plain(&md(source, 12, 0)),
            "Crate │\nTests │\nNotes\natuin-ai │\n412 │ ok\natuin-client\n│ 97 │ a\nmuch \
             longer\nnote"
        );
    }

    #[rstest]
    fn long_text_is_cut_to_the_budget_with_an_ellipsis() {
        let source = "# Title\n\nOne two three four five six seven eight nine ten.\n\nMore.";
        let lines = md(source, 12, 3);
        assert_eq!(plain(&lines), "Title\n\nOne two…");
        assert!(lines.iter().all(|l| l.width() <= 12));
        // Exactly fitting text isn't marked as cut.
        assert_eq!(plain(&md("one\n\ntwo", 12, 3)), "one\n\ntwo");
    }

    #[rstest]
    #[case::paragraphs("Some text that wraps a little at forty columns, twice over.\n\n")]
    #[case::code("```\nfn a() {}\n\n\n\nfn b() {}\n```\n\n")]
    #[case::setext("A heading\n=========\n\nand text\n\n")]
    #[case::blank_heavy("x\n\n\n\n\n\n\n\n\n\n\n\n")]
    fn long_texts_render_their_top_the_same_as_the_whole(#[case] unit: &str) {
        let source = unit.repeat(400);
        let whole = md(&source, 40, 0);
        for max in [1, 5, 14] {
            let top = md(&source, 40, max);
            // A cut drops a blank separator it would have ended on.
            let n = top.len();
            assert!(n == max || n == max - 1, "{max}: {n}");
            assert_eq!(top[..n - 1], whole[..n - 1], "{max}");
            assert!(top[n - 1].to_string().ends_with('…'));
        }
    }

    #[rstest]
    fn blank_lines_collapse() {
        let source = "\n\n\npara\n\n\n\n\n---\n\n\n\n> \n\n\n\n```\n```\n\npara\n\n\n";
        assert_eq!(plain(&md(source, 40, 0)), "para\n\n────────────────────────\n\npara");
        let packed = Opts {
            spacing: false,
            ..opts(40, 0)
        };
        assert_eq!(plain(&render("a\n\n\nb\n\n# c", &[], packed, &styles())), "a\nb\nc");
    }

    #[rstest]
    fn flat_rendering_runs_blocks_onto_one_line() {
        let source = "## Fixed\n\n- the `sync` test\n- the **flaky** one\n\n```\nfoo   bar\n```";
        let line = render_flat(source, &[], 80, &styles());
        assert_eq!(line.to_string(), "Fixed • the sync test • the flaky one foo bar");
        let cut = render_flat(source, &[], 16, &styles());
        assert_eq!(cut.to_string(), "Fixed • the syn…");
        assert_eq!(cut.width(), 16);
    }

    #[rstest]
    #[case::plain_text("the flaky sync test", "flaky", &["flaky"])]
    #[case::bold("the **flaky** test", "flaky", &["flaky"])]
    #[case::inline_code("run `cargo test sync` again", "sync", &["sync"])]
    #[case::code_block("x\n\n```\nlet sync = 1;\n```", "sync", &["sync"])]
    #[case::list_item("- one\n- fix sync\n", "sync", &["sync"])]
    #[case::heading("# Sync notes", "Sync", &["Sync"])]
    #[case::link_text("see [sync docs](https://x.y/sync)", "sync", &["sync"])]
    #[case::escaped("a \\*sync\\* b", "sync", &["sync"])]
    #[case::entity("fish &amp; sync", "&amp;", &["&"])]
    #[case::table("| a | b |\n|---|---|\n| sync | x |", "sync", &["sync"])]
    #[case::cjk("同步テスト sync 😀", "sync", &["sync"])]
    fn highlights_follow_the_text_through_the_markup(
        #[case] source: &str,
        #[case] term: &str,
        #[case] want: &[&str],
    ) {
        let hl = ranges(source, term);
        let lines = render(source, &hl, opts(80, 0), &styles());
        assert_eq!(highlighted(&lines), want, "{}", plain(&lines));
        let flat = render_flat(source, &hl, 80, &styles());
        assert_eq!(highlighted(&[flat]), want);
    }

    #[rstest]
    fn highlights_keep_the_markdown_style_underneath() {
        let source = "run `cargo test sync`";
        let lines = render(source, &ranges(source, "sync"), opts(80, 0), &styles());
        let span = lines[0].spans.iter().find(|s| s.content == "sync").unwrap();
        assert_eq!(span.style, styles().code.patch(styles().highlight));
    }

    #[rstest]
    fn highlights_on_markup_alone_mark_nothing() {
        let lines = render("**bold** text", std::slice::from_ref(&(0..2)), opts(80, 0), &styles());
        assert!(highlighted(&lines).is_empty());
    }

    #[rstest]
    fn plain_wrap_ellipsizes_its_last_line() {
        let spans = [Span::raw("an interactive resume picker\nfor atuin ai")];
        let lines = wrap_plain(&spans, 14, 2, styles().muted);
        assert_eq!(plain(&lines), "an interactive\nresume picker…");
    }

    /// A window's lines are the start of any longer one, with nothing cut short, so a view that
    /// scrolls on renders more without what it shows moving.
    #[rstest]
    fn windows_grow_without_moving() {
        let source: String = (0..40)
            .map(|i| format!("Paragraph {i} has **some** words in it.\n\n- one\n- two\n\n"))
            .collect();
        let opts = |max_lines| Opts {
            width: 30,
            max_lines,
            spacing: true,
            urls: false,
        };
        let (all, more) = render_window(&source, &[], opts(10_000), &styles());
        assert!(!more);
        for limit in [1, 7, 50, 128, all.len() - 1, all.len()] {
            let (lines, more) = render_window(&source, &[], opts(limit), &styles());
            assert_eq!(lines.len(), limit);
            assert_eq!(more, limit < all.len(), "{limit}");
            assert_eq!(plain(&lines), plain(&all[..limit]), "{limit}");
            assert!(!plain(&lines).ends_with('…'));
        }
    }

    #[rstest]
    #[case(&[3, 1, 5], 4, &[2, 1, 1])]
    #[case(&[3, 1, 5], 9, &[3, 1, 5])]
    #[case(&[3, 1, 5], 7, &[3, 1, 3])]
    #[case(&[3, 0, 5], 2, &[1, 0, 1])]
    #[case(&[3, 1, 5], 2, &[1, 1, 0])]
    fn allocation_gives_every_part_a_line_first(
        #[case] wants: &[usize],
        #[case] budget: usize,
        #[case] want: &[usize],
    ) {
        assert_eq!(allocate(wants, budget), want);
    }

    #[rstest]
    fn odd_input_does_not_panic() {
        let nested_lists: String = (0..200).map(|i| format!("{}- x\n", "  ".repeat(i))).collect();
        let nested_quotes = format!("{} deep", ">".repeat(500));
        let huge_line = "x".repeat(100_000);
        let controls = "a\u{1b}[31mred\u{7}\r\n\u{0}b\u{200b}c\u{604}d\te";
        for source in [
            nested_lists.as_str(),
            nested_quotes.as_str(),
            huge_line.as_str(),
            controls,
            "```",
            "|",
            "| a |\n|",
            "[x](",
            "\0[佉x",
            "x\u{604}<!",
            "👩‍👩‍👧‍👦 家族 e\u{301}",
        ] {
            for width in [0, 1, 2, 3, 7, 40] {
                for lines in [md(source, width, 0), md(source, width, 4)] {
                    for line in &lines {
                        assert!(line.width() <= width, "{source:?} at {width}: {line:?}");
                    }
                }
                let flat =
                    render_flat(source, std::slice::from_ref(&(0..source.len())), width, &styles());
                assert!(flat.width() <= width);
            }
        }
        assert!(!plain(&md(controls, 40, 0)).contains('\u{1b}'));
    }

    fn markdownish() -> impl Strategy<Value = String> {
        let piece = prop_oneof![
            Just("# ".to_owned()),
            Just("- ".to_owned()),
            Just("1. ".to_owned()),
            Just("> ".to_owned()),
            Just("```".to_owned()),
            Just("`".to_owned()),
            Just("**".to_owned()),
            Just("*".to_owned()),
            Just("_".to_owned()),
            Just("|".to_owned()),
            Just("|---|".to_owned()),
            Just("[a](b)".to_owned()),
            Just("\n".to_owned()),
            Just("\n\n".to_owned()),
            Just("    ".to_owned()),
            Just("\t".to_owned()),
            Just("&amp;".to_owned()),
            Just("\\".to_owned()),
            Just("<b>".to_owned()),
            Just("---".to_owned()),
            Just("[ ] ".to_owned()),
            "[a-z ]{1,12}",
            "\\PC{1,4}",
            any::<char>().prop_map(String::from),
        ];
        prop::collection::vec(piece, 0..40).prop_map(|p| p.concat())
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(128))]

        #[rstest]
        fn never_panics_or_overflows(
            source in markdownish(),
            width in 0usize..50,
            max_lines in 0usize..8,
            spacing in any::<bool>(),
            hl in prop::collection::vec((0usize..200, 0usize..20), 0..4),
        ) {
            let highlights: Vec<Range<usize>> = hl.iter().map(|(s, l)| *s..s + l).collect();
            let opts = Opts { width, max_lines, spacing, urls: true };
            let lines = render(&source, &highlights, opts, &styles());
            if max_lines > 0 {
                prop_assert!(lines.len() <= max_lines);
            }
            for line in &lines {
                prop_assert!(line.width() <= width, "{:?} at {}: {:?}", source, width, line);
            }
            let blank = |l: &Line<'_>| l.to_string().trim_matches(['│', ' ']).is_empty();
            for pair in lines.windows(2) {
                prop_assert!(!(blank(&pair[0]) && blank(&pair[1])), "double blank: {:?}", lines);
            }
            let flat = render_flat(&source, &highlights, width, &styles());
            prop_assert!(flat.width() <= width);
        }
    }
}
