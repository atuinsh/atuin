//! Server-side credit usage: fetching and the done-event snapshot type.
//!
//! The hub reports the user's period credit totals two ways: a `credits`
//! object on the chat `done` event, and `GET /api/cli/usage` for reading it
//! outside a chat. Both share the same shape, deserialized here as
//! [`UsageSnapshot`]. Snapshots are cached in ai.db (see `store`) so the TUI
//! can render usage immediately on open, then refreshed in the background.

use std::time::Duration;

use eyre::{Context, Result};
use reqwest::header::USER_AGENT;
use serde::{Deserialize, Serialize};

/// Cached usage older than this triggers a background refresh on TUI open.
pub(crate) const REFRESH_AFTER: Duration = Duration::from_secs(60);

/// Used/limit pair in credits (billable tokens × model multiplier).
/// Limits use the server's sentinels: -1 unlimited, 0 disabled.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct UsageBucket {
    pub used: i64,
    pub limit: i64,
}

/// The user's credit totals against their limits for the current period.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct UsageSnapshot {
    /// e.g. "calendar_monthly"
    pub period: String,
    /// RFC 3339 timestamp of the next period reset.
    pub resets_at: String,
    pub requests: UsageBucket,
    pub input: UsageBucket,
    pub output: UsageBucket,
}

impl UsageSnapshot {
    pub(crate) fn resets_in(&self) -> Option<Duration> {
        let reset_time = chrono::DateTime::parse_from_rfc3339(&self.resets_at).ok()?;
        let now = chrono::Utc::now().fixed_offset();
        let duration = reset_time - now;
        duration.to_std().ok()
    }

    pub(crate) fn as_percentage(&self) -> Option<f64> {
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
pub(crate) fn format_reset_delta(delta: Duration) -> String {
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
pub(crate) fn cache_key(token: &str) -> String {
    format!("{:016x}", xxhash_rust::xxh3::xxh3_64(token.as_bytes()))
}

/// Fetch current usage from the hub. Mirrors the `credits` object on the
/// chat `done` event, for refreshing without starting a chat.
pub(crate) async fn fetch_usage(endpoint: &str, token: &str) -> Result<UsageSnapshot> {
    atuin_common::tls::ensure_crypto_provider();
    let url = crate::stream::hub_url(endpoint, "/api/cli/usage")?;

    let response = reqwest::Client::new()
        .get(url)
        .header(USER_AGENT, crate::stream::APP_USER_AGENT)
        .bearer_auth(token)
        .timeout(Duration::from_secs(10))
        .send()
        .await
        .context("failed to fetch usage")?;

    let status = response.status();
    if !status.is_success() {
        eyre::bail!("usage request failed ({status})");
    }

    response
        .json::<UsageSnapshot>()
        .await
        .context("failed to parse usage response")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
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

    #[test]
    fn snapshot_roundtrips_through_json() {
        let snapshot = UsageSnapshot {
            period: "calendar_monthly".into(),
            resets_at: "2026-08-01T00:00:00Z".into(),
            requests: UsageBucket { used: 1, limit: 10 },
            input: UsageBucket { used: 2, limit: 20 },
            output: UsageBucket { used: 3, limit: 0 },
        };

        let json = serde_json::to_string(&snapshot).unwrap();
        assert_eq!(
            serde_json::from_str::<UsageSnapshot>(&json).unwrap(),
            snapshot
        );
    }

    #[test]
    fn as_percentage_averages_limited_buckets() {
        let mut snapshot = UsageSnapshot {
            period: "calendar_monthly".into(),
            resets_at: "2026-08-01T00:00:00Z".into(),
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
        assert_eq!(snapshot.as_percentage(), Some(70.0));

        // Unlimited/disabled buckets drop out of the average
        snapshot.output.limit = -1;
        assert_eq!(snapshot.as_percentage(), Some(50.0));

        snapshot.input.limit = 0;
        assert_eq!(snapshot.as_percentage(), None);
    }

    #[test]
    fn formats_reset_deltas() {
        let mins = |m: u64| Duration::from_secs(m * 60);
        assert_eq!(format_reset_delta(mins(4 * 24 * 60 + 300)), "4d");
        assert_eq!(format_reset_delta(mins(23 * 60 + 59)), "23h");
        assert_eq!(format_reset_delta(mins(56)), "56m");
        assert_eq!(format_reset_delta(Duration::from_secs(30)), "1m");
    }

    #[test]
    fn cache_key_distinguishes_tokens() {
        assert_ne!(cache_key("token-a"), cache_key("token-b"));
        assert_eq!(cache_key("token-a"), cache_key("token-a"));
    }
}
