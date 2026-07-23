use axum::Json;
use tracing::instrument;

use crate::handlers::ErrorResponseStatus;
use crate::router::UserAuth;

use atuin_common::api::*;

#[instrument]
pub async fn get() -> Result<Json<MeResponse>, ErrorResponseStatus<'static>> {
    Ok(Json(MeResponse {
        username: user.username,
    }))
}
