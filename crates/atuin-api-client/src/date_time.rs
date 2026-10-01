use serde::{Deserialize, Serialize};
use time::OffsetDateTime;

/// A `date-time` from the API, on the wire as RFC 3339, e.g. `2026-08-01T00:00:00Z`.
///
/// A newtype because the workspace's `time` serializes [`OffsetDateTime`] in its own
/// human-readable format, which rejects RFC 3339.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct DateTime(#[serde(with = "time::serde::rfc3339")] pub OffsetDateTime);

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;
    use rstest::rstest;
    use time::macros::datetime;

    use super::DateTime;

    #[rstest]
    #[case::utc("\"2026-08-01T00:00:00Z\"", DateTime(datetime!(2026-08-01 00:00:00 UTC)))]
    #[case::fraction(
        "\"2026-08-01T12:30:05.25Z\"",
        DateTime(datetime!(2026-08-01 12:30:05.25 UTC))
    )]
    #[case::offset(
        "\"2026-08-01T02:00:00+02:00\"",
        DateTime(datetime!(2026-08-01 02:00:00 +02:00))
    )]
    fn round_trips_rfc_3339(#[case] wire: &str, #[case] expected: DateTime) {
        let decoded: DateTime = serde_json::from_str(wire).unwrap();
        assert_eq!(decoded, expected);
        assert_eq!(serde_json::to_string(&decoded).unwrap(), wire);
    }
}
