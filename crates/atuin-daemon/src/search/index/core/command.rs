use std::collections::HashSet;

use atuin_client::history::{History, HistoryId};
use atuin_common::path::DisplayRichExt;
use easy_cast::Conv;
use lasso::{Rodeo, Spur};
use tracing::instrument;
use uuid::Uuid;

use crate::search::index::FrecencyMultipliers;

/// Data for a unique command.
#[derive(Debug)]
pub struct CommandData {
    /// History ID of the most recent invocation.
    pub most_recent_id: HistoryId,
    /// Timestamp of the most recent invocation (unix seconds).
    pub most_recent_timestamp: i64,
    /// Total number of times this command was used.
    pub count: u32,

    // Pre-computed indexes for O(1) filter lookups
    /// All directories where this command has been run (interned keys).
    directories: HashSet<Spur>,
    /// All hostnames where this command has been run (interned keys).
    hosts: HashSet<Spur>,
    /// All sessions where this command has been run (as 16-byte UUIDs).
    sessions: HashSet<Uuid>,
}

impl CommandData {
    /// Create a new CommandData from a history entry.
    ///
    /// Returns [`None`] if the history entry has invalid UUIDs.
    pub fn new(history: &History, interner: &mut Rodeo) -> Option<Self> {
        let session = Uuid::parse_str(&history.session).ok()?;
        let timestamp = history.timestamp.unix_timestamp();

        let dir_key =
            interner.get_or_intern(history.cwd.display_rich().trailing_slash(true).to_string());
        let host_key = interner.get_or_intern(history.cmd_origin.as_str());

        Some(Self {
            most_recent_id: history.id,
            most_recent_timestamp: timestamp,
            count: 1,
            directories: HashSet::from([dir_key]),
            hosts: HashSet::from([host_key]),
            sessions: HashSet::from([session]),
        })
    }

    /// Add an invocation from a history entry.
    ///
    /// Returns false if the history entry has invalid UUIDs.
    pub fn add_invocation(&mut self, history: &History, interner: &mut Rodeo) -> bool {
        let Some(session) = Uuid::parse_str(&history.session).ok() else {
            return false;
        };

        let timestamp = history.timestamp.unix_timestamp();

        self.count += 1;

        // Update pre-computed indexes for O(1) filter lookups
        let dir_key =
            interner.get_or_intern(history.cwd.display_rich().trailing_slash(true).to_string());
        self.directories.insert(dir_key);
        self.hosts.insert(interner.get_or_intern(history.cmd_origin.as_str()));
        self.sessions.insert(session);

        // Update the most recent run if this run is newer. Timestamps are whole seconds. Runs in the
        // same second are ordered by id instead. UUIDv7 ids are time-ordered, so the later run
        // normally wins. The result doesn't depend on the order that the runs arrive in.
        if (timestamp, history.id) > (self.most_recent_timestamp, self.most_recent_id) {
            self.most_recent_id = history.id;
            self.most_recent_timestamp = timestamp;
        }

        true
    }

    /// Compute frecency score based on count and recency.
    ///
    /// Uses a decay function where more recent commands score higher.
    /// The formula balances frequency (how often) with recency (how recent).
    ///
    /// `multipliers` allows tuning the weights.
    #[instrument(level = tracing::Level::TRACE, name = "index_frecency_compute", skip_all)]
    pub fn compute_frecency(&self, now: i64, multipliers: FrecencyMultipliers) -> u32 {
        // Time-based decay: score decreases as time passes
        let age_seconds = u64::conv((now.saturating_sub(self.most_recent_timestamp)).max(0));
        let age_hours = age_seconds / 3600;

        // Decay factor: recent commands get higher scores
        // - Last hour: multiplier ~1.0
        // - Last day: multiplier ~0.5
        // - Last week: multiplier ~0.1
        // - Older: multiplier approaches 0
        let recency_score: f64 = match age_hours {
            0 => 100.0,
            1..=6 => 90.0,
            7..=24 => 70.0,
            25..=72 => 50.0,
            73..=168 => 30.0,
            169..=720 => 15.0,
            _ => 5.0,
        };

        // Frequency boost: more uses = higher score (with diminishing returns)
        let frequency_score = (f64::from(self.count).ln() * 20.0).min(100.0);

        // Apply multipliers and combine scores, then round to u32
        #[expect(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            reason = "frecency is used for ordering only -- saturating is ok"
        )]
        {
            (((recency_score * multipliers.recency) + (frequency_score * multipliers.frequency))
                * multipliers.frecency)
                .round() as u32
        }
    }

    /// Check if any invocation matches an interned directory (exact match).
    ///
    /// O(1) integer-set lookup; the caller resolves the directory string to a
    /// `Spur` once per search.
    pub fn has_invocation_in_dir(&self, dir: Spur) -> bool {
        self.directories.contains(&dir)
    }

    /// Check if any invocation matches a directory prefix (workspace/repo root).
    ///
    /// O(n) where n = number of unique directories for this command.
    pub fn has_invocation_in_workspace(&self, prefix: &str, interner: &Rodeo) -> bool {
        self.directories.iter().any(|&spur| interner.resolve(&spur).starts_with(prefix))
    }

    /// Check if any invocation matches an interned hostname.
    ///
    /// O(1) integer-set lookup; the caller resolves the hostname to a `Spur`
    /// once per search.
    pub fn has_invocation_on_host(&self, hostname: Spur) -> bool {
        self.hosts.contains(&hostname)
    }

    /// Check if any invocation matches a session (as parsed UUID bytes).
    ///
    /// O(1) lookup; the caller parses the session string once per search.
    pub fn has_invocation_in_session(&self, session: &Uuid) -> bool {
        self.sessions.contains(session)
    }
}

