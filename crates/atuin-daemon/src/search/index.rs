//! Search index with frecency-based ranking.
//!
//! This module provides a deduplicated search index where each unique command
//! is stored once, with metadata about all its invocations. This enables:
//!
//! - Efficient fuzzy matching (fewer items to match)
//! - Frecency-based ranking (frequency + recency)
//! - Dynamic filtering by directory, host, session, etc.

use std::{
    collections::HashMap,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
};

use atuin_client::history::History;
use dashmap::{DashMap, DashSet};
use nucleo::{Injector, Nucleo, pattern};
use time::OffsetDateTime;
use tokio::sync::RwLock;
use tracing::{Level, instrument};

/// Data for a single invocation of a command.
#[derive(Debug, Clone)]
pub struct Invocation {
    /// When the command was run.
    pub timestamp: i64,
    /// The working directory when the command was run.
    pub cwd: String,
    /// The hostname where the command was run.
    pub hostname: String,
    /// The session ID.
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

    // Pre-computed per-context frecency for O(1) scoring
    /// Frecency per directory.
    dir_frecency: HashMap<String, FrecencyData>,
    /// Frecency per hostname.
    host_frecency: HashMap<String, FrecencyData>,
    /// Frecency per session.
    session_frecency: HashMap<String, FrecencyData>,
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
            dir_frecency: HashMap::new(),
            host_frecency: HashMap::new(),
            session_frecency: HashMap::new(),
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
        self.directories.insert(history.cwd.clone());
        self.hosts.insert(history.hostname.clone());
        self.sessions.insert(history.session.clone());

        // Update per-context frecency for O(1) scoring
        self.dir_frecency
            .entry(history.cwd.clone())
            .or_default()
            .record_use(timestamp);
        self.host_frecency
            .entry(history.hostname.clone())
            .or_default()
            .record_use(timestamp);
        self.session_frecency
            .entry(history.session.clone())
            .or_default()
            .record_use(timestamp);

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

    /// Compute frecency for invocations matching a directory.
    /// O(1) lookup using pre-computed per-directory frecency.
    pub fn frecency_for_dir(&self, dir: &str, now: i64) -> u32 {
        self.dir_frecency
            .get(dir)
            .map(|f| f.compute(now))
            .unwrap_or(0)
    }

    /// Compute frecency for invocations matching a workspace prefix.
    /// O(n) where n = number of unique directories for this command.
    pub fn frecency_for_workspace(&self, prefix: &str, now: i64) -> u32 {
        // Combine frecency from all directories matching the prefix
        let mut total_count = 0u32;
        let mut latest_used = 0i64;

        for (dir, frecency) in &self.dir_frecency {
            if dir.starts_with(prefix) {
                total_count += frecency.count;
                latest_used = latest_used.max(frecency.last_used);
            }
        }

        if total_count == 0 {
            return 0;
        }

        let combined = FrecencyData {
            count: total_count,
            last_used: latest_used,
        };
        combined.compute(now)
    }

    /// Compute frecency for invocations matching a hostname.
    /// O(1) lookup using pre-computed per-host frecency.
    pub fn frecency_for_host(&self, hostname: &str, now: i64) -> u32 {
        self.host_frecency
            .get(hostname)
            .map(|f| f.compute(now))
            .unwrap_or(0)
    }

