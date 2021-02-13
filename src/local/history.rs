use chrono;
use uuid::Uuid;

#[derive(Debug)]
pub struct History {
    pub id: String,
    pub timestamp: i64,
    pub duration: i64,
    pub exit: i64,
    pub command: String,
    pub cwd: String,
}

impl History {
    pub fn new(command: String, cwd: String, exit: i64, duration: i64) -> History {
        History {
            id: Uuid::new_v4().to_simple().to_string(),
            timestamp: chrono::Utc::now().timestamp_millis(),
            command,
            cwd,
            exit,
            duration,
        }
    }
}
