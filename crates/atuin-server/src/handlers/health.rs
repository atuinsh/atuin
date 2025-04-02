use axum::{Json, http, response::IntoResponse};

use serde::Serialize;

#[derive(Serialize)]
pub struct HealthResponse {
    pub status: &'static str,
}

pub async fn health_check() -> impl IntoResponse {
    (
        http::StatusCode::OK,
        Json(HealthResponse { status: "healthy" }),
    )
}
