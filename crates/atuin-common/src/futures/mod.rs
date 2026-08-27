use std::future::Future;
use std::num::NonZeroU32;
use std::ops::ControlFlow;
use std::time::Duration;

pub mod stream;

/// See [`retry`].
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

/// Poll the given function repeatedly, with a delay specified by `delay` and with a maximum timeout
/// specified by `timeout`.
///
/// The function must return a [`ControlFlow`], which, if it returns [`ControlFlow::Break`], will
/// exit out of the polling and return the value as [`Some`]. If `timeout` elapses first, returns
/// [`None`].
///
/// # Panics
///
/// Panics if called outside the context of a Tokio runtime with a time driver enabled.
pub async fn retry<B, C, Fut, F>(fxn: F, delay: Backoff, timeout: Duration) -> Result<B, Option<C>>
where
    F: Fn() -> Fut,
    Fut: Future<Output = ControlFlow<B, C>>,
{
    let jittered = |delay: Duration| -> Duration {
        let Ok(random) = getrandom::u64() else {
            return delay;
        };
        let nanos = u64::try_from(delay.as_nanos()).unwrap_or(u64::MAX);
        let magnitude = nanos / 10;
        let offset = random % magnitude.saturating_mul(2).saturating_add(1);
        Duration::from_nanos(nanos.saturating_sub(magnitude).saturating_add(offset))
    };

    let mut last = None;

    tokio::time::timeout(timeout, async {
        match delay {
            Backoff::Linear(period) => loop {
                match fxn().await {
                    ControlFlow::Break(value) => return value,
                    ControlFlow::Continue(reason) => last = Some(reason),
                }
                tokio::time::sleep(jittered(period)).await;
            },
            Backoff::Exponential {
                initial,
                max,
                factor,
            } => {
                let mut backoff = initial.min(max);
                loop {
                    match fxn().await {
                        ControlFlow::Break(value) => return value,
                        ControlFlow::Continue(reason) => last = Some(reason),
                    }
                    tokio::time::sleep(jittered(backoff).min(max)).await;
                    backoff = backoff.saturating_mul(factor.get()).min(max);
                }
            }
        }
    })
    .await
    .map_err(|_| last)
}

/// Equivalent to [`retry`], except the given function is not an [`AsyncFn`].
pub async fn retry_blocking<B, C, F>(
    fxn: F,
    delay: Backoff,
    timeout: Duration,
) -> Result<B, Option<C>>
where
    F: Fn() -> ControlFlow<B, C>,
{
    retry(|| std::future::ready(fxn()), delay, timeout).await
}
