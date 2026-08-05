//! inshellisense-style suggestion popup with fish-style ghost text.
//!
//! [`InputTracker`] follows the OSC 133 input zone, [`KeyFilter`] steals
//! Tab/arrows/Esc while the overlay is visible, and a UI thread owns the
//! injected [`SuggestionProvider`] — kept abstract so this crate stays free
//! of atuin-client dependencies.

use std::borrow::Cow;
use std::io::{Read, Write};
use std::os::fd::AsFd;
use std::sync::atomic::{AtomicU16, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use atuin_common::ansi;

use crate::compositor::{Compositor, OverlayContent, OverlayFlags, lock_unpoisoned};
use crate::osc133::{Event, Parser, Segment, Zone};
use crate::runtime::ActivityClock;

/// One dropdown entry: the full command line it would produce, plus where
/// it came from (rendered as an icon next to the suggestion).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Suggestion {
    pub text: String,
    pub source: SuggestionSource,
    /// Shell-syntax classification of `text` as ordered byte runs, for
    /// popup coloring. Empty renders unstyled.
    pub syntax: Vec<SyntaxSpan>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SuggestionSource {
    History,
    Completion,
}

/// One run of shell-syntax classification over [`Suggestion::text`].
///
/// A minimal mirror of the TUI theme's syntax meanings, so this crate can
/// reuse the classifier's verdicts without depending on atuin-client.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SyntaxSpan {
    pub len: usize,
    pub class: SyntaxClass,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SyntaxClass {
    Plain,
    Command,
    Flag,
    String,
    Variable,
    Comment,
}

#[cfg(test)]
impl Suggestion {
    /// Test shorthand: a history-sourced suggestion.
    pub(crate) fn history(text: &str) -> Self {
        Self {
            text: text.to_string(),
            source: SuggestionSource::History,
            syntax: Vec::new(),
        }
    }
}

/// Candidate completions for the current line, best first. Runs on the UI
/// thread; implementations enforce their own timeout.
pub type SuggestionProvider = Box<dyn Fn(&str) -> Vec<Suggestion> + Send>;

/// Rows in the persistent line emulator. Input that scrolls past this grid
/// has lost its own beginning, so suggestions stop until the next prompt —
/// nobody wants completions for a screenful of pasted text anyway.
const LINE_GRID_ROWS: u16 = 32;

/// Kill-line (`^U`): replaces the line when accepting a suggestion that
/// doesn't extend the typed prefix (fuzzy hits).
const KILL_LINE: u8 = 0x15;

/// Polls before a lone `ESC` counts as a real Escape press rather than the
/// start of a split key sequence.
const ESC_POLL_RETRIES: u32 = 3;
/// Sleep between those polls; retries × interval is the felt Escape delay.
const ESC_POLL_INTERVAL: Duration = Duration::from_millis(8);

/// How long the resize handshake may stay unanswered before the filter
/// gives up and releases any withheld bytes: a terminal that never replies
/// must not wedge stdin filtering forever.
const RESYNC_REPLY_TIMEOUT: Duration = Duration::from_secs(1);
/// Longest tail worth withholding as a possibly-split reply. DA1 answers
/// listing many attributes (xterm) are the longest legitimate one.
const MAX_REPLY_CARRY: usize = 48;

/// An input-zone change only counts as typing if a keystroke arrived this
/// recently. Echo follows a key within milliseconds; without one, the
/// "input" is program output — cat of a typescript containing OSC 133
/// markers — and must not conjure the popup.
const INPUT_ECHO_WINDOW: Duration = Duration::from_secs(1);

pub(crate) struct Suggest<W: Write> {
    pub(crate) tracker: InputTracker,
    pub(crate) keys: KeyFilter<W>,
}

/// Spawn the suggestion UI thread and hand back the two pump-side hooks.
pub(crate) fn spawn<W: Write + Send + 'static>(
    provider: SuggestionProvider,
    compositor: Arc<Mutex<Compositor<W>>>,
    flags: Arc<OverlayFlags>,
    cols: Arc<AtomicU16>,
    input_activity: Arc<ActivityClock>,
    session_ready: Option<Box<dyn FnOnce() + Send>>,
) -> Suggest<W> {
    let state = Arc::new(Mutex::new(PopupState::default()));
    let (ui_tx, ui_rx) = mpsc::channel();

    spawn_ui_thread(provider, compositor.clone(), ui_rx, state.clone());

    Suggest {
        tracker: InputTracker::new(ui_tx, cols, input_activity, session_ready),
        keys: KeyFilter {
            state,
            compositor,
            flags,
            resync: ResyncState::default(),
            key_scratch: Vec::new(),
            out_scratch: Vec::new(),
        },
    }
}

enum UiEvent {
    /// The command line changed; fetch suggestions and repaint.
    Query(String),
    /// Left the input zone; drop the overlay.
    Hide,
}

#[derive(Default)]
struct PopupState {
    /// Plain-text command line currently being edited.
    line: String,
    suggestions: std::sync::Arc<[Suggestion]>,
    selected: usize,
    /// Line the popup was dismissed (or a suggestion accepted) for;
    /// suppress the popup until the line changes again.
    dismissed_for: Option<String>,
}

impl PopupState {
    fn overlay_content(&self) -> Option<OverlayContent> {
        (!self.suggestions.is_empty()).then(|| OverlayContent {
            line: self.line.clone(),
            suggestions: self.suggestions.clone(),
            selected: self.selected,
        })
    }
}

// ---------------------------------------------------------------------------
// Input tracking (pty→stdout pump thread)
// ---------------------------------------------------------------------------

/// Follows the OSC 133 input zone in the pty output stream and reports the
/// in-progress command line to the UI thread.
pub(crate) struct InputTracker {
    parser: Parser,
    /// Persistent emulator for the input-zone echo. vt100 is a state
    /// machine, so feeding each chunk once equals re-emulating the whole
    /// accumulated input — without `ansi::to_plain_text`'s per-keystroke
    /// grid allocation and O(line²) reprocessing.
    line_screen: vt100::Parser,
    grid_cols: u16,
    /// Bytes fed since the last prompt; past the grid's capacity the line's
    /// beginning has scrolled away and suggestions stop.
    fed: usize,
    onlcr_scratch: Vec<u8>,
    /// A command ran since the last grid reset, so the next prompt marker
    /// is a genuinely new prompt rather than a redraw of the current line.
    ran_command: bool,
    /// The line as last reported. Redraw bursts scramble the grid cursor
    /// before their prompt marker arrives, so this — not the mid-redraw
    /// grid — is what a redraw must carry over.
    last_line: String,
    /// Reused per-keystroke line buffer.
    line_scratch: String,
    /// Last user keystroke; input-zone changes without a recent one are
    /// program output, not typing.
    input_activity: Arc<ActivityClock>,
    /// Fired at the first prompt marker: the shell finished starting.
    session_ready: Option<Box<dyn FnOnce() + Send>>,
    cols: Arc<AtomicU16>,
    ui_tx: Sender<UiEvent>,
}

