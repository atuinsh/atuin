//! Outbound backpressure and reconnect backoff.
//!
//! Both types are pure state machines with no I/O, so the transport's trickier
//! policy decisions stay unit-testable without a hub.

use std::time::Duration;

/// One outbound terminal frame: a monotonic sequence number and the bytes it
/// stamps.
///
/// `seq` and `data` are minted together under the parser lock (the seq
/// invariant — a keyframe stamped `seq = K` reflects exactly the output bytes
/// stamped `<= K`), so the pair travels as one value from the session to the
/// wire instead of as a transposable tuple.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Frame {
    pub(crate) seq: u64,
    pub(crate) data: Vec<u8>,
}

/// A bounded, latest-wins queue of outbound terminal frames.
///
/// When the socket stalls, buffering the whole backlog is pointless: a terminal
/// viewer only cares about the *current* screen. Once more than `cap` output
/// frames pile up the backlog is dropped wholesale and a keyframe is requested
/// instead, so the hub (and every viewer) resyncs in one frame rather than
/// replaying megabytes of stale scrollback.
pub(crate) struct OutboundQueue {
    cap: usize,
    output: Vec<Frame>,
    needs_keyframe: bool,
    awaiting_keyframe: bool,
}

impl OutboundQueue {
    /// Create an empty queue holding at most `cap` output frames.
    #[must_use]
    pub(crate) const fn new(cap: usize) -> Self {
        Self {
            cap,
            output: Vec::new(),
            needs_keyframe: false,
            awaiting_keyframe: false,
        }
    }

    /// Enqueue one `output` frame, collapsing the backlog to a keyframe request
    /// if that pushes the queue past its capacity.
    ///
    /// While a resync keyframe is outstanding the frame is **discarded** rather
    /// than queued. The dropped backlog never reached the wire, so the hub's
    /// replay buffer already has a hole at that point; letting later frames
    /// through would leave that hole permanently stitched into the buffer and a
    /// viewer joining before the keyframe would replay corrupt bytes. The
    /// keyframe supersedes everything discarded here — the parser had already
    /// processed those bytes when it was rendered.
    pub(crate) fn push_output(&mut self, frame: Frame) {
        if self.awaiting_keyframe {
            return;
        }
        self.output.push(frame);
        if self.output.len() > self.cap {
            // Drop the backlog; the next thing on the wire must be a keyframe
            // so the hub (and every viewer) resyncs cleanly.
            self.output.clear();
            self.needs_keyframe = true;
            self.awaiting_keyframe = true;
        }
    }

    /// Whether a keyframe must be *requested* from the session.
    #[must_use]
    pub(crate) const fn needs_keyframe(&self) -> bool {
        self.needs_keyframe
    }

    /// Whether a requested resync keyframe is still outstanding, during which
    /// no `output` may be sent.
    #[must_use]
    pub(crate) const fn awaiting_keyframe(&self) -> bool {
        self.awaiting_keyframe
    }

    /// Record that a keyframe has actually been written to the wire, ending the
    /// resync window and letting output flow again.
    pub(crate) const fn on_keyframe_sent(&mut self) {
        self.needs_keyframe = false;
        self.awaiting_keyframe = false;
    }

    /// Take everything currently queued, leaving the queue empty.
    pub(crate) fn drain_output(&mut self) -> Vec<Frame> {
        std::mem::take(&mut self.output)
    }

    /// Acknowledge that the requested keyframe has been sent.
    pub(crate) const fn clear_keyframe_flag(&mut self) {
        self.needs_keyframe = false;
    }
}

/// Exponential reconnect backoff, capped so a long outage still retries often
/// enough to pick the session back up promptly.
pub(crate) struct Backoff {
    attempt: u32,
    base: Duration,
    cap: Duration,
}

impl Backoff {
    /// A fresh backoff: 500 ms doubling up to a 10 s ceiling.
    #[must_use]
    pub(crate) const fn new() -> Self {
        Self {
            attempt: 0,
            base: Duration::from_millis(500),
            cap: Duration::from_secs(10),
        }
    }

