use std::borrow::Cow;

use axum::{response::IntoResponse, Json};
use serde::{Deserialize, Serialize};

pub mod history;
pub mod user;

pub async fn index() -> &'static str {
    "\"Through the fathomless deeps of space swims the star turtle Great A\u{2019}Tuin, bearing on its back the four giant elephants who carry on their shoulders the mass of the Discworld.\"\n\t-- Sir Terry Pratchett"
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