impl InputTracker {
    fn new(
        ui_tx: Sender<UiEvent>,
        cols: Arc<AtomicU16>,
        input_activity: Arc<ActivityClock>,
        session_ready: Option<Box<dyn FnOnce() + Send>>,
    ) -> Self {
        // MODEL_FLOOR, not 1: vt100 panics rendering wide glyphs into a
        // single-column grid.
        let grid_cols = cols
            .load(Ordering::Relaxed)
            .max(crate::compositor::MODEL_FLOOR);
        Self {
            parser: Parser::new(),
            line_screen: vt100::Parser::new(LINE_GRID_ROWS, grid_cols, 0),
            grid_cols,
            fed: 0,
            onlcr_scratch: Vec::new(),
            ran_command: false,
            last_line: String::new(),
            line_scratch: String::new(),
            input_activity,
            session_ready,
            cols,
            ui_tx,
        }
    }

    pub(crate) fn push(&mut self, data: &[u8]) {
        let line_screen = &mut self.line_screen;
        let onlcr_scratch = &mut self.onlcr_scratch;
        let fed = &mut self.fed;
        let grid_cols = &mut self.grid_cols;
        let ran_command = &mut self.ran_command;
        let last_line = &mut self.last_line;
        let session_ready = &mut self.session_ready;
        let cols = &self.cols;
        let mut input_changed = false;
        let mut hide = false;
        // Set when a prompt marker turns out to be a redraw of the current
        // line: the rest of the chunk is repaint choreography — cursor
        // moves sized for the real screen — not input.
        let mut skip_input = false;
        self.parser.segments(data, |segment| match segment {
            Segment::Text(Zone::Input, bytes) => {
                if skip_input {
                    return;
                }
                *fed += bytes.len();
                // Match ansi::to_plain_text: bare `\n` must return the
                // carriage like a terminal in onlcr mode would.
                onlcr_scratch.clear();
                onlcr_scratch.extend(ansi::onlcr(bytes.iter().copied()));
                line_screen.process(onlcr_scratch);
                input_changed = true;
            }
            Segment::Text(..) => {}
            Segment::Marker { located, .. } => {
                match located.event {
                    Event::PromptStart | Event::CommandStart => {
                        // First prompt: the shell's startup is over.
                        if let Some(ready) = session_ready.take() {
                            if crate::pty_proxy::env_flag("ATUIN_PTY_PROXY_TRACE") {
                                eprintln!(
                                    "atuin pty-proxy: trace: first prompt marker; session ready\r"
                                );
                            }
                            ready();
                        }
                        // A prompt marker with no command since the last
                        // one is a redraw of the current line (resize, ^L,
                        // reset-prompt). zsh reprints prompt AND buffer
                        // before the input marker, so the typed line never
                        // reappears in the input zone: carry it across the
                        // reset instead of forgetting it.
                        let seed = if *ran_command {
                            last_line.clear();
                            String::new()
                        } else {
                            last_line.clone()
                        };
                        *grid_cols = cols
                            .load(Ordering::Relaxed)
                            .max(crate::compositor::MODEL_FLOOR);
                        *line_screen = vt100::Parser::new(LINE_GRID_ROWS, *grid_cols, 0);
                        *fed = seed.len();
                        if !seed.is_empty() {
                            onlcr_scratch.clear();
                            onlcr_scratch.extend(ansi::onlcr(seed.bytes()));
                            line_screen.process(onlcr_scratch);
                            skip_input = true;
                        }
                        if located.event == Event::CommandStart {
                            *ran_command = false;
                        }
                    }
                    Event::CommandExecuted | Event::CommandFinished { .. } => {
                        *ran_command = true;
                    }
                }
                // Any marker starts a fresh zone: whatever was typed before
                // it in this chunk no longer needs a popup update of its own.
                hide = true;
                input_changed = false;
            }
        });

        if input_changed && !self.overflowed() && self.input_activity.idle() <= INPUT_ECHO_WINDOW {
            let mut line = std::mem::take(&mut self.line_scratch);
            line_up_to_cursor(self.line_screen.screen(), self.grid_cols, &mut line);
            // Pure cursor motion echoes back through the input zone too;
            // an unchanged line needs no re-fetch and no repaint.
            if line != self.last_line {
                self.last_line.clear();
                self.last_line.push_str(&line);
                // The only per-keystroke allocation left: the line crosses
                // to the UI thread.
                let _ = self.ui_tx.send(UiEvent::Query(line.clone()));
            }
            self.line_scratch = line;
        } else if hide || input_changed {
            let _ = self.ui_tx.send(UiEvent::Hide);
        }
    }

    fn overflowed(&self) -> bool {
        // Conservative: every fed byte could take a cell; leave slack rows
        // for cursor movement so the line start provably hasn't scrolled.
        self.fed > (LINE_GRID_ROWS as usize - 2) * self.grid_cols as usize
    }
}

/// The line typed so far: grid cells up to the cursor, nothing after.
///
/// Everything past the cursor is display, not input — the erase artifact
/// of a backspace echo, and above all zsh-autosuggestions' POSTDISPLAY
/// ghost, which is echoed inside the input zone and would otherwise be
/// queried as if the user had typed it. Typed trailing spaces are written
/// cells before the cursor, so they survive.
fn line_up_to_cursor(screen: &vt100::Screen, grid_cols: u16, line: &mut String) {
    line.clear();
    let (cursor_row, cursor_col) = screen.cursor_position();
    for row in 0..=cursor_row {
        if row > 0 && !screen.row_wrapped(row - 1) {
            line.push('\n');
        }
        let end = if row == cursor_row {
            cursor_col
        } else {
            grid_cols
        };
        let row_start = line.len();
        for col in 0..end {
            let Some(cell) = screen.cell(row, col) else {
                continue;
            };
            if cell.is_wide_continuation() {
                continue;
            }
            let contents = cell.contents();
            if contents.is_empty() {
                line.push(' ');
            } else {
                line.push_str(contents);
            }
        }
        if row != cursor_row && !screen.row_wrapped(row) {
            let kept = line[row_start..].trim_end().len();
            line.truncate(row_start + kept);
        }
    }
    // Trim \r\n edges in place; this runs per keystroke.
    line.truncate(line.trim_end_matches(['\r', '\n']).len());
    let lead = line.len() - line.trim_start_matches(['\r', '\n']).len();
    if lead > 0 {
        line.drain(..lead);
    }
}

