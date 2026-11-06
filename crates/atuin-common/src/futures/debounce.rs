use std::future::Future;
use std::time::Duration;

/// Waits out a burst of events: until [`Self::quiet`] passes without one, or [`Self::max`] since
/// the wait began, however they keep coming.
#[derive(Debug, Clone, Copy)]
pub struct Debounce {
    /// How long without an event ends the burst.
    pub quiet: Duration,
    /// The longest the wait lasts.
    pub max: Duration,
}

impl Debounce {
    /// Waits for the burst `next` reports to end. Each future `next` returns resolves on the
    /// burst's next event; one ready at once, such as a [`tokio::sync::Notify`] permit stored
    /// before the call, counts as one.
    ///
    /// # Panics
    ///
    /// Panics if called outside the context of a Tokio runtime with a time driver enabled.
    pub async fn settle<F, Fut>(self, mut next: F)
    where
        F: FnMut() -> Fut,
        Fut: Future,
    {
        let deadline = tokio::time::Instant::now() + self.max;
        loop {
            // Checked before each wait, as a source whose events are always ready never times out.
            let now = tokio::time::Instant::now();
            if now >= deadline {
                return;
            }
            let quiet_at = (now + self.quiet).min(deadline);
            if tokio::time::timeout_at(quiet_at, next()).await.is_err() {
                return;
            }
        }
    }
}
