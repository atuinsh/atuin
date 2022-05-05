use chrono::Utc;
use serde::{Deserialize, Serialize};
use eyre::Result;

use atuin_common::utils::uuid_v4;
use crate::history::History;


#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum EventType {
    Create,
    Delete
}


#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, sqlx::FromRow)]
pub struct Event {
    pub id: String,
    pub timestamp: chrono::DateTime<Utc>,
    pub hostname: String,
    pub event_type: EventType,

    pub history_id: String
}

impl Event {
    pub fn new_create(history: &History) -> Result<Event> {
        Ok(Event {
            id: uuid_v4(),
            timestamp: history.timestamp,
            hostname: history.hostname.clone(),
            event_type: EventType::Create,

            history_id: history.id.clone(),
        })
    }

    pub fn new_delete(history_id: &str) -> Result<Event> {
        let hostname = format!("{}:{}", whoami::hostname(), whoami::username());

        Ok(Event {
            id: uuid_v4(),
            timestamp: chrono::Utc::now(),
            hostname,
            event_type: EventType::Create,

            history_id: history_id.to_string(),
        })
    }
}
