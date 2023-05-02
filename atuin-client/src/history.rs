use std::env;

use chrono::Utc;
use serde::{Deserialize, Serialize};

use atuin_common::utils::uuid_v7;

mod builder;

/// A marker type used to seal the `History` struct, preventing it from being constructed directly.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub(self) struct HistorySeal;

/// Client-side history entry.
///
/// Client stores data unencrypted, and only encrypts it before sending to the server.
///
/// To create a new history entry, use one of the builders:
/// - [`History::import()`] to import an entry from the shell history file
/// - [`History::capture()`] to capture an entry via hook
/// - [`History::from_db()`] to create an instance from the database entry
///
/// ### Caution
/// Any new fields MUST be `Optional<T>` and marked with `#[serde(default)]` to ensure backwards
/// compatibility with older clients.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, sqlx::FromRow)]
pub struct History {
    /// A client-generated ID, used to identify the entry when syncing.
    ///
    /// Stored as `client_id` in the database.
    pub id: String,
    /// When the command was run.
    pub timestamp: chrono::DateTime<Utc>,
    /// How long the command took to run.
    pub duration: i64,
    /// The exit code of the command.
    pub exit: i64,
    /// The command that was run.
    pub command: String,
    /// The current working directory when the command was run.
    pub cwd: String,
    /// The session ID, associated with a terminal session.
    pub session: String,
    /// The hostname of the machine the command was run on.
    pub hostname: String,

    /// Timestamp, which is set when the entry is deleted, allowing a soft delete.
    #[serde(default)]
    pub deleted_at: Option<chrono::DateTime<Utc>>,

    /// Having this seal marker here we're ensuring that `History`
    /// can only be constructed directly by the [`crate::history`] module.
    ///
    /// All users of `History` must use builders, such as
    /// [`History::import()`], [`History::from_db()`] or [`History::capture()`].
    #[doc(hidden)]
    #[serde(skip)]
    _seal: HistorySeal,
}

impl History {
    #[allow(clippy::too_many_arguments)]
    fn new(
        timestamp: chrono::DateTime<Utc>,
        command: String,
        cwd: String,
        exit: i64,
        duration: i64,
        session: Option<String>,
        hostname: Option<String>,
        deleted_at: Option<chrono::DateTime<Utc>>,
    ) -> Self {
        let session = session
            .or_else(|| env::var("ATUIN_SESSION").ok())
            .unwrap_or_else(|| uuid_v7().as_simple().to_string());
        let hostname = hostname.unwrap_or_else(|| {
            format!(
                "{}:{}",
                env::var("ATUIN_HOST_NAME").unwrap_or_else(|_| whoami::hostname()),
                env::var("ATUIN_HOST_USER").unwrap_or_else(|_| whoami::username())
            )
        });

        Self {
            id: uuid_v7().as_simple().to_string(),
            timestamp,
            command,
            cwd,
            exit,
            duration,
            session,
            hostname,
            deleted_at,
            _seal: HistorySeal,
        }
    }

    /// Builder for a history entry that is imported from shell history.
    ///
    /// The only two required fields are `timestamp` and `command`.
    ///
    /// ## Examples
    /// ```
    /// use atuin_client::history::History;
    ///
    /// let history: History = History::import()
    ///     .timestamp(chrono::Utc::now())
    ///     .command("ls -la")
    ///     .build()
    ///     .into();
    /// ```
    ///
    /// If shell history contains more information, it can be added to the builder:
    /// ```
    /// use atuin_client::history::History;
    ///
    /// let history: History = History::import()
    ///     .timestamp(chrono::Utc::now())
    ///     .command("ls -la")
    ///     .cwd("/home/user")
    ///     .exit(0)
    ///     .duration(100)
    ///     .build()
    ///     .into();
    /// ```
    ///
    /// Unknown command or command without timestamp cannot be imported, which
    /// is forced at compile time:
    ///
    /// ```compile_fail
    /// use atuin_client::history::History;
    ///
    /// // this will not compile because timestamp is missing
    /// let history: History = History::import()
    ///     .command("ls -la")
    ///     .build()
    ///     .into();
    /// ```
    pub fn import() -> builder::HistoryImportedBuilder {
        builder::HistoryImported::builder()
    }

