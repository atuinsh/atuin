use std::future::Future;
use std::num::NonZeroU32;
use std::ops::ControlFlow;
use std::time::{Duration, Instant};

pub mod stream;

/// Jitter a delay by up to +/-10%.
#[must_use]
fn jittered(delay: Duration) -> Duration {
    let Ok(random) = getrandom::u64() else {
        return delay;
    };
    let nanos = u64::try_from(delay.as_nanos()).unwrap_or(u64::MAX);
    let magnitude = nanos / 10;
    let offset = random % magnitude.saturating_mul(2).saturating_add(1);
    Duration::from_nanos(nanos.saturating_sub(magnitude).saturating_add(offset))
}

/// See [`Backoff::retry`].
#[derive(Debug, Clone, Copy)]
pub enum Backoff {
    /// Repeatedly poll the function with the specified duration delay.
    ///
    /// A value of `100ms` will poll roughly every `100ms`, jittered by up to +/-10%.
    ///
    /// A value of [`Duration::ZERO`] spins: the function is polled as fast as possible with no
    /// delay between polls.
    Linear(Duration),

    /// Poll the future as required with exponential backoff.
    ///
    /// Polls are exponentially distributed. The first delay is `initial`, the next one will be
    /// `initial * factor` time after, all the way until the saturation point of `max`.
    ///
    /// Each delay is jittered by up to +/-10%.
    Exponential {
        /// The initial delay on the poll. Capped to `max`.
        initial: Duration,
        /// The absolute maximum delay the exponential backoff will use.
        max: Duration,
        /// The factor by which the delay will increase at each step.
        factor: NonZeroU32,
    },
}

impl Backoff {
    /// Poll the given function repeatedly, with a delay specified by `delay` and with a maximum
    /// timeout specified by `timeout`.
    ///
    /// Each call returns a [`ControlFlow`]. [`ControlFlow::Break`] stops the polling and returns
    /// its value as [`Ok`]. [`ControlFlow::Continue`] schedules another poll after the backoff
    /// delay, retaining its value as the reason for retrying. If `timeout` elapses first, returns
    /// [`Err`] carrying the most recent [`ControlFlow::Continue`] value, or [`None`] if no poll
    /// produced one before the timeout.
    ///
    /// **Be warned**: This function can possibly wait for longer than `timeout`, since it will
    /// unconditionally await the first call.
    ///
    /// # Panics
    ///
    /// Panics if called outside the context of a Tokio runtime with a time driver enabled.
    pub async fn retry<B, C, Fut, F>(self, mut fxn: F, timeout: Duration) -> Result<B, C>
    where
        F: FnMut() -> Fut,
        Fut: Future<Output = ControlFlow<B, C>>,
    {
        let mut last = match fxn().await {
            ControlFlow::Break(value) => return Ok(value),
            ControlFlow::Continue(reason) => reason,
        };

        tokio::time::timeout(timeout, async {
            match self {
                Self::Linear(period) => loop {
                    tokio::time::sleep(jittered(period)).await;
                    match fxn().await {
                        ControlFlow::Break(value) => return value,
                        ControlFlow::Continue(reason) => last = reason,
                    }
                },
                Self::Exponential {
                    initial,
                    max,
                    factor,
                } => {
                    let mut backoff = initial.min(max);
                    loop {
                        tokio::time::sleep(jittered(backoff).min(max)).await;
                        backoff = backoff.saturating_mul(factor.get()).min(max);
                        match fxn().await {
                            ControlFlow::Break(value) => return value,
                            ControlFlow::Continue(reason) => last = reason,
                        }
                    }
                }
            }
        })
        .await
        .map_err(|_| last)
    }

    /// Equivalent to [`Self::retry`], except the given function is synchronous rather than
    /// returning a future.
    pub async fn retry_sync<B, C, F>(self, mut fxn: F, timeout: Duration) -> Result<B, C>
    where
        F: FnMut() -> ControlFlow<B, C>,
    {
        self.retry(|| std::future::ready(fxn()), timeout).await
    }

    /// A blocking analogue of [`Self::retry`] for synchronous callers: it sleeps the current
    /// thread between attempts.
    pub fn retry_blocking<B, C, F>(self, mut fxn: F, timeout: Duration) -> Result<B, C>
    where
        F: FnMut() -> ControlFlow<B, C>,
    {
        let mut last = match fxn() {
            ControlFlow::Break(value) => return Ok(value),
            ControlFlow::Continue(reason) => reason,
        };

        let deadline = Instant::now().checked_add(timeout);
        let (mut backoff, max) = match self {
            Self::Linear(period) => (period, Duration::MAX),
            Self::Exponential { initial, max, .. } => (initial.min(max), max),
        };

        loop {
            let mut nap = jittered(backoff).min(max);
            if let Some(deadline) = deadline {
                let remaining = deadline.saturating_duration_since(Instant::now());
                if remaining.is_zero() {
                    return Err(last);
                }
                nap = nap.min(remaining);
            }
            std::thread::sleep(nap);

            if let Self::Exponential { factor, .. } = self {
                backoff = backoff.saturating_mul(factor.get()).min(max);
            }

            match fxn() {
                ControlFlow::Break(value) => return Ok(value),
                ControlFlow::Continue(reason) => last = reason,
            }
        }
    }

