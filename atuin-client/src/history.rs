use std::env;

use chrono::Utc;
use serde::{Deserialize, Serialize};

use atuin_common::utils::uuid_v7;

// Any new fields MUST be Optional<>!
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, sqlx::FromRow)]
pub struct History {
    pub id: String,
    pub timestamp: chrono::DateTime<Utc>,
    pub duration: i64,
    pub exit: i64,
    pub command: String,
    pub cwd: String,
    pub session: String,
    pub hostname: String,
    pub deleted_at: Option<chrono::DateTime<Utc>>,
}

// Forgive me, for I have sinned
// I need to replace rmp with something that is more backwards-compatible.
// Protobuf, or maybe just json
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
        }
    }

    pub fn success(&self) -> bool {
        self.exit == 0 || self.duration == -1
    }
}
