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
    /// thread between attempts instead of yielding to an async runtime, so it needs no runtime.
    /// The eager-first-call, backoff, and `timeout` semantics match [`Self::retry`].
    pub fn retry_blocking<B, C, F>(self, mut fxn: F, timeout: Duration) -> Result<B, C>
    where
        F: FnMut() -> ControlFlow<B, C>,
    {
        let mut last = match fxn() {
            ControlFlow::Break(value) => return Ok(value),
            ControlFlow::Continue(reason) => reason,
        };

        // `None` (a `checked_add` overflow) means no deadline. Linear has no cap, so `max` is
        // `Duration::MAX` there, making the `.min(max)` below a no-op.
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
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

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

    #[test]
    fn retry_blocking_breaks_and_times_out() {
        let backoff = Backoff::Linear(Duration::from_millis(1));

        let mut calls = 0;
        let ok: Result<u32, ()> = backoff.retry_blocking(
            || {
                calls += 1;
                if calls < 3 { ControlFlow::Continue(()) } else { ControlFlow::Break(calls) }
            },
            Duration::from_secs(1),
        );
        assert_eq!(ok, Ok(3));

        // Never breaks: gives up with the last Continue reason once the timeout elapses.
        let err: Result<(), u32> =
            backoff.retry_blocking(|| ControlFlow::Continue(7), Duration::from_millis(20));
        assert_eq!(err, Err(7));
    }
}
