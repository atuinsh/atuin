//! The child's screen model, fused with everything that must stay consistent
//! with it: the `seq` counter, the keyframe cadence, and the query replies.

use std::time::{Duration, Instant};

use crate::Size;
use crate::backpressure::Frame;
use crate::query;
use crate::render::keyframe_bytes;

/// Bytes of child output after which a fresh keyframe is emitted.
const KEYFRAME_BYTES: u64 = 256 * 1024;
/// Maximum interval between keyframes.
const KEYFRAME_INTERVAL: Duration = Duration::from_secs(5);
/// Render coalescing interval (~60 fps).
pub(super) const FRAME_INTERVAL: Duration = Duration::from_millis(16);
/// How often the keyframe ticker checks the cadence. Small enough that a
/// keyframe requested while the child is silent goes out promptly.
pub(super) const KEYFRAME_TICK: Duration = Duration::from_millis(100);

/// The child's screen model plus the frame sequencing built on top of it.
///
/// **The seq invariant** (spec §5): a keyframe stamped `seq = K` reflects
/// exactly the output bytes stamped `<= K`. This type is what keeps it true —
/// frames are only minted through `&mut self` methods, so the screen state and
/// the counter can never be observed out of step. The invariant is
/// *structural*: the session's central task is the single owner, and it both
/// mints and sends every frame, so frames enter the channel in seq order with
/// no lock involved.
pub(super) struct ScreenState {
    parser: vt100::Parser,
    /// The grid the parser was last built or resized to, floored at
    /// [`crate::MIN_COLS`] x [`crate::MIN_CHILD_ROWS`]. Kept alongside the
    /// parser because [`ScreenState::vt100_guarded`] needs a size to rebuild
    /// at, and `vt100` 0.16.2's own `Screen::size()` is not reachable from a
    /// parser that just unwound.
    size: Size,
    /// The next sequence number for `output`/`keyframe`, starting at 1.
    seq: u64,
    /// The next minting opportunity must also produce a keyframe. Starts
    /// `true` so the session opens with a complete frame for early joiners.
    want_keyframe: bool,
    /// When the last keyframe went out, driving the periodic cadence — no
    /// matter which select arm emitted it, the next one is due a full interval
    /// later.
    last_keyframe: Instant,
    /// Child bytes emitted since the last keyframe, for the byte-based cadence.
    bytes_since_keyframe: u64,
}

/// What one chunk of child output produced, in mint order: the `Output` frame,
/// then (when a request or the cadence came due) the keyframe reflecting it.
pub(super) struct ProcessOutcome {
    pub(super) output: Frame,
    pub(super) keyframe: Option<Frame>,
    /// Synthetic replies to the terminal queries found in the chunk (CPR /
    /// DA), computed from the **post-update** cursor position — see
    /// [`query::replies`].
    pub(super) replies: Vec<u8>,
}

impl ScreenState {
    /// A blank screen sized to the rows *available to the child* — the host's
    /// terminal minus the bar row, subtracted exactly once by `run_share`.
    ///
    /// The size is floored at [`crate::MIN_COLS`] x [`crate::MIN_CHILD_ROWS`]
    /// before it reaches `vt100`. Every caller already clamps or refuses
    /// upstream (`host_size_from`, `clamp_host_size`, `clamp_child`,
    /// `proxy_tap::clamp_tap_size`); this is the last of those gates and the
    /// only one that sits directly on the library that panics.
    pub(super) fn new(size: Size) -> Self {
        let size = clamp(size);
        Self {
            parser: vt100::Parser::new(size.rows, size.cols, 0),
            size,
            seq: 1,
            want_keyframe: true, // initial keyframe
            last_keyframe: Instant::now(),
            bytes_since_keyframe: 0,
        }
    }

    /// The current screen, for compositing.
    pub(super) fn screen(&self) -> &vt100::Screen {
        self.parser.screen()
    }