// ---------------------------------------------------------------------------
// Key interception (stdin→pty pump thread)
// ---------------------------------------------------------------------------

/// A key the filter may steal while the overlay is visible.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Key {
    Tab,
    Up,
    Down,
    Right,
    WordRight,
    Esc,
}

/// The single source of truth for interceptable keys: partial-prefix
/// detection, longest-match decoding, and dispatch all derive from it, so a
/// key exists in exactly one place.
const KEY_TABLE: &[(&[u8], Key)] = &[
    (b"\t", Key::Tab),
    (b"\x1b", Key::Esc),
    (b"\x1b[A", Key::Up),
    (b"\x1bOA", Key::Up),
    (b"\x1b[B", Key::Down),
    (b"\x1bOB", Key::Down),
    (b"\x1b[C", Key::Right),
    (b"\x1bOC", Key::Right),
    (b"\x1b[1;3C", Key::WordRight),
    (b"\x1b[1;5C", Key::WordRight),
    (b"\x1bf", Key::WordRight),
];

enum KeyAction {
    Forward,
    Consume,
    Replace(Vec<u8>),
}

pub(crate) struct KeyFilter<W: Write> {
    state: Arc<Mutex<PopupState>>,
    compositor: Arc<Mutex<Compositor<W>>>,
    flags: Arc<OverlayFlags>,
    resync: ResyncState,
    /// Reused per-chunk buffers: keystroke filtering while the popup
    /// shows must not allocate.
    key_scratch: Vec<u8>,
    out_scratch: Vec<u8>,
}

/// Bookkeeping for an in-flight resize cursor handshake. Only touched
/// while the resync flag is set; the filter lives on the single stdin
/// pump thread.
#[derive(Default)]
struct ResyncState {
    /// Trailing bytes that may still become a reply split across reads.
    carry: Vec<u8>,
    /// Last cursor report seen; the DA1 fence promotes it to the seed
    /// (earlier ones answered the shell's own queries, e.g. p10k).
    cursor: Option<CursorReport>,
    /// Self-heal deadline, armed when the filter first sees the flag —
    /// replies arrive on stdin, so nothing can be withheld before then.
    deadline: Option<Instant>,
}

/// One `ESC[row;colR` cursor report, 1-based as the terminal sends it.
#[derive(Clone, Copy, Debug, PartialEq)]
struct CursorReport {
    row: u16,
    col: u16,
}

impl<W: Write> KeyFilter<W> {
    /// Process one stdin chunk and return the bytes to forward to the pty.
    pub(crate) fn process<'a>(
        &'a mut self,
        chunk: &'a [u8],
        stdin: &mut (impl Read + AsFd),
    ) -> Cow<'a, [u8]> {
        // One atomic load in normal life; the branch only runs while a
        // resize cursor handshake is in flight.
        if self.flags.resync.load(Ordering::Acquire) {
            let kept = self.consume_resync(chunk);
            return Cow::Owned(self.filter_keys(&kept, stdin).into_owned());
        }
        self.filter_keys(chunk, stdin)
    }

    /// Pass-through when hidden. When visible, the chunk is tokenized so
    /// keys batched into one read (arrow auto-repeat, fast typing) are each
    /// intercepted or forwarded in order. A chunk ending in a possible key
    /// prefix (e.g. a lone `ESC`) briefly waits for its tail before
    /// deciding; an `ESC` that stays lone dismisses the popup.
    fn filter_keys<'a>(
        &'a mut self,
        chunk: &'a [u8],
        stdin: &mut (impl Read + AsFd),
    ) -> Cow<'a, [u8]> {
        if !self.visible() {
            return Cow::Borrowed(chunk);
        }
        // Every interceptable key starts with ESC or Tab; anything else is
        // a pure pass-through even while the overlay shows.
        if !chunk.contains(&0x1b) && !chunk.contains(&b'\t') {
            return Cow::Borrowed(chunk);
        }

        let mut bytes = std::mem::take(&mut self.key_scratch);
        bytes.clear();
        bytes.extend_from_slice(chunk);
        let mut out = std::mem::take(&mut self.out_scratch);
        out.clear();
        let mut pos = 0;
        while pos < bytes.len() {
            while is_partial_interceptable(&bytes[pos..]) {
                if !(wait_for_more(&*stdin) && read_more(&mut bytes, stdin)) {
                    break;
                }
            }

            let rest = &bytes[pos..];
            let Some((sequence, key)) = match_key(rest) else {
                out.push(bytes[pos]);
                pos += 1;
                continue;
            };
            pos += sequence.len();
            match self.intercept(key) {
                KeyAction::Forward => out.extend_from_slice(sequence),
                KeyAction::Consume => {}
                KeyAction::Replace(replacement) => out.extend_from_slice(&replacement),
            }
        }
        self.key_scratch = bytes;
        self.out_scratch = out;
        Cow::Borrowed(&self.out_scratch)
    }

    fn visible(&self) -> bool {
        self.flags.popup.load(Ordering::Acquire) || self.flags.ghost.load(Ordering::Acquire)
    }

    fn ghost_visible(&self) -> bool {
        self.flags.ghost.load(Ordering::Acquire)
    }

    fn intercept(&self, key: Key) -> KeyAction {
        match key {
            Key::Tab => self.accept(AcceptSpan::Full),
            // Right accepts the ghost, fish-style; a drawn ghost implies
            // cursor-at-EOL, where Right is otherwise a no-op.
            Key::Right if self.ghost_visible() => self.accept(AcceptSpan::Full),
            // Alt/Ctrl+Right (and Alt-f): accept one word of the ghost.
            Key::WordRight if self.ghost_visible() => self.accept(AcceptSpan::Word),
            Key::Right | Key::WordRight => KeyAction::Forward,
            Key::Down => self.navigate(1),
            Key::Up => self.navigate(-1),
            Key::Esc => self.dismiss(),
        }
    }

    fn navigate(&self, delta: isize) -> KeyAction {
        let mut st = lock_unpoisoned(&self.state);
        let len = st.suggestions.len();
        if len == 0 {
            return KeyAction::Forward;
        }
        st.selected = (st.selected as isize + delta).rem_euclid(len as isize) as usize;
        let content = st.overlay_content();
        drop(st);
        self.set_overlay(content);
        KeyAction::Consume
    }

    fn dismiss(&self) -> KeyAction {
        let mut st = lock_unpoisoned(&self.state);
        st.dismissed_for = Some(st.line.clone());
        st.suggestions = Vec::new().into();
        st.selected = 0;
        drop(st);
        self.set_overlay(None);
        KeyAction::Consume
    }

    fn accept(&self, span: AcceptSpan) -> KeyAction {
        let mut st = lock_unpoisoned(&self.state);
        if st.suggestions.is_empty() {
            return KeyAction::Forward;
        }
        let selected = st.suggestions[st.selected.min(st.suggestions.len() - 1)]
            .text
            .clone();
        let suffix = selected.strip_prefix(st.line.as_str()).map(str::to_owned);

        match span {
            AcceptSpan::Full => {
                st.dismissed_for = Some(selected.clone());
                st.suggestions = Vec::new().into();
                st.selected = 0;
                drop(st);
                // Eager hide is safe under the compositor: erasing always
                // runs with screen == model, so it can't race the echo.
                self.set_overlay(None);
                match suffix {
                    Some(suffix) if suffix.is_empty() => KeyAction::Consume,
                    Some(suffix) => KeyAction::Replace(suffix.into_bytes()),
                    // Fuzzy hit that doesn't extend the typed line: replace
                    // the whole line (kill-line, then the full command).
                    None => {
                        let mut bytes = vec![KILL_LINE];
                        bytes.extend_from_slice(selected.as_bytes());
                        KeyAction::Replace(bytes)
                    }
                }
            }
            AcceptSpan::Word => {
                drop(st);
                // Ghost text implies a prefix match; take its next word and
                // keep the popup alive — the echo re-queries and repaints.
                let Some(suffix) = suffix.filter(|s| !s.is_empty()) else {
                    return KeyAction::Forward;
                };
                KeyAction::Replace(first_word(&suffix).as_bytes().to_vec())
            }
        }
    }

    fn set_overlay(&self, content: Option<OverlayContent>) {
        lock_unpoisoned(&self.compositor).set_overlay(content);
    }

    /// Swallow the resize handshake's replies from the stdin stream: every
    /// cursor report up to the DA1 fence, seeding the model from the last
    /// one — the terminal answers in order, so that one is ours. Returns
    /// the bytes that were not part of the handshake.
    fn consume_resync(&mut self, chunk: &[u8]) -> Vec<u8> {
        let now = Instant::now();
        let deadline = *self
            .resync
            .deadline
            .get_or_insert(now + RESYNC_REPLY_TIMEOUT);

        let mut pending = std::mem::take(&mut self.resync.carry);
        pending.extend_from_slice(chunk);

        if now >= deadline {
            self.finish_resync(None);
            return pending;
        }

        let mut kept = Vec::with_capacity(pending.len());
        let mut i = 0;
        let mut fenced = false;
        while i < pending.len() {
            if pending[i] != 0x1b {
                kept.push(pending[i]);
                i += 1;
                continue;
            }
            match scan_reply(&pending[i..]) {
                ReplyScan::Cursor { len, report } => {
                    self.resync.cursor = Some(report);
                    i += len;
                }
                ReplyScan::Fence { len } => {
                    fenced = true;
                    // Handshake over: the rest is ordinary input again.
                    kept.extend_from_slice(&pending[i + len..]);
                    break;
                }
                ReplyScan::Partial if pending.len() - i <= MAX_REPLY_CARRY => {
                    self.resync.carry = pending[i..].to_vec();
                    break;
                }
                // Not a reply, or too long to ever become one: forward it.
                ReplyScan::Partial | ReplyScan::Other => {
                    kept.push(pending[i]);
                    i += 1;
                }
            }
        }

        if fenced {
            let seed = self.resync.cursor.take();
            self.finish_resync(seed);
        }
        kept
    }

    /// End the handshake, seeding the model when the fence confirmed a
    /// report; a timeout seeds nothing rather than trust a stale reply.
    fn finish_resync(&mut self, seed: Option<CursorReport>) {
        if let Some(report) = seed {
            lock_unpoisoned(&self.compositor).seed_cursor(report.row, report.col);
        }
        self.resync = ResyncState::default();
        self.flags.resync.store(false, Ordering::Release);
    }
}

