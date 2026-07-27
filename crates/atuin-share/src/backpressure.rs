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
}

impl OutboundQueue {
    /// Create an empty queue holding at most `cap` output frames.
    #[must_use]
    pub const fn new(cap: usize) -> Self {
        Self {
            cap,
            output: Vec::new(),
            needs_keyframe: false,
        }
    }

    /// Enqueue one `output` frame, collapsing the backlog to a keyframe request
    /// if that pushes the queue past its capacity.
    pub fn push_output(&mut self, seq: u64, data: Vec<u8>) {
        self.output.push((seq, data));
        if self.output.len() > self.cap {
            // Drop the backlog; the next thing on the wire must be a keyframe
            // so the hub (and every viewer) resyncs cleanly.
            self.output.clear();
            self.needs_keyframe = true;
        }
    }

    /// Whether a keyframe must be sent before any further output.
    #[must_use]
    pub const fn needs_keyframe(&self) -> bool {
        self.needs_keyframe
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