    /// Run a `vt100` operation, absorbing any panic inside the library.
    ///
    /// Defence in depth, mirroring `atuin_pty_proxy::screen::ParserState`'s
    /// guard of the same name and for the same reason: `vt100` 0.16.2 has
    /// panic paths beyond the degenerate-geometry ones the clamps remove, and
    /// this parser has no supervisor. It is owned by the session's central
    /// task, which is awaited directly by `run_share` — so an unwind out of
    /// here does not restart anything, it ends the share. (`TermGuard` still
    /// restores the tty on the way out, which is exactly why the failure looks
    /// like an unexplained exit rather than a broken terminal.)
    ///
    /// On a caught panic the model is rebuilt blank at the tracked size and a
    /// keyframe is requested, so the next mint repaints every viewer. Frame
    /// sequencing is untouched: `seq` lives outside the parser, so the seq
    /// invariant survives a rebuild.
    fn vt100_guarded(&mut self, op: impl FnOnce(&mut vt100::Parser)) {
        let caught =
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| op(&mut self.parser)));
        if caught.is_err() {
            self.parser = vt100::Parser::new(self.size.rows, self.size.cols, 0);
            self.want_keyframe = true;
        }
    }

    fn next_seq(&mut self) -> u64 {
        let seq = self.seq;
        self.seq += 1;
        seq
    }

    /// Feed one chunk of child output through the model and mint the frames it
    /// produces: always an `Output`, plus the keyframe when one was pending or
    /// the cadence came due.
    ///
    /// The cadence check runs *before* minting, so a due keyframe is produced
    /// in this same call and lands right after the output it reflects. The
    /// counters live here (not in the reader) because the ticker services the
    /// same cadence when the child goes quiet.
    #[must_use]
    pub(super) fn process_chunk(&mut self, chunk: &[u8]) -> ProcessOutcome {
        self.bytes_since_keyframe += chunk.len() as u64;
        if self.bytes_since_keyframe >= KEYFRAME_BYTES || self.keyframe_overdue() {
            self.want_keyframe = true;
        }

        self.vt100_guarded(|parser| parser.process(chunk));
        let output = Frame {
            seq: self.next_seq(),
            data: chunk.to_vec(),
        };
        let keyframe = if self.want_keyframe {
            Some(self.emit_keyframe())
        } else {
            None
        };
        // Answer device queries using the post-update cursor position.
        let replies = query::replies(chunk, self.parser.screen().cursor_position());
        ProcessOutcome {
            output,
            keyframe,
            replies,
        }
    }

    /// Mint a keyframe of the current screen with no accompanying output
    /// (startup, resize, hub resync, idle cadence), clearing any pending
    /// request and restarting both cadences.
    #[must_use]
    pub(super) fn emit_keyframe(&mut self) -> Frame {
        self.want_keyframe = false;
        let frame = Frame {
            seq: self.next_seq(),
            data: keyframe_bytes(self.parser.screen()),
        };
        self.bytes_since_keyframe = 0;
        self.last_keyframe = Instant::now();
        frame
    }

    /// Whether the ticker should mint a keyframe now: one is pending, or the
    /// periodic cadence has come due.
    #[must_use]
    pub(super) fn keyframe_due(&self) -> bool {
        self.want_keyframe || self.keyframe_overdue()
    }

    fn keyframe_overdue(&self) -> bool {
        self.last_keyframe.elapsed() >= KEYFRAME_INTERVAL
    }

    /// Resize the screen model and mint the repaint **in the same call**: the
    /// child may never write again, and a viewer resized without a keyframe
    /// would be left painting into a stale grid (spec §5.3).
    ///
    /// NB: `set_size` lives on `Screen`, not `Parser` — reach it via
    /// `screen_mut()`. (`vt100` 0.16.2 has no `Parser::set_size`.)
    ///
    /// The requested size is floored exactly as in [`ScreenState::new`], and
    /// the tracked size is updated **before** the call so a panic inside
    /// `vt100` rebuilds at the size that was asked for, not the one being
    /// left behind.
    #[must_use]
    pub(super) fn set_size(&mut self, size: Size) -> Frame {
        self.size = clamp(size);
        let (rows, cols) = (self.size.rows, self.size.cols);
        self.vt100_guarded(|parser| parser.screen_mut().set_size(rows, cols));
        self.emit_keyframe()
    }
}

