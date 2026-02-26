//! Search index with frecency-based ranking.
//!
//! This module provides a deduplicated search index where each unique command
//! is stored once, with metadata about all its invocations. This enables:
//!
//! - Efficient fuzzy matching (fewer items to match)
//! - Frecency-based ranking (frequency + recency)
//! - Dynamic filtering by directory, host, session, etc.

use std::{collections::HashMap, sync::Arc};

use atuin_client::history::History;
use dashmap::{DashMap, DashSet};
use nucleo::{Injector, Nucleo, pattern};
use time::OffsetDateTime;
use tokio::sync::RwLock;
use tracing::{Level, instrument};

use crate::components::search::with_trailing_slash;

/// Data for a single invocation of a command.
#[derive(Debug, Clone)]
pub struct Invocation {
    /// When the command was run.
    pub timestamp: i64,
    /// The working directory when the command was run.
    #[allow(dead_code)]
    pub cwd: String,
    /// The hostname where the command was run.
    #[allow(dead_code)]
    pub hostname: String,
    /// The session ID.
    #[allow(dead_code)]
    pub session: String,
    /// The history entry ID (for returning in search results).
    pub history_id: String,
}

impl From<&History> for Invocation {
    fn from(history: &History) -> Self {
        Self {
            timestamp: history.timestamp.unix_timestamp(),
            cwd: history.cwd.clone(),
            hostname: history.hostname.clone(),
            session: history.session.clone(),
            history_id: history.id.0.clone(),
        }
    }
}

/// Pre-computed frecency data for O(1) lookup.
#[derive(Debug, Clone, Default)]
pub struct FrecencyData {
    /// Total number of times this command was used.
    pub count: u32,
    /// Most recent usage timestamp (unix seconds).
    pub last_used: i64,
}

impl FrecencyData {
    /// Record a new usage of this command.
    pub fn record_use(&mut self, timestamp: i64) {
        self.count += 1;
        if timestamp > self.last_used {
            self.last_used = timestamp;
        }
    }

    /// Compute frecency score based on count and recency.
    ///
    /// Uses a decay function where more recent commands score higher.
    /// The formula balances frequency (how often) with recency (how recent).
    #[instrument(level = tracing::Level::TRACE, name = "index_frecency_compute")]
    pub fn compute(&self, now: i64) -> u32 {
        if self.count == 0 {
            return 0;
        }

        // Time-based decay: score decreases as time passes
        let age_seconds = (now - self.last_used).max(0) as u64;
        let age_hours = age_seconds / 3600;

        // Decay factor: recent commands get higher scores
        // - Last hour: multiplier ~1.0
        // - Last day: multiplier ~0.5
        // - Last week: multiplier ~0.1
        // - Older: multiplier approaches 0
        let recency_score = match age_hours {
            0 => 100,
            1..=6 => 90,
            7..=24 => 70,
            25..=72 => 50,
            73..=168 => 30,
            169..=720 => 15,
            _ => 5,
        };

        // Frequency boost: more uses = higher score (with diminishing returns)
        let frequency_score = ((self.count as f64).ln() * 20.0).min(100.0) as u32;

        // Combined score
        recency_score + frequency_score
    }
}

/// Data for a unique command, including all its invocations.
pub struct CommandData {
    /// The command text (stored for debugging/logging purposes).
    #[allow(dead_code)]
    pub command: String,
    /// All invocations of this command, sorted by timestamp (newest first).
    pub invocations: Vec<Invocation>,
    /// Pre-computed global frecency.
    pub global_frecency: FrecencyData,

    // Pre-computed indexes for O(1) filter lookups
    /// All directories where this command has been run.
    directories: DashSet<String>,
    /// All hostnames where this command has been run.
    hosts: DashSet<String>,
    /// All sessions where this command has been run.
    sessions: DashSet<String>,
}

impl CommandData {
    /// Create a new CommandData from a history entry.
    pub fn new(history: &History) -> Self {
        let mut data = Self {
            command: history.command.clone(),
            invocations: Vec::new(),
            global_frecency: FrecencyData::default(),
            directories: DashSet::new(),
            hosts: DashSet::new(),
            sessions: DashSet::new(),
        };
        data.add_invocation(history);
        data
    }

