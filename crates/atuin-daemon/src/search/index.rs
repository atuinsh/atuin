//! Search index with frecency-based ranking.
//!
//! This module provides a deduplicated search index where each unique command
//! is stored once, with metadata about all its invocations. This enables:
//!
//! - Efficient fuzzy matching (fewer items to match)
//! - Frecency-based ranking (frequency + recency)
//! - Dynamic filtering by directory, host, session, etc.

use std::borrow::Cow;
use std::cmp::Ordering;
use std::collections::HashSet;
use std::sync::{Arc, RwLock};

use atuin_client::history::History;
use atuin_client::settings::Search;
use atuin_common::filter::OrFilter;
use atuin_common::path::DisplayRichExt;
use dashmap::DashMap;
use lasso::{Spur, ThreadedRodeo};
use time::OffsetDateTime;
use tracing::{Level, instrument};
use uuid::Uuid;

use super::normalize_diacritics;

/// Parse a UUID string into a 16-byte array.
/// Returns None if the string is not a valid UUID.
fn parse_uuid_bytes(s: &str) -> Option<[u8; 16]> {
    Uuid::parse_str(s).ok().map(|u| *u.as_bytes())
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
    ///
    /// Multipliers allow tuning the relative weights:
    /// - `recency_mul`: Multiplier for recency score (default: 1.0)
    /// - `frequency_mul`: Multiplier for frequency score (default: 1.0)
    ///
    /// A multiplier of 0.0 disables that component, 1.0 is unchanged, 2.0 doubles weight.
    /// Values like 0.5 reduce weight by half, 1.5 increases by 50%, etc.
    #[instrument(level = tracing::Level::TRACE, name = "index_frecency_compute")]
    pub fn compute(&self, now: i64, recency_mul: f64, frequency_mul: f64) -> u32 {
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
        ((recency_score * recency_mul) + (frequency_score * frequency_mul)).round() as u32
    }
}

/// Data for a unique command.
pub struct CommandData {
    /// History ID of the most recent invocation (16-byte UUID).
    most_recent_id: [u8; 16],
    /// Timestamp of the most recent invocation.
    most_recent_timestamp: i64,
    /// Pre-computed global frecency.
    pub global_frecency: FrecencyData,

    // Pre-computed indexes for O(1) filter lookups
    // Using HashSet instead of DashSet since CommandData lives inside DashMap (already synchronized)
    /// All directories where this command has been run (interned keys).
    directories: HashSet<Spur>,
    /// All hostnames where this command has been run (interned keys).
    hosts: HashSet<Spur>,
    /// All sessions where this command has been run (as 16-byte UUIDs).
    sessions: HashSet<[u8; 16]>,
    /// Position of this command in `SearchIndex::haystack`, so filtered
    /// searches can walk the command map without hashing command strings.
    haystack_index: u32,
}

impl CommandData {
    /// Create a new CommandData from a history entry.
    ///
    /// Returns [`None`] if the history entry has invalid UUIDs or if the haystack index exceeds
    /// 2^32.
    pub fn new(history: &History, haystack_index: usize, interner: &ThreadedRodeo) -> Option<Self> {
        let Ok(haystack_index) = u32::try_from(haystack_index) else {
            // TODO: It's very unlikely that we'll have more than 2^32 history entries, but if that
            // ends up being a realistic possibility, we should handle this case better than simply
            // dropping new commands from the index.
            return None;
        };

        let history_id = parse_uuid_bytes(&history.id.0)?;
        let session = parse_uuid_bytes(&history.session)?;
        let timestamp = history.timestamp.unix_timestamp();

        let dir_key =
            interner.get_or_intern(history.cwd.display_rich().trailing_slash(true).to_string());
        let host_key = interner.get_or_intern(history.cmd_origin.as_str());

        let mut global_frecency = FrecencyData::default();
        global_frecency.record_use(timestamp);

        Some(Self {
            most_recent_id: history_id,
            most_recent_timestamp: timestamp,
            global_frecency,
            directories: HashSet::from([dir_key]),
            hosts: HashSet::from([host_key]),
            sessions: HashSet::from([session]),
            haystack_index,
        })
    }

