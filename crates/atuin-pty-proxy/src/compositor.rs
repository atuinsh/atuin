//! Single-writer compositor for the suggestion overlay.
//!
//! All terminal output flows through one lock, so overlay bytes only ever
//! land at escape-sequence boundaries and erase/repaint always run while
//! the physical screen matches the vt100 model. The model is "the screen
//! without the overlay" — overlay bytes are never fed to the parser — so
//! erasing means repainting covered rows from it.

use std::io::Write;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

const MAX_POPUP_ROWS: usize = 5;

/// Below this there is no room to render anything useful.
const MIN_POPUP_WIDTH: usize = 4;

/// Withheld-tail cap: a malformed, never-terminated escape sequence flushes
/// once it exceeds this rather than buffering forever.
const MAX_TAIL_BYTES: usize = 8192;

const SELECTED_STYLE: &[u8] = b"\x1b[0m\x1b[48;5;24m\x1b[97m";
const UNSELECTED_STYLE: &[u8] = b"\x1b[0m\x1b[48;5;236m\x1b[37m";
const GHOST_STYLE: &[u8] = b"\x1b[0m\x1b[38;5;242m";

/// What the suggestion UI wants painted. Suggestions are shared, not
/// cloned: every navigation keystroke re-sends this.
#[derive(Clone, Default)]
pub(crate) struct OverlayContent {
    /// Plain-text command line currently being edited.
    pub(crate) line: String,
    pub(crate) suggestions: std::sync::Arc<[String]>,
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

/// Recover a poisoned lock: the compositor and popup state stay usable
/// after a panic elsewhere, and eating output would freeze the terminal.
pub(crate) fn lock_unpoisoned<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

pub(crate) struct Compositor<W: Write> {
    parser: vt100::Parser,
    out: W,
    /// Withhold tails ending mid escape sequence / UTF-8 character so a
    /// later overlay paint can't splice into one; off when no overlay can paint.
    split_partials: bool,
    tail: Vec<u8>,
    content: Option<OverlayContent>,
    drawn: Option<DrawnRegion>,
    ghost_row: Option<u16>,
    window_offset: usize,
    flags: Arc<OverlayFlags>,
    /// Reused per-chunk paint buffers, so the overlay-active path allocates
    /// nothing steady-state.
    scratch: Vec<u8>,
    repaint_scratch: Vec<u8>,
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
            scratch: Vec::new(),
            repaint_scratch: Vec::new(),
        }
    }

    pub(crate) fn screen(&self) -> &vt100::Screen {
        self.parser.screen()
    }

    /// Apply a chunk of pty output: erase the overlay, forward the chunk,
    /// repaint the popup — one write per chunk, and with no overlay in
    /// play a pure pass-through (no copy, no allocation).
    ///
    /// Ghost text is not repainted here: the chunk may have moved the line;
    /// the suggestion pass after the echo redraws it.
    pub(crate) fn apply_pty(&mut self, data: &[u8]) -> std::io::Result<()> {
        let overlay_idle =
            self.drawn.is_none() && self.ghost_row.is_none() && self.content.is_none();

        // Fast path: nothing painted or pending. The boundary scan still
        // runs pre-write so a later paint can't splice into a sequence
        // this chunk left unterminated.
        if overlay_idle
            && self.tail.is_empty()
            && (!self.split_partials || complete_prefix_len(data) == data.len())
        {
            self.out.write_all(data)?;
            self.out.flush()?;
            self.parser.process(data);
            return Ok(());
        }

        let mut buf = std::mem::take(&mut self.scratch);
        buf.clear();
        self.erase_into(&mut buf);

        self.tail.extend_from_slice(data);
        let mut ready = std::mem::take(&mut self.tail);
        let ready_len = if self.split_partials && ready.len() <= MAX_TAIL_BYTES {
            complete_prefix_len(&ready)
        } else {
            ready.len()
        };
        self.tail = ready.split_off(ready_len);

        // Parsing a chunk costs microseconds, so erase + data + repaint go
        // out as one write instead of paying two syscall pairs per chunk.
        self.parser.process(&ready);
        let mut repaint = std::mem::take(&mut self.repaint_scratch);
        repaint.clear();
        self.draw_into(&mut repaint, false);

        let result = if buf.is_empty() && repaint.is_empty() {
            self.out.write_all(&ready).and_then(|()| self.out.flush())
        } else {
            buf.extend_from_slice(&ready);
            buf.extend_from_slice(&repaint);
            self.out.write_all(&buf).and_then(|()| self.out.flush())
        };
        self.update_flags();
        self.scratch = buf;
        self.repaint_scratch = repaint;
        result
    }

    /// Flush any withheld partial-sequence tail (e.g. on shutdown).
    pub(crate) fn flush_pending(&mut self) {
        if self.tail.is_empty() {
            return;
        }
        let tail = std::mem::take(&mut self.tail);
        self.parser.process(&tail);
        let _ = self.flush(&tail);
    }

    /// Replace the overlay content and repaint.
    pub(crate) fn set_overlay(&mut self, content: Option<OverlayContent>) {
        let mut buf = std::mem::take(&mut self.scratch);
        buf.clear();
        self.erase_into(&mut buf);
        self.content = content.filter(|content| !content.suggestions.is_empty());
        if self.content.is_none() {
            self.window_offset = 0;
        }
        self.draw_into(&mut buf, true);
        let _ = self.flush(&buf);
        self.scratch = buf;
    }

    /// Resize reflow scrambles screen and model unpredictably, so restoring
    /// covered rows from the model is unreliable here. Best effort: blank
    /// the popup's old below-cursor rows (they were blank before it drew);
    /// ghost text sits on the prompt line, which shells repaint after
    /// SIGWINCH anyway.
    pub(crate) fn resize(&mut self, rows: u16, cols: u16) {
        let drawn = self.drawn.take();
        let (old_cursor_row, _) = self.parser.screen().cursor_position();

        self.content = None;
        self.ghost_row = None;
        self.window_offset = 0;
        self.parser.screen_mut().set_size(rows, cols);

        let mut buf = Vec::new();
        if let Some(region) = drawn
            && region.first_row > old_cursor_row
        {
            buf.extend_from_slice(b"\x1b[0m");
            let end = region.first_row.saturating_add(region.count).min(rows);
            for row in region.first_row..end {
                move_to(&mut buf, row, 0);
                buf.extend_from_slice(b"\x1b[2K");
            }
            hand_back(&mut buf, self.parser.screen());
        }
        let _ = self.flush(&buf);
    }

    fn flush(&mut self, buf: &[u8]) -> std::io::Result<()> {
        let result = if buf.is_empty() {
            Ok(())
        } else {
            self.out.write_all(buf).and_then(|()| self.out.flush())
        };
        self.update_flags();
        result
    }

    fn update_flags(&self) {
        self.flags
            .popup
            .store(self.drawn.is_some(), Ordering::Release);
        self.flags
            .ghost
            .store(self.ghost_row.is_some(), Ordering::Release);
    }

    /// Restore every row the overlay covers from the model; valid only
    /// while screen == model + overlay, which erase-before-apply guarantees.
    fn erase_into(&mut self, buf: &mut Vec<u8>) {
        let drawn = self.drawn.take();
        let ghost_row = self.ghost_row.take();
        if drawn.is_none() && ghost_row.is_none() {
            return;
        }

        let covered = |row: u16| {
            ghost_row == Some(row)
                || drawn.is_some_and(|region| {
                    row >= region.first_row && row < region.first_row.saturating_add(region.count)
                })
        };
        let last = drawn
            .map(|region| region.first_row.saturating_add(region.count))
            .into_iter()
            .chain(ghost_row.map(|row| row + 1))
            .max()
            .unwrap_or(0);

        // One rows_formatted pass over popup and ghost rows together: the
        // iterator formats every row it yields, so passes are what cost.
        let screen = self.parser.screen();
        let (_, cols) = screen.size();
        for (row, bytes) in screen
            .rows_formatted(0, cols)
            .enumerate()
            .take(last as usize)
        {
            let row = row as u16;
            if covered(row) {
                restore_row(buf, row, &bytes);
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
        if rows < 2 || (cols as usize) < MIN_POPUP_WIDTH {
            return;
        }

        let selected = content.selected.min(content.suggestions.len() - 1);
        let ghost_suffix = content.suggestions[selected]
            .strip_prefix(content.line.as_str())
            .filter(|suffix| !suffix.is_empty());

        // A lone suggestion already shown in full as ghost text needs no dropdown.
        let solo_ghost = content.suggestions.len() == 1 && ghost_suffix.is_some();

        buf.extend_from_slice(b"\x1b[?25l");
        if !solo_ghost {
            self.drawn = draw_popup(buf, screen, content, selected, &mut self.window_offset);
        }

        // Ghost only into cells the model says are blank, so it never
        // covers mid-line edits or a right-side prompt.
        if with_ghost && let Some(suffix) = ghost_suffix {
            let budget = blank_cells_after_cursor(screen) as usize;
            let rollback = buf.len();
            move_to(buf, cursor_row, cursor_col);
            buf.extend_from_slice(GHOST_STYLE);
            let text_start = buf.len();
            write_fitted(buf, suffix, budget);
            if buf.len() == text_start {
                buf.truncate(rollback);
            } else {
                self.ghost_row = Some(cursor_row);
            }
        }

        hand_back(buf, screen);
    }
}

/// Paint the dropdown rows; returns the region drawn, if any.
///
/// `window_offset` persists across repaints so the visible slice doesn't
/// jump while the selection moves within it.
fn draw_popup(
    buf: &mut Vec<u8>,
    screen: &vt100::Screen,
    content: &OverlayContent,
    selected: usize,
    window_offset: &mut usize,
) -> Option<DrawnRegion> {
    let (rows, cols) = screen.size();
    let (cursor_row, cursor_col) = screen.cursor_position();
    let mut visible = content.suggestions.len().min(MAX_POPUP_ROWS);

    // Below if it fits, else above, else the roomier side shrunk — never
    // over the command line itself.
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
    if visible == 0 {
        return None;
    }

    // Slide the selection window to keep the highlight visible.
    if selected < *window_offset {
        *window_offset = selected;
    }
    if selected >= *window_offset + visible {
        *window_offset = selected + 1 - visible;
    }
    *window_offset = (*window_offset).min(content.suggestions.len() - visible);
    let window = &content.suggestions[*window_offset..*window_offset + visible];

    // Left-align with the start of the typed line, sliding left to fit.
    let line_width = content.line.width().min(u16::MAX as usize) as u16;
    let width = window
        .iter()
        .map(|s| s.width() + 2)
        .max()
        .unwrap_or(2)
        .clamp(MIN_POPUP_WIDTH, cols as usize);
    let col = cursor_col
        .saturating_sub(line_width)
        .min(cols.saturating_sub(width as u16));

    for (i, suggestion) in window.iter().enumerate() {
        let row = first_row + i as u16;
        move_to(buf, row, col);
        buf.extend_from_slice(if *window_offset + i == selected {
            SELECTED_STYLE
        } else {
            UNSELECTED_STYLE
        });

        buf.push(b' ');
        let used = 1 + write_fitted(buf, suggestion, width - 2);
        buf.resize(buf.len() + (width - used), b' ');
    }

    Some(DrawnRegion {
        first_row,
        count: visible as u16,
    })
}

/// Append the longest printable prefix of `text` fitting `budget` display
/// cells; returns the width appended. Control characters render as
/// placeholders — a stray `\n` in a suggestion would otherwise break out of
/// the overlay entirely.
fn write_fitted(buf: &mut Vec<u8>, text: &str, budget: usize) -> usize {
    let mut used = 0;
    for ch in text.chars().map(printable) {
        let ch_width = ch.width().unwrap_or(0);
        if used + ch_width > budget {
            break;
        }
        let mut utf8 = [0u8; 4];
        buf.extend_from_slice(ch.encode_utf8(&mut utf8).as_bytes());
        used += ch_width;
    }
    used
}

/// Rewrite one screen row from pre-formatted model bytes, which skip
/// default cells with cursor jumps — hence the clear first.
fn restore_row(buf: &mut Vec<u8>, row: u16, bytes: &[u8]) {
    buf.extend_from_slice(b"\x1b[0m");
    move_to(buf, row, 0);
    buf.extend_from_slice(b"\x1b[2K");
    buf.extend_from_slice(bytes);
}

/// Restore the shell's cursor state, then its drawing attributes — in that
/// order, since repositioning the cursor can itself alter attributes.
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

/// Contiguous blank cells at and after the cursor — the room ghost text may
/// draw into without covering real content.
fn blank_cells_after_cursor(screen: &vt100::Screen) -> u16 {
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

/// Longest prefix ending on an escape-sequence / UTF-8 boundary; the rest
/// must be withheld so later overlay bytes can't splice into a sequence.
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
    #[case::incomplete_utf8(b"abc\xc3".as_slice(), 3)]
    #[case::complete_utf8(b"abc\xc3\xa9".as_slice(), 5)]
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
        Compositor::new(10, 40, Vec::new(), Arc::new(OverlayFlags::default()), true)
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

    /// Join one row's cell contents, popup padding included.
    fn row_text(parser: &vt100::Parser, row: u16) -> String {
        let (_, cols) = parser.screen().size();
        (0..cols)
            .filter_map(|col| {
                parser
                    .screen()
                    .cell(row, col)
                    .map(|c| c.contents().to_string())
            })
            .collect()
    }

    #[rstest]
    fn passthrough_without_overlay_is_byte_identical() {
        let mut c = compositor();
        c.apply_pty(b"plain \x1b[31mred\x1b[0m text\r\n").unwrap();
        assert_eq!(c.out, b"plain \x1b[31mred\x1b[0m text\r\n");
    }

    /// The write-first fast path must still keep the model in sync, or the
    /// next erase/draw/snapshot would composite against stale state.
    #[rstest]
    fn fast_path_still_feeds_the_model() {
        let mut c = compositor();
        c.apply_pty(b"$ hello").unwrap();
        assert_eq!(c.out, b"$ hello");
        assert_eq!(c.parser.screen().contents().trim_end(), "$ hello");
        assert_eq!(c.parser.screen().cursor_position(), (0, 7));
    }

    #[rstest]
    fn withholds_partial_escape_until_complete() {
        let mut c = compositor();
        c.apply_pty(b"before\x1b[3").unwrap();
        assert_eq!(c.out, b"before");
        c.apply_pty(b"1mred").unwrap();
        assert_eq!(c.out, b"before\x1b[31mred");
    }

    #[rstest]
    fn overlay_never_splits_an_escape_sequence() {
        let mut c = compositor();
        c.apply_pty(b"$ g\x1b[3").unwrap(); // chunk ends mid-SGR
        c.set_overlay(Some(content("g", &["git status"], 0)));
        c.apply_pty(b"1mX").unwrap();

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
        c.apply_pty(b"$ git st").unwrap();
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
        c.apply_pty(b"$ git st").unwrap();
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
        c.apply_pty(b"$ stat").unwrap();
        // Not a prefix of the typed line: no ghost is possible, so the
        // dropdown is the only way to see the suggestion.
        c.set_overlay(Some(content("stat", &["git status"], 0)));

        assert!(c.flags.popup.load(Ordering::Acquire));
        assert!(!c.flags.ghost.load(Ordering::Acquire));
    }

    #[rstest]
    fn tiny_screen_popup_never_covers_the_command_line() {
        let flags = Arc::new(OverlayFlags::default());
        let mut c: Compositor<Vec<u8>> = Compositor::new(4, 40, Vec::new(), flags.clone(), true);
        // Cursor on row 1 of a 4-row screen: neither side fits 3 rows.
        c.apply_pty(b"one\r\n$ g").unwrap();
        let all = ["git a", "git b", "git c"];
        c.set_overlay(Some(content("g", &all, 0)));

        assert!(
            flags.popup.load(Ordering::Acquire),
            "popup shrunk, not dropped"
        );
        let checker = vt100::Parser::new(4, 40, 0);
        let mut checker = checker;
        checker.process(&c.out);
        let line = row_text(&checker, 1);
        assert!(
            line.starts_with("$ g"),
            "command line row untouched: {line:?}"
        );
    }

    #[rstest]
    fn ghost_text_draws_at_cursor() {
        let mut c = compositor();
        c.apply_pty(b"$ git st").unwrap();
        c.set_overlay(Some(content("git st", &["git status"], 0)));

        let checker = displayed(&c);
        let row = row_text(&checker, 0);
        assert!(
            row.starts_with("$ git status"),
            "ghost completes the line: {row:?}"
        );
        assert!(c.flags.ghost.load(Ordering::Acquire));
        // Checker cursor must be parked back over the ghost, where the
        // shell's real cursor is.
        assert_eq!(checker.screen().cursor_position(), (0, 8));
    }

    #[rstest]
    fn erase_survives_scrolling_output() {
        let mut c = compositor();
        c.apply_pty(b"$ git st").unwrap();
        c.set_overlay(Some(content("git st", &["git status", "git stash"], 0)));

        // A burst of output that scrolls the screen several lines while the
        // popup is visible, then the overlay is dropped.
        let burst = "\r\n".to_string() + &"line\r\n".repeat(12);
        c.apply_pty(burst.as_bytes()).unwrap();
        c.set_overlay(None);

        // The displayed screen must exactly match the model: no popup or
        // ghost remnants anywhere.
        let checker = displayed(&c);
        assert_eq!(screen_text(&checker), c.parser.screen().contents());
    }

    #[rstest]
    fn popup_windows_follow_selection() {
        let mut c = compositor();
        c.apply_pty(b"$ g").unwrap();
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
        c.apply_pty(b"$ vim\r\n\x1b[?1049h\x1b[2JEDITOR").unwrap();
        c.set_overlay(Some(content("x", &["xyz", "xzz"], 0)));

        let shown = screen_text(&displayed(&c));
        assert!(
            !shown.contains("xyz"),
            "no popup over alt screen: {shown:?}"
        );
        assert!(!c.flags.popup.load(Ordering::Acquire));
    }

    #[rstest]
    fn wide_characters_truncate_by_display_width() {
        let mut c = compositor();
        c.apply_pty(b"$ ").unwrap();
        // 40-col screen; suggestion of 30 wide chars = 60 cells must clamp.
        // (Two suggestions so the dropdown actually renders.)
        let wide = "\u{6f22}".repeat(30);
        c.set_overlay(Some(content("", &[&wide, "short"], 0)));

        let checker = displayed(&c);
        // Rows 1-2 hold the popup; the clamped wide row must not have
        // wrapped past it onto row 3.
        let row3 = row_text(&checker, 3);
        assert!(row3.trim().is_empty(), "no wrap past popup rows: {row3:?}");
    }

    #[rstest]
    fn restores_shell_colors_after_paint() {
        let mut c = compositor();
        // Shell leaves red as the active drawing attribute mid-stream.
        c.apply_pty(b"$ \x1b[31m").unwrap();
        c.set_overlay(Some(content("x", &["xyz"], 0)));
        c.set_overlay(None);
        c.apply_pty(b"R").unwrap();

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
    fn resize_blanks_stale_popup_rows() {
        let mut c = compositor();
        c.apply_pty(b"$ st").unwrap();
        // Non-prefix matches: dropdown only, drawn below the cursor.
        c.set_overlay(Some(content("st", &["git status", "git stash"], 0)));
        assert!(screen_text(&displayed(&c)).contains("git status"));

        c.resize(10, 40);
        let shown = screen_text(&displayed(&c));
        assert!(
            !shown.contains("git status"),
            "popup rows blanked on resize: {shown:?}"
        );
    }

    #[rstest]
    fn resize_drops_overlay_bookkeeping() {
        let mut c = compositor();
        c.apply_pty(b"$ git st").unwrap();
        c.set_overlay(Some(content("git st", &["git status"], 0)));
        c.resize(20, 60);
        assert!(!c.flags.popup.load(Ordering::Acquire));
        assert!(c.drawn.is_none() && c.ghost_row.is_none());
    }

    #[rstest]
    fn flush_pending_emits_withheld_tail() {
        let mut c = compositor();
        c.apply_pty(b"abc\x1b[3").unwrap();
        c.flush_pending();
        assert_eq!(c.out, b"abc\x1b[3");
    }
}
