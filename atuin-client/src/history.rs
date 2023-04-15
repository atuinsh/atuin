use std::env;

use chrono::Utc;
use serde::{Deserialize, Serialize};

use atuin_common::utils::uuid_v7;

/// Client-side history entry.
///
/// Client stores data unencrypted, and only encrypts it before sending to the server.
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
    #[serde(default)]
    /// Timestamp, which is set when the entry is deleted, allowing a soft delete.
    pub deleted_at: Option<chrono::DateTime<Utc>>,
}

impl History {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
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
        let hostname =
            hostname.unwrap_or_else(|| format!("{}:{}", whoami::hostname(), whoami::username()));

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
        }
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
    // deserializing HistoryWithoutDelete into History is not a problem
    #[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, sqlx::FromRow)]
    pub struct HistoryWithoutDelete {
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
        // deserializing HistoryWithoutDelete into History is not a problem with
        // #[serde(default)] attribute set in History

        #[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
        pub struct HistoryWithoutDelete {
            id: String,
            timestamp: chrono::DateTime<Utc>,
            duration: i64,
            exit: i64,
            command: String,
            cwd: String,
            session: String,
            hostname: String,
        }

        let history_without_delete = HistoryWithoutDelete {
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
        struct HistoryWithoutDelete {
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
        };

        let serialized = rmp_serde::to_vec(&history).expect("Failed to serialize");
        // this will panic
        let _deserialized: HistoryWithoutDelete =
            rmp_serde::from_slice(&serialized).expect("Failed to deserialize");
    }
}
