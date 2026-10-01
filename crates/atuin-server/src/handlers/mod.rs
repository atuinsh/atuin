use std::fmt;

use atuin_api_client::types::{ErrorResponse, IndexResponse};
use axum::extract::State;
use axum::response::IntoResponse;
use axum::{Json, http};
use tracing::instrument;

use crate::router::AppState;

pub mod health;
pub mod user;
pub mod v0;

const VERSION: &str = env!("CARGO_PKG_VERSION");

#[instrument(skip_all)]
pub async fn index(state: State<AppState>) -> Json<IndexResponse> {
    let homage = r#""Through the fathomless deeps of space swims the star turtle Great A'Tuin, bearing on its back the four giant elephants who carry on their shoulders the mass of the Discworld." -- Sir Terry Pratchett"#;

    let version = state.settings.fake_version.clone().unwrap_or(VERSION.to_string());

    Json(IndexResponse {
        homage: homage.to_string(),
        version,
    })
}

impl fmt::Display for ErrorResponseStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "status={} reason={}", self.status, self.error.reason)
    }
}

impl IntoResponse for ErrorResponseStatus {
    fn into_response(self) -> axum::response::Response {
        (self.status, Json(self.error)).into_response()
    }
}

pub struct ErrorResponseStatus {
    pub error: ErrorResponse,
    pub status: http::StatusCode,
}

pub trait RespExt {
    fn with_status(self, status: http::StatusCode) -> ErrorResponseStatus;
    fn reply(reason: &str) -> Self;
}

impl RespExt for ErrorResponse {
    fn with_status(self, status: http::StatusCode) -> ErrorResponseStatus {
        ErrorResponseStatus {
            error: self,
            status,
        }
    }

    /// A bare `reason`: `code` and the deprecated `error` and `errors` stay unset, which keeps
    /// them off the wire.
    fn reply(reason: &str) -> Self {
        Self {
            reason: reason.into(),
            code: None,
            error: None,
            errors: Vec::new(),
        }
    }
}
