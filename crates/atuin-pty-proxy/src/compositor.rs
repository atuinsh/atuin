//! Single-writer compositor for the suggestion overlay.
//!
//! Every byte that reaches the real terminal goes through [`Compositor`],
//! which owns the vt100 screen model, the output handle, and the overlay
//! (popup + ghost text) under one lock. That single-writer discipline is
//! what makes the overlay safe:
//!
//! * **No interleaving corruption.** Overlay bytes are only ever emitted at
//!   escape-sequence boundaries: pty chunks that end mid-sequence (or mid
//!   UTF-8 character) have the incomplete tail withheld until it completes.
//! * **No scroll races.** The overlay is erased *before* each pty chunk is
//!   applied and redrawn after, so the "restore covered rows from the model"
//!   trick always runs while the physical screen matches the model exactly.
//!   Erase, chunk, and redraw are emitted as one `write`.
//!
//! The model doubles as the compositing source: overlay bytes are never fed
//! to the parser, so the model is always "the screen without the overlay",
//! and erasing means repainting rows from it. Paints end with the model's
//! cursor state and drawing attributes, handing the terminal back to the
//! shell exactly as it left it.

use std::io::Write;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

pub(crate) const MAX_POPUP_ROWS: usize = 5;

/// Withheld-tail cap. A malformed, never-terminated escape sequence flushes
/// once it exceeds this rather than buffering forever.
const MAX_TAIL_BYTES: usize = 8192;

const SELECTED_STYLE: &[u8] = b"\x1b[0m\x1b[48;5;24m\x1b[97m";
const UNSELECTED_STYLE: &[u8] = b"\x1b[0m\x1b[48;5;236m\x1b[37m";
const GHOST_STYLE: &[u8] = b"\x1b[0m\x1b[38;5;242m";

/// What the suggestion UI wants painted.
#[derive(Clone, Default)]
pub(crate) struct OverlayContent {
    /// Plain-text command line currently being edited.
    pub(crate) line: String,
    pub(crate) suggestions: Vec<String>,
    pub(crate) selected: usize,
}

/// Lock-free view of what's currently painted, for the stdin thread's key
/// interception checks.
#[derive(Default)]
pub(crate) struct OverlayFlags {
    pub(crate) popup: AtomicBool,
    pub(crate) ghost: AtomicBool,
}

#[derive(Clone, Copy)]
struct DrawnRegion {
    first_row: u16,
    count: u16,
}

pub(crate) struct Compositor<W: Write> {
    parser: vt100::Parser,
    out: W,
    /// Withhold chunk tails that end mid escape sequence / UTF-8 character,
    /// so overlay bytes are never spliced into one. Only needed when an
    /// overlay may actually paint.
    split_partials: bool,
    tail: Vec<u8>,
    content: Option<OverlayContent>,
    drawn: Option<DrawnRegion>,
    ghost_row: Option<u16>,
    window_offset: usize,
    flags: Arc<OverlayFlags>,
}

impl<W: Write> Compositor<W> {
    pub(crate) fn new(
        rows: u16,
        cols: u16,
        out: W,
        flags: Arc<OverlayFlags>,
        split_partials: bool,
    ) -> Self {
        Self {
            parser: vt100::Parser::new(rows, cols, 0),
            out,
            split_partials,
            tail: Vec::new(),
            content: None,
            drawn: None,
            ghost_row: None,
            window_offset: 0,
            flags,
        }
    }

    pub(crate) fn screen(&self) -> &vt100::Screen {
        self.parser.screen()
    }

