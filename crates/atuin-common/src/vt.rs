//! Minimal terminal-emulator wrapper around [`rio_vt`], exposing the small
//! parser/screen surface atuin consumes (previously provided by the `vt100`
//! crate): feeding bytes, plain-text extraction, per-cell reads, and
//! ANSI-formatted row snapshots for the PTY proxy.

use rio_vt::{
    ansi::CursorShape,
    config::colors::{AnsiColor, NamedColor},
    crosswords::{
        Crosswords, CrosswordsSize,
        pos::Column,
        square::{ContentTag, Square, Wide},
        style::{Style, StyleFlags},
    },
    event::{EventListener, RioEvent, WindowId},
    performer::handler::Processor,
};

/// Listener that discards terminal events; atuin's emulation is one-way
/// (there is no PTY to answer DA/DSR queries on these paths).
#[derive(Clone, Default)]
struct VoidListener;

impl EventListener for VoidListener {
    fn event(&self) -> (Option<RioEvent>, bool) {
        (None, false)
    }

    fn send_event(&self, _event: RioEvent, _window_id: WindowId) {}
}

/// In-memory terminal emulator.
pub struct Parser {
    term: Crosswords<VoidListener>,
    processor: Processor,
}

impl Parser {
    /// Create an emulator with a `rows` x `cols` grid and `scrollback` lines
    /// of history.
    pub fn new(rows: u16, cols: u16, scrollback: usize) -> Self {
        Parser {
            term: Crosswords::new(
                CrosswordsSize::new(cols.max(1) as usize, rows.max(1) as usize),
                CursorShape::Block,
                VoidListener,
                WindowId::from(0),
                0,
                scrollback,
            ),
            processor: Processor::default(),
        }
    }

    /// Feed terminal output bytes into the emulator.
    pub fn process(&mut self, bytes: &[u8]) {
        self.processor.advance(&mut self.term, bytes);
    }

    /// Resize the grid. Content reflows.
    pub fn set_size(&mut self, rows: u16, cols: u16) {
        self.term.resize(CrosswordsSize::new(
            cols.max(1) as usize,
            rows.max(1) as usize,
        ));
    }

    /// Immutable view of the current screen.
    pub fn screen(&self) -> Screen<'_> {
        Screen {
            rows: self.term.visible_rows(),
            term: &self.term,
        }
    }
}

/// A single cell's textual contents.
pub struct Cell {
    contents: String,
}

impl Cell {
    /// The cell's text: empty for blank cells and wide-character spacers.
    pub fn contents(&self) -> &str {
        &self.contents
    }
}

/// Snapshot view of the visible screen.
pub struct Screen<'a> {
    term: &'a Crosswords<VoidListener>,
    rows: Vec<rio_vt::crosswords::grid::row::Row<Square>>,
}

