use axum::{http, response::IntoResponse, Json};

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