    /// Compute frecency for invocations matching a session.
    /// O(1) lookup using pre-computed per-session frecency.
    pub fn frecency_for_session(&self, session: &str, now: i64) -> u32 {
        self.session_frecency
            .get(session)
            .map(|f| f.compute(now))
            .unwrap_or(0)
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

/// Granularity for timestamp bucketing in cache (in seconds).
/// Frecency scores are cached within this time window.
const TIMESTAMP_BUCKET_SECONDS: i64 = 60;

/// Cached filter and scorer data.
struct FilterScorerCache {
    /// The filter mode this cache was built for.
    filter_mode: IndexFilterMode,
    /// The timestamp bucket this cache was built for.
    timestamp_bucket: i64,
    /// The generation counter when this cache was built.
    generation: u64,
    /// Pre-computed frecency map (command -> frecency score).
    frecency_map: Arc<HashMap<String, u32>>,
}

/// A deduplicated search index with frecency-based ranking.
///
/// Commands are stored by their text, with metadata about all invocations.
/// Nucleo handles fuzzy matching, while frecency is computed via scorer callback.
pub struct SearchIndex {
    /// Map from command text to command data.
    /// Using DashMap for concurrent read/write access, wrapped in Arc for sharing with scorer.
    commands: Arc<DashMap<String, CommandData>>,
    /// Nucleo fuzzy matcher - items are command strings.
    nucleo: RwLock<Nucleo<String>>,
    /// Injector for adding new commands to Nucleo.
    injector: Injector<String>,
    /// Generation counter - incremented when commands are added.
    generation: AtomicU64,
    /// Cached filter/scorer data.
    cache: RwLock<Option<FilterScorerCache>>,
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
            generation: AtomicU64::new(0),
            cache: RwLock::new(None),
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

        // Invalidate cache - frecency scores may have changed
        self.generation.fetch_add(1, Ordering::Relaxed);
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
    #[instrument(skip_all, level = tracing::Level::TRACE, name = "index_search", fields(query = %query))]
    pub async fn search(
        &self,
        query: &str,
        filter_mode: IndexFilterMode,
        _context: &QueryContext,
        limit: u32,
    ) -> Vec<String> {
        let mut nucleo = self.nucleo.write().await;
        let now = OffsetDateTime::now_utc().unix_timestamp();
        let timestamp_bucket = now / TIMESTAMP_BUCKET_SECONDS;
        let current_generation = self.generation.load(Ordering::Relaxed);

        // Check if we can use cached filter/scorer
        let frecency_map = {
            let cache_guard = self.cache.read().await;
            if let Some(ref cache) = *cache_guard {
                if cache.filter_mode == filter_mode
                    && cache.timestamp_bucket == timestamp_bucket
                    && cache.generation == current_generation
                {
                    // Cache hit - reuse frecency map
                    Some(cache.frecency_map.clone())
                } else {
                    None
                }
            } else {
                None
            }
        };

        let frecency_map = match frecency_map {
            Some(map) => map,
            None => {
                // Cache miss - rebuild and cache
                let map = self.build_frecency_map(&filter_mode, now);
                let mut cache_guard = self.cache.write().await;
                *cache_guard = Some(FilterScorerCache {
                    filter_mode: filter_mode.clone(),
                    timestamp_bucket,
                    generation: current_generation,
                    frecency_map: map.clone(),
                });
                map
            }
        };

        // Build filter and scorer from the frecency map
        let (filter, scorer) = Self::build_filter_and_scorer_from_map(&filter_mode, frecency_map);
        nucleo.set_filter(filter);
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

    /// Build frecency map for all commands matching the filter mode.
    ///
    /// For Global mode, includes all commands.
    /// For filtered modes, only includes commands that pass the filter.
    #[instrument(skip_all, level = tracing::Level::TRACE, name = "build_frecency_map")]
    fn build_frecency_map(&self, mode: &IndexFilterMode, now: i64) -> Arc<HashMap<String, u32>> {
        let mut frecency_map: HashMap<String, u32> = HashMap::new();

        for entry in self.commands.iter() {
            let frecency = match mode {
                IndexFilterMode::Global => Some(entry.global_frecency.compute(now)),
                IndexFilterMode::Directory(dir) => {
                    if entry.has_invocation_in_dir(dir) {
                        Some(entry.frecency_for_dir(dir, now))
                    } else {
                        None
                    }
                }
                IndexFilterMode::Workspace(prefix) => {
                    if entry.has_invocation_in_workspace(prefix) {
                        Some(entry.frecency_for_workspace(prefix, now))
                    } else {
                        None
                    }
                }
                IndexFilterMode::Host(hostname) => {
                    if entry.has_invocation_on_host(hostname) {
                        Some(entry.frecency_for_host(hostname, now))
                    } else {
                        None
                    }
                }
                IndexFilterMode::Session(session) => {
                    if entry.has_invocation_in_session(session) {
                        Some(entry.frecency_for_session(session, now))
                    } else {
                        None
                    }
                }
            };

            if let Some(frecency) = frecency {
                frecency_map.insert(entry.key().clone(), frecency);
            }
        }

        Arc::new(frecency_map)
    }

    /// Build filter and scorer from a pre-computed frecency map.
    ///
    /// Both filter and scorer use the same map for O(1) lock-free lookups.
    fn build_filter_and_scorer_from_map(
        mode: &IndexFilterMode,
        frecency_map: Arc<HashMap<String, u32>>,
    ) -> (
        Option<nucleo::Filter<String>>,
        Option<nucleo::Scorer<String>>,
    ) {
        // Scorer: look up pre-computed frecency
        let scorer_map = frecency_map.clone();
        let scorer = Arc::new(move |cmd: &String, fuzzy_score: u32| {
            let frecency = scorer_map.get(cmd).copied().unwrap_or(0);
            fuzzy_score + (frecency * 10)
        });

        // For Global mode, no filter needed
        if matches!(mode, IndexFilterMode::Global) {
            return (None, Some(scorer));
        }

        // Filter: check if command is in the frecency map (meaning it passed the filter)
        let filter = Arc::new(move |cmd: &String| frecency_map.contains_key(cmd));

        (Some(filter), Some(scorer))
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
        let history1 = make_history(
            "git status",
            "/home/user/project",
            datetime!(2024-01-01 10:00 UTC),
        );
        let history2 = make_history(
            "git status",
            "/home/user/other",
            datetime!(2024-01-01 12:00 UTC),
        );

        let mut data = CommandData::new(&history1);
        assert_eq!(data.invocations.len(), 1);
        assert_eq!(data.global_frecency.count, 1);

        data.add_invocation(&history2);
        assert_eq!(data.invocations.len(), 2);
        assert_eq!(data.global_frecency.count, 2);

        // Most recent should be first
        assert_eq!(data.invocations[0].cwd, "/home/user/other");
        assert_eq!(data.invocations[1].cwd, "/home/user/project");
    }

    #[test]
    fn command_data_filters() {
        let h1 = make_history(
            "git status",
            "/home/user/project",
            datetime!(2024-01-01 10:00 UTC),
        );
        let h2 = make_history(
            "git status",
            "/home/user/other",
            datetime!(2024-01-01 12:00 UTC),
        );

        let mut data = CommandData::new(&h1);
        data.add_invocation(&h2);

        assert!(data.has_invocation_in_dir("/home/user/project"));
        assert!(data.has_invocation_in_dir("/home/user/other"));
        assert!(!data.has_invocation_in_dir("/home/user/missing"));

        assert!(data.has_invocation_in_workspace("/home/user"));
        assert!(data.has_invocation_in_workspace("/home"));
        assert!(!data.has_invocation_in_workspace("/var"));
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
                IndexFilterMode::Directory("/home/user/project".into()),
                &QueryContext::default(),
                10,
            )
            .await;
        assert_eq!(results.len(), 2); // git status and git commit
    }
}