    /// Add an invocation from a history entry.
    pub fn add_invocation(&mut self, history: &History) {
        let timestamp = history.timestamp.unix_timestamp();

        // Update global frecency
        self.global_frecency.record_use(timestamp);

        // Update pre-computed indexes for O(1) filter lookups
        self.directories.insert(with_trailing_slash(&history.cwd));
        self.hosts.insert(history.hostname.clone());
        self.sessions.insert(history.session.clone());

        let invocation = Invocation::from(history);

        // Insert sorted by timestamp (newest first)
        let pos = self
            .invocations
            .iter()
            .position(|inv| inv.timestamp < timestamp)
            .unwrap_or(self.invocations.len());
        self.invocations.insert(pos, invocation);
    }

    /// Get the most recent history ID for this command.
    pub fn most_recent_id(&self) -> Option<&str> {
        self.invocations.first().map(|inv| inv.history_id.as_str())
    }

    /// Check if any invocation matches a directory filter (exact match).
    /// O(1) lookup using pre-computed index.
    pub fn has_invocation_in_dir(&self, dir: &str) -> bool {
        self.directories.contains(dir)
    }

    /// Check if any invocation matches a directory prefix (workspace/git root).
    /// O(n) where n = number of unique directories for this command.
    pub fn has_invocation_in_workspace(&self, prefix: &str) -> bool {
        self.directories.iter().any(|d| d.starts_with(prefix))
    }

    /// Check if any invocation matches a hostname.
    /// O(1) lookup using pre-computed index.
    pub fn has_invocation_on_host(&self, hostname: &str) -> bool {
        self.hosts.contains(hostname)
    }

    /// Check if any invocation matches a session.
    /// O(1) lookup using pre-computed index.
    pub fn has_invocation_in_session(&self, session: &str) -> bool {
        self.sessions.contains(session)
    }
}

/// Filter mode for search queries.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IndexFilterMode {
    /// No filtering - search all commands.
    Global,
    /// Filter to commands run in a specific directory.
    Directory(String),
    /// Filter to commands run in a workspace (directory prefix).
    Workspace(String),
    /// Filter to commands run on a specific host.
    Host(String),
    /// Filter to commands run in a specific session.
    Session(String),
}

/// Context for search queries.
#[derive(Debug, Clone, Default)]
pub struct QueryContext {
    pub cwd: Option<String>,
    pub git_root: Option<String>,
    pub hostname: Option<String>,
    pub session_id: Option<String>,
}

/// A deduplicated search index with frecency-based ranking.
///
/// Commands are stored by their text, with metadata about all invocations.
/// Nucleo handles fuzzy matching, while frecency is computed via scorer callback.
///
/// Global frecency is precomputed by a background task and used for scoring.
/// If frecency data is not available, search still works but without frecency ranking;
/// although this should never happen due to precomputing the frecency map.
pub struct SearchIndex {
    /// Map from command text to command data.
    /// Using DashMap for concurrent read/write access, wrapped in Arc for sharing with scorer.
    commands: Arc<DashMap<String, CommandData>>,
    /// Nucleo fuzzy matcher - items are command strings.
    nucleo: RwLock<Nucleo<String>>,
    /// Injector for adding new commands to Nucleo.
    injector: Injector<String>,
    /// Precomputed global frecency map (command -> frecency score).
    /// Updated by background task. If None, search works without frecency.
    frecency_map: RwLock<Option<Arc<HashMap<String, u32>>>>,
}

impl SearchIndex {
    /// Create a new empty search index.
    pub fn new() -> Self {
        let nucleo_config = nucleo::Config::DEFAULT;
        // Single column for command text
        let nucleo = Nucleo::<String>::new(nucleo_config, Arc::new(|| {}), None, 1);
        let injector = nucleo.injector();

        Self {
            commands: Arc::new(DashMap::new()),
            nucleo: RwLock::new(nucleo),
            injector,
            frecency_map: RwLock::new(None),
        }
    }

    /// Add a history entry to the index.
    ///
    /// If the command already exists, updates its invocation data.
    /// If it's a new command, adds it to both the map and Nucleo.
    pub fn add_history(&self, history: &History) {
        let command = &history.command;

        if let Some(mut entry) = self.commands.get_mut(command) {
            // Existing command - just update invocations
            entry.add_invocation(history);
        } else {
            // New command - add to both map and Nucleo
            let data = CommandData::new(history);
            self.commands.insert(command.clone(), data);
            self.injector.push(command.clone(), |cmd, cols| {
                cols[0] = cmd.clone().into();
            });
        }
        // Note: frecency_map is rebuilt by background task, not invalidated here
    }

    /// Add multiple history entries to the index.
    pub fn add_histories(&self, histories: &[History]) {
        for history in histories {
            self.add_history(history);
        }
    }

