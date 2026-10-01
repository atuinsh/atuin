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
    fn round_trips_rfc_3339() {
        let wire = "\"2026-08-01T00:00:00Z\"";
        let decoded: DateTime = serde_json::from_str(wire).unwrap();
        assert_eq!(decoded, DateTime(datetime!(2026-08-01 00:00:00 UTC)));
        assert_eq!(serde_json::to_string(&decoded).unwrap(), wire);
    }
}
