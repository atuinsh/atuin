use chrono::Utc;
use serde::{Deserialize, Serialize};
use std::borrow::Cow;

#[derive(Debug, Serialize, Deserialize)]
pub struct UserResponse {
    pub username: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct RegisterRequest {
    pub email: String,
    pub username: String,
    pub password: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct RegisterResponse {
    pub session: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct LoginRequest {
    pub username: String,
    pub password: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct LoginResponse {
    pub session: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct AddHistoryRequest {
    pub id: String,
    pub timestamp: chrono::DateTime<Utc>,
    pub data: String,
    pub hostname: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct CountResponse {
    pub count: i64,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct SyncHistoryRequest {
    pub sync_ts: chrono::DateTime<chrono::FixedOffset>,
    pub history_ts: chrono::DateTime<chrono::FixedOffset>,
    pub host: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct SyncHistoryResponse {
    pub history: Vec<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ErrorResponse<'a> {
    pub reason: Cow<'a, str>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct IndexResponse {
    pub homage: String,
    pub version: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct AddEventRequest {
    pub id: String,
    pub timestamp: chrono::DateTime<Utc>,
    pub data: String,
    pub hostname: String,
    pub event_type: EventType,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct SyncEventRequest {
    pub sync_ts: chrono::DateTime<chrono::FixedOffset>,
    pub event_ts: chrono::DateTime<chrono::FixedOffset>,
    pub host: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct SyncEventResponse {
    pub events: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum EventType {
    CreateHistory,
    DeleteHistory,
}

impl std::fmt::Display for EventType {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(f, "{:?}", self)
    }
}