/// Floor a child geometry at what `vt100` 0.16.2 survives — see
/// [`crate::MIN_COLS`] and [`crate::MIN_CHILD_ROWS`] for the two panic sites
/// each number comes from.
fn clamp(size: Size) -> Size {
    Size {
        cols: size.cols.max(crate::MIN_COLS),
        rows: size.rows.max(crate::MIN_CHILD_ROWS),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SIZE: Size = Size { cols: 80, rows: 24 };

    /// A `ScreenState` with the initial keyframe already drained, so cadence
    /// tests start from a clean slate.
    fn settled() -> ScreenState {
        let mut state = ScreenState::new(SIZE);
        let _ = state.emit_keyframe();
        state
    }

    #[test]
    fn a_fresh_screen_wants_its_initial_keyframe() {
        assert!(ScreenState::new(SIZE).keyframe_due());
    }

    #[test]
    fn first_chunk_mints_output_then_the_initial_keyframe() {
        let mut state = ScreenState::new(SIZE);
        let outcome = state.process_chunk(b"hi");
        assert_eq!(outcome.output.seq, 1);
        assert_eq!(outcome.output.data, b"hi");
        let keyframe = outcome
            .keyframe
            .expect("the initial keyframe rides the first chunk");
        assert_eq!(keyframe.seq, 2, "minted after the output it reflects");
    }

    #[test]
    fn seq_is_monotonic_across_outputs_and_keyframes() {
        let mut state = settled(); // seq 1 was the drained initial keyframe
        let first = state.process_chunk(b"a");
        let second = state.process_chunk(b"b");
        let keyframe = state.emit_keyframe();
        assert_eq!(first.output.seq, 2);
        assert!(first.keyframe.is_none());
        assert_eq!(second.output.seq, 3);
        assert_eq!(keyframe.seq, 4);
    }

    /// The seq invariant, concretely: a keyframe minted by `process_chunk`
    /// reflects the **post-chunk** screen, never the one before it.
    #[test]
    fn keyframe_reflects_the_screen_after_the_chunk_it_follows() {
        let mut state = settled();
        state.last_keyframe = Instant::now() - KEYFRAME_INTERVAL; // cadence due
        let outcome = state.process_chunk(b"hello");

        let mut expected = vt100::Parser::new(SIZE.rows, SIZE.cols, 0);
        expected.process(b"hello");
        let keyframe = outcome.keyframe.expect("the interval cadence was due");
        assert_eq!(keyframe.data, expected.screen().contents_formatted());
    }

    #[test]
    fn byte_cadence_mints_a_keyframe_once_enough_output_accumulated() {
        let mut state = settled();
        assert!(state.process_chunk(&[b'x'; 1024]).keyframe.is_none());
        let big = vec![b'x'; KEYFRAME_BYTES as usize];
        assert!(state.process_chunk(&big).keyframe.is_some());
    }

    #[test]
    fn interval_cadence_mints_a_keyframe_when_overdue() {
        let mut state = settled();
        assert!(!state.keyframe_due());
        state.last_keyframe = Instant::now() - KEYFRAME_INTERVAL;
        assert!(state.keyframe_due());
        assert!(state.process_chunk(b"x").keyframe.is_some());
    }

    #[test]
    fn emitting_a_keyframe_restarts_both_cadences() {
        let mut state = settled();
        let big = vec![b'x'; KEYFRAME_BYTES as usize];
        assert!(state.process_chunk(&big).keyframe.is_some());
        // The byte counter and the interval clock were both just reset.
        assert!(!state.keyframe_due());
        assert!(state.process_chunk(b"y").keyframe.is_none());
    }

    #[test]
    fn set_size_resizes_the_model_and_mints_the_repaint_in_the_same_call() {
        let mut state = settled();
        let keyframe = state.set_size(Size { cols: 40, rows: 10 });
        assert_eq!(state.screen().size(), (10, 40));

        let expected = vt100::Parser::new(10, 40, 0);
        assert_eq!(keyframe.data, expected.screen().contents_formatted());
        assert!(
            !state.keyframe_due(),
            "set_size serviced the pending request"
        );
    }

    /// The degenerate-geometry floor, end to end through the model.
    ///
    /// The size is asked for at 0x0 — what `crossterm::terminal::size()`
    /// reports under `script -q /dev/null CMD > file` — and must come back
    /// floored, never at the 1x1 that panics `vt100`'s `grid.rs`.
    ///
    /// The text MUST wrap. An empty feed proves nothing here: the compositor's
    /// own `composite_region_bottom_never_rises_above_row_2` survives a
    /// zero-row child precisely because it never draws, and the observed
    /// panics were all in *drawing* (`row.rs` index-out-of-bounds,
    /// `grid.rs` scroll underflow). So this pushes far more text than fits and
    /// forces several wraps and scrolls.
    #[test]
    fn the_smallest_survivable_screen_takes_wrapping_text_and_keyframes() {
        let mut state = ScreenState::new(Size { cols: 0, rows: 0 });
        assert_eq!(
            state.screen().size(),
            (crate::MIN_CHILD_ROWS, crate::MIN_COLS),
            "a 0x0 request must be floored, not honoured"
        );

        // Wraps every cell and scrolls many times over on a 1x2 grid.
        const WRAPPING: &[u8] = b"wrap this line and keep going well past the end";
        let outcome = state.process_chunk(WRAPPING);
        assert_eq!(outcome.output.data, WRAPPING, "the chunk still fans out");
        let _ = state.emit_keyframe();

        // Newlines, carriage returns and SGR on the same tiny grid.
        let _ = state.process_chunk(b"\r\n\x1b[31mred\x1b[0m\r\nmore\r\n");
        let keyframe = state.emit_keyframe();
        assert!(
            !keyframe.data.is_empty(),
            "a keyframe at the floor is still a real repaint"
        );
    }

    /// `set_size` floors the same way, including the mid-session shrink to a
    /// hub-negotiated 1x1 that `clamp_child` would already have caught.
    #[test]
    fn set_size_floors_a_degenerate_request_and_still_repaints() {
        let mut state = ScreenState::new(SIZE);
        let _ = state.process_chunk(b"hello world");
        let keyframe = state.set_size(Size { cols: 1, rows: 1 });
        assert_eq!(
            state.screen().size(),
            (crate::MIN_CHILD_ROWS, crate::MIN_COLS)
        );
        assert!(!keyframe.data.is_empty());
        // ...and the floored grid still takes wrapping text afterwards.
        let _ = state.process_chunk(b"and more text that wraps repeatedly");
    }

    /// The guard's contract: a panic inside `vt100` rebuilds the model blank
    /// at the tracked size and asks for a repaint, rather than unwinding out
    /// through the session task and ending the share. Driven with a closure
    /// that panics outright, since the geometry clamps remove the known
    /// upstream panic paths.
    #[test]
    fn a_vt100_panic_rebuilds_the_model_instead_of_killing_the_session() {
        let mut state = settled();
        let _ = state.process_chunk(b"visible text");
        assert!(state.screen().contents().contains("visible text"));

        let previous = std::panic::take_hook();
        std::panic::set_hook(Box::new(|_| {})); // keep the test output clean
        state.vt100_guarded(|_| panic!("vt100 went bang"));
        std::panic::set_hook(previous);

        assert_eq!(state.screen().size(), (SIZE.rows, SIZE.cols));
        assert_eq!(state.screen().contents(), "", "rebuilt blank");
        assert!(state.keyframe_due(), "a rebuild must request a repaint");
        // The seq counter lives outside the parser, so sequencing survives.
        assert_eq!(state.emit_keyframe().seq, 3);
    }

    #[test]
    fn replies_are_computed_from_the_post_update_cursor() {
        let mut state = settled();
        // "ab" moves the cursor to column 2 (0-indexed) before the CPR probe,
        // so the 1-indexed reply must say column 3 — not the pre-chunk 1.
        let outcome = state.process_chunk(b"ab\x1b[6n");
        assert_eq!(outcome.replies, b"\x1b[1;3R");
    }
}
