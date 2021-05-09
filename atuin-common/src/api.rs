use std::{borrow::Cow, convert::Infallible};

use chrono::Utc;
use serde::{Deserialize, Serialize};
use warp::{reply::Response, Reply};

#[derive(Debug, Serialize, Deserialize)]
pub struct UserResponse<'a> {
    pub username: Cow<'a, str>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct RegisterRequest<'a> {
    pub email: Cow<'a, str>,
    pub username: Cow<'a, str>,
    pub password: Cow<'a, str>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct RegisterResponse<'a> {
    pub session: Cow<'a, str>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct LoginRequest<'a> {
    pub username: Cow<'a, str>,
    pub password: Cow<'a, str>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct LoginResponse<'a> {
    pub session: Cow<'a, str>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct AddHistoryRequest<'a, D> {
    pub id: Cow<'a, str>,
    pub timestamp: chrono::DateTime<Utc>,
    pub data: D,
    pub hostname: Cow<'a, str>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct CountResponse {
    pub count: i64,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct SyncHistoryRequest<'a> {
    pub sync_ts: chrono::DateTime<chrono::FixedOffset>,
    pub history_ts: chrono::DateTime<chrono::FixedOffset>,
    pub host: Cow<'a, str>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct SyncHistoryResponse {
    pub history: Vec<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ErrorResponse<'a> {
    pub reason: Cow<'a, str>,
}

impl Reply for ErrorResponse<'_> {
    fn into_response(self) -> Response {
        warp::reply::json(&self).into_response()
    }
}

pub struct ErrorResponseStatus<'a> {
    pub error: ErrorResponse<'a>,
    pub status: warp::http::StatusCode,
}

impl Reply for ErrorResponseStatus<'_> {
    fn into_response(self) -> Response {
        warp::reply::with_status(self.error, self.status).into_response()
    }
}

impl<'a> ErrorResponse<'a> {
    pub fn with_status(self, status: warp::http::StatusCode) -> ErrorResponseStatus<'a> {
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
pub fn reply_error<T, E>(e: E) -> ReplyResult<T, E> {
    Ok(ReplyEither::Err(e))
}

pub type JSONResult<E> = Result<ReplyEither<warp::reply::Json, E>, Infallible>;
pub fn reply_json<E>(t: impl Serialize) -> JSONResult<E> {
    reply(warp::reply::json(&t))
}

pub fn reply<T, E>(t: T) -> ReplyResult<T, E> {
    Ok(ReplyEither::Ok(t))
}
