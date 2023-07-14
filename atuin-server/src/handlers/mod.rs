use atuin_common::api::{ErrorResponse, IndexResponse};
use atuin_server_database::Database;
use axum::{extract::State, response::IntoResponse, Json};

use crate::router::AppState;

pub mod history;
pub mod status;
pub mod user;

const VERSION: &str = env!("CARGO_PKG_VERSION");

pub async fn index<DB: Database>(state: State<AppState<DB>>) -> Json<IndexResponse> {
    let homage = r#""Through the fathomless deeps of space swims the star turtle Great A'Tuin, bearing on its back the four giant elephants who carry on their shoulders the mass of the Discworld." -- Sir Terry Pratchett"#;

    // Error with a -1 response
    // It's super unlikley this will happen
    let count = state.database.total_history().await.unwrap_or(-1);

    Json(IndexResponse {
        homage: homage.to_string(),
        version: VERSION.to_string(),
        total_history: count,
    })
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

pub trait RespExt<'a> {
    fn with_status(self, status: http::StatusCode) -> ErrorResponseStatus<'a>;
    fn reply(reason: &'a str) -> Self;
}

impl<'a> RespExt<'a> for ErrorResponse<'a> {
    fn with_status(self, status: http::StatusCode) -> ErrorResponseStatus<'a> {
        ErrorResponseStatus {
            error: self,
            status,
        }
    }

    fn reply(reason: &'a str) -> ErrorResponse {
        Self {
            reason: reason.into(),
        }
    }
}
