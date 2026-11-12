use std::future::Future;
use std::num::NonZeroU32;
use std::ops::ControlFlow;
use std::time::{Duration, Instant};

mod debounce;
pub mod stream;

pub use debounce::Debounce;

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
    Constant(Duration),

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

        let mut delays = self.delays();
        tokio::time::timeout(timeout, async {
            loop {
                tokio::time::sleep(delays.next_delay()).await;
                match fxn().await {
                    ControlFlow::Break(value) => return value,
                    ControlFlow::Continue(reason) => last = reason,
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
        let mut delays = self.delays();

        loop {
            let mut nap = delays.next_delay();
            if let Some(deadline) = deadline {
                let remaining = deadline.saturating_duration_since(Instant::now());
                if remaining.is_zero() {
                    return Err(last);
                }
                nap = nap.min(remaining);
            }
            std::thread::sleep(nap);

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
    pub async fn retry_forever<B, C, Fut, F>(self, fxn: F) -> B
    where
        F: FnMut() -> Fut,
        Fut: Future<Output = ControlFlow<B, C>>,
    {
        self.retry_forever_or_sooner(fxn, std::future::pending::<()>).await
    }

    /// Equivalent to [`Self::retry_forever`], except that each future `interrupt` returns cuts
    /// the delay it races short: the next call comes at once, and the delays start over.
    ///
    /// `interrupt` races only the delays, so it never cancels a call in flight. One ready at once,
    /// such as a [`tokio::sync::Notify`] permit stored during a call, cuts the next delay to
    /// nothing.
    ///
    /// # Panics
    ///
    /// Panics if called outside the context of a Tokio runtime with a time driver enabled.
    pub async fn retry_forever_or_sooner<B, C, Fut, F, I, Interrupted>(
        self,
        mut fxn: F,
        mut interrupt: I,
    ) -> B
    where
        F: FnMut() -> Fut,
        Fut: Future<Output = ControlFlow<B, C>>,
        I: FnMut() -> Interrupted,
        Interrupted: Future,
    {
        let mut delays = self.delays();
        loop {
            if let ControlFlow::Break(value) = fxn().await {
                return value;
            }

            tokio::select! {
                () = tokio::time::sleep(delays.next_delay()) => {}
                _ = interrupt() => delays = self.delays(),
            }
        }
    }

    /// The delays between calls, in turn, as [`Self::retry_forever`] waits them out, for a caller
    /// that keeps a backoff across calls of its own.
    #[must_use]
    pub fn delays(self) -> Delays {
        let next = match self {
            Self::Constant(period) => period,
            Self::Exponential { initial, max, .. } => initial.min(max),
        };
        Delays {
            backoff: self,
            next,
        }
    }
}

/// See [`Backoff::delays`].
#[derive(Debug, Clone)]
pub struct Delays {
    backoff: Backoff,
    /// The next delay, before jitter.
    next: Duration,
}

impl Delays {
    /// The next delay, jittered.
    pub fn next_delay(&mut self) -> Duration {
        let delay = jittered(self.next);
        let Backoff::Exponential { max, factor, .. } = self.backoff else {
            return delay;
        };
        self.next = self.next.saturating_mul(factor.get()).min(max);
        delay.min(max)
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
        let backoff = Backoff::Constant(Duration::from_millis(1));
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
        let backoff = Backoff::Constant(Duration::from_millis(1));
        let result: Result<(), u32> =
            backoff.retry_blocking(|| ControlFlow::Continue(7), Duration::from_millis(20));
        assert_eq!(result, Err(7));
    }

    #[rstest]
    #[case::constant(Backoff::Constant(Duration::from_secs(10)), &[10, 10, 10])]
    #[case::exponential_up_to_max(
        Backoff::Exponential {
            initial: Duration::from_secs(10),
            max: Duration::from_secs(60),
            factor: NonZeroU32::new(2).unwrap(),
        },
        &[10, 20, 40, 60, 60],
    )]
    #[case::initial_past_max(
        Backoff::Exponential {
            initial: Duration::from_secs(90),
            max: Duration::from_secs(60),
            factor: NonZeroU32::new(2).unwrap(),
        },
        &[60, 60],
    )]
    fn delays_follow_the_schedule(#[case] backoff: Backoff, #[case] secs: &[u64]) {
        let mut delays = backoff.delays();
        for &secs in secs {
            let expected = Duration::from_secs(secs);
            let delay = delays.next_delay();
            assert!(
                (expected * 9 / 10..=expected * 11 / 10).contains(&delay),
                "{delay:?} is not within 10% of {expected:?}"
            );
            if let Backoff::Exponential { max, .. } = backoff {
                assert!(delay <= max, "{delay:?} is past the max, {max:?}");
            }
        }
    }
}
