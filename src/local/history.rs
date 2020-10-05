use chrono;

#[derive(Debug)]
pub struct History {
    pub timestamp: i64,
    pub command: String,
    pub cwd: String,
}

impl History {
    pub fn new(command: String, cwd: String) -> History {
        History {
            timestamp: chrono::Utc::now().timestamp_millis(),
            command,
            cwd,
        }
    }
}
