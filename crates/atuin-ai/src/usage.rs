//! Server-side credit usage: fetching, and the readings the status bar shows.
//!
//! The hub reports the user's period credit totals two ways: a `credits`
//! object on the chat `done` event, and `GET /api/cli/usage` for reading it
//! outside a chat. Both decode into the generated [`UsageSnapshot`].
//! Snapshots are cached in ai.db (see `store`) so the TUI can render usage
//! immediately on open, then refreshed in the background. The cache holds the
//! snapshot's JSON unversioned, so a spec change to `UsageSnapshot` changes
//! what it reads; a row that no longer decodes is a cache miss.

use std::time::Duration;

use atuin_api_client::{ApiBody, Client};
use atuin_api_client::types::UsageSnapshot;
use eyre::Result;
use reqwest::Url;
use secrecy::{ExposeSecret, SecretString};
use time::OffsetDateTime;

/// Cached usage older than this triggers a background refresh on TUI open.
pub const REFRESH_AFTER: Duration = Duration::from_secs(60);

/// The status bar's readings of a [`UsageSnapshot`].
pub trait UsageSnapshotExt {
    /// Time left until the period resets, or `None` once it has.
    fn resets_in(&self) -> Option<Duration>;

    /// The fuller of the input and output buckets, in percent, or `None` when neither is limited.
    ///
    /// A bucket's `limit` uses the server's sentinels: -1 unlimited, 0 disabled.
    fn as_percentage(&self) -> Option<f64>;
}

impl UsageSnapshotExt for UsageSnapshot {
    fn resets_in(&self) -> Option<Duration> {
        Duration::try_from(self.resets_at.0 - OffsetDateTime::now_utc()).ok()
    }

    fn as_percentage(&self) -> Option<f64> {
        let input_percentage = if self.input.limit > 0 {
            Some(self.input.used as f64 / self.input.limit as f64 * 100.0)
        } else {
            None
        };

        let output_percentage = if self.output.limit > 0 {
            Some(self.output.used as f64 / self.output.limit as f64 * 100.0)
        } else {
            None
        };

        match (input_percentage, output_percentage) {
            (Some(input), Some(output)) if input > output => Some(input),
            (Some(_), Some(output)) => Some(output),
            (Some(input), None) => Some(input),
            (None, Some(output)) => Some(output),
            (None, None) => None,
        }
    }
}

/// Format a reset delta as its largest sensible unit: "4d", "23h", or "56m".
/// Sub-minute deltas render as "1m" — "0m" would read as already reset.
pub fn format_reset_delta(delta: Duration) -> String {
    let minutes = delta.as_secs() / 60;
    if minutes >= 24 * 60 {
        format!("{}d", minutes / (24 * 60))
    } else if minutes >= 60 {
        format!("{}h", minutes / 60)
    } else {
        format!("{}m", minutes.max(1))
    }
}

/// Key for the local usage cache. The client never learns its hub user id,
/// so rows are keyed by a hash of the auth token: a different login (or a
/// rotated token) simply misses the cache and refetches.
pub fn cache_key(token: &SecretString) -> String {
    format!("{:016x}", xxhash_rust::xxh3::xxh3_64(token.expose_secret().as_bytes()))
}

/// Fetch current usage from the hub. Mirrors the `credits` object on the
/// chat `done` event, for refreshing without starting a chat.
pub async fn fetch_usage(endpoint: &Url, token: &SecretString) -> Result<UsageSnapshot> {
    Ok(Client::for_ai(endpoint, Some(token))?.get_usage().body().await?)
}

#[cfg(test)]
mod tests {
    use atuin_api_client::DateTime;
    use atuin_api_client::types::UsageBucket;
    use rstest::rstest;
    use time::macros::datetime;

    use super::*;

    #[rstest]
    fn deserializes_server_payload() {
        // Shape documented in the hub's CliUsageController / credits_payload.
        let json = r#"{
            "period": "calendar_monthly",
            "resets_at": "2026-08-01T00:00:00Z",
            "requests": {"used": 3, "limit": -1},
            "input": {"used": 12345, "limit": 5000000},
            "output": {"used": 678, "limit": 1000000}
        }"#;

        let snapshot: UsageSnapshot = serde_json::from_str(json).unwrap();
        assert_eq!(snapshot.period, "calendar_monthly");
        assert_eq!(snapshot.requests.limit, -1);
        assert_eq!(snapshot.input.used, 12345);
        assert_eq!(snapshot.output.limit, 1_000_000);
    }

    #[rstest]
    fn snapshot_roundtrips_through_json() {
        let snapshot = UsageSnapshot {
            period: "calendar_monthly".into(),
            resets_at: DateTime(datetime!(2026-08-01 00:00 UTC)),
            requests: UsageBucket { used: 1, limit: 10 },
            input: UsageBucket { used: 2, limit: 20 },
            output: UsageBucket { used: 3, limit: 0 },
        };

        let json = serde_json::to_string(&snapshot).unwrap();
        assert_eq!(serde_json::from_str::<UsageSnapshot>(&json).unwrap(), snapshot);
    }

    #[rstest]
    fn as_percentage_uses_higher_limited_bucket() {
        let mut snapshot = UsageSnapshot {
            period: "calendar_monthly".into(),
            resets_at: DateTime(datetime!(2026-08-01 00:00 UTC)),
            requests: UsageBucket { used: 3, limit: -1 },
            input: UsageBucket {
                used: 50,
                limit: 100,
            },
            output: UsageBucket {
                used: 90,
                limit: 100,
            },
        };
        assert_eq!(snapshot.as_percentage(), Some(90.0));

        // Unlimited/disabled buckets drop out of the average
        snapshot.output.limit = -1;
        assert_eq!(snapshot.as_percentage(), Some(50.0));

        snapshot.input.limit = 0;
        assert_eq!(snapshot.as_percentage(), None);
    }

    #[rstest]
    #[case::days(Duration::from_secs((4 * 24 * 60 + 300) * 60), "4d")]
    #[case::hours(Duration::from_secs((23 * 60 + 59) * 60), "23h")]
    #[case::minutes(Duration::from_secs(56 * 60), "56m")]
    #[case::sub_minute(Duration::from_secs(30), "1m")]
    fn format_reset_delta_cases(#[case] delta: Duration, #[case] expected: &str) {
        assert_eq!(format_reset_delta(delta), expected);
    }

    #[rstest]
    fn cache_key_distinguishes_tokens() {
        let a = SecretString::from("token-a");
        let b = SecretString::from("token-b");
        assert_ne!(cache_key(&a), cache_key(&b));
        assert_eq!(cache_key(&a), cache_key(&a.clone()));
    }
}
