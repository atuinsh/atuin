//! Wall-clock instants.

use std::time::SystemTime;

use easy_cast::Conv;

/// Extensions to [`SystemTime`].
pub trait SystemTimeExt {
    /// Milliseconds since the Unix epoch, clamped to `0` before it and to `i64::MAX` past it.
    fn saturating_unix_millis(self) -> i64;
}

impl SystemTimeExt for SystemTime {
    fn saturating_unix_millis(self) -> i64 {
        self.duration_since(Self::UNIX_EPOCH)
            .map_or(0, |since| i64::try_conv(since.as_millis()).unwrap_or(i64::MAX))
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

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
}