impl Screen<'_> {
    /// Screen size as `(rows, cols)`.
    pub fn size(&self) -> (u16, u16) {
        (
            self.term.screen_lines() as u16,
            self.term.columns() as u16,
        )
    }

    /// Cursor position as `(row, col)`, clamped to the grid.
    pub fn cursor_position(&self) -> (u16, u16) {
        let pos = self.term.cursor().pos;
        (
            u16::try_from(pos.row.0.max(0)).unwrap_or(u16::MAX),
            u16::try_from(pos.col.0).unwrap_or(u16::MAX),
        )
    }

    /// The cell at `(row, col)`, or `None` when out of bounds.
    pub fn cell(&self, row: u16, col: u16) -> Option<Cell> {
        let row = self.rows.get(usize::from(row))?;
        if usize::from(col) >= self.term.columns() {
            return None;
        }
        let square = &row[Column(usize::from(col))];
        let c = square.c();
        let contents = if matches!(square.wide(), Wide::Spacer | Wide::LeadingSpacer)
            || c == '\0'
            || c == ' '
        {
            String::new()
        } else {
            c.to_string()
        };
        Some(Cell { contents })
    }

    /// Plain text of the visible screen. Soft-wrapped rows are joined without
    /// a newline; hard line breaks become `\n`. Trailing whitespace is kept on
    /// wrapped rows (they are full by definition) and trimmed elsewhere.
    pub fn contents(&self) -> String {
        let cols = self.term.columns();
        let mut out = String::new();
        for (idx, row) in self.rows.iter().enumerate() {
            let wrapped = cols > 0 && row[Column(cols - 1)].wrapline();
            let mut line = String::with_capacity(cols);
            for col in 0..cols {
                let square = &row[Column(col)];
                if matches!(square.wide(), Wide::Spacer | Wide::LeadingSpacer) {
                    continue;
                }
                let c = square.c();
                line.push(if c == '\0' { ' ' } else { c });
            }
            if wrapped {
                out.push_str(&line);
            } else {
                out.push_str(line.trim_end());
                if idx + 1 < self.rows.len() {
                    out.push('\n');
                }
            }
        }
        // Trailing blank rows are grid padding, not content.
        while out.ends_with('\n') {
            out.pop();
        }
        out
    }

    /// Visible rows re-encoded as ANSI escape sequences, spanning `width`
    /// columns starting at column `start`. Each row starts from default
    /// attributes and resets them at the end, so rows can be written to a
    /// terminal independently.
    pub fn rows_formatted(&self, start: u16, width: u16) -> impl Iterator<Item = Vec<u8>> + '_ {
        let styles = self.term.grid.style_set.styles();
        let cols = self.term.columns();
        let start = usize::from(start);
        let end = start.saturating_add(usize::from(width)).min(cols);
        self.rows.iter().map(move |row| {
            let mut out: Vec<u8> = Vec::with_capacity(end.saturating_sub(start) * 2);
            let mut current: Option<(AnsiColor, AnsiColor, StyleFlags)> = None;
            let mut styled = false;
            // Trailing blank cells with default styling are dropped.
            let mut visible_end = start;
            for col in (start..end).rev() {
                let square = &row[Column(col)];
                if square.c() != ' ' && square.c() != '\0'
                    || square_style(square, styles) != default_style()
                {
                    visible_end = col + 1;
                    break;
                }
            }
            for col in start..visible_end {
                let square = &row[Column(col)];
                if matches!(square.wide(), Wide::Spacer | Wide::LeadingSpacer) {
                    continue;
                }
                let style = square_style(square, styles);
                if current != Some(style) {
                    write_sgr(&mut out, style);
                    styled = style != default_style();
                    current = Some(style);
                }
                let c = square.c();
                let mut buf = [0u8; 4];
                out.extend_from_slice(if c == '\0' { " " } else { c.encode_utf8(&mut buf) }.as_bytes());
            }
            if styled {
                out.extend_from_slice(b"\x1b[0m");
            }
            out
        })
    }
}

fn default_style() -> (AnsiColor, AnsiColor, StyleFlags) {
    (
        AnsiColor::Named(NamedColor::Foreground),
        AnsiColor::Named(NamedColor::Background),
        StyleFlags::empty(),
    )
}

/// Resolve a square's foreground, background, and attribute flags.
fn square_style(square: &Square, styles: &[Style]) -> (AnsiColor, AnsiColor, StyleFlags) {
    match square.content_tag() {
        ContentTag::Codepoint => {
            let style = styles
                .get(square.style_id() as usize)
                .copied()
                .unwrap_or_default();
            (style.fg, style.bg, style.flags)
        }
        ContentTag::BgPalette => (
            AnsiColor::Named(NamedColor::Foreground),
            AnsiColor::Indexed(square.bg_palette_index()),
            StyleFlags::empty(),
        ),
        ContentTag::BgRgb => {
            let (r, g, b) = square.bg_rgb();
            (
                AnsiColor::Named(NamedColor::Foreground),
                AnsiColor::Spec(rio_vt::config::colors::ColorRgb { r, g, b }),
                StyleFlags::empty(),
            )
        }
    }
}