    /// Poll the given function until it returns [`ControlFlow::Break`], returning that value.
    ///
    /// Unlike [`Self::retry`], there is no timeout and thus no error case: a persistently failing
    /// operation is retried forever. The delay between attempts follows the backoff schedule and,
    /// for [`Self::Exponential`], saturates at `max` and stays there -- so a long outage keeps being
    /// probed at the ceiling cadence until it recovers, never abandoned and never reset back to
    /// `initial`. Timing matches [`Self::retry`]: the first call is eager (no initial delay).
    ///
    /// [`ControlFlow::Continue`] values are discarded (there is no error to carry them into).
    ///
    /// # Panics
    ///
    /// Panics if called outside the context of a Tokio runtime with a time driver enabled.
    pub async fn retry_forever<B, C, Fut, F>(self, mut fxn: F) -> B
    where
        F: FnMut() -> Fut,
        Fut: Future<Output = ControlFlow<B, C>>,
    {
        if let ControlFlow::Break(value) = fxn().await {
            return value;
        }

        match self {
            Self::Linear(period) => loop {
                tokio::time::sleep(jittered(period)).await;
                if let ControlFlow::Break(value) = fxn().await {
                    return value;
                }
            },
            Self::Exponential {
                initial,
                max,
                factor,
            } => {
                let mut backoff = initial.min(max);
                loop {
                    tokio::time::sleep(jittered(backoff).min(max)).await;
                    backoff = backoff.saturating_mul(factor.get()).min(max);
                    if let ControlFlow::Break(value) = fxn().await {
                        return value;
                    }
                }
            }
        }
    }

    /// A resettable sequence of jittered delays following this backoff schedule.
    #[must_use]
    pub fn schedule(self) -> Schedule {
        Schedule::new(self)
    }
}

/// A stateful, jittered delay sequence produced by [`Backoff::schedule`].
#[derive(Debug, Clone, Copy)]
pub struct Schedule {
    backoff: Backoff,
    current: Duration,
}

impl Schedule {
    fn new(backoff: Backoff) -> Self {
        let current = match backoff {
            Backoff::Linear(period) => period,
            Backoff::Exponential { initial, max, .. } => initial.min(max),
        };
        Self { backoff, current }
    }

    /// The next delay to wait, advancing the schedule.
    pub fn next_delay(&mut self) -> Duration {
        let delay = match self.backoff {
            Backoff::Linear(_) => jittered(self.current),
            Backoff::Exponential { max, .. } => jittered(self.current).min(max),
        };
        if let Backoff::Exponential { max, factor, .. } = self.backoff {
            self.current = self.current.saturating_mul(factor.get()).min(max);
        }
        delay
    }

    /// Reset the schedule to its initial delay.
    pub fn reset(&mut self) {
        *self = Self::new(self.backoff);
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use rstest::rstest;
    use tokio::time::Instant;

    use super::*;

    /// A failed attempt must wait a full backoff before the next one: only the eager first call is
    /// un-delayed. Guards against the retry loop firing a second attempt back-to-back with the
    /// first at the start of an episode.
    #[tokio::test(start_paused = true)]
    async fn second_attempt_waits_for_the_backoff() {
        let initial = Duration::from_secs(10);
        let calls = AtomicUsize::new(0);
        let backoff = Backoff::Exponential {
            initial,
            max: Duration::from_secs(600),
            factor: NonZeroU32::new(2).unwrap(),
        };

        let start = Instant::now();
        // Fail once, succeed on the second attempt.
        let _: Result<(), ()> = backoff
            .retry_sync(
                || {
                    if calls.fetch_add(1, Ordering::SeqCst) == 0 {
                        ControlFlow::Continue(())
                    } else {
                        ControlFlow::Break(())
                    }
                },
                Duration::from_secs(3600),
            )
            .await;

        assert_eq!(calls.load(Ordering::SeqCst), 2, "expected exactly two attempts");
        assert!(
            start.elapsed() >= initial / 2,
            "second attempt fired without a backoff delay ({:?} elapsed)",
            start.elapsed()
        );
    }

    #[rstest]
    #[case::first_attempt(1)]
    #[case::after_retries(4)]
    fn retry_blocking_breaks_after(#[case] attempts: u32) {
        let backoff = Backoff::Linear(Duration::from_millis(1));
        let mut calls = 0;
        let result: Result<u32, ()> = backoff.retry_blocking(
            || {
                calls += 1;
                if calls < attempts {
                    ControlFlow::Continue(())
                } else {
                    ControlFlow::Break(calls)
                }
            },
            Duration::from_secs(1),
        );
        assert_eq!(result, Ok(attempts));
    }

    #[rstest]
    fn retry_blocking_gives_up_after_timeout() {
        // Never breaks: returns the last Continue reason once the timeout elapses.
        let backoff = Backoff::Linear(Duration::from_millis(1));
        let result: Result<(), u32> =
            backoff.retry_blocking(|| ControlFlow::Continue(7), Duration::from_millis(20));
        assert_eq!(result, Err(7));
    }

    fn millis(delay: Duration) -> u64 {
        u64::try_from(delay.as_millis()).expect("test delay fits u64")
    }

    #[rstest]
    fn schedule_grows_then_saturates_within_jitter() {
        let backoff = Backoff::Exponential {
            initial: Duration::from_millis(100),
            max: Duration::from_millis(400),
            factor: NonZeroU32::new(2).expect("2 is nonzero"),
        };
        let mut schedule = backoff.schedule();
        for expected in [100u64, 200, 400, 400] {
            let got = millis(schedule.next_delay());
            let (lo, hi) = (expected * 9 / 10, (expected * 11 / 10).min(400));
            assert!(got >= lo && got <= hi, "delay {got}ms not within [{lo},{hi}]ms");
        }
        schedule.reset();
        let got = millis(schedule.next_delay());
        assert!((90..=110).contains(&got), "reset delay {got}ms not near initial");
    }

    #[rstest]
    fn linear_schedule_is_constant_within_jitter() {
        let mut schedule = Backoff::Linear(Duration::from_millis(100)).schedule();
        for _ in 0..4 {
            let got = millis(schedule.next_delay());
            assert!((90..=110).contains(&got), "delay {got}ms not within jitter of 100ms");
        }
    }
}