    /// Builder for a history entry that is captured via hook.
    ///
    /// This builder is used only at the `start` step of the hook,
    /// so it doesn't have any fields which are known only after
    /// the command is finished, such as `exit` or `duration`.
    ///
    /// ## Examples
    /// ```rust
    /// use atuin_client::history::History;
    ///
    /// let history: History = History::capture()
    ///     .timestamp(chrono::Utc::now())
    ///     .command("ls -la")
    ///     .cwd("/home/user")
    ///     .build()
    ///     .into();
    /// ```
    ///
    /// Command without any required info cannot be captured, which is forced at compile time:
    ///
    /// ```compile_fail
    /// use atuin_client::history::History;
    ///
    /// // this will not compile because `cwd` is missing
    /// let history: History = History::capture()
    ///     .timestamp(chrono::Utc::now())
    ///     .command("ls -la")
    ///     .build()
    ///     .into();
    /// ```
    pub fn capture() -> builder::HistoryCapturedBuilder {
        builder::HistoryCaptured::builder()
    }

    /// Builder for a history entry that is imported from the database.
    ///
    /// All fields are required, as they are all present in the database.
    ///
    /// ```compile_fail
    /// use atuin_client::history::History;
    ///
    /// // this will not compile because `id` field is missing
    /// let history: History = History::from_db()
    ///     .timestamp(chrono::Utc::now())
    ///     .command("ls -la".to_string())
    ///     .cwd("/home/user".to_string())
    ///     .exit(0)
    ///     .duration(100)
    ///     .session("somesession".to_string())
    ///     .hostname("localhost".to_string())
    ///     .deleted_at(None)
    ///     .build()
    ///     .into();
    /// ```
    pub fn from_db() -> builder::HistoryFromDbBuilder {
        builder::HistoryFromDb::builder()
    }

    pub fn success(&self) -> bool {
        self.exit == 0 || self.duration == -1
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // @utter-step:
    // left it here just to show that it
    // deserializing HistoryOld into History is not a problem
    #[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, sqlx::FromRow)]
    pub struct HistoryOld {
        pub id: String,
        pub timestamp: chrono::DateTime<Utc>,
        pub duration: i64,
        pub exit: i64,
        pub command: String,
        pub cwd: String,
        pub session: String,
        pub hostname: String,
    }

    #[test]
    fn test_backwards_compatibility() {
        // left it here just to show that it
        // deserializing HistoryOld into History is not a problem with
        // #[serde(default)] attribute set in History

        #[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
        pub struct HistoryOld {
            id: String,
            timestamp: chrono::DateTime<Utc>,
            duration: i64,
            exit: i64,
            command: String,
            cwd: String,
            session: String,
            hostname: String,
        }

        let history_without_delete = HistoryOld {
            id: "test".to_string(),
            timestamp: chrono::Utc::now(),
            duration: 0,
            exit: 0,
            command: "test".to_string(),
            cwd: "test".to_string(),
            session: "test".to_string(),
            hostname: "test".to_string(),
        };

        let serialized = rmp_serde::to_vec(&history_without_delete).expect("Failed to serialize");
        let deserialized: History =
            rmp_serde::from_slice(&serialized).expect("Failed to deserialize");

        assert!(deserialized.deleted_at.is_none());
    }

    #[test]
    #[should_panic = "Failed to deserialize: LengthMismatch(8)"]
    fn test_forwards_compatibility_with_rmp() {
        // this test specifies that currently old clients are failing to deserialize History objects
        // from newer versions, using the rmp_serde crate.

        // This should not be a problem with self-describing messages, such as JSON,
        // so we should consider switching to JSON/other self-describing formats in the future.
        #[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
        struct HistoryOld {
            id: String,
            timestamp: chrono::DateTime<Utc>,
            duration: i64,
            exit: i64,
            command: String,
            cwd: String,
            session: String,
            hostname: String,
        }

        let history = History {
            id: "test".to_string(),
            timestamp: chrono::Utc::now(),
            duration: 0,
            exit: 0,
            command: "test".to_string(),
            cwd: "test".to_string(),
            session: "test".to_string(),
            hostname: "test".to_string(),
            deleted_at: None,
            _seal: HistorySeal,
        };

        let serialized = rmp_serde::to_vec(&history).expect("Failed to serialize");
        // this will panic
        let _deserialized: HistoryOld =
            rmp_serde::from_slice(&serialized).expect("Failed to deserialize");
    }
}