#[cfg(test)]
mod tests {
    use atuin_client::settings::Search;
    use rstest::rstest;
    use time::OffsetDateTime;
    use time::macros::datetime;

    use super::*;
    use crate::search::index::core::tests::make_history;

    /// A [`CommandData`] used `count` times, most recently at `most_recent_timestamp`.
    fn command_data(count: u32, most_recent_timestamp: i64) -> CommandData {
        let history = make_history("git status", "/tmp", datetime!(2024-01-01 10:00 UTC));
        let mut data = CommandData::new(&history, &mut Rodeo::new()).unwrap();
        data.count = count;
        data.most_recent_timestamp = most_recent_timestamp;
        data
    }

    /// Frecency multipliers with the given recency and frequency weights.
    fn multipliers(recency: f64, frequency: f64) -> FrecencyMultipliers {
        FrecencyMultipliers {
            recency,
            frequency,
            frecency: 1.0,
        }
    }

    #[rstest]
    fn command_data_compute_frecency() {
        let now = 1_000_000_i64;

        // Recent command (with default multipliers of 1.0)
        let recent = command_data(5, now - 60); // 1 minute ago
        assert!(recent.compute_frecency(now, multipliers(1.0, 1.0)) > 100); // High score

        // Old command
        let old = command_data(5, now - 86400 * 30); // 30 days ago
        assert!(
            old.compute_frecency(now, multipliers(1.0, 1.0))
                < recent.compute_frecency(now, multipliers(1.0, 1.0))
        );

        // Frequently used old command
        let frequent_old = command_data(100, now - 86400 * 7); // 1 week ago
        // Should still have decent score due to frequency
        assert!(frequent_old.compute_frecency(now, multipliers(1.0, 1.0)) > 50);
    }

    #[rstest]
    fn command_data_compute_frecency_with_multipliers() {
        let now = 1_000_000_i64;

        let data = command_data(5, now - 60); // 1 minute ago (recency_score = 100)

        // Default multipliers (1.0, 1.0)
        let default_score = data.compute_frecency(now, multipliers(1.0, 1.0));

        // Double recency weight
        let double_recency = data.compute_frecency(now, multipliers(2.0, 1.0));
        assert!(double_recency > default_score);

        // Double frequency weight
        let double_frequency = data.compute_frecency(now, multipliers(1.0, 2.0));
        assert!(double_frequency > default_score);

        // Zero out recency (only frequency counts)
        let no_recency = data.compute_frecency(now, multipliers(0.0, 1.0));
        assert!(no_recency < default_score);

        // Zero out frequency (only recency counts)
        let no_frequency = data.compute_frecency(now, multipliers(1.0, 0.0));
        assert!(no_frequency < default_score);

        // Zero both (should be zero)
        let no_score = data.compute_frecency(now, multipliers(0.0, 0.0));
        assert_eq!(no_score, 0);

        // Fractional multipliers
        let half_recency = data.compute_frecency(now, multipliers(0.5, 1.0));
        assert!(half_recency < default_score);
        assert!(half_recency > no_recency);

        // 1.5x multiplier
        let boost_recency = data.compute_frecency(now, multipliers(1.5, 1.0));
        assert!(boost_recency > default_score);
        assert!(boost_recency < double_recency);
    }

