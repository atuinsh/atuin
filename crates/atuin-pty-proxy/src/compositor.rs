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

use crate::suggest::{Suggestion, SuggestionSource, SyntaxClass, SyntaxSpan};

const MAX_POPUP_ROWS: usize = 5;

/// Below this there is no room to render anything useful.
const MIN_POPUP_WIDTH: usize = 4;

/// Withheld-tail cap: a malformed, never-terminated escape sequence flushes
/// once it exceeds this rather than buffering forever.
const MAX_TAIL_BYTES: usize = 8192;

const SELECTED_STYLE: &[u8] = b"\x1b[0m\x1b[48;5;24m\x1b[97m";
const SELECTED_FG: &[u8] = b"\x1b[97m";
const UNSELECTED_STYLE: &[u8] = b"\x1b[0m\x1b[48;5;236m\x1b[37m";
const UNSELECTED_FG: &[u8] = b"\x1b[37m";
const GHOST_STYLE: &[u8] = b"\x1b[0m\x1b[38;5;242m";

/// Foreground per syntax class, mirroring the TUI theme's defaults —
/// plain ANSI palette colors, so both UIs follow the terminal scheme.
/// `None` (plain text, operators) keeps the row's own foreground.
fn class_fg(class: SyntaxClass) -> Option<&'static [u8]> {
    match class {
        SyntaxClass::Plain => None,
        SyntaxClass::Command => Some(b"\x1b[92m"),
        SyntaxClass::Flag => Some(b"\x1b[36m"),
        SyntaxClass::String => Some(b"\x1b[33m"),
        SyntaxClass::Variable => Some(b"\x1b[95m"),
        SyntaxClass::Comment => Some(b"\x1b[90m"),
    }
}

/// Nerd-font source icons, one cell each. Terminals without a nerd font
/// show a replacement glyph in that cell; the suggestion itself is intact.
const HISTORY_ICON: char = '\u{f1da}'; //  (fa-history)
const COMPLETION_ICON: char = '\u{f120}'; //  (fa-terminal)
/// Leading pad + icon + separating space + one trailing pad cell.
const ROW_CHROME_WIDTH: usize = 4;

/// Scrollbar glyphs for the popup's right edge, shown only when there are
/// more suggestions than visible rows.
const SCROLL_THUMB: char = '█';
const SCROLL_TRACK: char = '│';

fn source_icon(source: SuggestionSource) -> char {
    match source {
        SuggestionSource::History => HISTORY_ICON,
        SuggestionSource::Completion => COMPLETION_ICON,
    }
}

/// Dropdown rows restart at the current shell word: a row
/// cut mid-word ("atus --short") is harder to read than one showing the
/// whole token ("status --short"). `line_head` is the typed line up to the
/// token start; non-prefix suggestions fall back to their full text.
fn shown_from_token<'a>(s: &'a Suggestion, line_head: &str) -> &'a str {
    s.text.strip_prefix(line_head).unwrap_or(&s.text)
}

/// Byte offset where the current shell word starts.
fn token_start(line: &str) -> usize {
    let mut start = 0;
    let mut quote = None;
    let mut escaped = false;

    for (i, ch) in line.char_indices() {
        if escaped {
            escaped = false;
            continue;
        }

        match quote {
            Some('\'') => {
                if ch == '\'' {
                    quote = None;
                }
            }
            Some('"') => match ch {
                '"' => quote = None,
                '\\' => escaped = true,
                _ => {}
            },
            _ => match ch {
                '\'' | '"' => quote = Some(ch),
                '\\' => escaped = true,
                _ if ch.is_whitespace() => start = i + ch.len_utf8(),
                _ => {}
            },
        }
    }

    start
}

/// What the suggestion UI wants painted. Suggestions are shared, not
/// cloned: every navigation keystroke re-sends this.
#[derive(Clone, Default)]
pub(crate) struct OverlayContent {
    /// Plain-text command line currently being edited.
    pub(crate) line: String,
    pub(crate) suggestions: std::sync::Arc<[Suggestion]>,
    pub(crate) selected: usize,
}

/// Lock-free view of what's currently painted, for the stdin thread's key
/// interception checks.
#[derive(Default)]
pub(crate) struct OverlayFlags {
    pub(crate) popup: AtomicBool,
    pub(crate) ghost: AtomicBool,
    /// A cursor-position query is in flight; the stdin filter must consume
    /// the reply before it reaches the shell as junk keystrokes.
    pub(crate) resync: AtomicBool,
}

/// CPR query plus DA1 fence: the terminal answers in order, so the last
/// cursor report before the DA1 reply is the answer to this query, and
/// earlier reports belong to queries the shell sent (p10k, iTerm2).
pub(crate) const CURSOR_HANDSHAKE: &[u8] = b"\x1b[6n\x1b[c";

#[derive(Clone, Copy)]
struct DrawnRegion {
    first_row: u16,
    count: u16,
}