/// Emit a reset followed by the SGR parameters for `style`.
fn write_sgr(out: &mut Vec<u8>, (fg, bg, flags): (AnsiColor, AnsiColor, StyleFlags)) {
    out.extend_from_slice(b"\x1b[0");
    if flags.contains(StyleFlags::BOLD) {
        out.extend_from_slice(b";1");
    }
    if flags.contains(StyleFlags::DIM) {
        out.extend_from_slice(b";2");
    }
    if flags.contains(StyleFlags::ITALIC) {
        out.extend_from_slice(b";3");
    }
    if flags.intersects(StyleFlags::ALL_UNDERLINES) {
        out.extend_from_slice(b";4");
    }
    if flags.contains(StyleFlags::INVERSE) {
        out.extend_from_slice(b";7");
    }
    if flags.contains(StyleFlags::HIDDEN) {
        out.extend_from_slice(b";8");
    }
    if flags.contains(StyleFlags::STRIKEOUT) {
        out.extend_from_slice(b";9");
    }
    write_color(out, fg, false);
    write_color(out, bg, true);
    out.push(b'm');
}

fn write_color(out: &mut Vec<u8>, color: AnsiColor, background: bool) {
    let base: u16 = if background { 40 } else { 30 };
    match color {
        AnsiColor::Named(NamedColor::Foreground | NamedColor::Background) => {}
        AnsiColor::Named(named) => {
            let idx = named as u32;
            if idx < 8 {
                out.extend_from_slice(format!(";{}", base + idx as u16).as_bytes());
            } else if idx < 16 {
                out.extend_from_slice(format!(";{}", base + 60 + (idx as u16 - 8)).as_bytes());
            }
        }
        AnsiColor::Indexed(idx) => {
            out.extend_from_slice(format!(";{};5;{}", base + 8, idx).as_bytes());
        }
        AnsiColor::Spec(rgb) => {
            out.extend_from_slice(format!(";{};2;{};{};{}", base + 8, rgb.r, rgb.g, rgb.b).as_bytes());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn screen_text(input: &[u8], rows: u16, cols: u16) -> String {
        let mut parser = Parser::new(rows, cols, 0);
        parser.process(input);
        parser.screen().contents()
    }

    #[test]
    fn plain_text_round_trip() {
        assert_eq!(screen_text(b"hello", 4, 20), "hello");
    }

    #[test]
    fn soft_wrapped_rows_join_without_newline() {
        assert_eq!(screen_text(b"abcdefghij", 4, 4), "abcdefghij");
    }

    #[test]
    fn hard_newlines_are_preserved(){
        assert_eq!(screen_text(b"one\r\ntwo", 4, 20), "one\ntwo");
    }

    #[test]
    fn formatted_rows_reproduce_styles() {
        let mut parser = Parser::new(2, 20, 0);
        parser.process(b"\x1b[1;31mred\x1b[0m plain");
        let rows: Vec<Vec<u8>> = parser.screen().rows_formatted(0, 20).collect();
        let first = String::from_utf8(rows[0].clone()).unwrap();
        assert!(first.contains("\x1b[0;1;31mred"), "got: {first:?}");
        // The style change back to default resets attributes before "plain".
        assert!(first.contains("\x1b[0m plain"), "got: {first:?}");
        assert!(rows[1].is_empty());
    }

    #[test]
    fn cell_reads_and_cursor() {
        let mut parser = Parser::new(3, 10, 0);
        parser.process(b"hi");
        let screen = parser.screen();
        assert_eq!(screen.size(), (3, 10));
        assert_eq!(screen.cell(0, 0).unwrap().contents(), "h");
        assert_eq!(screen.cell(0, 1).unwrap().contents(), "i");
        assert_eq!(screen.cell(0, 2).unwrap().contents(), "");
        assert!(screen.cell(3, 0).is_none());
        assert_eq!(screen.cursor_position(), (0, 2));
    }

    #[test]
    fn resize_reflows() {
        let mut parser = Parser::new(2, 10, 0);
        parser.process(b"hello");
        parser.set_size(4, 5);
        assert_eq!(parser.screen().contents(), "hello");
    }
}
