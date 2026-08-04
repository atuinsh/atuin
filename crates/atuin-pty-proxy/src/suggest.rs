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
use std::time::Duration;

use atuin_common::ansi;

use crate::compositor::{Compositor, OverlayContent, OverlayFlags, lock_unpoisoned};
use crate::osc133::{Event, Parser, Segment, Zone};

/// One dropdown entry: the full command line it would produce, plus where
/// it came from (rendered as an icon next to the suggestion).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Suggestion {
    pub text: String,
    pub source: SuggestionSource,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SuggestionSource {
    History,
    Completion,
}

#[cfg(test)]
impl Suggestion {
    /// Test shorthand: a history-sourced suggestion.
    pub(crate) fn history(text: &str) -> Self {
        Self {
            text: text.to_string(),
            source: SuggestionSource::History,
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
) -> Suggest<W> {
    let state = Arc::new(Mutex::new(PopupState::default()));
    let (ui_tx, ui_rx) = mpsc::channel();

    spawn_ui_thread(provider, compositor.clone(), ui_rx, state.clone());

    Suggest {
        tracker: InputTracker::new(ui_tx, cols),
        keys: KeyFilter {
            state,
            compositor,
            flags,
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
    cols: Arc<AtomicU16>,
    ui_tx: Sender<UiEvent>,
}

impl InputTracker {
    fn new(ui_tx: Sender<UiEvent>, cols: Arc<AtomicU16>) -> Self {
        let grid_cols = cols.load(Ordering::Relaxed).max(1);
        Self {
            parser: Parser::new(),
            line_screen: vt100::Parser::new(LINE_GRID_ROWS, grid_cols, 0),
            grid_cols,
            fed: 0,
            onlcr_scratch: Vec::new(),
            cols,
            ui_tx,
        }
    }

    pub(crate) fn push(&mut self, data: &[u8]) {
        let line_screen = &mut self.line_screen;
        let onlcr_scratch = &mut self.onlcr_scratch;
        let fed = &mut self.fed;
        let grid_cols = &mut self.grid_cols;
        let cols = &self.cols;
        let mut input_changed = false;
        let mut hide = false;
        self.parser.segments(data, |segment| match segment {
            Segment::Text(Zone::Input, bytes) => {
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
                if matches!(located.event, Event::PromptStart | Event::CommandStart) {
                    *grid_cols = cols.load(Ordering::Relaxed).max(1);
                    *line_screen = vt100::Parser::new(LINE_GRID_ROWS, *grid_cols, 0);
                    *fed = 0;
                }
                // Any marker starts a fresh zone: whatever was typed before
                // it in this chunk no longer needs a popup update of its own.
                hide = true;
                input_changed = false;
            }
        });

        if input_changed && !self.overflowed() {
            let _ = self.ui_tx.send(UiEvent::Query(self.current_line()));
        } else if hide || input_changed {
            let _ = self.ui_tx.send(UiEvent::Hide);
        }
    }

    fn overflowed(&self) -> bool {
        // Conservative: every fed byte could take a cell; leave slack rows
        // for cursor movement so the line start provably hasn't scrolled.
        self.fed > (LINE_GRID_ROWS as usize - 2) * self.grid_cols as usize
    }

    /// The line as displayed, mirroring `ansi::to_plain_text`'s trimming.
    fn current_line(&self) -> String {
        let contents = self.line_screen.screen().contents();
        let trimmed = contents.trim_end();
        let mut line = String::with_capacity(trimmed.len());
        for (i, row) in trimmed.lines().enumerate() {
            if i > 0 {
                line.push('\n');
            }
            line.push_str(row.trim_end());
        }
        line.trim_matches(|c| c == '\r' || c == '\n').to_string()
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
}

impl<W: Write> KeyFilter<W> {
    /// Process one stdin chunk and return the bytes to forward to the pty.
    ///
    /// Pass-through when hidden. When visible, the chunk is tokenized so
    /// keys batched into one read (arrow auto-repeat, fast typing) are each
    /// intercepted or forwarded in order. A chunk ending in a possible key
    /// prefix (e.g. a lone `ESC`) briefly waits for its tail before
    /// deciding; an `ESC` that stays lone dismisses the popup.
    pub(crate) fn process<'a>(
        &self,
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

        let mut bytes = chunk.to_vec();
        let mut out = Vec::new();
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
        Cow::Owned(out)
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
        while let Ok(first) = ui_rx.recv() {
            // Coalesce bursts (per-keystroke echo chunks): only the most
            // recent event matters for what ends up on screen.
            let mut event = first;
            while let Ok(next) = ui_rx.try_recv() {
                event = next;
            }

            match event {
                UiEvent::Query(line) => handle_query(&provider, &compositor, &state, line),
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

fn handle_query<W: Write>(
    provider: &SuggestionProvider,
    compositor: &Arc<Mutex<Compositor<W>>>,
    state: &Arc<Mutex<PopupState>>,
    line: String,
) {
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

    let content = {
        let mut st = lock_unpoisoned(state);
        st.suggestions = suggestions.into();
        st.selected = 0;
        st.overlay_content()
    };

    lock_unpoisoned(compositor).set_overlay(content);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compositor::OverlayFlags;
    use rstest::rstest;
    use std::os::unix::net::UnixStream;

    // -- InputTracker -------------------------------------------------------

    fn tracker() -> (InputTracker, Receiver<UiEvent>) {
        let (ui_tx, ui_rx) = mpsc::channel();
        (
            InputTracker::new(ui_tx, Arc::new(AtomicU16::new(80))),
            ui_rx,
        )
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

        handle_query(&provider, &compositor, &state, "git st".to_string());
        assert!(flags.popup.load(Ordering::Acquire));

        state.lock().unwrap().dismissed_for = Some("git st".to_string());
        handle_query(&provider, &compositor, &state, "git st".to_string());
        assert!(!flags.popup.load(Ordering::Acquire));

        // A changed line clears the dismissal.
        handle_query(&provider, &compositor, &state, "git sta".to_string());
        assert!(flags.popup.load(Ordering::Acquire));
        assert!(state.lock().unwrap().dismissed_for.is_none());
    }

    #[rstest]
    fn empty_line_clears_overlay() {
        let (flags, compositor) = test_compositor();
        let state = Arc::new(Mutex::new(PopupState::default()));
        let provider: SuggestionProvider = Box::new(|_| vec![Suggestion::history("anything")]);

        handle_query(&provider, &compositor, &state, "g".to_string());
        assert!(flags.popup.load(Ordering::Acquire));
        handle_query(&provider, &compositor, &state, "".to_string());
        assert!(!flags.popup.load(Ordering::Acquire));
    }
}