    /// Apply a chunk of pty output: erase the overlay (screen == model),
    /// forward the chunk to both the model and the terminal, then repaint
    /// the popup. All in one write, so no other output can interleave.
    ///
    /// Ghost text is deliberately *not* repainted here: this chunk may have
    /// changed the command line, and painting the stale suffix at the new
    /// cursor would briefly duplicate the freshly echoed characters. The
    /// suggestion pass that follows the echo repaints it.
    pub(crate) fn apply_pty(&mut self, data: &[u8]) {
        let mut buf = Vec::with_capacity(data.len() + 256);
        self.erase_into(&mut buf);

        self.tail.extend_from_slice(data);
        let pending = std::mem::take(&mut self.tail);
        let ready_len = if self.split_partials && pending.len() <= MAX_TAIL_BYTES {
            complete_prefix_len(&pending)
        } else {
            pending.len()
        };
        let (ready, tail) = pending.split_at(ready_len);
        self.tail = tail.to_vec();

        self.parser.process(ready);
        buf.extend_from_slice(ready);

        self.draw_into(&mut buf, false);
        self.flush(buf);
    }

    /// Flush any withheld partial-sequence tail (e.g. on shutdown).
    pub(crate) fn flush_pending(&mut self) {
        if self.tail.is_empty() {
            return;
        }
        let tail = std::mem::take(&mut self.tail);
        self.parser.process(&tail);
        self.flush(tail);
    }

    /// Replace the overlay content and repaint.
    pub(crate) fn set_overlay(&mut self, content: Option<OverlayContent>) {
        let mut buf = Vec::with_capacity(256);
        self.erase_into(&mut buf);
        self.content = content.filter(|content| !content.suggestions.is_empty());
        if self.content.is_none() {
            self.window_offset = 0;
        }
        self.draw_into(&mut buf, true);
        self.flush(buf);
    }

    /// The terminal reflows on resize, scrambling both the screen and the
    /// model in terminal-specific ways; drop the overlay bookkeeping without
    /// painting and let the next suggestion pass repaint cleanly.
    pub(crate) fn resize(&mut self, rows: u16, cols: u16) {
        self.content = None;
        self.drawn = None;
        self.ghost_row = None;
        self.window_offset = 0;
        self.parser.screen_mut().set_size(rows, cols);
        self.update_flags();
    }

    fn flush(&mut self, buf: Vec<u8>) {
        if !buf.is_empty() {
            let _ = self.out.write_all(&buf);
            let _ = self.out.flush();
        }
        self.update_flags();
    }

    fn update_flags(&self) {
        self.flags.popup.store(self.drawn.is_some(), Ordering::Release);
        self.flags.ghost.store(self.ghost_row.is_some(), Ordering::Release);
    }

    /// Restore every row the overlay covers from the model. Only valid while
    /// screen == model + overlay, which the erase-before-apply cycle
    /// guarantees. No-op (and never touches `buf`) when nothing is drawn.
    fn erase_into(&mut self, buf: &mut Vec<u8>) {
        let drawn = self.drawn.take();
        let ghost_row = self.ghost_row.take();
        if drawn.is_none() && ghost_row.is_none() {
            return;
        }

        let screen = self.parser.screen();
        if let Some(region) = drawn {
            for row in region.first_row..region.first_row.saturating_add(region.count) {
                restore_row(buf, screen, row);
            }
        }
        if let Some(row) = ghost_row {
            let covered = drawn.is_some_and(|region| {
                row >= region.first_row && row < region.first_row.saturating_add(region.count)
            });
            if !covered {
                restore_row(buf, screen, row);
            }
        }
        hand_back(buf, screen);
    }

