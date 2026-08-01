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
    pub(super) fn new(size: Size) -> Self {
        Self {
            parser: vt100::Parser::new(size.rows, size.cols, 0),
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

        self.parser.process(chunk);
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
    #[must_use]
    pub(super) fn set_size(&mut self, size: Size) -> Frame {
        self.parser.screen_mut().set_size(size.rows, size.cols);
        self.emit_keyframe()
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

    #[test]
    fn replies_are_computed_from_the_post_update_cursor() {
        let mut state = settled();
        // "ab" moves the cursor to column 2 (0-indexed) before the CPR probe,
        // so the 1-indexed reply must say column 3 — not the pre-chunk 1.
        let outcome = state.process_chunk(b"ab\x1b[6n");
        assert_eq!(outcome.replies, b"\x1b[1;3R");
    }
}
