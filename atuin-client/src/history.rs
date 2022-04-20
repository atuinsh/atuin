use std::env;

use chrono::Utc;

use atuin_common::utils::uuid_v4;

// Any new fields MUST be Optional<>!
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, sqlx::FromRow)]
pub struct History {
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
    pub fn new(
        timestamp: chrono::DateTime<Utc>,
        command: String,
        cwd: String,
        exit: i64,
        duration: i64,
        session: Option<String>,
        hostname: Option<String>,
    ) -> Self {
        let session = session
            .or_else(|| env::var("ATUIN_SESSION").ok())
            .unwrap_or_else(uuid_v4);
        let hostname =
            hostname.unwrap_or_else(|| format!("{}:{}", whoami::hostname(), whoami::username()));

        Self {
            id: uuid_v4(),
            timestamp,
            command,
            cwd,
            exit,
            duration,
            session,
            hostname,
        }
    }
}
