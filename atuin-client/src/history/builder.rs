use chrono::Utc;
use typed_builder::TypedBuilder;

use super::History;

/// Builder for a history entry that is imported from shell history.
///
/// The only two required fields are `timestamp` and `command`.
#[derive(Debug, Clone, TypedBuilder)]
pub struct HistoryImported {
    timestamp: chrono::DateTime<Utc>,
    #[builder(setter(into))]
    command: String,
    #[builder(default = "unknown".into(), setter(into))]
    cwd: String,
    #[builder(default = -1)]
    exit: i64,
    #[builder(default = -1)]
    duration: i64,
    #[builder(default, setter(strip_option, into))]
    session: Option<String>,
    #[builder(default, setter(strip_option, into))]
    hostname: Option<String>,
}

impl From<HistoryImported> for History {
    fn from(imported: HistoryImported) -> Self {
        History::new(
            imported.timestamp,
            imported.command,
            imported.cwd,
            imported.exit,
            imported.duration,
            imported.session,
            imported.hostname,
            None,
        )
    }
}

/// Builder for a history entry that is captured via hook.
///
/// This builder is used only at the `start` step of the hook,
/// so it doesn't have any fields which are known only after
/// the command is finished, such as `exit` or `duration`.
#[derive(Debug, Clone, TypedBuilder)]
pub struct HistoryCaptured {
    timestamp: chrono::DateTime<Utc>,
    #[builder(setter(into))]
    command: String,
    #[builder(setter(into))]
    cwd: String,
}

impl From<HistoryCaptured> for History {
    fn from(captured: HistoryCaptured) -> Self {
        History::new(
            captured.timestamp,
            captured.command,
            captured.cwd,
            -1,
            -1,
            None,
            None,
            None,
        )
    }
}

/// Builder for a history entry that is loaded from the database.
///
/// All fields are required, as they are all present in the database.
#[derive(Debug, Clone, TypedBuilder)]
pub struct HistoryFromDb {
    id: String,
    timestamp: chrono::DateTime<Utc>,
    command: String,
    cwd: String,
    exit: i64,
    duration: i64,
    session: String,
    hostname: String,
    deleted_at: Option<chrono::DateTime<Utc>>,
}

impl From<HistoryFromDb> for History {
    fn from(from_db: HistoryFromDb) -> Self {
        History {
            id: from_db.id,
            timestamp: from_db.timestamp,
            exit: from_db.exit,
            command: from_db.command,
            cwd: from_db.cwd,
            duration: from_db.duration,
            session: from_db.session,
            hostname: from_db.hostname,
            deleted_at: from_db.deleted_at,
        }
    }
}