    /// Paint the popup (and, when `with_ghost`, the ghost text) from the
    /// current model state. Assumes the overlay is currently erased.
    fn draw_into(&mut self, buf: &mut Vec<u8>, with_ghost: bool) {
        let Some(content) = &self.content else {
            return;
        };
        let screen = self.parser.screen();
        // Never paint over full-screen applications.
        if screen.alternate_screen() {
            return;
        }
        let (rows, cols) = screen.size();
        let (cursor_row, cursor_col) = screen.cursor_position();
        if rows < 2 || cols < 4 {
            return;
        }

        let selected = content.selected.min(content.suggestions.len() - 1);
        let ghost_suffix = content.suggestions[selected]
            .strip_prefix(content.line.as_str())
            .filter(|suffix| !suffix.is_empty());

        // A single suggestion that's already shown in full as ghost text
        // doesn't need a dropdown too.
        let solo_ghost = content.suggestions.len() == 1 && ghost_suffix.is_some();

        buf.extend_from_slice(b"\x1b[?25l");
        if !solo_ghost {
            let mut visible = content.suggestions.len().min(MAX_POPUP_ROWS);

            // Below the cursor when the whole popup fits, above when that
            // side fits, otherwise the roomier side — shrunk to fit so the
            // popup never covers the command line itself.
            let below = (rows - cursor_row - 1) as usize;
            let above = cursor_row as usize;
            let first_row = if below >= visible {
                cursor_row + 1
            } else if above >= visible {
                cursor_row - visible as u16
            } else if below >= above {
                visible = below;
                cursor_row + 1
            } else {
                visible = above;
                0
            };

            if visible > 0 {
                // Slide the selection window to keep the highlight visible.
                if selected < self.window_offset {
                    self.window_offset = selected;
                }
                if selected >= self.window_offset + visible {
                    self.window_offset = selected + 1 - visible;
                }
                self.window_offset =
                    self.window_offset.min(content.suggestions.len() - visible);
                let window =
                    &content.suggestions[self.window_offset..self.window_offset + visible];

                // Left-align with the start of the typed line, sliding left
                // to fit.
                let line_width = content.line.width().min(u16::MAX as usize) as u16;
                let width = window
                    .iter()
                    .map(|s| s.width() + 2)
                    .max()
                    .unwrap_or(2)
                    .min(cols as usize)
                    .max(4);
                let col = cursor_col
                    .saturating_sub(line_width)
                    .min(cols.saturating_sub(width as u16));

                for (i, suggestion) in window.iter().enumerate() {
                    let row = first_row + i as u16;
                    move_to(buf, row, col);
                    buf.extend_from_slice(if self.window_offset + i == selected {
                        SELECTED_STYLE
                    } else {
                        UNSELECTED_STYLE
                    });

                    let mut text = String::with_capacity(width + 1);
                    let mut used = 1;
                    text.push(' ');
                    // Control characters would corrupt the popup row (a
                    // stray \n in a suggestion breaks out of it entirely);
                    // render them visibly.
                    for ch in suggestion.chars().map(printable) {
                        let ch_width = ch.width().unwrap_or(0);
                        if used + ch_width > width - 1 {
                            break;
                        }
                        text.push(ch);
                        used += ch_width;
                    }
                    while used < width {
                        text.push(' ');
                        used += 1;
                    }
                    buf.extend_from_slice(text.as_bytes());
                }
                self.drawn = Some(DrawnRegion {
                    first_row,
                    count: visible as u16,
                });
            }
        }

        // Fish-style ghost text: the selected suggestion's suffix, dim, at
        // the cursor — but only into cells the model says are blank, so it
        // never covers mid-line edits or a right-side prompt. Fuzzy matches
        // that don't extend the typed line have no suffix to ghost.
        if with_ghost && let Some(suffix) = ghost_suffix {
            let budget = blank_cells_after_cursor(screen) as usize;
            let mut text = String::new();
            let mut used = 0;
            for ch in suffix.chars().map(printable) {
                let ch_width = ch.width().unwrap_or(0);
                if used + ch_width > budget {
                    break;
                }
                text.push(ch);
                used += ch_width;
            }
            if !text.is_empty() {
                move_to(buf, cursor_row, cursor_col);
                buf.extend_from_slice(GHOST_STYLE);
                buf.extend_from_slice(text.as_bytes());
                self.ghost_row = Some(cursor_row);
            }
        }

        hand_back(buf, screen);
    }
}

/// Rewrite one screen row from the model. The pre-formatted row bytes skip
/// default cells with cursor jumps, so clear the line first.
fn restore_row(buf: &mut Vec<u8>, screen: &vt100::Screen, row: u16) {
    let (_, cols) = screen.size();
    buf.extend_from_slice(b"\x1b[0m");
    move_to(buf, row, 0);
    buf.extend_from_slice(b"\x1b[2K");
    if let Some(bytes) = screen.rows_formatted(0, cols).nth(row as usize) {
        buf.extend_from_slice(&bytes);
    }
}