    /// Add an invocation from a history entry.
    /// Returns false if the history entry has invalid UUIDs.
    pub fn add_invocation(&mut self, history: &History, interner: &ThreadedRodeo) -> bool {
        let Some(history_id) = parse_uuid_bytes(&history.id.0) else {
            return false;
        };
        let Some(session) = parse_uuid_bytes(&history.session) else {
            return false;
        };

        let timestamp = history.timestamp.unix_timestamp();

        // Update global frecency
        self.global_frecency.record_use(timestamp);

        // Update pre-computed indexes for O(1) filter lookups
        let dir_key =
            interner.get_or_intern(history.cwd.display_rich().trailing_slash(true).to_string());
        self.directories.insert(dir_key);
        self.hosts.insert(interner.get_or_intern(history.cmd_origin.as_str()));
        self.sessions.insert(session);

        // Update most recent if this invocation is newer
        if timestamp > self.most_recent_timestamp {
            self.most_recent_id = history_id;
            self.most_recent_timestamp = timestamp;
        }

        true
    }

    /// Get the most recent history ID for this command.
    pub fn most_recent_id(&self) -> [u8; 16] {
        self.most_recent_id
    }

    /// Check if any invocation matches an interned directory (exact match).
    /// O(1) integer-set lookup; the caller resolves the directory string to a
    /// `Spur` once per search.
    pub fn has_invocation_in_dir(&self, dir: Spur) -> bool {
        self.directories.contains(&dir)
    }

    /// Check if any invocation matches a directory prefix (workspace/git root).
    /// O(n) where n = number of unique directories for this command.
    pub fn has_invocation_in_workspace(&self, prefix: &str, interner: &ThreadedRodeo) -> bool {
        self.directories.iter().any(|&spur| interner.resolve(&spur).starts_with(prefix))
    }

    /// Check if any invocation matches an interned hostname.
    /// O(1) integer-set lookup; the caller resolves the hostname to a `Spur`
    /// once per search.
    pub fn has_invocation_on_host(&self, hostname: Spur) -> bool {
        self.hosts.contains(&hostname)
    }