/// Verdict on one ESC-led span while a resize handshake is in flight.
#[derive(Debug, PartialEq)]
enum ReplyScan {
    /// Complete `ESC[row;colR`, `len` bytes long.
    Cursor { len: usize, report: CursorReport },
    /// Complete DA1 reply (`ESC[?...c`), `len` bytes long.
    Fence { len: usize },
    /// Still a viable prefix of a reply; more bytes could complete it.
    Partial,
    /// Definitely not a handshake reply.
    Other,
}

/// Match `bytes` (starting at ESC) against the two handshake replies.
fn scan_reply(bytes: &[u8]) -> ReplyScan {
    match bytes.get(1) {
        None => return ReplyScan::Partial,
        Some(b'[') => {}
        Some(_) => return ReplyScan::Other,
    }
    let fence = bytes.get(2) == Some(&b'?');
    let mut i = if fence { 3 } else { 2 };
    loop {
        let Some(&byte) = bytes.get(i) else {
            return ReplyScan::Partial;
        };
        if byte.is_ascii_digit() || byte == b';' {
            i += 1;
            continue;
        }
        return match byte {
            b'c' if fence => ReplyScan::Fence { len: i + 1 },
            b'R' if !fence => match parse_cursor(&bytes[2..i]) {
                Some(report) => ReplyScan::Cursor { len: i + 1, report },
                None => ReplyScan::Other,
            },
            _ => ReplyScan::Other,
        };
    }
}

/// `row;col`, exactly two in-range params — anything else is some other
/// CSI (a keyboard sequence, say) and must pass through untouched.
fn parse_cursor(params: &[u8]) -> Option<CursorReport> {
    let params = std::str::from_utf8(params).ok()?;
    let mut parts = params.split(';');
    let row = parts.next()?.parse().ok()?;
    let col = parts.next()?.parse().ok()?;
    parts.next().is_none().then_some(CursorReport { row, col })
}

enum AcceptSpan {
    Full,
    Word,
}

/// Leading whitespace plus the first word of `suffix`.
fn first_word(suffix: &str) -> &str {
    let mut end = 0;
    let mut in_word = false;
    for (idx, ch) in suffix.char_indices() {
        if ch.is_whitespace() {
            if in_word {
                break;
            }
        } else {
            in_word = true;
        }
        end = idx + ch.len_utf8();
    }
    &suffix[..end]
}

/// True if `bytes` is a proper prefix of some interceptable key sequence —
/// i.e. more bytes could still turn it into one.
fn is_partial_interceptable(bytes: &[u8]) -> bool {
    KEY_TABLE
        .iter()
        .any(|(seq, _)| seq.len() > bytes.len() && seq.starts_with(bytes))
}

/// Longest interceptable key at the start of `rest`. A bare `ESC` only
/// counts when nothing follows it — `ESC [` etc. is the start of some other
/// key's sequence, not an Escape press.
fn match_key(rest: &[u8]) -> Option<(&'static [u8], Key)> {
    KEY_TABLE
        .iter()
        .filter(|(seq, _)| rest.starts_with(seq))
        .max_by_key(|(seq, _)| seq.len())
        .copied()
        .filter(|(seq, _)| *seq != b"\x1b" || rest.len() == 1)
}