/// Return the terminal to the state the shell expects: the model's cursor
/// position and visibility, then its active drawing attributes (attributes
/// last — restoring the cursor may itself alter them).
fn hand_back(buf: &mut Vec<u8>, screen: &vt100::Screen) {
    buf.extend_from_slice(&screen.cursor_state_formatted());
    buf.extend_from_slice(&screen.attributes_formatted());
}

fn move_to(buf: &mut Vec<u8>, row: u16, col: u16) {
    let _ = write!(buf, "\x1b[{};{}H", row + 1, col + 1);
}

/// Replace control characters with a visible placeholder so suggestion text
/// can never emit terminal controls from inside an overlay row.
fn printable(ch: char) -> char {
    match ch {
        '\n' | '\r' => '\u{23ce}', // ⏎
        ch if ch.is_control() => ' ',
        ch => ch,
    }
}

/// Count contiguous cells at and after the cursor whose contents are empty
/// or whitespace, stopping at the first real glyph or the end of the row.
pub(crate) fn blank_cells_after_cursor(screen: &vt100::Screen) -> u16 {
    let (row, col) = screen.cursor_position();
    let (_, cols) = screen.size();

    let mut blank = 0;
    for candidate in col..cols {
        let occupied = screen
            .cell(row, candidate)
            .is_some_and(|cell| !cell.contents().trim().is_empty());
        if occupied {
            break;
        }
        blank += 1;
    }
    blank
}

// ---------------------------------------------------------------------------
// Escape-sequence-safe chunk splitting
// ---------------------------------------------------------------------------

/// Length of the longest prefix of `data` that ends at an escape-sequence
/// and UTF-8 character boundary. Bytes past it belong to an incomplete
/// sequence and must be withheld until the rest arrives, so that overlay
/// bytes emitted after the prefix can never be spliced into a sequence.
fn complete_prefix_len(data: &[u8]) -> usize {
    let mut end = 0;
    let mut i = 0;
    while i < data.len() {
        if data[i] == 0x1b {
            match escape_sequence_len(&data[i..]) {
                Some(len) => {
                    i += len;
                    end = i;
                }
                None => return end,
            }
        } else {
            let len = utf8_len(data[i]);
            if i + len > data.len() {
                return end;
            }
            i += len;
            end = i;
        }
    }
    end
}

/// Length of the escape sequence starting at `data[0] == ESC`, or `None` if
/// it continues past the end of `data`.
fn escape_sequence_len(data: &[u8]) -> Option<usize> {
    match *data.get(1)? {
        // ESC ESC: the first ESC is cancelled; re-scan from the second.
        0x1b => Some(1),
        // CSI: parameter/intermediate bytes, then a final byte in @..~.
        b'[' => data[2..]
            .iter()
            .position(|byte| (0x40..=0x7e).contains(byte))
            .map(|pos| pos + 3),
        // OSC: terminated by BEL, C1 ST, or ESC \.
        b']' => string_terminator(&data[2..], true).map(|pos| pos + 2),
        // DCS / SOS / PM / APC: terminated by ST only.
        b'P' | b'X' | b'^' | b'_' => string_terminator(&data[2..], false).map(|pos| pos + 2),
        // SS3: exactly one more byte.
        b'O' => data.get(2).map(|_| 3),
        // ESC + intermediates (SP../), then a final byte in 0..~.
        0x20..=0x2f => data[1..]
            .iter()
            .position(|byte| !(0x20..=0x2f).contains(byte))
            .and_then(|pos| (0x30..=0x7e).contains(&data[1 + pos]).then_some(pos + 2)),
        // ESC + single character.
        _ => Some(2),
    }
}