    /// Check if any invocation matches a session (as parsed UUID bytes).
    /// O(1) lookup; the caller parses the session string once per search.
    pub fn has_invocation_in_session(&self, session: &[u8; 16]) -> bool {
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

/// A "compiled" form of [`IndexFilterMode`], with most of the strings interned and parsed.
enum CompiledFilter<'a> {
    All,
    Directory(Spur),
    Workspace(&'a str),
    Host(Spur),
    Session([u8; 16]),
    /// Used when a target (host/dir/session) has never been seen by the index -- nothing can match.
    Nothing,
}

impl IndexFilterMode {
    /// "Compile" this filter by interning and parsing its strings.
    fn compile(&self, interner: &ThreadedRodeo) -> CompiledFilter<'_> {
        match self {
            Self::Global => CompiledFilter::All,
            Self::Directory(dir) => {
                interner.get(dir).map_or(CompiledFilter::Nothing, CompiledFilter::Directory)
            }
            Self::Workspace(prefix) => CompiledFilter::Workspace(prefix),
            Self::Host(hostname) => {
                interner.get(hostname).map_or(CompiledFilter::Nothing, CompiledFilter::Host)
            }
            Self::Session(session) => {
                parse_uuid_bytes(session).map_or(CompiledFilter::Nothing, CompiledFilter::Session)
            }
        }
    }
}

/// Shareable frecency map: command -> frecency score.
type FrecencyMap = Arc<Vec<u32>>;

/// One entry in the fuzzy matcher's haystack: the original string plus the normalized version
/// (diacritics removed) we actually match against.
struct HaystackEntry {
    /// The original text of the entry.
    pub original: Arc<str>,
    /// The [normalized](normalize_diacritics) version of the text, with diacritics removed.
    pub normalized: Arc<str>,
}

impl HaystackEntry {
    /// Create a new [`HaystackEntry`] from an [`Arc<str>`].
    pub fn new(text: Arc<str>) -> Self {
        let normalized = match normalize_diacritics(&text) {
            Cow::Borrowed(_) => text.clone(),
            Cow::Owned(normalized) => normalized.into(),
        };
        Self {
            original: text,
            normalized,
        }
    }
}

/// Represents how closely a command matches a search query.
#[derive(Clone, Copy, Eq, PartialEq)]
struct Score {
    // Fields must be in this order so we rank by fuzzy score first and only use frecency
    // for ties. See #3702.
    pub fuzzy_score: u16,
    pub frecency: u32,
    pub index: u32,
}

/// Scores are ordered as follows:
///
/// * Fuzzy score (highest first)
/// * If equal, frecency (highest first)
/// * If equal, index (lowest first)
impl Ord for Score {
    fn cmp(&self, other: &Self) -> Ordering {
        (other.fuzzy_score, other.frecency, self.index).cmp(&(
            self.fuzzy_score,
            self.frecency,
            other.index,
        ))
    }
}

impl PartialOrd for Score {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

/// A deduplicated search index with frecency-based ranking.
///
/// Commands are stored by their text, with metadata about all invocations.
/// Frizbee handles fuzzy matching; results are ranked by match quality,
/// with frecency breaking ties between equally good matches.
///
/// Global frecency is precomputed by a background task and used for scoring.
/// If frecency data is not available, search still works but without frecency ranking;
/// although this should never happen due to precomputing the frecency map.
pub struct SearchIndex {
    /// Map from command text to command data.
    ///
    /// Using `DashMap` for concurrent read/write access, wrapped in `Arc` for sharing with scorer.
    /// Keys are `Arc<str>` to enable zero-copy sharing with frecency_map.
    commands: Arc<DashMap<Arc<str>, CommandData>>,

    /// Unique commands in insertion order — the fuzzy matcher's haystack.
    /// Shares the `Arc<str>` allocations with `commands`.
    ///
    /// Lock order: paths that need both this lock and a `commands` shard lock
    /// must take this one first (see `add_history`, `search`,
    /// `rebuild_frecency`); acquiring in the reverse order can deadlock.
    haystack: RwLock<Vec<HaystackEntry>>,

    /// Precomputed global frecency map. Updated by background task.
    ///
    /// Aligned with [`Self::haystack`] by index, so scoring a match is an
    /// array access instead of hashing the command string. Commands added after
    /// the last rebuild sit past the end and score 0 until the next rebuild.
    frecency_map: RwLock<Option<FrecencyMap>>,

    /// String interner for deduplicating cwd, hostname, and directory paths.
    interner: Arc<ThreadedRodeo>,

    /// Controls which shells' commands are included.
    pub shells: OrFilter<Vec<String>>,
}

impl SearchIndex {
    /// Create a new empty search index.
    pub fn new(shells: OrFilter<Vec<String>>) -> Self {
        Self {
            commands: Arc::new(DashMap::new()),
            haystack: RwLock::new(Vec::new()),
            frecency_map: RwLock::new(None),
            interner: Arc::new(ThreadedRodeo::new()),
            shells,
        }
    }

    /// Add a history entry to the index.
    ///
    /// If the command already exists, updates its invocation data.
    /// If it's a new command, adds it to both the map and Nucleo.
    pub fn add_history(&self, history: &History) {
        if history.is_agent() {
            return;
        }
        if !self.shells.contains(history.shell.as_deref().unwrap_or_default()) {
            return;
        }

        let command = history.command.as_str();

        // DashMap with Arc<str> keys can be looked up with &str via Borrow trait
        if let Some(mut entry) = self.commands.get_mut(command) {
            // Existing command - just update invocations
            entry.add_invocation(history, &self.interner);
        } else {
            let mut haystack = self.haystack.write().unwrap();
            match self.commands.entry(Arc::from(command)) {
                dashmap::Entry::Occupied(mut entry) => {
                    entry.get_mut().add_invocation(history, &self.interner);
                }
                dashmap::Entry::Vacant(vacant) => {
                    let Some(data) = CommandData::new(history, haystack.len(), &self.interner)
                    else {
                        return; // Skip invalid commands
                    };
                    haystack.push(HaystackEntry::new(vacant.insert(data).key().clone()));
                }
            }
        }
        // Note: frecency_map is rebuilt by background task, not invalidated here
    }