    /// The delay to wait before the next reconnect attempt.
    pub(crate) fn next_delay(&mut self) -> Duration {
        let d = self.base * 2u32.saturating_pow(self.attempt);
        self.attempt = self.attempt.saturating_add(1);
        d.min(self.cap)
    }

    /// Reset after a successful connection, so the next outage starts at `base`.
    pub(crate) const fn reset(&mut self) {
        self.attempt = 0;
    }
}

impl Default for Backoff {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(seq: u64) -> Frame {
        Frame {
            seq,
            data: vec![seq as u8],
        }
    }

    fn named(seq: u64, data: &[u8]) -> Frame {
        Frame {
            seq,
            data: data.to_vec(),
        }
    }

    #[test]
    fn queue_starts_empty_with_no_flags() {
        let mut q = OutboundQueue::new(3);
        assert!(!q.needs_keyframe());
        assert!(!q.awaiting_keyframe());
        assert!(q.drain_output().is_empty());
    }

    #[test]
    fn drain_returns_frames_in_order_and_empties_the_queue() {
        let mut q = OutboundQueue::new(3);
        q.push_output(named(1, b"a"));
        q.push_output(named(2, b"b"));
        assert_eq!(q.drain_output(), vec![named(1, b"a"), named(2, b"b")]);
        assert!(q.drain_output().is_empty());
        assert!(!q.needs_keyframe());
    }

    #[test]
    fn overflow_collapses_the_backlog_into_a_keyframe_request() {
        let mut q = OutboundQueue::new(3);
        for seq in 1..=4 {
            q.push_output(frame(seq));
        }
        // The 4th push crossed the cap: backlog dropped wholesale, resync begins.
        assert!(q.drain_output().is_empty());
        assert!(q.needs_keyframe());
        assert!(q.awaiting_keyframe());
    }

    #[test]
    fn output_pushed_while_awaiting_a_keyframe_is_discarded() {
        let mut q = OutboundQueue::new(1);
        q.push_output(named(1, b"a"));
        q.push_output(named(2, b"b")); // overflow → resync window opens
        q.push_output(named(3, b"c")); // inside the window: discarded
        assert!(q.drain_output().is_empty());
        assert!(q.awaiting_keyframe());
    }

    #[test]
    fn clear_keyframe_flag_keeps_the_resync_window_open() {
        let mut q = OutboundQueue::new(1);
        q.push_output(named(1, b"a"));
        q.push_output(named(2, b"b")); // overflow
        q.clear_keyframe_flag();
        // No longer *asking* for a keyframe, but output must still be held
        // back until one is actually written to the wire.
        assert!(!q.needs_keyframe());
        assert!(q.awaiting_keyframe());
        q.push_output(named(3, b"c"));
        assert!(q.drain_output().is_empty());
    }

    #[test]
    fn on_keyframe_sent_ends_the_resync_window() {
        let mut q = OutboundQueue::new(1);
        q.push_output(named(1, b"a"));
        q.push_output(named(2, b"b")); // overflow
        q.on_keyframe_sent();
        assert!(!q.needs_keyframe());
        assert!(!q.awaiting_keyframe());
        // Output flows again.
        q.push_output(named(3, b"c"));
        assert_eq!(q.drain_output(), vec![named(3, b"c")]);
    }

    #[test]
    fn backoff_doubles_from_500ms_and_caps_at_10s() {
        let mut b = Backoff::new();
        let series: Vec<u64> = (0..7).map(|_| b.next_delay().as_millis() as u64).collect();
        assert_eq!(series, vec![500, 1000, 2000, 4000, 8000, 10_000, 10_000]);
    }

    #[test]
    fn backoff_reset_restarts_the_series() {
        let mut b = Backoff::new();
        for _ in 0..5 {
            let _ = b.next_delay();
        }
        b.reset();
        assert_eq!(b.next_delay(), Duration::from_millis(500));
        assert_eq!(b.next_delay(), Duration::from_secs(1));
    }
}
