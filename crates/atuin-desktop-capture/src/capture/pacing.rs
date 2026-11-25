//! How long capture waits before and between reads, and how often it tries a read again.

use std::time::Duration;

use tokio::time::Instant;

use super::config::Timing;
use super::record::Trigger;
use crate::backend::Completeness;

/// How many times longer than its last read a window rests before content changes can make
/// capture read it again.
///
/// Some applications send change events when nothing visible changes. The factor bounds the
/// share of time that reading takes to a twentieth, regardless of what a read of the window
/// costs, and leaves a window that is cheap to read responsive.
const READ_COST_FACTOR: u32 = 20;

/// How many times longer than its last read a background window rests before its changes can
/// make capture read it again.
///
/// The user is not looking at a background window, so its reads take a hundredth of the time
/// at most, a fifth of what the active window may take.
const BACKGROUND_COST_FACTOR: u32 = 100;

/// How long a window must stay quiet after a change of the kind of `trigger`.
pub const fn settle(timing: &Timing, trigger: Trigger) -> Duration {
    match trigger {
        Trigger::Edited => timing.content_settle,
        _ => timing.activation_settle,
    }
}

/// A burst of changes in one window, which settles before capture reads the window.
#[derive(Debug, Clone, Copy)]
pub struct Burst {
    first: Instant,
    last: Instant,
    /// The trigger of the burst's first change.
    trigger: Trigger,
}

impl Burst {
    /// A burst whose first change, of the kind of `trigger`, came at `now`.
    pub const fn new(now: Instant, trigger: Trigger) -> Self {
        Self {
            first: now,
            last: now,
            trigger,
        }
    }

    /// Adds a change at `now` to the burst.
    pub const fn with_change(self, now: Instant) -> Self {
        Self { last: now, ..self }
    }

    pub const fn trigger(&self) -> Trigger {
        self.trigger
    }

    /// When the burst settles: once the window stayed quiet after the last change, and at the
    /// longest wait after the first change at the latest.
    pub fn settled(&self, timing: &Timing) -> Instant {
        (self.last + settle(timing, self.trigger)).min(self.first + timing.max_wait)
    }
}

/// The rest that a read buys, in proportion to the time that the read took.
#[derive(Debug, Clone, Copy)]
pub struct Pacing {
    factor: u32,
    min: Duration,
    max: Duration,
}

impl Pacing {
    /// The rest of the active window before its next content snapshot.
    pub const fn active(timing: &Timing) -> Self {
        Self {
            factor: READ_COST_FACTOR,
            min: timing.min_interval,
            max: timing.max_interval,
        }
    }

    /// The rest between two reads of the sweep.
    pub const fn sweep(timing: &Timing) -> Self {
        Self {
            factor: READ_COST_FACTOR,
            min: timing.sweep_rest,
            max: timing.max_interval,
        }
    }

    /// The rest of a background window before its changes can make capture read it again.
    pub const fn background(timing: &Timing) -> Self {
        Self {
            factor: BACKGROUND_COST_FACTOR,
            min: timing.background_interval,
            max: timing.background_max_interval,
        }
    }

    pub fn rest_after(self, read_time: Duration) -> Duration {
        (read_time * self.factor).clamp(self.min, self.max)
    }
}

/// How many times in a row a window is read again after its read ran out of time or failed.
///
/// An application that is busy answers slowly for a while. For example, an application can be
/// busy while another application starts. An application that is always slow gives a partial
/// read each time.
pub const MAX_SLOW_RETRIES: u8 = 3;

/// How many times in a row an empty read is tried again without waiting for a change.
pub const MAX_EMPTY_RETRIES: u8 = 3;

/// Whether a read is tried again.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Retry {
    Again,
    /// The read is not tried again, because the read doesn't need a retry or its retries in a
    /// row are used up.
    Stop,
}

/// Counts the retries of a read, up to `MAX` in a row.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Retries<const MAX: u8>(u8);

impl<const MAX: u8> Retries<MAX> {
    /// Counts one more retry, unless `MAX` retries in a row are used up.
    pub const fn try_again(&mut self) -> Retry {
        if self.0 < MAX {
            self.0 += 1;
            Retry::Again
        } else {
            Retry::Stop
        }
    }

    pub const fn reset(&mut self) {
        self.0 = 0;
    }

    /// Counts one more retry, and starts the count over once `MAX` retries in a row are used
    /// up.
    pub const fn try_again_or_start_over(&mut self) -> Retry {
        let retry = self.try_again();
        if let Retry::Stop = retry {
            self.reset();
        }
        retry
    }
}

impl SlowRetries {
    /// Whether a read that ended with `completeness` is tried again.
    ///
    /// A read that ran out of time is tried again until the retries in a row are used up. Any
    /// other read starts the count over.
    pub const fn after(&mut self, completeness: Completeness) -> Retry {
        if let Completeness::OutOfTime = completeness {
            self.try_again_or_start_over()
        } else {
            self.reset();
            Retry::Stop
        }
    }
}

/// The retries of a read that ran out of time or failed.
pub type SlowRetries = Retries<MAX_SLOW_RETRIES>;

/// The retries of a read that found nothing to read.
pub type EmptyRetries = Retries<MAX_EMPTY_RETRIES>;
