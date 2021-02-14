use std::env;

use uuid::Uuid;

#[derive(Debug)]
pub struct History {
    pub id: String,
    pub timestamp: i64,
    pub duration: i64,
    pub exit: i64,
    pub command: String,
    pub cwd: String,
    pub session: String,
    pub hostname: String,
}

impl History {
    pub fn new(
        timestamp: i64,
        command: String,
        cwd: String,
        exit: i64,
        duration: i64,
        session: Option<String>,
        hostname: Option<String>,
    ) -> Self {
        let session = session
            .or_else(|| env::var("ATUIN_SESSION").ok())
            .unwrap_or_else(|| Uuid::new_v4().to_simple().to_string());
        let hostname =
            hostname.unwrap_or_else(|| hostname::get().unwrap().to_str().unwrap().to_string());

        Self {
            id: Uuid::new_v4().to_simple().to_string(),
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