    /// The overall frecency multiplier scales the combined score. Negative multipliers are clamped
    /// to zero.
    #[rstest]
    #[case::double(2.0, 2.0)]
    #[case::half(0.5, 0.5)]
    #[case::zero(0.0, 0.0)]
    #[case::negative(-1.0, 0.0)]
    fn command_data_compute_frecency_overall_multiplier(
        #[case] frecency_mul: f64,
        #[case] expected_factor: f64,
    ) {
        let now = 1_000_000_i64;
        let data = command_data(5, now - 60);

        let base = data.compute_frecency(now, FrecencyMultipliers::default());
        let settings = Search {
            frecency_score_multiplier: frecency_mul,
            ..Search::default()
        };
        let scaled = data.compute_frecency(now, FrecencyMultipliers::from(&settings));
        assert!((f64::from(scaled) - f64::from(base) * expected_factor).abs() <= 1.0);
    }

    /// The most recent invocation wins whichever order invocations arrive in, including
    /// before the Unix epoch. A later timestamp wins even over a larger id. Runs in the same second
    /// are ordered by id.
    #[rstest]
    #[case::after_epoch(datetime!(2024-01-01 10:00 UTC), 2, datetime!(2024-01-01 12:00 UTC), 1)]
    #[case::before_epoch(datetime!(1960-01-01 10:00 UTC), 2, datetime!(1960-01-01 12:00 UTC), 1)]
    #[case::same_second(datetime!(2024-01-01 10:00:00.2 UTC), 1, datetime!(2024-01-01 10:00:00.7 UTC), 2)]
    fn command_data_add_invocation(
        #[case] older_time: OffsetDateTime,
        #[case] older_id: u128,
        #[case] newer_time: OffsetDateTime,
        #[case] newer_id: u128,
        #[values(false, true)] newer_first: bool,
    ) {
        let mut interner = Rodeo::new();

        let (dir1, dir2) = if cfg!(windows) {
            ("C:\\Users\\User\\project", "C:\\Users\\User\\other")
        } else {
            ("/home/user/project", "/home/user/other")
        };

        let mut older = make_history("git status", dir1, older_time);
        older.id = HistoryId::new(Uuid::from_u128(older_id));
        let mut newer = make_history("git status", dir2, newer_time);
        newer.id = HistoryId::new(Uuid::from_u128(newer_id));
        let (first, second) = if newer_first {
            (&newer, &older)
        } else {
            (&older, &newer)
        };

        let mut data = CommandData::new(first, &mut interner).unwrap();
        assert_eq!(data.count, 1);
        assert_eq!(data.most_recent_id, first.id);
        assert_eq!(data.most_recent_timestamp, first.timestamp.unix_timestamp());

        data.add_invocation(second, &mut interner);
        assert_eq!(data.count, 2);
        assert_eq!(data.most_recent_id, newer.id);
        assert_eq!(data.most_recent_timestamp, newer_time.unix_timestamp());
    }

    #[rstest]
    fn command_data_filters() {
        let mut interner = Rodeo::new();

        let (dir1, dir2) = if cfg!(windows) {
            ("C:\\Users\\User\\project", "C:\\Users\\User\\other")
        } else {
            ("/home/user/project", "/home/user/other")
        };

        let h1 = make_history("git status", dir1, datetime!(2024-01-01 10:00 UTC));
        let h2 = make_history("git status", dir2, datetime!(2024-01-01 12:00 UTC));

        let mut data = CommandData::new(&h1, &mut interner).unwrap();
        data.add_invocation(&h2, &mut interner);

        let (check1, check2, check3) = if cfg!(windows) {
            (
                "C:\\Users\\User\\project".display_rich().trailing_slash(true).to_string(),
                "C:\\Users\\User\\other".display_rich().trailing_slash(true).to_string(),
                "C:\\Users\\User\\missing".display_rich().trailing_slash(true).to_string(),
            )
        } else {
            (
                "/home/user/project".display_rich().trailing_slash(true).to_string(),
                "/home/user/other".display_rich().trailing_slash(true).to_string(),
                "/home/user/missing".display_rich().trailing_slash(true).to_string(),
            )
        };

        let in_dir =
            |dir: &str| interner.get(dir).is_some_and(|spur| data.has_invocation_in_dir(spur));
        assert!(in_dir(&check1));
        assert!(in_dir(&check2));
        assert!(!in_dir(&check3));

        let (check1, check2, check3) = if cfg!(windows) {
            (
                "C:\\Users\\User".display_rich().trailing_slash(true).to_string(),
                "C:\\Users".display_rich().trailing_slash(true).to_string(),
                "C:\\Users\\User\\var".display_rich().trailing_slash(true).to_string(),
            )
        } else {
            (
                "/home/user".display_rich().trailing_slash(true).to_string(),
                "/home".display_rich().trailing_slash(true).to_string(),
                "/var".display_rich().trailing_slash(true).to_string(),
            )
        };

        assert!(data.has_invocation_in_workspace(&check1, &interner));
        assert!(data.has_invocation_in_workspace(&check2, &interner));
        assert!(!data.has_invocation_in_workspace(&check3, &interner));
    }
}