    /// Add multiple history entries to the index.
    #[instrument(skip_all, level = tracing::Level::TRACE, name = "index_add_histories", fields(count = histories.len()))]
    pub fn add_histories(&self, histories: &[History]) {
        for history in histories {
            self.add_history(history);
        }
    }

    /// Get the number of unique commands in the index.
    pub fn command_count(&self) -> usize {
        self.commands.len()
    }

    /// Search for commands matching a query.
    ///
    /// Returns an iterator of history IDs as parsed UUIDs (most recent invocation per command).
    /// Uses precomputed global frecency for scoring if available.
    #[instrument(skip_all, level = tracing::Level::TRACE, name = "index_search", fields(query = %query))]
    pub fn search(
        &self,
        query: &str,
        filter_mode: &IndexFilterMode,
        limit: u32,
    ) -> impl Iterator<Item = [u8; 16]> {
        // Get precomputed frecency map (may be None if not yet computed)
        let frecency_map = self.frecency_map.read().unwrap().clone();

        let query = super::truncate_query(query);
        // Match accent-insensitively: the haystack side is normalized in
        // add_history, so an accented query must be normalized too
        let query = normalize_diacritics(query);

        let haystack = self.haystack.read().unwrap();
        let filter = filter_mode.compile(&self.interner);

        // Filter pre-pass: collect the candidate commands for this filter mode. This is sorted
        // vector of haystack indices.
        let get_candidates = || match &filter {
            CompiledFilter::All => haystack.iter().enumerate().map(|(i, _)| i as u32).collect(),
            CompiledFilter::Nothing => Vec::new(),
            _ => {
                let mut indices: Vec<u32> = self
                    .commands
                    .iter()
                    .filter(|entry| (entry.haystack_index as usize) < haystack.len())
                    .filter(|entry| match &filter {
                        CompiledFilter::All | CompiledFilter::Nothing => unreachable!(),
                        CompiledFilter::Directory(dir) => entry.has_invocation_in_dir(*dir),
                        CompiledFilter::Workspace(prefix) => {
                            entry.has_invocation_in_workspace(prefix, &self.interner)
                        }
                        CompiledFilter::Host(hostname) => entry.has_invocation_on_host(*hostname),
                        CompiledFilter::Session(session) => {
                            entry.has_invocation_in_session(session)
                        }
                    })
                    .map(|entry| entry.haystack_index)
                    .collect();
                indices.sort_unstable();
                indices
            }
        };

        let candidates =
            tracing::span!(Level::TRACE, "index_search_filter").in_scope(get_candidates);

        let candidate_frecency = |candidate_index: usize| {
            let hay_idx = candidates[candidate_index] as usize;
            frecency_map.as_ref().and_then(|f| f.get(hay_idx).copied()).unwrap_or(0)
        };

        let config = frizbee::Config::default()
            .casing(frizbee::CaseMatching::Smart)
            .sort(frizbee::SortStrategy::IndexAsc);
        let mut matcher = frizbee::Matcher::from_query(&query, &config);

        // An empty query matches every candidate with fuzzy score 0, so skip
        // the matcher and rank purely by frecency
        let mut scored: Vec<Score> = if matcher.patterns().is_empty() {
            (0..candidates.len())
                .map(|i| Score {
                    fuzzy_score: 0,
                    frecency: candidate_frecency(i),
                    index: i as u32,
                })
                .collect()
        } else {
            // This is a vec of `&Arc<str>` instead of `&str` because `&Arc<str>` is the size of one
            // pointer while `&str` is the size of two.
            let normalized_commands: Vec<&Arc<str>> =
                candidates.iter().map(|i| &haystack[*i as usize].normalized).collect();
            // Use all cores when the number of commands is sufficiently large.
            let threads = std::thread::available_parallelism().map_or(1, |n| n.get());
            let matches = tracing::span!(Level::TRACE, "index_search_match").in_scope(|| {
                if threads > 1 && normalized_commands.len() >= 10_000 {
                    matcher.match_list_parallel(&normalized_commands, threads)
                } else {
                    matcher.match_list(&normalized_commands)
                }
            });
            matches
                .iter()
                .map(|m| Score {
                    fuzzy_score: m.score,
                    frecency: candidate_frecency(m.index as usize),
                    index: m.index,
                })
                .collect()
        };

        tracing::span!(Level::TRACE, "index_search_results").in_scope(|| {
            // only the top `limit` results are returned, so partition them
            // out before sorting instead of sorting every match
            let limit = limit as usize;
            if scored.len() > limit {
                scored.select_nth_unstable(limit);
                scored.truncate(limit);
            }
            scored.sort_unstable();
            scored.into_iter().filter_map(move |score| {
                let haystack_index = candidates[score.index as usize];
                self.commands
                    .get(haystack[haystack_index as usize].original.as_ref())
                    .map(|data| data.most_recent_id())
            })
        })
    }

