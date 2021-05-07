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


pub enum ReplyEither<T, E> {
    Ok(T),
    Err(E),
}

impl<T: Reply, E: Reply> Reply for ReplyEither<T, E> {
    fn into_response(self) -> Response {
        match self {
            ReplyEither::Ok(t) => t.into_response(),
            ReplyEither::Err(e) => e.into_response(),
        }
    }
}

pub type ReplyResult<T, E> = Result<ReplyEither<T, E>, Infallible>;
pub fn reply_error<T, E>(e: E) ->  ReplyResult<T, E> {
    return Ok(ReplyEither::Err(e));
}

pub type JSONResult<E> = Result<ReplyEither<warp::reply::Json, E>, Infallible>;
pub fn reply_json<E>(t: impl Serialize) -> JSONResult<E> {
    return reply(warp::reply::json(&t));
}

pub fn reply<T, E>(t: T) -> ReplyResult<T, E> {
    return Ok(ReplyEither::Ok(t));
}
