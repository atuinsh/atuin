use atuin_common::utils::normalize_optional_string;
use atuin_domain::record::CmdOrigin;
use typed_builder::TypedBuilder;

use super::{AuthorKind, History, is_known_agent};

/// Builder for a history entry that is imported from shell history.
///
/// The only two required fields are `timestamp` and `command`.
#[derive(Debug, Clone, TypedBuilder)]
pub struct HistoryImported {
    timestamp: time::OffsetDateTime,
    #[builder(setter(into))]
    command: String,
    #[builder(default = "unknown".into(), setter(into))]
    cwd: String,
    #[builder(default = Self::DEFAULT_EXIT)]
    exit: i64,
    #[builder(default = Self::DEFAULT_DURATION)]
    duration: i64,
    #[builder(default, setter(strip_option, into))]
    session: Option<String>,
    #[builder(default, setter(strip_option, into))]
    cmd_origin: Option<CmdOrigin>,
    #[builder(default, setter(strip_option, into))]
    author: Option<String>,
    #[builder(default, setter(strip_option, into))]
    intent: Option<String>,
    #[builder(default, setter(strip_option, into))]
    shell: Option<String>,
    #[builder(default)]
    author_kind: Option<AuthorKind>,
}

impl HistoryImported {
    pub const DEFAULT_EXIT: i64 = -1;
    pub const DEFAULT_DURATION: i64 = -1;
}

impl From<HistoryImported> for History {
    fn from(imported: HistoryImported) -> Self {
        Self::new(
            imported.timestamp,
            imported.command,
            imported.cwd,
            imported.exit,
            imported.duration,
            imported.session,
            imported.cmd_origin,
            imported.author,
            imported.intent,
            None,
            imported.shell,
            imported.author_kind,
        )
    }
}

/// Builder for a history entry that is captured via hook.
///
/// This builder is used only at the `start` step of the hook,
/// so it doesn't have any fields which are known only after
/// the command is finished, such as `exit` or `duration`.
#[derive(Debug, Clone, TypedBuilder)]
#[builder(field_defaults(setter(strip_option(ignore_invalid, fallback_suffix = "_opt"))))]
pub struct HistoryCaptured {
    timestamp: time::OffsetDateTime,
    #[builder(setter(into))]
    command: String,
    #[builder(setter(into))]
    cwd: String,
    #[builder(default, setter(into))]
    author: Option<String>,
    #[builder(default, setter(into))]
    cmd_origin: Option<CmdOrigin>,
    #[builder(default, setter(into))]
    intent: Option<String>,
    #[builder(default, setter(into))]
    shell: Option<String>,
    #[builder(default)]
    author_kind: Option<AuthorKind>,
}

impl From<HistoryCaptured> for History {
    fn from(captured: HistoryCaptured) -> Self {
        // Only agent integrations state an author; humans never do. That makes a stated
        // known-agent name a far stronger signal that an agent ran this than anything we can
        // infer after the fact — unless it is also the current username, which says nothing
        // (see [`History::is_agent`]). An explicit kind still wins.
        let author = normalize_optional_string(captured.author);
        let cmd_origin = captured.cmd_origin.unwrap_or_else(CmdOrigin::probe_current);
        let author_kind = captured.author_kind.or_else(|| {
            author
                .as_deref()
                .is_some_and(|author| {
                    is_known_agent(author) && author != cmd_origin.user().into_inner()
                })
                .then_some(AuthorKind::Agent)
        });

        Self::new(
            captured.timestamp,
            captured.command,
            captured.cwd,
            -1,
            -1,
            None,
            Some(cmd_origin),
            author,
            captured.intent,
            None,
            captured.shell,
            author_kind,
        )
    }
}

/// Builder for a history entry that is loaded from the database.
///
/// All fields are required, as they are all present in the database.
#[derive(Debug, Clone, TypedBuilder)]
pub struct HistoryFromDb {
    id: String,
    timestamp: time::OffsetDateTime,
    command: String,
    cwd: String,
    exit: i64,
    duration: i64,
    session: String,
    hostname: String,
    author: String,
    intent: Option<String>,
    deleted_at: Option<time::OffsetDateTime>,
    shell: Option<String>,
    author_kind: Option<AuthorKind>,
}

impl From<HistoryFromDb> for History {
    // Reads a `hostname` column that predates the strict `host:user` format.
    fn from(from_db: HistoryFromDb) -> Self {
        Self {
            id: from_db.id.into(),
            timestamp: from_db.timestamp,
            exit: from_db.exit,
            command: from_db.command,
            cwd: from_db.cwd,
            duration: from_db.duration,
            session: from_db.session,
            #[allow(deprecated)]
            cmd_origin: CmdOrigin::parse_lenient(from_db.hostname),
            author: from_db.author,
            intent: from_db.intent,
            deleted_at: from_db.deleted_at,
            shell: from_db.shell,
            author_kind: from_db.author_kind,
        }
    }
}

/// Builder for a history entry that is captured via hook and sent to the daemon
///
/// This builder is similar to Capture, but we just require more information up front.
/// For the old setup, we could just rely on History::new to read some of the missing
/// data. This is no longer the case.
#[derive(Debug, Clone, TypedBuilder)]
pub struct HistoryDaemonCapture {
    timestamp: time::OffsetDateTime,
    #[builder(setter(into))]
    command: String,
    #[builder(setter(into))]
    cwd: String,
    #[builder(setter(into))]
    session: String,
    #[builder(setter(into))]
    cmd_origin: CmdOrigin,
    #[builder(default, setter(strip_option, into))]
    author: Option<String>,
    #[builder(default, setter(strip_option, into))]
    intent: Option<String>,
    #[builder(default, setter(strip_option, into))]
    shell: Option<String>,
    #[builder(default)]
    author_kind: Option<AuthorKind>,
}

impl From<HistoryDaemonCapture> for History {
    fn from(captured: HistoryDaemonCapture) -> Self {
        Self::new(
            captured.timestamp,
            captured.command,
            captured.cwd,
            -1,
            -1,
            Some(captured.session),
            Some(captured.cmd_origin),
            captured.author,
            captured.intent,
            None,
            captured.shell,
            captured.author_kind,
        )
    }
}
