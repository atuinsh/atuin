use std::convert::Infallible;

use chrono::Utc;
use serde::Serialize;
use warp::{reply::Response, Reply};

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
pub struct ErrorResponse {
    pub reason: String,
}

impl Reply for ErrorResponse {
    fn into_response(self) -> Response {
        warp::reply::json(&self).into_response()
    }
}

pub struct ErrorResponseStatus {
    pub error: ErrorResponse,
    pub status: warp::http::StatusCode,
}

impl Reply for ErrorResponseStatus {
    fn into_response(self) -> Response {
        warp::reply::with_status(self.error, self.status).into_response()
    }
}

impl ErrorResponse {
    pub fn with_status(self, status: warp::http::StatusCode) -> ErrorResponseStatus {
        ErrorResponseStatus {
            error: self,
            status,
        }
    }

    pub fn reply(reason: &str) -> ErrorResponse {
        Self {
            reason: reason.to_string(),
        }
    }
}

pub enum JSONResult<T> {
    Ok(T),
    Err(ErrorResponseStatus),
}

pub fn json_error<T>(e: ErrorResponseStatus) -> JSONResponse<T> {
    return Ok(JSONResult::Err(e));
}

pub fn json<T>(t: T) -> JSONResponse<T> {
    return Ok(JSONResult::Ok(t));
}

impl<T: Send + Serialize> Reply for JSONResult<T> {
    fn into_response(self) -> Response {
        match self {
            JSONResult::Ok(t) => warp::reply::json(&t).into_response(),
            JSONResult::Err(e) => e.into_response(),
        }
    }
}

pub enum ReplyResult<T> {
    Ok(T),
    Err(ErrorResponseStatus),
}

impl<T: Reply> Reply for ReplyResult<T> {
    fn into_response(self) -> Response {
        match self {
            ReplyResult::Ok(t) => t.into_response(),
            ReplyResult::Err(e) => e.into_response(),
        }
    }
}

pub fn reply_error<T>(e: ErrorResponseStatus) -> ReplyResponse<T> {
    return Ok(ReplyResult::Err(e));
}

pub fn reply<T>(t: T) -> ReplyResponse<T> {
    return Ok(ReplyResult::Ok(t));
}

pub type ReplyResponse<T> = Result<ReplyResult<T>, Infallible>;
pub type JSONResponse<T> = Result<JSONResult<T>, Infallible>;
