use axum::{Json, http, response::IntoResponse};

use serde::Serialize;

#[derive(Serialize, utoipa::ToSchema)]
pub struct HealthResponse {
    #[schema(value_type = String)]
    pub status: &'static str,
}

#[utoipa::path(
    get,
    path = "/healthz",
    operation_id = "health_check",
    responses((status = 200, description = "Server is healthy", body = HealthResponse)),
)]
pub async fn health_check() -> impl IntoResponse {
    (
        http::StatusCode::OK,
        Json(HealthResponse { status: "healthy" }),
    )
}
