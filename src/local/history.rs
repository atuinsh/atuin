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
    ) -> History {
        // get the current session or just generate a random string
        let env_session =
            env::var("ATUIN_SESSION").unwrap_or(Uuid::new_v4().to_simple().to_string());

        // best attempt at getting the current hostname, or just unknown
        let os_hostname = hostname::get().unwrap();
        let os_hostname = os_hostname.to_str().unwrap();
        let os_hostname = String::from(os_hostname);

        let session = session.unwrap_or(env_session);
        let hostname = hostname.unwrap_or(os_hostname);

        History {
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