fn read_more(bytes: &mut Vec<u8>, stdin: &mut impl Read) -> bool {
    let mut more = [0u8; 16];
    match stdin.read(&mut more) {
        Ok(n) if n > 0 => {
            bytes.extend_from_slice(&more[..n]);
            true
        }
        _ => false,
    }
}

/// FIONREAD rather than poll(2), which doesn't work on tty fds on macOS.
fn wait_for_more(fd: impl AsFd) -> bool {
    for _ in 0..ESC_POLL_RETRIES {
        if rustix::io::ioctl_fionread(&fd).unwrap_or(0) > 0 {
            return true;
        }
        std::thread::sleep(ESC_POLL_INTERVAL);
    }
    rustix::io::ioctl_fionread(&fd).unwrap_or(0) > 0
}

// ---------------------------------------------------------------------------
// UI thread: querying and overlay updates
// ---------------------------------------------------------------------------

fn spawn_ui_thread<W: Write + Send + 'static>(
    provider: SuggestionProvider,
    compositor: Arc<Mutex<Compositor<W>>>,
    ui_rx: Receiver<UiEvent>,
    state: Arc<Mutex<PopupState>>,
) {
    std::thread::spawn(move || {
        let mut preempted = None;
        loop {
            let first = match preempted.take() {
                Some(event) => event,
                None => match ui_rx.recv() {
                    Ok(event) => event,
                    Err(_) => break,
                },
            };
            // Coalesce bursts (per-keystroke echo chunks): only the most
            // recent event matters for what ends up on screen.
            let mut event = first;
            while let Ok(next) = ui_rx.try_recv() {
                event = next;
            }

            match event {
                // A provider call can take its full timeout; an event that
                // arrived meanwhile (an Enter's Hide, a newer line) must
                // not wait behind painting its stale result.
                UiEvent::Query(line) => {
                    preempted = handle_query(&provider, &compositor, &state, line, &ui_rx);
                }
                UiEvent::Hide => {
                    let mut st = lock_unpoisoned(&state);
                    st.suggestions = Vec::new().into();
                    st.selected = 0;
                    drop(st);
                    lock_unpoisoned(&compositor).set_overlay(None);
                }
            }
        }
    });
}

