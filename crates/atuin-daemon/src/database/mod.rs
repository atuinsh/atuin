//! Database module for the daemon gRPC storage service.
//!
//! This exposes the `atuin_client::database::Database` surface over gRPC so
//! the CLI never has to open the local SQLite database itself — the daemon is
//! the sole owner of on-disk history storage.
//!
//! This module contains the proto-generated types plus conversions to and from
//! the corresponding `atuin_client` types.

use std::path::PathBuf;

use atuin_client::{
    database::{Context as DbContext, OptFilters},
    history::{History, HistoryId},
    settings::{FilterMode as DbFilterMode, SearchMode as DbSearchMode},
};
use time::OffsetDateTime;

// Include the generated proto code
tonic::include_proto!("database");

fn ts_to_nanos(ts: OffsetDateTime) -> u64 {
    ts.unix_timestamp_nanos().max(0) as u64
}

fn nanos_to_ts(nanos: u64) -> OffsetDateTime {
    OffsetDateTime::from_unix_timestamp_nanos(nanos as i128)
        .unwrap_or(OffsetDateTime::UNIX_EPOCH)
}

impl From<History> for HistoryRecord {
    fn from(h: History) -> Self {
        Self {
            id: h.id.0,
            timestamp: ts_to_nanos(h.timestamp),
            duration: h.duration,
            exit: h.exit,
            command: h.command,
            cwd: h.cwd,
            session: h.session,
            hostname: h.hostname,
            author: h.author,
            intent: h.intent,
            deleted_at: h.deleted_at.map(ts_to_nanos),
        }
    }
}

impl From<HistoryRecord> for History {
    fn from(r: HistoryRecord) -> Self {
        Self {
            id: HistoryId(r.id),
            timestamp: nanos_to_ts(r.timestamp),
            duration: r.duration,
            exit: r.exit,
            command: r.command,
            cwd: r.cwd,
            session: r.session,
            hostname: r.hostname,
            author: r.author,
            intent: r.intent,
            deleted_at: r.deleted_at.map(nanos_to_ts),
        }
    }
}

impl From<DbContext> for Context {
    fn from(c: DbContext) -> Self {
        Self {
            session: c.session,
            cwd: c.cwd,
            hostname: c.hostname,
            host_id: c.host_id,
            git_root: c.git_root.map(|p| p.to_string_lossy().into_owned()),
        }
    }
}

impl From<Context> for DbContext {
    fn from(c: Context) -> Self {
        Self {
            session: c.session,
            cwd: c.cwd,
            hostname: c.hostname,
            host_id: c.host_id,
            git_root: c.git_root.map(PathBuf::from),
        }
    }
}

impl From<OptFilters> for OptFiltersMsg {
    fn from(o: OptFilters) -> Self {
        Self {
            exit: o.exit,
            exclude_exit: o.exclude_exit,
            only_failed: o.only_failed,
            cwd: o.cwd,
            exclude_cwd: o.exclude_cwd,
            before: o.before,
            after: o.after,
            limit: o.limit,
            offset: o.offset,
            reverse: o.reverse,
            include_duplicates: o.include_duplicates,
            authors: o.authors,
        }
    }
}

impl From<OptFiltersMsg> for OptFilters {
    fn from(o: OptFiltersMsg) -> Self {
        Self {
            exit: o.exit,
            exclude_exit: o.exclude_exit,
            only_failed: o.only_failed,
            cwd: o.cwd,
            exclude_cwd: o.exclude_cwd,
            before: o.before,
            after: o.after,
            limit: o.limit,
            offset: o.offset,
            reverse: o.reverse,
            include_duplicates: o.include_duplicates,
            authors: o.authors,
        }
    }
}

impl From<DbSearchMode> for SearchMode {
    fn from(m: DbSearchMode) -> Self {
        match m {
            DbSearchMode::Prefix => SearchMode::Prefix,
            DbSearchMode::FullText => SearchMode::FullText,
            DbSearchMode::Fuzzy => SearchMode::Fuzzy,
            DbSearchMode::Skim => SearchMode::Skim,
            DbSearchMode::DaemonFuzzy => SearchMode::DaemonFuzzy,
        }
    }
}

impl From<SearchMode> for DbSearchMode {
    fn from(m: SearchMode) -> Self {
        match m {
            SearchMode::Prefix => DbSearchMode::Prefix,
            SearchMode::FullText => DbSearchMode::FullText,
            SearchMode::Fuzzy => DbSearchMode::Fuzzy,
            SearchMode::Skim => DbSearchMode::Skim,
            SearchMode::DaemonFuzzy => DbSearchMode::DaemonFuzzy,
        }
    }
}

impl From<DbFilterMode> for FilterMode {
    fn from(m: DbFilterMode) -> Self {
        match m {
            DbFilterMode::Global => FilterMode::Global,
            DbFilterMode::Host => FilterMode::Host,
            DbFilterMode::Session => FilterMode::Session,
            DbFilterMode::Directory => FilterMode::Directory,
            DbFilterMode::Workspace => FilterMode::Workspace,
            DbFilterMode::SessionPreload => FilterMode::SessionPreload,
        }
    }
}

impl From<FilterMode> for DbFilterMode {
    fn from(m: FilterMode) -> Self {
        match m {
            FilterMode::Global => DbFilterMode::Global,
            FilterMode::Host => DbFilterMode::Host,
            FilterMode::Session => DbFilterMode::Session,
            FilterMode::Directory => DbFilterMode::Directory,
            FilterMode::Workspace => DbFilterMode::Workspace,
            FilterMode::SessionPreload => DbFilterMode::SessionPreload,
        }
    }
}

impl Records {
    pub fn into_history(self) -> Vec<History> {
        self.records.into_iter().map(History::from).collect()
    }

    pub fn from_history(history: Vec<History>) -> Self {
        Self {
            records: history.into_iter().map(HistoryRecord::from).collect(),
        }
    }
}
