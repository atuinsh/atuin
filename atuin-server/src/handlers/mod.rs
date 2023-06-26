use atuin_common::api::{ErrorResponse, IndexResponse};
use axum::{response::IntoResponse, Json};

pub mod history;
pub mod record;
pub mod status;
pub mod user;

const VERSION: &str = env!("CARGO_PKG_VERSION");

pub async fn index() -> Json<IndexResponse> {
    let homage = r#""Through the fathomless deeps of space swims the star turtle Great A'Tuin, bearing on its back the four giant elephants who carry on their shoulders the mass of the Discworld." -- Sir Terry Pratchett"#;

    Json(IndexResponse {
        homage: homage.to_string(),
        version: VERSION.to_string(),
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
