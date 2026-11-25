//! Wall-clock instants.

use std::time::{Duration, SystemTime};

use easy_cast::Conv;

/// Extensions to [`SystemTime`].
pub trait SystemTimeExt {
    /// Milliseconds since the Unix epoch, clamped to `0` before it and to `i64::MAX` past it.
    fn saturating_unix_millis(self) -> i64;

    /// The time `millis` milliseconds after the Unix epoch. `None` before the epoch, and past
    /// what the platform's clock can represent.
    fn checked_from_unix_millis(millis: i64) -> Option<Self>
    where
        Self: Sized;
}

impl SystemTimeExt for SystemTime {
    fn saturating_unix_millis(self) -> i64 {
        self.duration_since(Self::UNIX_EPOCH)
            .map_or(0, |since| i64::try_conv(since.as_millis()).unwrap_or(i64::MAX))
    }

    fn checked_from_unix_millis(millis: i64) -> Option<Self> {
        Self::UNIX_EPOCH.checked_add(Duration::from_millis(u64::try_from(millis).ok()?))
    }
}

#[cfg(test)]
mod tests {
    use rstest::rstest;

    use super::*;

    #[rstest]
    #[case::epoch(SystemTime::UNIX_EPOCH, 0)]
    #[case::after(SystemTime::UNIX_EPOCH + Duration::from_millis(1_500), 1_500)]
    #[case::sub_millisecond_truncates(SystemTime::UNIX_EPOCH + Duration::from_micros(1_999), 1)]
    #[case::before(SystemTime::UNIX_EPOCH - Duration::from_secs(1), 0)]
    fn saturating_unix_millis(#[case] time: SystemTime, #[case] expected: i64) {
        assert_eq!(time.saturating_unix_millis(), expected);
    }

    #[rstest]
    #[case::epoch(0, Some(SystemTime::UNIX_EPOCH))]
    #[case::after(1_500, Some(SystemTime::UNIX_EPOCH + Duration::from_millis(1_500)))]
    #[case::before(-1, None)]
    fn checked_from_unix_millis(#[case] millis: i64, #[case] expected: Option<SystemTime>) {
        assert_eq!(SystemTime::checked_from_unix_millis(millis), expected);
    }
}