    /// Rebuild the global frecency map.
    ///
    /// This should be called by a background task periodically.
    /// The map is used for scoring search results.
    ///
    /// Uses multipliers from search settings:
    /// - `recency_score_multiplier`: Weight for recency component
    /// - `frequency_score_multiplier`: Weight for frequency component
    /// - `frecency_score_multiplier`: Overall multiplier for final score
    #[instrument(skip_all, level = tracing::Level::DEBUG, name = "rebuild_frecency")]
    pub fn rebuild_frecency(&self, search_settings: &Search) {
        let now = OffsetDateTime::now_utc().unix_timestamp();

        // Clamp multipliers to non-negative values to prevent broken frecency ranking
        // (negative values would produce unexpected results when cast to u32)
        let recency_mul = search_settings.recency_score_multiplier.max(0.0);
        let frequency_mul = search_settings.frequency_score_multiplier.max(0.0);
        let frecency_mul = search_settings.frecency_score_multiplier.max(0.0);

        // Aligned with `haystack` by index; see FrecencyMap
        let frecencies: Vec<u32> = {
            let haystack = self.haystack.read().unwrap();
            haystack
                .iter()
                .map(|hay| {
                    self.commands.get(hay.original.as_ref()).map_or(0, |data| {
                        let frecency =
                            data.global_frecency.compute(now, recency_mul, frequency_mul);
                        // Apply overall frecency multiplier and round to u32
                        (f64::from(frecency) * frecency_mul).round() as u32
                    })
                })
                .collect()
        };

        *self.frecency_map.write().unwrap() = Some(Arc::new(frecencies));
    }
}

impl Default for SearchIndex {
    fn default() -> Self {
        Self::new(OrFilter::all())
    }
}

#[cfg(test)]
mod tests {
    use rstest::rstest;
    use time::macros::datetime;

    use super::*;

    fn make_history(command: &str, cwd: &str, timestamp: OffsetDateTime) -> History {
        History::import().timestamp(timestamp).command(command).cwd(cwd).build().into()
    }

    #[test]
    fn frecency_data_compute() {
        let now = 1000000i64;

        // Recent command (with default multipliers of 1.0)
        let recent = FrecencyData {
            count: 5,
            last_used: now - 60, // 1 minute ago
        };
        assert!(recent.compute(now, 1.0, 1.0) > 100); // High score

        // Old command
        let old = FrecencyData {
            count: 5,
            last_used: now - 86400 * 30, // 30 days ago
        };
        assert!(old.compute(now, 1.0, 1.0) < recent.compute(now, 1.0, 1.0));

        // Frequently used old command
        let frequent_old = FrecencyData {
            count: 100,
            last_used: now - 86400 * 7, // 1 week ago
        };
        // Should still have decent score due to frequency
        assert!(frequent_old.compute(now, 1.0, 1.0) > 50);
    }