    /// Get the number of unique commands in the index.
    pub fn command_count(&self) -> usize {
        self.commands.len()
    }

    /// Get the number of items in Nucleo (should match command_count).
    pub async fn nucleo_item_count(&self) -> u32 {
        self.nucleo.read().await.snapshot().item_count()
    }

    /// Search for commands matching a query.
    ///
    /// Returns a list of history IDs (most recent invocation per command).
    /// Uses precomputed global frecency for scoring if available.
    #[instrument(skip_all, level = tracing::Level::TRACE, name = "index_search", fields(query = %query))]
    pub async fn search(
        &self,
        query: &str,
        filter_mode: IndexFilterMode,
        _context: &QueryContext,
        limit: u32,
    ) -> Vec<String> {
        let mut nucleo = self.nucleo.write().await;

        // Get precomputed frecency map (may be None if not yet computed)
        let frecency_map = self.frecency_map.read().await.clone();

        // Build filter based on mode
        let filter = self.build_filter(&filter_mode);
        nucleo.set_filter(filter);

        // Build scorer from precomputed frecency (or None if not available)
        let scorer = Self::build_scorer(frecency_map);
        nucleo.set_scorer(scorer);

        // Update pattern
        nucleo.pattern.reparse(
            0,
            query,
            pattern::CaseMatching::Smart,
            pattern::Normalization::Smart,
            false,
        );

        tracing::span!(Level::TRACE, "index_search_tick").in_scope(|| {
            // Tick until complete
            while nucleo.tick(10).running {}
        });

        // Collect results
        let snapshot = nucleo.snapshot();
        let matched_count = snapshot.matched_item_count().min(limit);

        tracing::span!(Level::TRACE, "index_search_results").in_scope(|| {
            snapshot
                .matched_items(..matched_count)
                .filter_map(|item| {
                    let cmd = item.data;
                    self.commands
                        .get(cmd)
                        .and_then(|data| data.most_recent_id().map(|s| s.to_string()))
                })
                .collect()
        })
    }

    /// Rebuild the global frecency map.
    ///
    /// This should be called by a background task periodically.
    /// The map is used for scoring search results.
    #[instrument(skip_all, level = tracing::Level::DEBUG, name = "rebuild_frecency")]
    pub async fn rebuild_frecency(&self) {
        let now = OffsetDateTime::now_utc().unix_timestamp();
        let mut frecency_map: HashMap<String, u32> = HashMap::new();

        for entry in self.commands.iter() {
            let frecency = entry.global_frecency.compute(now);
            frecency_map.insert(entry.key().clone(), frecency);
        }

        *self.frecency_map.write().await = Some(Arc::new(frecency_map));
    }

    /// Build filter predicate for the given mode.
    fn build_filter(&self, mode: &IndexFilterMode) -> Option<nucleo::Filter<String>> {
        // For Global mode, no filter needed
        if matches!(mode, IndexFilterMode::Global) {
            return None;
        }

        // Pre-compute which commands pass the filter
        let passing_commands: Arc<std::collections::HashSet<String>> = {
            let mut set = std::collections::HashSet::new();
            for entry in self.commands.iter() {
                let passes = match mode {
                    IndexFilterMode::Global => unreachable!(),
                    IndexFilterMode::Directory(dir) => entry.has_invocation_in_dir(dir),
                    IndexFilterMode::Workspace(prefix) => entry.has_invocation_in_workspace(prefix),
                    IndexFilterMode::Host(hostname) => entry.has_invocation_on_host(hostname),
                    IndexFilterMode::Session(session) => entry.has_invocation_in_session(session),
                };
                if passes {
                    set.insert(entry.key().clone());
                }
            }
            Arc::new(set)
        };

        Some(Arc::new(move |cmd: &String| passing_commands.contains(cmd)))
    }

    /// Build scorer from precomputed frecency map.
    ///
    /// Returns None if frecency map is not available (search still works, just without frecency ranking).
    fn build_scorer(
        frecency_map: Option<Arc<HashMap<String, u32>>>,
    ) -> Option<nucleo::Scorer<String>> {
        let map = frecency_map?;
        Some(Arc::new(move |cmd: &String, fuzzy_score: u32| {
            let frecency = map.get(cmd).copied().unwrap_or(0);
            fuzzy_score + frecency
        }))
    }
}

impl Default for SearchIndex {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use time::macros::datetime;

    fn make_history(command: &str, cwd: &str, timestamp: OffsetDateTime) -> History {
        History::import()
            .timestamp(timestamp)
            .command(command)
            .cwd(cwd)
            .build()
            .into()
    }

