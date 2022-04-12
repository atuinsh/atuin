use std::borrow::Cow;

use axum::{response::IntoResponse, Json};
use chrono::Utc;
use serde::Serialize;

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

impl<'a> IntoResponse for ErrorResponseStatus<'a> {
    fn into_response(self) -> axum::response::Response {
        (self.status, Json(self.error)).into_response()
    }
}

pub struct ErrorResponseStatus<'a> {
    pub error: ErrorResponse<'a>,
    pub status: http::StatusCode,
}

impl<'a> ErrorResponse<'a> {
    pub fn with_status(self, status: http::StatusCode) -> ErrorResponseStatus<'a> {
        ErrorResponseStatus {
            error: self,
            status,
        }
    }

    pub fn reply(reason: &'a str) -> ErrorResponse {
        Self {
            reason: reason.into(),
        }
    }
}
