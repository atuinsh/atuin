use chrono::Utc;
use serde::{Deserialize, Serialize};

use crate::history::History;
use atuin_common::api::EventType;
use atuin_common::utils::{hash_bytes, hash_str, uuid_v4};
use eyre::Result;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, sqlx::FromRow)]
pub struct Event {
    pub id: String,
    pub timestamp: chrono::DateTime<Utc>,
    pub hostname: String,
    pub event_type: EventType,

    pub data: Vec<u8>,
    pub previous: String,
    pub checksum: String,
}

impl Event {
    pub fn new_create_history(history: &History, previous: String) -> Result<Event> {
        let data = rmp_serde::to_vec(history)?;
        let checksum = hash_bytes(&data);

        Ok(Event {
            id: uuid_v4(),
            timestamp: history.timestamp,
            hostname: history.hostname.clone(),
            event_type: EventType::CreateHistory,

            data,
            previous,
            checksum,
        })
    }

    pub fn new_delete_history(history_id: &str, previous: String) -> Event {
        let hostname = format!("{}:{}", whoami::hostname(), whoami::username());

        Event {
            id: uuid_v4(),
            timestamp: chrono::Utc::now(),
            hostname,
            event_type: EventType::DeleteHistory,

            data: history_id.as_bytes().to_owned(),
            checksum: hash_str(history_id),
            previous,
        }
    }
}