    #[test]
    fn frecency_data_compute_with_multipliers() {
        let now = 1000000i64;

        let data = FrecencyData {
            count: 5,
            last_used: now - 60, // 1 minute ago (recency_score = 100)
        };

        // Default multipliers (1.0, 1.0)
        let default_score = data.compute(now, 1.0, 1.0);

        // Double recency weight
        let double_recency = data.compute(now, 2.0, 1.0);
        assert!(double_recency > default_score);

        // Double frequency weight
        let double_frequency = data.compute(now, 1.0, 2.0);
        assert!(double_frequency > default_score);

        // Zero out recency (only frequency counts)
        let no_recency = data.compute(now, 0.0, 1.0);
        assert!(no_recency < default_score);

        // Zero out frequency (only recency counts)
        let no_frequency = data.compute(now, 1.0, 0.0);
        assert!(no_frequency < default_score);

        // Zero both (should be zero)
        let no_score = data.compute(now, 0.0, 0.0);
        assert_eq!(no_score, 0);

        // Fractional multipliers
        let half_recency = data.compute(now, 0.5, 1.0);
        assert!(half_recency < default_score);
        assert!(half_recency > no_recency);

        // 1.5x multiplier
        let boost_recency = data.compute(now, 1.5, 1.0);
        assert!(boost_recency > default_score);
        assert!(boost_recency < double_recency);
    }

    #[test]
    fn command_data_add_invocation() {
        let interner = ThreadedRodeo::new();

        let (dir1, dir2) = if cfg!(windows) {
            ("C:\\Users\\User\\project", "C:\\Users\\User\\other")
        } else {
            ("/home/user/project", "/home/user/other")
        };

        let history1 = make_history("git status", dir1, datetime!(2024-01-01 10:00 UTC));
        let history2 = make_history("git status", dir2, datetime!(2024-01-01 12:00 UTC));

        let mut data = CommandData::new(&history1, 0, &interner).unwrap();
        assert_eq!(data.global_frecency.count, 1);
        let id1 = data.most_recent_id();

        data.add_invocation(&history2, &interner);
        assert_eq!(data.global_frecency.count, 2);

        // Most recent ID should update to history2 (newer timestamp)
        let id2 = data.most_recent_id();
        assert_ne!(id1, id2);
    }