/// Where the open popup is pinned. Kept while the same token is being
/// completed, so rows growing and shrinking as candidates change can't
/// shuffle or shrink the popup under the user's eyes.
#[derive(Clone, Copy)]
struct PopupAnchor {
    /// Byte offset where the current shell word begins.
    token_start: usize,
    /// Column actually drawn at, after any right-edge clamping.
    col: u16,
    /// Drawn width; grow-only while the token is unchanged.
    width: usize,
}

/// Recover a poisoned lock: the compositor and popup state stay usable
/// after a panic elsewhere, and eating output would freeze the terminal.
pub(crate) fn lock_unpoisoned<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// vt100 panics on degenerate grids (line wrap with a single row, wide
/// glyphs with a single column), so the model never goes below this; the
/// real terminal size is kept separately to gate overlay drawing, which
/// is pointless at such sizes anyway.
pub(crate) const MODEL_FLOOR: u16 = 4;

pub(crate) struct Compositor<W: Write> {
    parser: vt100::Parser,
    /// The terminal's actual size; the model may be floored above it.
    real_rows: u16,
    real_cols: u16,
    out: W,
    /// Withhold tails ending mid escape sequence / UTF-8 character so a
    /// later overlay paint can't splice into one; off when no overlay can paint.
    split_partials: bool,
    tail: Vec<u8>,
    content: Option<OverlayContent>,
    drawn: Option<DrawnRegion>,
    ghost_row: Option<u16>,
    window_offset: usize,
    anchor: Option<PopupAnchor>,
    flags: Arc<OverlayFlags>,
    /// Reused per-chunk paint buffers. Together with the cell-based row
    /// restore they keep the erase/repaint cycle allocation-free except
    /// `hand_back`'s two small vt100 Vecs — cursor restoration has a
    /// pending-wrap case plain CUP can't express, so the library call stays.
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
            parser: vt100::Parser::new(rows.max(MODEL_FLOOR), cols.max(MODEL_FLOOR), 0),
            real_rows: rows,
            real_cols: cols,
            out,
            split_partials,
            tail: Vec::new(),
            content: None,
            drawn: None,
            ghost_row: None,
            window_offset: 0,
            anchor: None,
            flags,
            scratch: Vec::new(),
            repaint_scratch: Vec::new(),
        }
    }

    #[cfg(test)]
    pub(crate) fn screen(&self) -> &vt100::Screen {
        self.parser.screen()
    }

    /// Align the model cursor with the real terminal's (1-based CPR
    /// coordinates). The model starts blank but the user's screen usually
    /// doesn't ("Last login:", motd, rc output), and overlays are placed in
    /// model coordinates — without this offset every overlay draws above
    /// the row the user actually sees. Content above the cursor stays
    /// unknown; that's fine, model and terminal consume an identical byte
    /// stream from here on.
    pub(crate) fn seed_cursor(&mut self, row: u16, col: u16) {
        self.parser.process(format!("\x1b[{row};{col}H").as_bytes());
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

        // The boundary scan is the one per-chunk cost proportional to the
        // data, so it runs at most once. With an empty tail the buffer below
        // *is* `data`, so both paths can share this answer; otherwise the
        // joined buffer has to be scanned instead and this stays `None`.
        let data_ready =
            (self.split_partials && self.tail.is_empty()).then(|| complete_prefix_len(data));

        // Fast path: nothing painted or pending. The boundary scan still
        // runs pre-write so a later paint can't splice into a sequence
        // this chunk left unterminated.
        if overlay_idle
            && self.tail.is_empty()
            && data_ready.is_none_or(|ready| ready == data.len())
        {
            self.out.write_all(data)?;
            self.out.flush()?;
            self.parser.process(data);
            return Ok(());
        }

        let mut buf = std::mem::take(&mut self.scratch);
        buf.clear();
        self.erase_into(&mut buf);

        // The tail keeps its allocation: consumed bytes are compacted out
        // after the write instead of split into a fresh Vec per chunk.
        self.tail.extend_from_slice(data);
        let ready_len = if self.split_partials {
            let ready = match data_ready {
                Some(ready) => ready,
                None => complete_prefix_len(&self.tail),
            };
            // The cap bounds what is *withheld*, not what arrived. Measuring
            // the whole buffer would trip on any carryover plus one
            // full-size pty read — the everyday case under a shell that
            // repaints in colour — and turn boundary splitting off for
            // exactly the chunk that needs it, letting the next overlay
            // paint land inside an unterminated escape sequence.
            if self.tail.len() - ready > MAX_TAIL_BYTES {
                self.tail.len()
            } else {
                ready
            }
        } else {
            self.tail.len()
        };

        // Parsing a chunk costs microseconds, so erase + data + repaint go
        // out as one write instead of paying two syscall pairs per chunk.
        self.parser.process(&self.tail[..ready_len]);
        let mut repaint = std::mem::take(&mut self.repaint_scratch);
        repaint.clear();
        self.draw_into(&mut repaint, false);

        let result = if buf.is_empty() && repaint.is_empty() {
            self.out
                .write_all(&self.tail[..ready_len])
                .and_then(|()| self.out.flush())
        } else {
            buf.extend_from_slice(&self.tail[..ready_len]);
            buf.extend_from_slice(&repaint);
            self.out.write_all(&buf).and_then(|()| self.out.flush())
        };
        self.tail.copy_within(ready_len.., 0);
        self.tail.truncate(self.tail.len() - ready_len);
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
        if let Some(content) = &self.content {
            let next_token_start = token_start(&content.line);
            if self
                .anchor
                .is_some_and(|anchor| anchor.token_start != next_token_start)
            {
                self.window_offset = 0;
                self.anchor = None;
            }
        } else {
            self.window_offset = 0;
            self.anchor = None;
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
        self.anchor = None;
        self.real_rows = rows;
        self.real_cols = cols;
        self.parser
            .screen_mut()
            .set_size(rows.max(MODEL_FLOOR), cols.max(MODEL_FLOOR));

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

    /// Start a mid-session cursor resync: real terminals reflow wrapped
    /// lines on resize but the vt100 model does not, so the model cursor
    /// drifts and overlays would paint at stale rows. The stdin-side
    /// KeyFilter consumes the replies and seeds the model; without a
    /// suggestion provider that filter doesn't exist, so stay inert.
    pub(crate) fn begin_cursor_resync(&mut self) {
        if !self.split_partials {
            return;
        }
        // At most one query in flight: a second reply with nobody left
        // expecting it would leak to the shell as junk keystrokes.
        if self.flags.resync.swap(true, Ordering::AcqRel) {
            return;
        }
        // Straight to the terminal, not through the parser: the query
        // prints nothing and must not disturb the model.
        let _ = self
            .out
            .write_all(CURSOR_HANDSHAKE)
            .and_then(|()| self.out.flush());
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

        // Rebuild covered rows straight from model cells: vt100's
        // rows_formatted would format AND allocate every row up to the
        // popup — most of the screen when the prompt sits low — per erase,
        // i.e. per echoed keystroke.
        let screen = self.parser.screen();
        let (_, cols) = screen.size();
        for row in 0..last {
            if covered(row) {
                restore_row_from_cells(buf, screen, row, cols);
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
        // Layout math uses the REAL terminal size: the model may be floored
        // above it, and drawing past the real edge would scroll the screen.
        let (rows, cols) = (self.real_rows, self.real_cols);
        let (cursor_row, cursor_col) = screen.cursor_position();
        if rows < 2 || (cols as usize) < MIN_POPUP_WIDTH || cursor_row >= rows {
            return;
        }

        let selected = content.selected.min(content.suggestions.len() - 1);
        let selection = &content.suggestions[selected];
        // With no word started, a shell completion is the shell listing the
        // directory rather than finishing something the user typed — every
        // file, in the order the engine happened to emit them. That belongs in
        // the dropdown to browse, not appended to the line as ghost text the
        // next Right arrow would accept. History is unaffected: it completes
        // the whole line, so it is an answer even when the word is empty.
        let word_started = content
            .line
            .chars()
            .last()
            .is_some_and(|c| !c.is_whitespace());
        let ghostable = word_started || selection.source != SuggestionSource::Completion;
        let ghost_suffix = ghostable
            .then(|| selection.text.strip_prefix(content.line.as_str()))
            .flatten()
            .filter(|suffix| !suffix.is_empty());

        // A lone suggestion already shown in full as ghost text needs no dropdown.
        let solo_ghost = content.suggestions.len() == 1 && ghost_suffix.is_some();

        buf.extend_from_slice(b"\x1b[?25l");
        if !solo_ghost {
            self.drawn = draw_popup(
                buf,
                screen,
                (rows, cols),
                content,
                selected,
                &mut self.window_offset,
                &mut self.anchor,
            );
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
/// `window_offset` and `anchor` persist across repaints so the visible
/// slice doesn't jump while the selection moves within it, and the popup
/// column stays put while the same token is being completed.
fn draw_popup(
    buf: &mut Vec<u8>,
    screen: &vt100::Screen,
    terminal_size: (u16, u16),
    content: &OverlayContent,
    selected: usize,
    window_offset: &mut usize,
    anchor: &mut Option<PopupAnchor>,
) -> Option<DrawnRegion> {
    let (rows, cols) = terminal_size;
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

    // Rows pick up at the token being typed, and the popup anchors at that
    // token's column, so each row reads as a completion of the current
    // word rather than a fragment split mid-word.
    let current_token_start = token_start(&content.line);
    let (line_head, partial_token) = content.line.split_at(current_token_start);
    let scrollbar = content.suggestions.len() > visible;
    let chrome = ROW_CHROME_WIDTH + usize::from(scrollbar);
    // Width covers the whole list — never just the visible window, so
    // scrolling can't make the popup breathe.
    let computed_width = content
        .suggestions
        .iter()
        .map(|s| shown_from_token(s, line_head).width() + chrome)
        .max()
        .unwrap_or(chrome)
        .clamp(MIN_POPUP_WIDTH, cols as usize);

    // Proportional thumb: its length mirrors the visible share of the
    // list, and its travel spans the track as the window scrolls.
    let (thumb_top, thumb_len) = if scrollbar {
        let total = content.suggestions.len();
        let len = (visible * visible).div_ceil(total).max(1);
        let top = (*window_offset * (visible - len) + (total - visible) / 2) / (total - visible);
        (top, len)
    } else {
        (0, 0)
    };
    let partial_width = partial_token.width().min(u16::MAX as usize) as u16;
    let token_col = cursor_col.saturating_sub(partial_width);
    let same_token = anchor.filter(|a| a.token_start == current_token_start);
    // Same token: the geometry only ever grows and the column stays put —
    // candidates coming and going as keystrokes narrow the set must not
    // make the popup shuffle or shrink under the user's eyes.
    let (col, width) = match same_token {
        Some(a) => {
            let available = cols.saturating_sub(a.col) as usize;
            (a.col, computed_width.max(a.width).min(available))
        }
        None => {
            let width = computed_width;
            let rightmost = cols.saturating_sub(width as u16);
            (token_col.min(rightmost), width)
        }
    };
    *anchor = Some(PopupAnchor {
        token_start: current_token_start,
        col,
        width,
    });

    for (i, suggestion) in window.iter().enumerate() {
        let row = first_row + i as u16;
        move_to(buf, row, col);
        let is_selected = *window_offset + i == selected;
        buf.extend_from_slice(if is_selected {
            SELECTED_STYLE
        } else {
            UNSELECTED_STYLE
        });

        buf.push(b' ');
        let mut utf8 = [0u8; 4];
        buf.extend_from_slice(
            source_icon(suggestion.source)
                .encode_utf8(&mut utf8)
                .as_bytes(),
        );
        buf.push(b' ');
        let shown = shown_from_token(suggestion, line_head);
        let used = (ROW_CHROME_WIDTH - 1)
            + write_fitted_syntax(
                buf,
                shown,
                suggestion.text.len() - shown.len(),
                &suggestion.syntax,
                if is_selected {
                    SELECTED_FG
                } else {
                    UNSELECTED_FG
                },
                width.saturating_sub(chrome),
            );
        let pad_to = width - usize::from(scrollbar);
        buf.resize(buf.len() + pad_to.saturating_sub(used), b' ');
        if scrollbar {
            let glyph = if (thumb_top..thumb_top + thumb_len).contains(&i) {
                SCROLL_THUMB
            } else {
                SCROLL_TRACK
            };
            buf.extend_from_slice(glyph.encode_utf8(&mut utf8).as_bytes());
        }
    }

    Some(DrawnRegion {
        first_row,
        count: visible as u16,
    })
}

/// [`write_fitted`], coloring each character by its syntax class. `skip`
/// is the byte offset of `text` within the classified suggestion (rows
/// show a token-anchored suffix); `base_fg` restores the row's foreground
/// for plain runs and after the text, so padding and the scrollbar keep
/// the row color.
fn write_fitted_syntax(
    buf: &mut Vec<u8>,
    text: &str,
    skip: usize,
    syntax: &[SyntaxSpan],
    base_fg: &'static [u8],
    budget: usize,
) -> usize {
    if syntax.is_empty() {
        return write_fitted(buf, text, budget);
    }

    let mut spans = syntax.iter();
    let mut span_end = 0usize;
    let mut class = SyntaxClass::Plain;
    let mut painted: Option<&[u8]> = None;
    let mut used = 0;
    for (i, ch) in text.char_indices() {
        while skip + i >= span_end {
            let Some(span) = spans.next() else {
                class = SyntaxClass::Plain;
                span_end = usize::MAX;
                break;
            };
            span_end += span.len;
            class = span.class;
        }

        let ch = printable(ch);
        let ch_width = ch.width().unwrap_or(0);
        if used + ch_width > budget {
            break;
        }
        let fg = class_fg(class);
        if fg != painted {
            buf.extend_from_slice(fg.unwrap_or(base_fg));
            painted = fg;
        }
        let mut utf8 = [0u8; 4];
        buf.extend_from_slice(ch.encode_utf8(&mut utf8).as_bytes());
        used += ch_width;
    }
    if painted.is_some() {
        buf.extend_from_slice(base_fg);
    }
    used
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

/// Rewrite one screen row from the model's cells, allocation-free.
///
/// Cells with content or attributes are re-emitted with their SGR state;
/// runs of untouched default cells become cursor jumps. Attribute runs
/// change rarely (a prompt has a handful), so the SGR churn stays small.
fn restore_row_from_cells(buf: &mut Vec<u8>, screen: &vt100::Screen, row: u16, cols: u16) {
    buf.extend_from_slice(b"\x1b[0m");
    move_to(buf, row, 0);
    buf.extend_from_slice(b"\x1b[2K");

    let mut attrs = CellAttrs::DEFAULT;
    let mut gap = 0u16;
    for col in 0..cols {
        let Some(cell) = screen.cell(row, col) else {
            break;
        };
        if cell.is_wide_continuation() {
            continue;
        }
        let cell_attrs = CellAttrs::of(cell);
        if !cell.has_contents() && cell_attrs == CellAttrs::DEFAULT {
            gap += 1;
            continue;
        }
        if gap > 0 {
            // 2K already blanked these cells; just step over them.
            let _ = write!(buf, "\x1b[{gap}C");
            gap = 0;
        }
        if cell_attrs != attrs {
            cell_attrs.emit(buf);
            attrs = cell_attrs;
        }
        if cell.has_contents() {
            buf.extend_from_slice(cell.contents().as_bytes());
        } else {
            // No glyph, but a non-default background must survive.
            buf.push(b' ');
        }
    }
}

/// One cell's SGR-visible state, for run-length re-emission.
#[derive(Clone, Copy, PartialEq)]
struct CellAttrs {
    fg: vt100::Color,
    bg: vt100::Color,
    bold: bool,
    dim: bool,
    italic: bool,
    underline: bool,
    inverse: bool,
}

impl CellAttrs {
    const DEFAULT: Self = Self {
        fg: vt100::Color::Default,
        bg: vt100::Color::Default,
        bold: false,
        dim: false,
        italic: false,
        underline: false,
        inverse: false,
    };

    fn of(cell: &vt100::Cell) -> Self {
        Self {
            fg: cell.fgcolor(),
            bg: cell.bgcolor(),
            bold: cell.bold(),
            dim: cell.dim(),
            italic: cell.italic(),
            underline: cell.underline(),
            inverse: cell.inverse(),
        }
    }

    /// Emit as one reset-plus-attributes SGR sequence.
    fn emit(&self, buf: &mut Vec<u8>) {
        buf.extend_from_slice(b"\x1b[0");
        for (on, code) in [
            (self.bold, "1"),
            (self.dim, "2"),
            (self.italic, "3"),
            (self.underline, "4"),
            (self.inverse, "7"),
        ] {
            if on {
                let _ = write!(buf, ";{code}");
            }
        }
        match self.fg {
            vt100::Color::Default => {}
            vt100::Color::Idx(n) => {
                let _ = write!(buf, ";38;5;{n}");
            }
            vt100::Color::Rgb(r, g, b) => {
                let _ = write!(buf, ";38;2;{r};{g};{b}");
            }
        }
        match self.bg {
            vt100::Color::Default => {}
            vt100::Color::Idx(n) => {
                let _ = write!(buf, ";48;5;{n}");
            }
            vt100::Color::Rgb(r, g, b) => {
                let _ = write!(buf, ";48;2;{r};{g};{b}");
            }
        }
        buf.push(b'm');
    }
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

    #[rstest]
    #[case::mid_token("git st", 4)]
    #[case::single_token("gi", 0)]
    #[case::trailing_space("git ", 4)]
    #[case::empty("", 0)]
    #[case::wide_whitespace("a\u{3000}b", 4)]
    #[case::escaped_space("cd My\\ Do", 3)]
    #[case::double_quoted_space("cd \"My Do", 3)]
    #[case::single_quoted_space("cd 'My Do", 3)]
    fn finds_current_shell_word(#[case] line: &str, #[case] expected: usize) {
        assert_eq!(token_start(line), expected);
    }

    /// Column of the first written (non-erased) cell in a row: where the
    /// popup was drawn. Erased cells have empty contents, written pad
    /// spaces don't.
    fn popup_col(c: &Compositor<Vec<u8>>, row: u16) -> u16 {
        let checker = displayed(c);
        let screen = checker.screen();
        (0..40)
            .find(|&col| {
                screen
                    .cell(row, col)
                    .is_some_and(|cell| !cell.contents().is_empty())
            })
            .expect("popup row has content")
    }

    #[rstest]
    fn popup_column_stays_put_while_completing_one_token() {
        let mut c = compositor(); // 10 rows x 40 cols
        c.apply_pty(b"$ do abc").unwrap();

        // Wide rows clamp the popup left of the token column.
        let long = "do abcdefghijklmnopqrstuvwxyz0123456789";
        c.set_overlay(Some(content("do abc", &[long, "do abcd"], 0)));
        let clamped = popup_col(&c, 1);
        assert!(clamped < 5, "clamped by the right edge: {clamped}");

        // Narrower candidate set, same token: the popup must not slide
        // back toward the token column mid-typing.
        c.set_overlay(Some(content("do abc", &["do abcd", "do abce"], 0)));
        assert_eq!(popup_col(&c, 1), clamped);

        // Closing and reopening re-anchors at the token column.
        c.set_overlay(None);
        c.set_overlay(Some(content("do abc", &["do abcd", "do abce"], 0)));
        assert_eq!(popup_col(&c, 1), 5);
    }

    #[rstest]
    fn popup_anchor_survives_echo_repaint_until_the_next_token() {
        let mut c = compositor();
        c.apply_pty(b"$ do a").unwrap();
        c.set_overlay(Some(content("do a", &["do alpha", "do amber"], 0)));
        let anchored = popup_col(&c, 1);

        // The shell echo lands before the refreshed query. Repainting the
        // old candidates during that gap must not follow the cursor right.
        c.apply_pty(b"b").unwrap();
        assert_eq!(popup_col(&c, 1), anchored);

        c.set_overlay(Some(content("do ab", &["do about", "do absent"], 0)));
        assert_eq!(popup_col(&c, 1), anchored);

        // A wider result is truncated into the remaining room rather than
        // pulling the already-visible box left.
        let wide = "do abcdefghijklmnopqrstuvwxyz0123456789";
        c.set_overlay(Some(content("do ab", &[wide, "do absent"], 0)));
        assert_eq!(popup_col(&c, 1), anchored);

        c.apply_pty(b" ").unwrap();
        assert_eq!(popup_col(&c, 1), anchored);

        c.set_overlay(Some(content("do ab ", &["do ab next", "do ab now"], 0)));
        assert_eq!(popup_col(&c, 1), 8);
    }

    /// The cell-based row restore must reproduce styled content exactly:
    /// colors, attributes, wide glyphs, and gaps.
    #[rstest]
    fn erase_restores_styled_rows_exactly() {
        let mut c = compositor();
        // Row 1 (under the future popup): colored, bold, wide chars, a gap.
        c.apply_pty(b"$ g\r\n\x1b[31;1mred\x1b[0m \xe4\xbd\xa0\xe5\xa5\xbd\x1b[44m  \x1b[0m\x1b[5Cend\x1b[1;4H")
            .unwrap();
        let before = displayed(&c);

        c.set_overlay(Some(content("g", &["git a", "git b"], 0)));
        c.set_overlay(None);

        let after = displayed(&c);
        for col in 0..40u16 {
            let b = before.screen().cell(1, col).unwrap();
            let a = after.screen().cell(1, col).unwrap();
            assert_eq!(
                (b.contents(), b.fgcolor(), b.bgcolor(), b.bold()),
                (a.contents(), a.fgcolor(), a.bgcolor(), a.bold()),
                "cell (1,{col}) must survive erase"
            );
        }
    }

    #[rstest]
    fn scrollbar_tracks_the_window() {
        let mut c = compositor();
        c.apply_pty(b"$ g").unwrap();
        let all: Vec<String> = (0..7).map(|i| format!("git c{i}")).collect();
        let refs: Vec<&str> = all.iter().map(String::as_str).collect();

        // 7 suggestions, 5 visible rows: popup at col 2 ("g" token), width
        // 6 (text) + 5 (chrome incl. scrollbar) — bar in column 12.
        let bar = |c: &Compositor<Vec<u8>>, row: u16| -> String {
            let checker = displayed(c);
            let cell = checker.screen().cell(row, 12);
            cell.map(|cell| cell.contents().to_string())
                .unwrap_or_default()
        };

        c.set_overlay(Some(content("g", &refs, 0)));
        assert_eq!(bar(&c, 1), SCROLL_THUMB.to_string(), "thumb starts at top");
        assert_eq!(bar(&c, 5), SCROLL_TRACK.to_string());

        // Selecting the last entry scrolls the window; the thumb follows
        // to the bottom of the track.
        c.set_overlay(Some(content("g", &refs, 6)));
        assert_eq!(bar(&c, 1), SCROLL_TRACK.to_string());
        assert_eq!(bar(&c, 5), SCROLL_THUMB.to_string(), "thumb at bottom");
    }

    /// First and last written cell of a row: the popup's drawn extent.
    fn row_written_span(c: &Compositor<Vec<u8>>, row: u16) -> (u16, u16) {
        let checker = displayed(c);
        let screen = checker.screen();
        let mut written = (0..40).filter(|&col| {
            screen
                .cell(row, col)
                .is_some_and(|cell| !cell.contents().is_empty())
        });
        let first = written.next().expect("row has content");
        (first, written.next_back().unwrap_or(first))
    }

    #[rstest]
    fn popup_width_is_stable_while_scrolling() {
        let mut c = compositor();
        c.apply_pty(b"$ g").unwrap();
        // The longest entry sits beyond the first window: the width must
        // account for it up front, not upon scrolling it into view.
        let all: Vec<String> = (0..7)
            .map(|i| match i {
                6 => "git muchlongerentry".to_string(),
                _ => format!("git c{i}"),
            })
            .collect();
        let refs: Vec<&str> = all.iter().map(String::as_str).collect();

        c.set_overlay(Some(content("g", &refs, 0)));
        let span = row_written_span(&c, 1);
        assert!(span.1 - span.0 >= 19, "sized for the longest entry");

        c.set_overlay(Some(content("g", &refs, 6)));
        assert_eq!(row_written_span(&c, 1), span, "no breathing on scroll");
    }

    #[rstest]
    fn few_suggestions_render_without_scrollbar() {
        let mut c = compositor();
        c.apply_pty(b"$ g").unwrap();
        c.set_overlay(Some(content("g", &["git a", "git b"], 0)));

        // Rows are icon + space + text + one pad cell — no bar column.
        let checker = displayed(&c);
        let row = row_text(&checker, 1);
        assert!(!row.contains(SCROLL_THUMB), "no scrollbar: {row:?}");
        assert!(!row.contains(SCROLL_TRACK), "no scrollbar: {row:?}");
    }

    #[rstest]
    fn popup_rows_render_syntax_colors() {
        let mut c = compositor();
        c.apply_pty(b"$ g").unwrap();
        let mut styled = Suggestion::history("git -a");
        styled.syntax = vec![
            span(3, SyntaxClass::Command),
            span(1, SyntaxClass::Plain),
            span(2, SyntaxClass::Flag),
        ];
        c.set_overlay(Some(OverlayContent {
            line: "g".to_string(),
            suggestions: vec![styled, Suggestion::history("got x")].into(),
            selected: 1,
        }));

        // Row text starts at col 5 (anchor 2 + pad + icon + space): "git -a".
        let checker = displayed(&c);
        let fg = |row, col| checker.screen().cell(row, col).unwrap().fgcolor();
        assert_eq!(fg(1, 5), vt100::Color::Idx(10), "command is green");
        assert_eq!(fg(1, 8), vt100::Color::Idx(7), "plain keeps the row fg");
        assert_eq!(fg(1, 9), vt100::Color::Idx(6), "flag is cyan");
        // Unstyled suggestion on the selected row: uniform bright fg.
        assert_eq!(fg(2, 5), vt100::Color::Idx(15));
    }

    #[rstest]
    fn syntax_spans_follow_the_token_offset() {
        let mut c = compositor();
        c.apply_pty(b"$ git c").unwrap();
        let mut styled = Suggestion::history("git commit");
        styled.syntax = vec![span(4, SyntaxClass::Command), span(6, SyntaxClass::String)];
        c.set_overlay(Some(OverlayContent {
            line: "git c".to_string(),
            suggestions: vec![styled, Suggestion::history("git clone")].into(),
            selected: 0,
        }));

        // The row shows "commit" (token-anchored, 4 bytes skipped), so its
        // first char must take the span covering byte 4, not byte 0.
        let checker = displayed(&c);
        let cell = checker.screen().cell(1, 9).unwrap();
        assert_eq!(cell.contents(), "c");
        assert_eq!(cell.fgcolor(), vt100::Color::Idx(3), "string, not command");
    }

    fn span(len: usize, class: SyntaxClass) -> SyntaxSpan {
        SyntaxSpan { len, class }
    }

    // -- Compositor behaviour ---------------------------------------------

    fn compositor() -> Compositor<Vec<u8>> {
        Compositor::new(10, 40, Vec::new(), Arc::new(OverlayFlags::default()), true)
    }

    fn content(line: &str, suggestions: &[&str], selected: usize) -> OverlayContent {
        OverlayContent {
            line: line.to_string(),
            suggestions: suggestions.iter().map(|s| Suggestion::history(s)).collect(),
            selected,
        }
    }

    /// As [`content`], but the suggestions came from the shell's completions.
    fn completion_content(line: &str, suggestions: &[&str]) -> OverlayContent {
        OverlayContent {
            line: line.to_string(),
            suggestions: suggestions
                .iter()
                .map(|s| Suggestion {
                    text: (*s).to_string(),
                    source: SuggestionSource::Completion,
                    syntax: Vec::new(),
                })
                .collect(),
            selected: 0,
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

        // Rows restart at the token being typed ("st" → "status"); the
        // earlier part of the line is never repeated — it's on screen
        // directly above.
        let checker = displayed(&c);
        let first = row_text(&checker, 1);
        let second = row_text(&checker, 2);
        assert!(first.contains("status"), "token remainder: {first:?}");
        assert!(
            !first.contains("git"),
            "typed prefix not repeated: {first:?}"
        );
        assert!(second.contains("stash"));
        assert!(c.flags.popup.load(Ordering::Acquire));

        c.set_overlay(None);
        let shown = screen_text(&displayed(&c));
        assert_eq!(shown.trim_end(), "$ git st");
        assert!(!c.flags.popup.load(Ordering::Acquire));
    }

    /// Typing a space leaves the shell nothing to filter on, so its
    /// completions become the whole directory. Appending the first of those
    /// to the line as ghost text offers a file the user never asked for, and
    /// the next Right arrow would take it. The dropdown still lists them.
    #[rstest]
    fn completions_for_an_empty_word_list_without_ghosting() {
        let mut c = compositor();
        c.apply_pty(b"$ echo hello ").unwrap();
        c.set_overlay(Some(completion_content(
            "echo hello ",
            &["echo hello alpha-dir/", "echo hello beta-dir/"],
        )));

        assert!(
            !c.flags.ghost.load(Ordering::Acquire),
            "a directory listing must not be pre-appended to the line"
        );
        assert!(
            c.flags.popup.load(Ordering::Acquire),
            "but it is still there to browse"
        );
        let shown = screen_text(&displayed(&c));
        assert!(
            shown
                .lines()
                .next()
                .is_some_and(|l| l.trim_end() == "$ echo hello"),
            "the typed line is untouched: {shown:?}"
        );

        // One character of a word, and the shell is finishing what was
        // started — ghost text is exactly what is wanted.
        let mut c = compositor();
        c.apply_pty(b"$ echo hello a").unwrap();
        c.set_overlay(Some(completion_content(
            "echo hello a",
            &["echo hello alpha-dir/"],
        )));
        assert!(c.flags.ghost.load(Ordering::Acquire));

        // History completes a whole line, so it ghosts even on an empty word.
        let mut c = compositor();
        c.apply_pty(b"$ echo hello ").unwrap();
        c.set_overlay(Some(content("echo hello ", &["echo hello world"], 0)));
        assert!(c.flags.ghost.load(Ordering::Acquire));
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
    fn popup_layout_uses_the_physical_terminal_rows() {
        let flags = Arc::new(OverlayFlags::default());
        let mut c: Compositor<Vec<u8>> = Compositor::new(2, 40, Vec::new(), flags, true);
        c.apply_pty(b"one\r\n$ g").unwrap();
        c.set_overlay(Some(content("g", &["git a", "git b", "git c"], 0)));

        let drawn = c.drawn.expect("popup is visible above the prompt");
        assert_eq!(drawn.first_row, 0);
        assert_eq!(drawn.count, 1);
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
    fn cursor_resync_queries_once_and_skips_the_model() {
        let mut c = compositor();
        c.apply_pty(b"$ git st").unwrap();
        c.begin_cursor_resync();
        assert!(c.out.ends_with(CURSOR_HANDSHAKE));
        assert!(c.flags.resync.load(Ordering::Acquire));
        assert_eq!(
            c.parser.screen().cursor_position(),
            (0, 8),
            "query bytes must not reach the model"
        );

        let sent = c.out.len();
        c.begin_cursor_resync();
        assert_eq!(c.out.len(), sent, "one query in flight at a time");
    }

    #[rstest]
    fn cursor_resync_is_inert_without_suggestions() {
        let flags = Arc::new(OverlayFlags::default());
        let mut c: Compositor<Vec<u8>> = Compositor::new(10, 40, Vec::new(), flags.clone(), false);
        c.begin_cursor_resync();
        assert!(c.out.is_empty(), "nobody would consume the reply");
        assert!(!flags.resync.load(Ordering::Acquire));
    }

    #[rstest]
    fn flush_pending_emits_withheld_tail() {
        let mut c = compositor();
        c.apply_pty(b"abc\x1b[3").unwrap();
        c.flush_pending();
        assert_eq!(c.out, b"abc\x1b[3");
    }

    /// The cap bounds the withheld remainder, not the buffer. The pty pump
    /// reads into exactly `MAX_TAIL_BYTES`, so measuring the buffer meant
    /// any carryover plus one full read turned splitting off — for the
    /// everyday chunk, under a shell that repaints in colour.
    #[rstest]
    fn a_full_chunk_on_top_of_carryover_still_splits() {
        let mut c = compositor();
        c.apply_pty(b"$ g\x1b[3").unwrap(); // 1-byte-plus carryover
        c.set_overlay(Some(content("g", &["git status"], 0)));

        // A full-size read that itself ends mid-sequence.
        let mut chunk = b"1mX".to_vec();
        chunk.extend(std::iter::repeat_n(b'y', MAX_TAIL_BYTES - 3 - 2));
        chunk.extend_from_slice(b"\x1b["); // unterminated tail
        assert_eq!(chunk.len(), MAX_TAIL_BYTES);
        c.apply_pty(&chunk).unwrap();

        assert_eq!(&c.tail, b"\x1b[", "the unterminated tail is still withheld");
        assert!(
            !c.out.ends_with(b"\x1b["),
            "no overlay paint may follow a half-written sequence"
        );
    }

    /// A sequence that never terminates must not buffer forever, though.
    #[rstest]
    fn an_endless_sequence_is_eventually_flushed() {
        let mut c = compositor();
        let mut chunk = b"\x1b[".to_vec();
        chunk.extend(std::iter::repeat_n(b'1', MAX_TAIL_BYTES + 1));
        c.apply_pty(&chunk).unwrap();
        assert!(c.tail.is_empty(), "gave up withholding");
        assert_eq!(c.out.len(), chunk.len());
    }
}
