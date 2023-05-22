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
pub struct DeleteUserResponse {}

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
pub struct StatusResponse {
    pub count: i64,
    pub username: String,
    pub deleted: Vec<String>,

    // These could/should also go on the index of the server
    // However, we do not request the server index as a part of normal sync
    // I'd rather slightly increase the size of this response, than add an extra HTTP request
    pub page_size: i64, // max page size supported by the server
    pub version: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct DeleteHistoryRequest {
    pub client_id: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct MessageResponse {
    pub message: String,
}
