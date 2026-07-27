//! Outbound backpressure and reconnect backoff.
//!
//! Both types are pure state machines with no I/O, so the transport's trickier
//! policy decisions stay unit-testable without a hub.

use std::time::Duration;

/// A bounded, latest-wins queue of outbound terminal frames.
///
/// When the socket stalls, buffering the whole backlog is pointless: a terminal
/// viewer only cares about the *current* screen. Once more than `cap` output
/// frames pile up the backlog is dropped wholesale and a keyframe is requested
/// instead, so the hub (and every viewer) resyncs in one frame rather than
/// replaying megabytes of stale scrollback.
pub struct OutboundQueue {
    cap: usize,
    output: Vec<(u64, Vec<u8>)>,
    needs_keyframe: bool,
    awaiting_keyframe: bool,
}

impl OutboundQueue {
    /// Create an empty queue holding at most `cap` output frames.
    #[must_use]
    pub const fn new(cap: usize) -> Self {
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
    pub fn push_output(&mut self, seq: u64, data: Vec<u8>) {
        if self.awaiting_keyframe {
            return;
        }
        self.output.push((seq, data));
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
    pub const fn needs_keyframe(&self) -> bool {
        self.needs_keyframe
    }

    /// Whether a requested resync keyframe is still outstanding, during which
    /// no `output` may be sent.
    #[must_use]
    pub const fn awaiting_keyframe(&self) -> bool {
        self.awaiting_keyframe
    }

    /// Record that a keyframe has actually been written to the wire, ending the
    /// resync window and letting output flow again.
    pub const fn on_keyframe_sent(&mut self) {
        self.needs_keyframe = false;
        self.awaiting_keyframe = false;
    }

    /// Take everything currently queued, leaving the queue empty.
    pub fn drain_output(&mut self) -> Vec<(u64, Vec<u8>)> {
        std::mem::take(&mut self.output)
    }

    /// Acknowledge that the requested keyframe has been sent.
    pub const fn clear_keyframe_flag(&mut self) {
        self.needs_keyframe = false;
    }
}

/// Exponential reconnect backoff, capped so a long outage still retries often
/// enough to pick the session back up promptly.
pub struct Backoff {
    attempt: u32,
    base: Duration,
    cap: Duration,
}

impl Backoff {
    /// A fresh backoff: 500 ms doubling up to a 10 s ceiling.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            attempt: 0,
            base: Duration::from_millis(500),
            cap: Duration::from_secs(10),
        }
    }

    /// The delay to wait before the next reconnect attempt.
    pub fn next_delay(&mut self) -> Duration {
        let d = self.base * 2u32.saturating_pow(self.attempt);
        self.attempt = self.attempt.saturating_add(1);
        d.min(self.cap)
    }

    /// Reset after a successful connection, so the next outage starts at `base`.
    pub const fn reset(&mut self) {
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

    #[test]
    fn queue_collapses_output_backlog_to_a_keyframe_on_overflow() {
        let mut q = OutboundQueue::new(3); // capacity 3 output frames
        for i in 1..=5 {
            q.push_output(i, vec![b'x']);
        }
        // On overflow the queue keeps only a resync marker: the caller must
        // send a fresh keyframe instead of the dropped backlog.
        assert!(
            q.needs_keyframe(),
            "overflow must request a keyframe resync"
        );
        assert!(q.drain_output().len() <= 3);
    }

    #[test]
    fn queue_sends_no_output_between_the_drop_and_the_resync_keyframe() {
        // The dropped frames were never written to the wire, so the hub's
        // "keyframe + bytes since" buffer would have a hole in it. Sending any
        // further output before the resync keyframe cements that hole: a viewer
        // joining in the window replays the stale keyframe plus bytes that skip
        // the dropped range. The hub carefully avoids this on its own overflow
        // path; the CLI must not create it from this side.
        let mut q = OutboundQueue::new(3);
        for i in 1..=5 {
            q.push_output(i, vec![b'x']);
        }
        assert!(
            q.needs_keyframe(),
            "overflow must request a keyframe resync"
        );

        // What the transport does on overflow: drop the backlog and ask the
        // session for a keyframe.
        q.drain_output();
        q.clear_keyframe_flag();
        assert!(
            q.awaiting_keyframe(),
            "the queue must stay in resync until the keyframe is actually sent"
        );

        // Output produced while the keyframe is in flight must not reach the
        // wire — the keyframe already reflects those bytes.
        for i in 6..=9 {
            q.push_output(i, vec![b'y']);
        }
        assert!(
            q.drain_output().is_empty(),
            "no output may be sent between the drop and the resync keyframe"
        );

        // Once the keyframe is on the wire the stream resumes normally.
        q.on_keyframe_sent();
        assert!(!q.awaiting_keyframe());
        q.push_output(20, vec![b'z']);
        assert_eq!(q.drain_output(), vec![(20, vec![b'z'])]);
    }

    #[test]
    fn backoff_grows_then_resets() {
        let mut b = Backoff::new();
        let d0 = b.next_delay();
        let d1 = b.next_delay();
        assert!(d1 > d0);
        assert!(b.next_delay() <= std::time::Duration::from_secs(10)); // capped
        b.reset();
        assert_eq!(b.next_delay(), d0);
    }
}
