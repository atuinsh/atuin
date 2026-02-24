use std::fmt;
use std::io;
use std::io::Write;
use std::ops::Range;

use crossterm::Command;
use crossterm::cursor::MoveTo;
use crossterm::queue;
use crossterm::style::Color as CrosstermColor;
use crossterm::style::Colors;
use crossterm::style::Print;
use crossterm::style::SetAttribute;
use crossterm::style::SetBackgroundColor;
use crossterm::style::SetColors;
use crossterm::style::SetForegroundColor;
use crossterm::terminal::Clear;
use crossterm::terminal::ClearType;
use ratatui::layout::Size;
use ratatui::prelude::Backend;
use ratatui::prelude::IntoCrossterm;
use ratatui::style::Color;
use ratatui::style::Modifier;
use ratatui::text::Line;
use ratatui::text::Span;
use textwrap::Options;

/// Insert lines above the live viewport into real terminal scrollback.
pub fn insert_history_lines<B>(
    terminal: &mut super::custom_terminal::Terminal<B>,
    lines: Vec<Line<'static>>,
) -> io::Result<()>
where
    B: Backend<Error = io::Error> + Write,
{
    if lines.is_empty() {
        return Ok(());
    }

    let screen_size = terminal.backend().size().unwrap_or(Size::new(0, 0));

    let mut area = terminal.viewport_area;
    let mut should_update_area = false;
    let last_cursor_pos = terminal.last_known_cursor_pos;
    let writer = terminal.backend_mut();

    let wrapped = word_wrap_lines_borrowed(&lines, area.width.max(1) as usize);
    let wrapped_lines = wrapped.len() as u16;

    let cursor_top = if area.bottom() < screen_size.height {
        let scroll_amount = wrapped_lines.min(screen_size.height - area.bottom());

        let top_1based = area.top() + 1;
        queue!(writer, SetScrollRegion(top_1based..screen_size.height))?;
        queue!(writer, MoveTo(0, area.top()))?;
        for _ in 0..scroll_amount {
            queue!(writer, Print("\x1bM"))?;
        }
        queue!(writer, ResetScrollRegion)?;

        let cursor_top = area.top().saturating_sub(1);
        area.y += scroll_amount;
        should_update_area = true;
        cursor_top
    } else {
        area.top().saturating_sub(1)
    };

    let has_history_region = area.top() > 0;
    if has_history_region {
        queue!(writer, SetScrollRegion(1..area.top()))?;
    }
    queue!(writer, MoveTo(0, cursor_top))?;

    for line in wrapped {
        queue!(writer, Print("\r\n"))?;
        queue!(
            writer,
            SetColors(Colors::new(
                line.style
                    .fg
                    .map(IntoCrossterm::into_crossterm)
                    .unwrap_or(CrosstermColor::Reset),
                line.style
                    .bg
                    .map(IntoCrossterm::into_crossterm)
                    .unwrap_or(CrosstermColor::Reset)
            ))
        )?;
        queue!(writer, Clear(ClearType::UntilNewLine))?;

        let merged_spans: Vec<Span> = line
            .spans
            .iter()
            .map(|span| Span {
                style: span.style.patch(line.style),
                content: span.content.clone(),
            })
            .collect();

        write_spans(writer, merged_spans.iter())?;
    }

    if has_history_region {
        queue!(writer, ResetScrollRegion)?;
    }
    queue!(writer, MoveTo(last_cursor_pos.x, last_cursor_pos.y))?;

    let _ = writer;
    if should_update_area {
        terminal.set_viewport_area(area);
    }

    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SetScrollRegion(pub Range<u16>);

impl Command for SetScrollRegion {
    fn write_ansi(&self, f: &mut impl fmt::Write) -> fmt::Result {
        write!(f, "\x1b[{};{}r", self.0.start, self.0.end)
    }

    #[cfg(windows)]
    fn execute_winapi(&self) -> io::Result<()> {
        panic!("tried to execute SetScrollRegion command using WinAPI, use ANSI instead");
    }

    #[cfg(windows)]
    fn is_ansi_code_supported(&self) -> bool {
        true
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ResetScrollRegion;

impl Command for ResetScrollRegion {
    fn write_ansi(&self, f: &mut impl fmt::Write) -> fmt::Result {
        write!(f, "\x1b[r")
    }

    #[cfg(windows)]
    fn execute_winapi(&self) -> io::Result<()> {
        panic!("tried to execute ResetScrollRegion command using WinAPI, use ANSI instead");
    }

    #[cfg(windows)]
    fn is_ansi_code_supported(&self) -> bool {
        true
    }
}

#[derive(Debug, Clone)]
struct RtOptions {
    width: usize,
    line_ending: textwrap::LineEnding,
    break_words: bool,
    wrap_algorithm: textwrap::WrapAlgorithm,
    word_separator: textwrap::WordSeparator,
    word_splitter: textwrap::WordSplitter,
}

impl RtOptions {
    fn new(width: usize) -> Self {
        Self {
            width,
            line_ending: textwrap::LineEnding::LF,
            break_words: true,
            word_separator: textwrap::WordSeparator::new(),
            wrap_algorithm: textwrap::WrapAlgorithm::FirstFit,
            word_splitter: textwrap::WordSplitter::HyphenSplitter,
        }
    }
}

impl From<usize> for RtOptions {
    fn from(width: usize) -> Self {
        Self::new(width)
    }
}

fn wrap_ranges_trim<'a, O>(text: &str, width_or_options: O) -> Vec<Range<usize>>
where
    O: Into<Options<'a>>,
{
    let opts = width_or_options.into();
    let mut lines: Vec<Range<usize>> = Vec::new();
    for line in textwrap::wrap(text, opts).iter() {
        match line {
            std::borrow::Cow::Borrowed(slice) => {
                let start = unsafe { slice.as_ptr().offset_from(text.as_ptr()) as usize };
                let end = start + slice.len();
                lines.push(start..end);
            }
            std::borrow::Cow::Owned(_) => panic!("wrap_ranges_trim: unexpected owned string"),
        }
    }
    lines
}

fn slice_line_spans(
    original: &Line<'static>,
    span_bounds: &[(Range<usize>, ratatui::style::Style)],
    range: &Range<usize>,
) -> Line<'static> {
    let start_byte = range.start;
    let end_byte = range.end;
    let mut acc: Vec<Span<'static>> = Vec::new();

    for (index, (span_range, style)) in span_bounds.iter().enumerate() {
        let span_start = span_range.start;
        let span_end = span_range.end;

        if span_end <= start_byte {
            continue;
        }
        if span_start >= end_byte {
            break;
        }

        let seg_start = start_byte.max(span_start);
        let seg_end = end_byte.min(span_end);
        if seg_end > seg_start {
            let local_start = seg_start - span_start;
            let local_end = seg_end - span_start;
            let content = original.spans[index].content.as_ref();
            let slice = content[local_start..local_end].to_string();
            acc.push(Span {
                style: *style,
                content: std::borrow::Cow::Owned(slice),
            });
        }

        if span_end >= end_byte {
            break;
        }
    }

    Line {
        style: original.style,
        alignment: original.alignment,
        spans: acc,
    }
}

fn word_wrap_line(
    line: &Line<'static>,
    width_or_options: impl Into<RtOptions>,
) -> Vec<Line<'static>> {
    let mut flat = String::new();
    let mut span_bounds = Vec::new();
    let mut acc = 0usize;
    for span in &line.spans {
        let text = span.content.as_ref();
        let start = acc;
        flat.push_str(text);
        acc += text.len();
        span_bounds.push((start..acc, span.style));
    }

    let rt_opts: RtOptions = width_or_options.into();
    let opts = Options::new(rt_opts.width)
        .line_ending(rt_opts.line_ending)
        .break_words(rt_opts.break_words)
        .wrap_algorithm(rt_opts.wrap_algorithm)
        .word_separator(rt_opts.word_separator)
        .word_splitter(rt_opts.word_splitter);

    let wrapped_ranges = wrap_ranges_trim(&flat, opts);
    if wrapped_ranges.is_empty() {
        return vec![Line::default()];
    }

    wrapped_ranges
        .iter()
        .map(|range| {
            let mut wrapped = slice_line_spans(line, &span_bounds, range);
            for span in &mut wrapped.spans {
                span.style = span.style.patch(line.style);
            }
            wrapped.style = line.style;
            wrapped
        })
        .collect()
}

fn word_wrap_lines_borrowed(
    lines: &[Line<'static>],
    width_or_options: impl Into<RtOptions>,
) -> Vec<Line<'static>> {
    let options = width_or_options.into();
    lines
        .iter()
        .flat_map(|line| word_wrap_line(line, options.clone()))
        .collect()
}

struct ModifierDiff {
    pub from: Modifier,
    pub to: Modifier,
}

impl ModifierDiff {
    fn queue<W>(self, mut writer: W) -> io::Result<()>
    where
        W: io::Write,
    {
        use crossterm::style::Attribute as CrosstermAttribute;

        let removed = self.from - self.to;
        if removed.contains(Modifier::REVERSED) {
            queue!(writer, SetAttribute(CrosstermAttribute::NoReverse))?;
        }
        if removed.contains(Modifier::BOLD) {
            queue!(writer, SetAttribute(CrosstermAttribute::NormalIntensity))?;
            if self.to.contains(Modifier::DIM) {
                queue!(writer, SetAttribute(CrosstermAttribute::Dim))?;
            }
        }
        if removed.contains(Modifier::ITALIC) {
            queue!(writer, SetAttribute(CrosstermAttribute::NoItalic))?;
        }
        if removed.contains(Modifier::UNDERLINED) {
            queue!(writer, SetAttribute(CrosstermAttribute::NoUnderline))?;
        }
        if removed.contains(Modifier::DIM) {
            queue!(writer, SetAttribute(CrosstermAttribute::NormalIntensity))?;
        }
        if removed.contains(Modifier::CROSSED_OUT) {
            queue!(writer, SetAttribute(CrosstermAttribute::NotCrossedOut))?;
        }
        if removed.contains(Modifier::SLOW_BLINK) || removed.contains(Modifier::RAPID_BLINK) {
            queue!(writer, SetAttribute(CrosstermAttribute::NoBlink))?;
        }

        let added = self.to - self.from;
        if added.contains(Modifier::REVERSED) {
            queue!(writer, SetAttribute(CrosstermAttribute::Reverse))?;
        }
        if added.contains(Modifier::BOLD) {
            queue!(writer, SetAttribute(CrosstermAttribute::Bold))?;
        }
        if added.contains(Modifier::ITALIC) {
            queue!(writer, SetAttribute(CrosstermAttribute::Italic))?;
        }
        if added.contains(Modifier::UNDERLINED) {
            queue!(writer, SetAttribute(CrosstermAttribute::Underlined))?;
        }
        if added.contains(Modifier::DIM) {
            queue!(writer, SetAttribute(CrosstermAttribute::Dim))?;
        }
        if added.contains(Modifier::CROSSED_OUT) {
            queue!(writer, SetAttribute(CrosstermAttribute::CrossedOut))?;
        }
        if added.contains(Modifier::SLOW_BLINK) {
            queue!(writer, SetAttribute(CrosstermAttribute::SlowBlink))?;
        }
        if added.contains(Modifier::RAPID_BLINK) {
            queue!(writer, SetAttribute(CrosstermAttribute::RapidBlink))?;
        }

        Ok(())
    }
}

fn write_spans<'a, I>(mut writer: &mut impl Write, content: I) -> io::Result<()>
where
    I: IntoIterator<Item = &'a Span<'a>>,
{
    let mut fg = Color::Reset;
    let mut bg = Color::Reset;
    let mut last_modifier = Modifier::empty();

    for span in content {
        let mut modifier = Modifier::empty();
        modifier.insert(span.style.add_modifier);
        modifier.remove(span.style.sub_modifier);
        if modifier != last_modifier {
            let diff = ModifierDiff {
                from: last_modifier,
                to: modifier,
            };
            diff.queue(&mut writer)?;
            last_modifier = modifier;
        }

        let next_fg = span.style.fg.unwrap_or(Color::Reset);
        let next_bg = span.style.bg.unwrap_or(Color::Reset);
        if next_fg != fg || next_bg != bg {
            queue!(
                writer,
                SetColors(Colors::new(
                    next_fg.into_crossterm(),
                    next_bg.into_crossterm()
                ))
            )?;
            fg = next_fg;
            bg = next_bg;
        }

        queue!(writer, Print(span.content.clone()))?;
    }

    queue!(
        writer,
        SetForegroundColor(CrosstermColor::Reset),
        SetBackgroundColor(CrosstermColor::Reset),
        SetAttribute(crossterm::style::Attribute::Reset),
    )
}

#[cfg(test)]
mod tests {
    use std::fmt;
    use std::io;
    use std::io::Write;

    use super::*;
    use pretty_assertions::assert_eq;
    use ratatui::backend::Backend;
    use ratatui::backend::ClearType;
    use ratatui::backend::WindowSize;
    use ratatui::buffer::Cell;
    use ratatui::layout::Position;
    use ratatui::layout::Rect;
    use ratatui::layout::Size;
    use ratatui::prelude::CrosstermBackend;

    /// Wraps a crossterm backend with a vt100 parser so tests can inspect final screen state.
    struct VT100Backend {
        crossterm_backend: CrosstermBackend<vt100::Parser>,
    }

    impl VT100Backend {
        fn new(width: u16, height: u16) -> Self {
            crossterm::style::force_color_output(true);
            Self {
                crossterm_backend: CrosstermBackend::new(vt100::Parser::new(height, width, 0)),
            }
        }

        fn vt100(&self) -> &vt100::Parser {
            self.crossterm_backend.writer()
        }
    }

    impl Write for VT100Backend {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            self.crossterm_backend.writer_mut().write(buf)
        }

        fn flush(&mut self) -> io::Result<()> {
            self.crossterm_backend.writer_mut().flush()
        }
    }

    impl fmt::Display for VT100Backend {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            write!(f, "{}", self.crossterm_backend.writer().screen().contents())
        }
    }

    impl Backend for VT100Backend {
        type Error = io::Error;

        fn draw<'a, I>(&mut self, content: I) -> io::Result<()>
        where
            I: Iterator<Item = (u16, u16, &'a Cell)>,
        {
            self.crossterm_backend.draw(content)
        }

        fn hide_cursor(&mut self) -> io::Result<()> {
            self.crossterm_backend.hide_cursor()
        }

        fn show_cursor(&mut self) -> io::Result<()> {
            self.crossterm_backend.show_cursor()
        }

        fn get_cursor_position(&mut self) -> io::Result<Position> {
            Ok(self.vt100().screen().cursor_position().into())
        }

        fn set_cursor_position<P: Into<Position>>(&mut self, position: P) -> io::Result<()> {
            self.crossterm_backend.set_cursor_position(position)
        }

        fn clear(&mut self) -> io::Result<()> {
            self.crossterm_backend.clear()
        }

        fn clear_region(&mut self, clear_type: ClearType) -> io::Result<()> {
            self.crossterm_backend.clear_region(clear_type)
        }

        fn append_lines(&mut self, line_count: u16) -> io::Result<()> {
            self.crossterm_backend.append_lines(line_count)
        }

        fn size(&self) -> io::Result<Size> {
            let (rows, cols) = self.vt100().screen().size();
            Ok(Size::new(cols, rows))
        }

        fn window_size(&mut self) -> io::Result<WindowSize> {
            Ok(WindowSize {
                columns_rows: self.vt100().screen().size().into(),
                pixels: Size {
                    width: 640,
                    height: 480,
                },
            })
        }

        fn flush(&mut self) -> io::Result<()> {
            self.crossterm_backend.writer_mut().flush()
        }

        fn scroll_region_up(
            &mut self,
            region: std::ops::Range<u16>,
            scroll_by: u16,
        ) -> io::Result<()> {
            self.crossterm_backend.scroll_region_up(region, scroll_by)
        }

        fn scroll_region_down(
            &mut self,
            region: std::ops::Range<u16>,
            scroll_by: u16,
        ) -> io::Result<()> {
            self.crossterm_backend.scroll_region_down(region, scroll_by)
        }
    }

    #[test]
    fn writes_bold_then_regular_spans() {
        use ratatui::style::Stylize;

        let spans = ["A".bold(), "B".into()];

        let mut actual: Vec<u8> = Vec::new();
        write_spans(&mut actual, spans.iter()).expect("write spans should succeed");

        let mut expected: Vec<u8> = Vec::new();
        queue!(
            expected,
            SetAttribute(crossterm::style::Attribute::Bold),
            Print("A"),
            SetAttribute(crossterm::style::Attribute::NormalIntensity),
            Print("B"),
            SetForegroundColor(CrosstermColor::Reset),
            SetBackgroundColor(CrosstermColor::Reset),
            SetAttribute(crossterm::style::Attribute::Reset),
        )
        .expect("queue expected output");

        assert_eq!(
            String::from_utf8(actual).expect("actual output should be utf8"),
            String::from_utf8(expected).expect("expected output should be utf8")
        );
    }

    #[test]
    fn history_insert_restores_cursor() {
        let width: u16 = 40;
        let height: u16 = 6;
        let backend = VT100Backend::new(width, height);
        let mut terminal = super::super::custom_terminal::Terminal::with_options(backend)
            .expect("terminal should initialize");
        terminal.set_viewport_area(Rect::new(0, height - 2, width, 2));

        terminal
            .set_cursor_position(Position { x: 7, y: 3 })
            .expect("cursor position set should succeed");
        let before = terminal.last_known_cursor_pos;

        insert_history_lines(&mut terminal, vec![Line::from("history line")])
            .expect("history insert should succeed");

        assert_eq!(before, terminal.last_known_cursor_pos);
    }

    #[test]
    fn history_insert_wraps_long_lines_consistently() {
        let width: u16 = 16;
        let height: u16 = 8;
        let backend = VT100Backend::new(width, height);
        let mut terminal = super::super::custom_terminal::Terminal::with_options(backend)
            .expect("terminal should initialize");
        terminal.set_viewport_area(Rect::new(0, height - 1, width, 1));

        insert_history_lines(
            &mut terminal,
            vec![Line::from("this is a long line that should wrap")],
        )
        .expect("history insert should succeed");

        let screen = terminal.backend().vt100().screen();
        let non_empty_rows = (0..height)
            .filter(|row| {
                (0..width).any(|col| {
                    screen
                        .cell(*row, col)
                        .is_some_and(|cell| cell.has_contents() && cell.contents() != " ")
                })
            })
            .count();

        assert!(non_empty_rows >= 2, "expected wrapped output across rows");
    }

    #[test]
    fn history_insert_writes_text_to_vt100_backend() {
        let width: u16 = 30;
        let height: u16 = 8;
        let backend = VT100Backend::new(width, height);
        let mut terminal = super::super::custom_terminal::Terminal::with_options(backend)
            .expect("terminal should initialize");
        terminal.set_viewport_area(Rect::new(0, height - 1, width, 1));

        insert_history_lines(
            &mut terminal,
            vec![Line::from("user: list files"), Line::from("$ ls -la")],
        )
        .expect("history insert should succeed");

        let rows: Vec<String> = terminal.backend().vt100().screen().rows(0, width).collect();
        let joined = rows.join("\n");
        assert!(joined.contains("user: list files"));
        assert!(joined.contains("$ ls -la"));
    }
}