/// Offset just past the string terminator (BEL if `allow_bel`, C1 ST, or
/// ESC \) in `data`, or `None` if unterminated.
fn string_terminator(data: &[u8], allow_bel: bool) -> Option<usize> {
    let mut i = 0;
    while i < data.len() {
        match data[i] {
            0x07 if allow_bel => return Some(i + 1),
            0x9c => return Some(i + 1),
            0x1b => {
                if *data.get(i + 1)? == b'\\' {
                    return Some(i + 2);
                }
                i += 1;
            }
            _ => i += 1,
        }
    }
    None
}

fn utf8_len(byte: u8) -> usize {
    match byte {
        0xc0..=0xdf => 2,
        0xe0..=0xef => 3,
        0xf0..=0xf7 => 4,
        _ => 1,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rstest::rstest;

    // -- Safe splitting --------------------------------------------------

    #[rstest]
    #[case::plain_text(b"hello".as_slice(), 5)]
    #[case::trailing_esc(b"abc\x1b".as_slice(), 3)]
    #[case::trailing_csi_start(b"abc\x1b[".as_slice(), 3)]
    #[case::trailing_csi_params(b"abc\x1b[31".as_slice(), 3)]
    #[case::complete_csi(b"abc\x1b[31m".as_slice(), 8)]
    #[case::csi_then_text(b"\x1b[31mred".as_slice(), 8)]
    #[case::unterminated_osc(b"\x1b]0;title".as_slice(), 0)]
    #[case::osc_bel(b"\x1b]0;title\x07x".as_slice(), 11)]
    #[case::osc_esc_backslash(b"\x1b]0;t\x1b\\".as_slice(), 7)]
    #[case::osc_pending_st(b"\x1b]0;t\x1b".as_slice(), 0)]
    #[case::incomplete_utf8(b"caf\xc3".as_slice(), 3)]
    #[case::complete_utf8(b"caf\xc3\xa9".as_slice(), 5)]
    #[case::split_wide_char(b"\xe6\x97".as_slice(), 0)]
    #[case::esc_esc_csi(b"\x1b\x1b[A".as_slice(), 4)]
    #[case::ss3_partial(b"\x1bO".as_slice(), 0)]
    #[case::ss3_complete(b"\x1bOA".as_slice(), 3)]
    #[case::esc_intermediate(b"\x1b(B".as_slice(), 3)]
    #[case::esc_intermediate_partial(b"\x1b(".as_slice(), 0)]
    #[case::esc_single(b"\x1b7".as_slice(), 2)]
    #[case::dcs_unterminated(b"\x1bPdata".as_slice(), 0)]
    #[case::dcs_terminated(b"\x1bPd\x1b\\".as_slice(), 5)]
    fn splits_at_sequence_boundaries(#[case] data: &[u8], #[case] expected: usize) {
        assert_eq!(complete_prefix_len(data), expected);
    }

    // -- Compositor behaviour ---------------------------------------------

    fn compositor() -> Compositor<Vec<u8>> {
        Compositor::new(
            10,
            40,
            Vec::new(),
            Arc::new(OverlayFlags::default()),
            true,
        )
    }

    fn content(line: &str, suggestions: &[&str], selected: usize) -> OverlayContent {
        OverlayContent {
            line: line.to_string(),
            suggestions: suggestions.iter().map(ToString::to_string).collect(),
            selected,
        }
    }

    /// Replay everything the compositor wrote into a fresh checker terminal
    /// and return its screen contents.
    fn displayed(compositor: &Compositor<Vec<u8>>) -> vt100::Parser {
        let mut checker = vt100::Parser::new(10, 40, 0);
        checker.process(&compositor.out);
        checker
    }

    fn screen_text(parser: &vt100::Parser) -> String {
        parser.screen().contents()
    }

    #[rstest]
    fn passthrough_without_overlay_is_byte_identical() {
        let mut c = compositor();
        c.apply_pty(b"plain \x1b[31mred\x1b[0m text\r\n");
        assert_eq!(c.out, b"plain \x1b[31mred\x1b[0m text\r\n");
    }

    #[rstest]
    fn withholds_partial_escape_until_complete() {
        let mut c = compositor();
        c.apply_pty(b"before\x1b[3");
        assert_eq!(c.out, b"before");
        c.apply_pty(b"1mred");
        assert_eq!(c.out, b"before\x1b[31mred");
    }

    #[rstest]
    fn overlay_never_splits_an_escape_sequence() {
        let mut c = compositor();
        c.apply_pty(b"$ g\x1b[3"); // chunk ends mid-SGR
        c.set_overlay(Some(content("g", &["git status"], 0)));
        c.apply_pty(b"1mX");

        // The checker terminal must see the SGR sequence intact: the X is
        // red, not preceded by stray popup bytes inside the sequence.
        let checker = displayed(&c);
        let screen = checker.screen();
        let x_cell = screen.cell(0, 3).expect("cell");
        assert_eq!(x_cell.contents(), "X");
        assert_eq!(x_cell.fgcolor(), vt100::Color::Idx(1));
    }

    #[rstest]
    fn popup_draws_below_cursor_and_erases_cleanly() {
        let mut c = compositor();
        c.apply_pty(b"$ git st");
        c.set_overlay(Some(content("git st", &["git status", "git stash"], 0)));

        let shown = screen_text(&displayed(&c));
        assert!(shown.contains("git status"), "popup visible: {shown:?}");
        assert!(shown.contains("git stash"));
        assert!(c.flags.popup.load(Ordering::Acquire));

        c.set_overlay(None);
        let shown = screen_text(&displayed(&c));
        assert_eq!(shown.trim_end(), "$ git st");
        assert!(!c.flags.popup.load(Ordering::Acquire));
    }

    #[rstest]
    fn single_prefix_suggestion_shows_ghost_without_dropdown() {
        let mut c = compositor();
        c.apply_pty(b"$ git st");
        c.set_overlay(Some(content("git st", &["git status"], 0)));

        assert!(c.flags.ghost.load(Ordering::Acquire));
        assert!(
            !c.flags.popup.load(Ordering::Acquire),
            "no dropdown when the lone suggestion is fully shown as ghost"
        );
        let shown = screen_text(&displayed(&c));
        assert_eq!(shown.trim_end(), "$ git status");
    }

    #[rstest]
    fn single_fuzzy_suggestion_still_shows_dropdown() {
        let mut c = compositor();
        c.apply_pty(b"$ stat");
        // Not a prefix of the typed line: no ghost is possible, so the
        // dropdown is the only way to see the suggestion.
        c.set_overlay(Some(content("stat", &["git status"], 0)));

        assert!(c.flags.popup.load(Ordering::Acquire));
        assert!(!c.flags.ghost.load(Ordering::Acquire));
    }

    #[rstest]
    fn tiny_screen_popup_never_covers_the_command_line() {
        let flags = Arc::new(OverlayFlags::default());
        let mut c: Compositor<Vec<u8>> = Compositor::new(4, 40, Vec::new(), flags, true);
        // Cursor on row 1 of a 4-row screen: neither side fits 3 rows.
        c.apply_pty(b"one\r\n$ g");
        let all = ["git a", "git b", "git c"];
        c.set_overlay(Some(content("g", &all, 0)));

        let region = c.drawn.expect("popup drawn");
        let cursor_row = c.parser.screen().cursor_position().0;
        for row in region.first_row..region.first_row + region.count {
            assert_ne!(row, cursor_row, "popup row overlaps the command line");
        }
    }

    #[rstest]
    fn ghost_text_draws_at_cursor() {
        let mut c = compositor();
        c.apply_pty(b"$ git st");
        c.set_overlay(Some(content("git st", &["git status"], 0)));

        let checker = displayed(&c);
        let row: String = (0..40)
            .filter_map(|col| checker.screen().cell(0, col).map(|c| c.contents().to_string()))
            .collect();
        assert!(row.starts_with("$ git status"), "ghost completes the line: {row:?}");
        assert!(c.flags.ghost.load(Ordering::Acquire));
        // Checker cursor must be parked back over the ghost, where the
        // shell's real cursor is.
        assert_eq!(checker.screen().cursor_position(), (0, 8));
    }

    #[rstest]
    fn erase_survives_scrolling_output() {
        let mut c = compositor();
        c.apply_pty(b"$ git st");
        c.set_overlay(Some(content("git st", &["git status", "git stash"], 0)));

        // A burst of output that scrolls the screen several lines while the
        // popup is visible, then the overlay is dropped.
        let burst = "\r\n".to_string() + &"line\r\n".repeat(12);
        c.apply_pty(burst.as_bytes());
        c.set_overlay(None);

        // The displayed screen must exactly match the model: no popup or
        // ghost remnants anywhere.
        let checker = displayed(&c);
        assert_eq!(screen_text(&checker), c.parser.screen().contents());
    }

    #[rstest]
    fn popup_windows_follow_selection() {
        let mut c = compositor();
        c.apply_pty(b"$ g");
        let all: Vec<String> = (0..9).map(|i| format!("git command-{i}")).collect();
        let all_refs: Vec<&str> = all.iter().map(String::as_str).collect();

        c.set_overlay(Some(content("g", &all_refs, 7)));
        let shown = screen_text(&displayed(&c));
        assert!(shown.contains("command-7"), "selected visible: {shown:?}");
        assert!(!shown.contains("command-0"), "window scrolled: {shown:?}");
    }

    #[rstest]
    fn no_paint_on_alternate_screen() {
        let mut c = compositor();
        c.apply_pty(b"$ vim\r\n\x1b[?1049h\x1b[2JEDITOR");
        c.set_overlay(Some(content("x", &["xyz", "xzz"], 0)));

        let shown = screen_text(&displayed(&c));
        assert!(!shown.contains("xyz"), "no popup over alt screen: {shown:?}");
        assert!(!c.flags.popup.load(Ordering::Acquire));
    }

    #[rstest]
    fn wide_characters_truncate_by_display_width() {
        let mut c = compositor();
        c.apply_pty(b"$ ");
        // 40-col screen; suggestion of 30 wide chars = 60 cells must clamp.
        // (Two suggestions so the dropdown actually renders.)
        let wide = "\u{6f22}".repeat(30);
        c.set_overlay(Some(content("", &[&wide, "short"], 0)));

        let checker = displayed(&c);
        // Rows 1-2 hold the popup; the clamped wide row must not have
        // wrapped past it onto row 3.
        let row3: String = (0..40)
            .filter_map(|col| checker.screen().cell(3, col).map(|c| c.contents().to_string()))
            .collect();
        assert!(row3.trim().is_empty(), "no wrap past popup rows: {row3:?}");
    }

    #[rstest]
    fn restores_shell_colors_after_paint() {
        let mut c = compositor();
        // Shell leaves red as the active drawing attribute mid-stream.
        c.apply_pty(b"$ \x1b[31m");
        c.set_overlay(Some(content("x", &["xyz"], 0)));
        c.set_overlay(None);
        c.apply_pty(b"R");

        let checker = displayed(&c);
        let cell = checker.screen().cell(0, 2).expect("cell");
        assert_eq!(cell.contents(), "R");
        assert_eq!(
            cell.fgcolor(),
            vt100::Color::Idx(1),
            "overlay must not clobber the shell's active SGR state"
        );
    }

    #[rstest]
    fn resize_drops_overlay_bookkeeping() {
        let mut c = compositor();
        c.apply_pty(b"$ git st");
        c.set_overlay(Some(content("git st", &["git status"], 0)));
        c.resize(20, 60);
        assert!(!c.flags.popup.load(Ordering::Acquire));
        assert!(c.drawn.is_none() && c.ghost_row.is_none());
    }

    #[rstest]
    fn flush_pending_emits_withheld_tail() {
        let mut c = compositor();
        c.apply_pty(b"abc\x1b[3");
        c.flush_pending();
        assert_eq!(c.out, b"abc\x1b[3");
    }
}