/// Fetch and paint suggestions for `line`. Returns an event that arrived
/// during the fetch, in which case the stale result was discarded unpainted
/// and the caller should process the newer event instead.
fn handle_query<W: Write>(
    provider: &SuggestionProvider,
    compositor: &Arc<Mutex<Compositor<W>>>,
    state: &Arc<Mutex<PopupState>>,
    line: String,
    ui_rx: &Receiver<UiEvent>,
) -> Option<UiEvent> {
    let suppressed = {
        let mut st = lock_unpoisoned(state);
        st.line = line.clone();
        if st.dismissed_for.as_deref() != Some(line.as_str()) {
            st.dismissed_for = None;
        }
        st.dismissed_for.is_some()
    };

    let suggestions: Vec<Suggestion> = if suppressed || line.trim().is_empty() {
        Vec::new()
    } else {
        provider(&line)
            .into_iter()
            .filter(|s| s.text != line)
            .collect()
    };

    if let Ok(newer) = ui_rx.try_recv() {
        return Some(newer);
    }

    let content = {
        let mut st = lock_unpoisoned(state);
        st.suggestions = suggestions.into();
        st.selected = 0;
        st.overlay_content()
    };

    lock_unpoisoned(compositor).set_overlay(content);
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compositor::OverlayFlags;
    use rstest::rstest;
    use std::os::unix::net::UnixStream;

    // -- InputTracker -------------------------------------------------------

    /// An empty, never-fed UI channel for direct handle_query calls.
    fn idle_rx() -> Receiver<UiEvent> {
        let (tx, rx) = mpsc::channel();
        std::mem::forget(tx);
        rx
    }

    fn tracker() -> (InputTracker, Receiver<UiEvent>) {
        let (tracker, rx, _clock) = tracker_with_clock();
        (tracker, rx)
    }

    fn tracker_with_clock() -> (InputTracker, Receiver<UiEvent>, Arc<ActivityClock>) {
        let (ui_tx, ui_rx) = mpsc::channel();
        let clock = Arc::new(ActivityClock::new());
        // Most tests simulate echo of live typing.
        clock.touch();
        (
            InputTracker::new(ui_tx, Arc::new(AtomicU16::new(80)), clock.clone(), None),
            ui_rx,
            clock,
        )
    }

    #[rstest]
    fn joins_soft_wrapped_rows_in_the_tracked_line() {
        let mut parser = vt100::Parser::new(4, 8, 0);
        parser.process(b"123456789");
        let mut line = String::new();

        line_up_to_cursor(parser.screen(), 8, &mut line);

        assert_eq!(line, "123456789");
    }

    #[rstest]
    fn preserves_hard_newlines_in_the_tracked_line() {
        let mut parser = vt100::Parser::new(4, 8, 0);
        parser.process(b"1234\r\n5678");
        let mut line = String::new();

        line_up_to_cursor(parser.screen(), 8, &mut line);

        assert_eq!(line, "1234\n5678");
    }

    /// The session-ready hook fires exactly once, at the first prompt
    /// marker — the moment the shell's startup is over.
    #[rstest]
    fn session_ready_fires_once_at_first_prompt() {
        use std::sync::atomic::AtomicUsize;
        let fired = Arc::new(AtomicUsize::new(0));
        let hook = {
            let fired = fired.clone();
            Box::new(move || {
                fired.fetch_add(1, Ordering::Relaxed);
            })
        };
        let (ui_tx, _ui_rx) = mpsc::channel();
        let clock = Arc::new(ActivityClock::new());
        let mut tracker = InputTracker::new(ui_tx, Arc::new(AtomicU16::new(80)), clock, Some(hook));

        tracker.push(b"banner text, no markers yet");
        assert_eq!(fired.load(Ordering::Relaxed), 0);

        tracker.push(b"\x1b]133;A\x07$ \x1b]133;B\x07");
        assert_eq!(fired.load(Ordering::Relaxed), 1);

        tracker.push(b"ls\r\n\x1b]133;C\x07\x1b]133;A\x07$ \x1b]133;B\x07");
        assert_eq!(
            fired.load(Ordering::Relaxed),
            1,
            "later prompts don't re-fire"
        );
    }

    /// Program output containing OSC 133 markers (cat of a typescript, a
    /// replayed CI log) must not conjure the popup: input-zone changes only
    /// count as typing when a keystroke arrived recently.
    #[rstest]
    fn output_without_keystrokes_does_not_query() {
        let (ui_tx, ui_rx) = mpsc::channel();
        // Never-touched clock: no keystroke has happened yet.
        let clock = Arc::new(ActivityClock::new());
        let mut tracker =
            InputTracker::new(ui_tx, Arc::new(AtomicU16::new(80)), clock.clone(), None);

        tracker.push(b"\x1b]133;A\x07$ \x1b]133;B\x07ls -la");
        assert!(
            last_query(&ui_rx).is_none(),
            "no query without a recent keystroke"
        );

        // The user starts typing: queries resume.
        clock.touch();
        tracker.push(b"x");
        assert_eq!(last_query(&ui_rx).as_deref(), Some("ls -lax"));
    }

    fn last_query(rx: &Receiver<UiEvent>) -> Option<String> {
        let mut last = None;
        while let Ok(event) = rx.try_recv() {
            if let UiEvent::Query(line) = event {
                last = Some(line);
            }
        }
        last
    }

    #[rstest]
    fn tracks_typed_line_in_input_zone() {
        let (mut tracker, rx) = tracker();
        tracker.push(b"\x1b]133;A\x07$ \x1b]133;B\x07gi");
        assert_eq!(last_query(&rx).as_deref(), Some("gi"));

        tracker.push(b"t st");
        assert_eq!(last_query(&rx).as_deref(), Some("git st"));
    }

    #[rstest]
    fn replays_backspaces() {
        let (mut tracker, rx) = tracker();
        tracker.push(b"\x1b]133;B\x07gix\x08 \x08t");
        assert_eq!(last_query(&rx).as_deref(), Some("git"));
    }

    /// zsh's SIGWINCH/^L redisplay reprints prompt AND buffer before the
    /// input marker, then only repositions the cursor: the typed line
    /// never reappears in the input zone and must be carried across the
    /// marker reset. (Byte sequence captured from a live zsh under tmux.)
    #[rstest]
    fn prompt_redraw_without_command_keeps_the_line() {
        let (mut tracker, rx) = tracker();
        tracker.push(b"\x1b]133;A\x07$ \x1b]133;B\x07git p");
        assert_eq!(last_query(&rx).as_deref(), Some("git p"));

        tracker.push(
            b"\r\r\x1b[0m\x1b[27m\x1b[24m\x1b[J\x1b]133;A;cl=line\x07$ git p\x1b[K\x1b[43C\x1b]133;B\x07\x1b[43D",
        );
        tracker.push(b"u");
        assert_eq!(last_query(&rx).as_deref(), Some("git pu"));
    }

    /// After a command actually ran, the next prompt is genuinely new: the
    /// carried line must not leak into it.
    #[rstest]
    fn new_prompt_after_a_command_still_resets() {
        let (mut tracker, rx) = tracker();
        tracker.push(b"\x1b]133;B\x07git p\r\n\x1b]133;C\x07output\r\n");
        tracker.push(b"\x1b]133;A\x07$ \x1b]133;B\x07");
        tracker.push(b"ls");
        assert_eq!(last_query(&rx).as_deref(), Some("ls"));
    }

    /// zsh-autosuggestions paints its ghost after the cursor, inside the
    /// input zone. It is display, not input: queries must stop at the
    /// cursor or the ghost gets treated as typed text.
    #[rstest]
    fn text_painted_after_the_cursor_is_not_input() {
        let (mut tracker, rx) = tracker();
        // Type "git ", then a plugin paints a dim ghost and moves the
        // cursor back to the end of the typed text.
        tracker.push(b"\x1b]133;B\x07git \x1b[90mstatus --short\x1b[0m\x1b[14D");
        assert_eq!(last_query(&rx).as_deref(), Some("git "));

        // Typing continues over the ghost.
        tracker.push(b"p");
        assert_eq!(last_query(&rx).as_deref(), Some("git p"));
    }

    /// A trailing space is when next-word suggestions should begin — the
    /// query must carry it instead of waiting for the word's first letter.
    #[rstest]
    fn trailing_space_starts_the_next_word() {
        let (mut tracker, rx) = tracker();
        tracker.push(b"\x1b]133;B\x07git");
        assert_eq!(last_query(&rx).as_deref(), Some("git"));

        tracker.push(b" ");
        assert_eq!(last_query(&rx).as_deref(), Some("git "));

        // Backspacing the space shrinks the query back.
        tracker.push(b"\x08 \x08");
        assert_eq!(last_query(&rx).as_deref(), Some("git"));
    }

    #[rstest]
    fn hides_when_command_executes() {
        let (mut tracker, rx) = tracker();
        tracker.push(b"\x1b]133;B\x07ls");
        let _ = last_query(&rx);

        tracker.push(b"\r\n\x1b]133;C\x07output");
        assert!(matches!(rx.try_recv(), Ok(UiEvent::Hide)));
    }

    #[rstest]
    fn output_zone_bytes_do_not_query() {
        let (mut tracker, rx) = tracker();
        tracker.push(b"\x1b]133;C\x07some command output\r\n");
        while let Ok(event) = rx.try_recv() {
            assert!(!matches!(event, UiEvent::Query(_)));
        }
    }

    /// The tracker must produce identical results no matter how the stream
    /// is split across pushes.
    #[rstest]
    fn split_at_every_boundary_is_equivalent() {
        let stream = b"\x1b]133;A\x07$ \x1b]133;B\x07gi\x1b[32mt\x1b[0m st\x08\x08status";
        let (mut whole, whole_rx) = tracker();
        whole.push(stream);
        let expected = last_query(&whole_rx);
        assert!(expected.is_some());

        for split in 1..stream.len() {
            let (mut split_tracker, rx) = tracker();
            split_tracker.push(&stream[..split]);
            split_tracker.push(&stream[split..]);
            assert_eq!(
                last_query(&rx),
                expected,
                "diverged when split at byte {split}"
            );
        }
    }

    // -- KeyFilter ----------------------------------------------------------

    struct Fixture {
        filter: KeyFilter<Vec<u8>>,
        stdin: UnixStream,
        peer: UnixStream,
    }

    type SharedCompositor = Arc<Mutex<Compositor<Vec<u8>>>>;

    /// A Vec-backed compositor plus its visibility flags.
    fn test_compositor() -> (Arc<OverlayFlags>, SharedCompositor) {
        let flags = Arc::new(OverlayFlags::default());
        let compositor = Arc::new(Mutex::new(Compositor::new(
            24,
            80,
            Vec::new(),
            flags.clone(),
            true,
        )));
        (flags, compositor)
    }

    /// Build a filter over a real (Vec-backed) compositor with the overlay
    /// painted, so visibility flags reflect a live popup.
    fn fixture(line: &str, suggestions: &[&str], selected: usize) -> Fixture {
        let (flags, compositor) = test_compositor();

        let state = Arc::new(Mutex::new(PopupState {
            line: line.to_string(),
            suggestions: suggestions.iter().map(|s| Suggestion::history(s)).collect(),
            selected,
            dismissed_for: None,
        }));

        {
            let mut c = compositor.lock().unwrap();
            c.apply_pty(format!("$ {line}").as_bytes()).unwrap();
            c.set_overlay(state.lock().unwrap().overlay_content());
        }

        let (stdin, peer) = UnixStream::pair().unwrap();
        stdin.set_nonblocking(false).unwrap();

        Fixture {
            filter: KeyFilter {
                state,
                compositor,
                flags,
                resync: ResyncState::default(),
                key_scratch: Vec::new(),
                out_scratch: Vec::new(),
            },
            stdin,
            peer,
        }
    }

    #[rstest]
    fn tab_accepts_prefix_suffix_and_hides() {
        let mut fx = fixture("git st", &["git status", "git stash"], 0);
        let out = fx.filter.process(b"\t", &mut fx.stdin);
        assert_eq!(&*out, b"atus");
        assert!(!fx.filter.visible(), "overlay hidden after accept");
    }

    #[rstest]
    fn right_arrow_accepts_ghost() {
        let mut fx = fixture("git st", &["git status"], 0);
        assert!(fx.filter.ghost_visible());
        let out = fx.filter.process(b"\x1b[C", &mut fx.stdin);
        assert_eq!(&*out, b"atus");
    }

    #[rstest]
    fn fuzzy_accept_replaces_whole_line() {
        let mut fx = fixture("stat", &["git status"], 0);
        let out = fx.filter.process(b"\t", &mut fx.stdin);
        assert_eq!(&*out, b"\x15git status");
    }

    #[rstest]
    fn word_accept_takes_one_word_and_keeps_popup_state() {
        let mut fx = fixture("git", &["git status --short"], 0);
        let out = fx.filter.process(b"\x1b[1;3C", &mut fx.stdin);
        assert_eq!(&*out, b" status");
        let st = fx.filter.state.lock().unwrap();
        assert!(!st.suggestions.is_empty(), "popup survives word accept");
        assert!(st.dismissed_for.is_none());
    }

    #[rstest]
    fn navigation_wraps_and_repaints() {
        let mut fx = fixture("g", &["git status", "grep foo"], 0);
        assert!(fx.filter.process(b"\x1b[B", &mut fx.stdin).is_empty());
        assert_eq!(fx.filter.state.lock().unwrap().selected, 1);
        assert!(fx.filter.process(b"\x1b[B", &mut fx.stdin).is_empty());
        assert_eq!(fx.filter.state.lock().unwrap().selected, 0);
        assert!(fx.filter.process(b"\x1b[A", &mut fx.stdin).is_empty());
        assert_eq!(fx.filter.state.lock().unwrap().selected, 1);
    }

    #[rstest]
    fn batched_keys_are_each_handled_in_order() {
        let mut fx = fixture("g", &["git status", "grep foo"], 0);
        // A typed char and a Down arrow coalesced into one read: the char
        // forwards, the arrow navigates.
        let out = fx.filter.process(b"x\x1b[B", &mut fx.stdin);
        assert_eq!(&*out, b"x");
        assert_eq!(fx.filter.state.lock().unwrap().selected, 1);

        // Arrow auto-repeat: two Downs in one read both apply.
        let out = fx.filter.process(b"\x1b[B\x1b[B", &mut fx.stdin);
        assert!(out.is_empty());
        assert_eq!(fx.filter.state.lock().unwrap().selected, 1);

        // Navigation batched with an accept: nav applies first, then the
        // accept emits the newly selected suggestion's suffix.
        let out = fx.filter.process(b"\x1b[B\t", &mut fx.stdin);
        assert_eq!(&*out, b"it status");
    }

    #[rstest]
    fn unmatched_escape_sequences_pass_through_intact() {
        let mut fx = fixture("git st", &["git status"], 0);
        // Ctrl+Left shares a prefix with the intercepted Ctrl+Right; it must
        // neither dismiss (bare-ESC misfire) nor be mangled.
        let out = fx.filter.process(b"\x1b[1;5D", &mut fx.stdin);
        assert_eq!(&*out, b"\x1b[1;5D");
        assert!(fx.filter.visible(), "popup not dismissed by unrelated CSI");
    }

    #[rstest]
    fn split_arrow_sequence_is_reassembled() {
        let mut fx = fixture("g", &["git status", "grep foo"], 0);
        // The tail of the Down arrow arrives on the fd before the filter
        // asks for it, as with a slow link delivering ESC first.
        fx.peer.write_all(b"[B").unwrap();
        let out = fx.filter.process(b"\x1b", &mut fx.stdin);
        assert!(out.is_empty(), "navigation consumed: {out:?}");
        assert_eq!(fx.filter.state.lock().unwrap().selected, 1);
    }

    #[rstest]
    fn lone_escape_dismisses_after_timeout() {
        let mut fx = fixture("git st", &["git status"], 0);
        let out = fx.filter.process(b"\x1b", &mut fx.stdin);
        assert!(out.is_empty());
        assert!(!fx.filter.visible());
        assert_eq!(
            fx.filter.state.lock().unwrap().dismissed_for.as_deref(),
            Some("git st")
        );
    }

    #[rstest]
    fn hidden_overlay_forwards_everything_untouched() {
        let mut fx = fixture("git st", &["git status"], 0);
        fx.filter.compositor.lock().unwrap().set_overlay(None);
        for chunk in [b"\t".as_slice(), b"\x1b[A", b"\x1b", b"\x1b[C"] {
            assert_eq!(&*fx.filter.process(chunk, &mut fx.stdin), chunk);
        }
    }

    #[rstest]
    fn unrelated_keys_forward_while_visible() {
        let mut fx = fixture("git st", &["git status"], 0);
        assert_eq!(&*fx.filter.process(b"x", &mut fx.stdin), b"x");
        assert_eq!(&*fx.filter.process(b"\x7f", &mut fx.stdin), b"\x7f");
        // Left arrow is never intercepted.
        assert_eq!(&*fx.filter.process(b"\x1b[D", &mut fx.stdin), b"\x1b[D");
    }

    // -- Resize cursor resync -------------------------------------------

    fn arm_resync(fx: &Fixture) {
        fx.filter.flags.resync.store(true, Ordering::Release);
    }

    fn resync_pending(fx: &Fixture) -> bool {
        fx.filter.flags.resync.load(Ordering::Acquire)
    }

    /// 0-based model cursor, to check what `seed_cursor` received.
    fn model_cursor(fx: &Fixture) -> (u16, u16) {
        fx.filter
            .compositor
            .lock()
            .unwrap()
            .screen()
            .cursor_position()
    }

    #[rstest]
    #[case::lone_esc(b"\x1b".as_slice(), ReplyScan::Partial)]
    #[case::csi_start(b"\x1b[".as_slice(), ReplyScan::Partial)]
    #[case::private_start(b"\x1b[?".as_slice(), ReplyScan::Partial)]
    #[case::cursor_prefix(b"\x1b[12;3".as_slice(), ReplyScan::Partial)]
    #[case::cursor(
        b"\x1b[12;34R".as_slice(),
        ReplyScan::Cursor { len: 8, report: CursorReport { row: 12, col: 34 } }
    )]
    #[case::fence(b"\x1b[?64;1;9c".as_slice(), ReplyScan::Fence { len: 10 })]
    #[case::arrow_key(b"\x1b[A".as_slice(), ReplyScan::Other)]
    #[case::ss3_key(b"\x1bOA".as_slice(), ReplyScan::Other)]
    #[case::three_params(b"\x1b[1;2;3R".as_slice(), ReplyScan::Other)]
    #[case::private_cpr(b"\x1b[?12;3R".as_slice(), ReplyScan::Other)]
    #[case::row_overflow(b"\x1b[99999;1R".as_slice(), ReplyScan::Other)]
    fn classifies_handshake_replies(#[case] bytes: &[u8], #[case] expected: ReplyScan) {
        assert_eq!(scan_reply(bytes), expected);
    }

    #[rstest]
    fn resync_consumes_reply_and_seeds_model() {
        let mut fx = fixture("git st", &["git status"], 0);
        arm_resync(&fx);
        let out = fx.filter.process(b"\x1b[12;5R\x1b[?1;2c", &mut fx.stdin);
        assert!(out.is_empty(), "reply fully swallowed: {out:?}");
        assert!(!resync_pending(&fx));
        assert_eq!(model_cursor(&fx), (11, 4), "seeded from the 1-based report");
    }

    #[rstest]
    fn resync_reassembles_reply_split_across_reads() {
        let mut fx = fixture("git st", &["git status"], 0);
        arm_resync(&fx);
        assert!(fx.filter.process(b"\x1b[12;", &mut fx.stdin).is_empty());
        assert!(resync_pending(&fx), "still waiting for the fence");
        assert!(fx.filter.process(b"5R\x1b[?1;2c", &mut fx.stdin).is_empty());
        assert!(!resync_pending(&fx));
        assert_eq!(model_cursor(&fx), (11, 4));
    }

    #[rstest]
    fn resync_seeds_from_last_report_before_fence() {
        let mut fx = fixture("git st", &["git status"], 0);
        arm_resync(&fx);
        // A stale p10k reply, type-ahead, our reply, the fence, more keys.
        let out = fx
            .filter
            .process(b"\x1b[3;42Rls\x1b[24;1R\x1b[?1;2cx", &mut fx.stdin);
        assert_eq!(&*out, b"lsx", "non-reply bytes forwarded in order");
        assert_eq!(model_cursor(&fx), (23, 0));
        assert!(!resync_pending(&fx));
    }

    #[rstest]
    fn resync_forwards_unrelated_bytes_while_waiting() {
        let mut fx = fixture("git st", &["git status"], 0);
        arm_resync(&fx);
        let out = fx.filter.process(b"echo hi", &mut fx.stdin);
        assert_eq!(&*out, b"echo hi");
        assert!(resync_pending(&fx), "junk must not end the handshake");
    }

    #[rstest]
    fn cursor_reports_pass_through_when_no_resync_pending() {
        let mut fx = fixture("git st", &["git status"], 0);
        // p10k's own CPR reply: not ours to touch outside a handshake.
        let chunk = b"\x1b[42;7R";
        assert_eq!(&*fx.filter.process(chunk, &mut fx.stdin), chunk);
        assert_eq!(model_cursor(&fx), (0, 8), "model cursor untouched");
    }

    #[rstest]
    fn resync_timeout_flushes_carried_bytes_unseeded() {
        let mut fx = fixture("git st", &["git status"], 0);
        arm_resync(&fx);
        assert!(fx.filter.process(b"\x1b[12;", &mut fx.stdin).is_empty());
        // Expire the armed deadline without sleeping.
        fx.filter.resync.deadline = Some(Instant::now() - Duration::from_millis(1));
        let out = fx.filter.process(b"ok", &mut fx.stdin);
        assert_eq!(&*out, b"\x1b[12;ok", "withheld bytes released to the shell");
        assert!(!resync_pending(&fx));
        assert_eq!(model_cursor(&fx), (0, 8), "no seed from a timed-out reply");
    }

    #[rstest]
    fn first_word_takes_leading_space_and_word() {
        assert_eq!(first_word(" status --short"), " status");
        assert_eq!(first_word("atus"), "atus");
        assert_eq!(first_word("  "), "  ");
    }

    // -- UI thread ----------------------------------------------------------

    #[rstest]
    fn query_paints_and_dismissal_suppresses() {
        let (flags, compositor) = test_compositor();
        compositor.lock().unwrap().apply_pty(b"$ git st").unwrap();
        let state = Arc::new(Mutex::new(PopupState::default()));
        // Two suggestions so the dropdown renders (a lone prefix match
        // would show as ghost text only).
        let provider: SuggestionProvider = Box::new(|_| {
            vec![
                Suggestion::history("git status"),
                Suggestion::history("git stash"),
            ]
        });

        handle_query(
            &provider,
            &compositor,
            &state,
            "git st".to_string(),
            &idle_rx(),
        );
        assert!(flags.popup.load(Ordering::Acquire));

        state.lock().unwrap().dismissed_for = Some("git st".to_string());
        handle_query(
            &provider,
            &compositor,
            &state,
            "git st".to_string(),
            &idle_rx(),
        );
        assert!(!flags.popup.load(Ordering::Acquire));

        // A changed line clears the dismissal.
        handle_query(
            &provider,
            &compositor,
            &state,
            "git sta".to_string(),
            &idle_rx(),
        );
        assert!(flags.popup.load(Ordering::Acquire));
        assert!(state.lock().unwrap().dismissed_for.is_none());
    }

    #[rstest]
    fn empty_line_clears_overlay() {
        let (flags, compositor) = test_compositor();
        let state = Arc::new(Mutex::new(PopupState::default()));
        let provider: SuggestionProvider = Box::new(|_| vec![Suggestion::history("anything")]);

        handle_query(&provider, &compositor, &state, "g".to_string(), &idle_rx());
        assert!(flags.popup.load(Ordering::Acquire));
        handle_query(&provider, &compositor, &state, "".to_string(), &idle_rx());
        assert!(!flags.popup.load(Ordering::Acquire));
    }
}