    #[test]
    fn frecency_data_compute() {
        let now = 1000000i64;

        // Recent command
        let recent = FrecencyData {
            count: 5,
            last_used: now - 60, // 1 minute ago
        };
        assert!(recent.compute(now) > 100); // High score

        // Old command
        let old = FrecencyData {
            count: 5,
            last_used: now - 86400 * 30, // 30 days ago
        };
        assert!(old.compute(now) < recent.compute(now));

        // Frequently used old command
        let frequent_old = FrecencyData {
            count: 100,
            last_used: now - 86400 * 7, // 1 week ago
        };
        // Should still have decent score due to frequency
        assert!(frequent_old.compute(now) > 50);
    }

    #[test]
    fn command_data_add_invocation() {
        let (dir1, dir2) = if cfg!(windows) {
            ("C:\\Users\\User\\project", "C:\\Users\\User\\other")
        } else {
            ("/home/user/project", "/home/user/other")
        };

        let history1 = make_history("git status", dir1, datetime!(2024-01-01 10:00 UTC));
        let history2 = make_history("git status", dir2, datetime!(2024-01-01 12:00 UTC));

        let mut data = CommandData::new(&history1);
        assert_eq!(data.invocations.len(), 1);
        assert_eq!(data.global_frecency.count, 1);

        data.add_invocation(&history2);
        assert_eq!(data.invocations.len(), 2);
        assert_eq!(data.global_frecency.count, 2);

        // Most recent should be first
        assert_eq!(data.invocations[0].cwd, dir2);
        assert_eq!(data.invocations[1].cwd, dir1);
    }

    #[test]
    fn command_data_filters() {
        let (dir1, dir2) = if cfg!(windows) {
            ("C:\\Users\\User\\project", "C:\\Users\\User\\other")
        } else {
            ("/home/user/project", "/home/user/other")
        };

        let h1 = make_history("git status", dir1, datetime!(2024-01-01 10:00 UTC));
        let h2 = make_history("git status", dir2, datetime!(2024-01-01 12:00 UTC));

        let mut data = CommandData::new(&h1);
        data.add_invocation(&h2);

        let (check1, check2, check3) = if cfg!(windows) {
            (
                with_trailing_slash("C:\\Users\\User\\project"),
                with_trailing_slash("C:\\Users\\User\\other"),
                with_trailing_slash("C:\\Users\\User\\missing"),
            )
        } else {
            (
                with_trailing_slash("/home/user/project"),
                with_trailing_slash("/home/user/other"),
                with_trailing_slash("/home/user/missing"),
            )
        };

        assert!(data.has_invocation_in_dir(&check1));
        assert!(data.has_invocation_in_dir(&check2));
        assert!(!data.has_invocation_in_dir(&check3));

        let (check1, check2, check3) = if cfg!(windows) {
            (
                with_trailing_slash("C:\\Users\\User"),
                with_trailing_slash("C:\\Users"),
                with_trailing_slash("C:\\Users\\User\\var"),
            )
        } else {
            (
                with_trailing_slash("/home/user"),
                with_trailing_slash("/home"),
                with_trailing_slash("/var"),
            )
        };

        assert!(data.has_invocation_in_workspace(&check1));
        assert!(data.has_invocation_in_workspace(&check2));
        assert!(!data.has_invocation_in_workspace(&check3));
    }

    #[tokio::test]
    async fn search_index_add_and_search() {
        let index = SearchIndex::new();

        let h1 = make_history(
            "git status",
            "/home/user/project",
            datetime!(2024-01-01 10:00 UTC),
        );
        let h2 = make_history(
            "git commit -m 'test'",
            "/home/user/project",
            datetime!(2024-01-01 10:05 UTC),
        );
        let h3 = make_history(
            "ls -la",
            "/home/user/other",
            datetime!(2024-01-01 10:10 UTC),
        );

        index.add_history(&h1);
        index.add_history(&h2);
        index.add_history(&h3);

        assert_eq!(index.command_count(), 3);

        // Search for "git" - should match 2 commands
        let results = index
            .search("git", IndexFilterMode::Global, &QueryContext::default(), 10)
            .await;
        assert_eq!(results.len(), 2);

        // Search with directory filter
        let results = index
            .search(
                "",
                IndexFilterMode::Directory(with_trailing_slash("/home/user/project")),
                &QueryContext::default(),
                10,
            )
            .await;
        assert_eq!(results.len(), 2); // git status and git commit
    }
}