    #[test]
    fn command_data_filters() {
        let interner = ThreadedRodeo::new();

        let (dir1, dir2) = if cfg!(windows) {
            ("C:\\Users\\User\\project", "C:\\Users\\User\\other")
        } else {
            ("/home/user/project", "/home/user/other")
        };

        let h1 = make_history("git status", dir1, datetime!(2024-01-01 10:00 UTC));
        let h2 = make_history("git status", dir2, datetime!(2024-01-01 12:00 UTC));

        let mut data = CommandData::new(&h1, 0, &interner).unwrap();
        data.add_invocation(&h2, &interner);

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

    #[test]
    fn search_index_add_and_search() {
        let index = SearchIndex::default();

        let h1 = make_history("git status", "/home/user/project", datetime!(2024-01-01 10:00 UTC));
        let h2 = make_history(
            "git commit -m 'test'",
            "/home/user/project",
            datetime!(2024-01-01 10:05 UTC),
        );
        let h3 = make_history("ls -la", "/home/user/other", datetime!(2024-01-01 10:10 UTC));

        index.add_history(&h1);
        index.add_history(&h2);
        index.add_history(&h3);

        assert_eq!(index.command_count(), 3);

        // Search for "git" - should match 2 commands
        assert_eq!(index.search("git", &IndexFilterMode::Global, 10).count(), 2);

        // Search with directory filter
        // git status and git commit
        let count = index
            .search(
                "",
                &IndexFilterMode::Directory(
                    "/home/user/project".display_rich().trailing_slash(true).to_string(),
                ),
                10,
            )
            .count();
        assert_eq!(count, 2);
    }

    /// Regression test for #3702: a frequently-run command whose match is
    /// scattered across words must not outrank a contiguous match, no matter
    /// how large its frecency score is.
    #[test]
    fn contiguous_match_beats_frequent_scattered_match() {
        let index = SearchIndex::default();

        // contiguous match for "foo bar", run once
        let contiguous = make_history("foo bar --baz", "/tmp", datetime!(2024-01-01 10:00 UTC));
        index.add_history(&contiguous);

        // scattered match (b..a..r spread over "build-analyzer-report"), run
        // 200 times so its frecency dwarfs any fuzzy score difference
        for _ in 0..200 {
            let h =
                make_history("foo build-analyzer-report", "/tmp", datetime!(2024-01-01 10:00 UTC));
            index.add_history(&h);
        }

        index.rebuild_frecency(&Search::default());

        let results: Vec<_> = index.search("foo bar", &IndexFilterMode::Global, 10).collect();
        assert_eq!(results.len(), 2);
        assert_eq!(
            results[0],
            index.commands.get("foo bar --baz").unwrap().most_recent_id(),
            "contiguous match must rank above the high-frecency scattered match"
        );
    }

    /// Frecency still orders results between equally good matches, so
    /// most-recently/frequently-used behavior is preserved where match
    /// quality can't differentiate.
    #[test]
    fn equal_matches_order_by_frecency() {
        let index = SearchIndex::default();

        index.add_history(&make_history("echo alpha", "/tmp", datetime!(2024-01-01 10:00 UTC)));
        // same fuzzy score for the query, much higher frecency
        for _ in 0..50 {
            let h = make_history("echo beta", "/tmp", datetime!(2024-01-01 10:00 UTC));
            index.add_history(&h);
        }

        index.rebuild_frecency(&Search::default());

        let results: Vec<_> = index.search("echo", &IndexFilterMode::Global, 10).collect();
        assert_eq!(results.len(), 2);
        assert_eq!(
            results[0],
            index.commands.get("echo beta").unwrap().most_recent_id(),
            "ties in match quality should be broken by frecency"
        );
    }

    /// Regression test: frizbee does no unicode normalization of its own, so
    /// the index normalizes both sides — an unaccented query must keep
    /// matching accented history entries (nucleo's old Normalization::Smart
    /// behavior), and an accented query must still find its own entry.
    #[test]
    fn diacritics_normalized_for_matching() {
        let index = SearchIndex::default();

        index.add_history(&make_history("echo déjà-vu", "/tmp", datetime!(2024-01-01 10:00 UTC)));
        index.add_history(&make_history("echo plain", "/tmp", datetime!(2024-01-01 10:00 UTC)));

        let expected = index.commands.get("echo déjà-vu").unwrap().most_recent_id();

        let results: Vec<_> = index.search("deja", &IndexFilterMode::Global, 10).collect();
        assert_eq!(results, vec![expected]);

        let results: Vec<_> = index.search("déjà", &IndexFilterMode::Global, 10).collect();
        assert_eq!(results, vec![expected]);
    }

    /// A deterministic synthetic corpus large enough to cross the 10k
    /// parallel-matching threshold, shaped like real history (repeated
    /// prefixes, multi-word commands, a few accented entries).
    fn equivalence_corpus() -> Vec<String> {
        let prefixes = [
            "git status",
            "git push origin",
            "cargo build --release",
            "docker compose up",
            "ls -la",
            "echo déjà",
            "kubectl get pods -n",
            "make -j",
        ];
        (0..12_000).map(|i| format!("{} run-{i}", prefixes[i % prefixes.len()])).collect()
    }

    /// The parallel-matching switch must be invisible in the results:
    /// frizbee's match_list_parallel must return exactly the same matches,
    /// scores, and order as match_list for our config. Guards the 10k
    /// threshold in search(), which machines cross as history grows.
    #[test]
    fn parallel_matching_equals_serial() {
        let corpus = equivalence_corpus();
        let haystack: Vec<&str> = corpus.iter().map(String::as_str).collect();
        let config = frizbee::Config::default()
            .casing(frizbee::CaseMatching::Smart)
            .sort(frizbee::SortStrategy::IndexAsc);

        // multi-atom and negated atoms exercise the progressive-filter path
        for query in ["git", "git p", "docker compose up", "cargo !release", "g"] {
            let serial = frizbee::Matcher::from_query(query, &config).match_list(&haystack);
            for threads in [2, 8] {
                let parallel = frizbee::Matcher::from_query(query, &config)
                    .match_list_parallel(&haystack, threads);
                assert_eq!(serial.len(), parallel.len(), "query {query:?}");
                for (s, p) in serial.iter().zip(&parallel) {
                    assert_eq!(
                        (s.index, s.score),
                        (p.index, p.score),
                        "query {query:?}, {threads} threads"
                    );
                }
            }
        }
    }

    /// End-to-end determinism above the parallel threshold: the same query
    /// against the same index must return the same ranked IDs every time.
    #[test]
    fn search_results_stable_above_parallel_threshold() {
        let index = SearchIndex::default();
        for (i, command) in equivalence_corpus().iter().enumerate() {
            // spread timestamps so frecency scores actually differ
            let ts = datetime!(2024-01-01 00:00 UTC) + time::Duration::minutes((i % 1440) as i64);
            index.add_history(&make_history(command, "/tmp", ts));
        }
        assert!(index.command_count() > 10_000, "corpus must cross threshold");
        index.rebuild_frecency(&Search::default());

        for query in ["git", "git p", "docker compose up", "deja", ""] {
            let first: Vec<_> = index.search(query, &IndexFilterMode::Global, 200).collect();
            for _ in 0..2 {
                let again: Vec<_> = index.search(query, &IndexFilterMode::Global, 200).collect();
                assert_eq!(first, again, "query {query:?} returned unstable results");
            }
        }
    }

    /// Queries longer than frizbee can score are truncated instead of
    /// panicking in Matcher::from_query.
    #[test]
    fn long_query_truncated_not_panicking() {
        let index = SearchIndex::default();
        index.add_history(&make_history("echo hello", "/tmp", datetime!(2024-01-01 10:00 UTC)));

        let long_query = "a".repeat(5000);
        assert!(index.search(&long_query, &IndexFilterMode::Global, 10).next().is_none());
    }

    #[rstest]
    #[case::all(&[], 7)]
    #[case::bash(&["bash"], 1)]
    #[case::bash_unknown(&["bash", ""], 5)]
    #[case::bash_zsh(&["bash", "zsh"], 3)]
    #[case::unknown(&[""], 4)]
    #[case::fish(&["fish"], 0)]
    #[case::fish_unknown(&["fish", ""], 4)]
    fn search_with_shell_filter(#[case] shells: &[&str], #[case] expected_count: usize) {
        let filter =
            OrFilter::from_list(shells.iter().map(|s| (*s).to_owned()).collect::<Vec<_>>())
                .unwrap_or_default();
        let index = SearchIndex::new(filter);

        for (command, shell) in [
            ("echo unknown1", None),
            ("echo zsh1", Some("zsh")),
            ("echo unknown2", None),
            ("echo bash", Some("bash")),
            ("echo unknown3", None),
            ("echo unknown4", None),
            ("echo zsh2", Some("zsh")),
        ] {
            let mut history = make_history(command, "/tmp", OffsetDateTime::now_utc());
            history.shell = shell.map(str::to_owned);
            index.add_history(&history);
        }

        let results: Vec<_> = index.search("echo", &IndexFilterMode::Global, 100).collect();
        assert_eq!(results.len(), expected_count, "{results:?}");
    }
}
