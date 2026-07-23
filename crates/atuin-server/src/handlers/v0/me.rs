use axum::Json;
use tracing::instrument;

use crate::handlers::ErrorResponseStatus;
use crate::router::UserAuth;

use atuin_common::api::*;

#[utoipa::path(
    get,
    path = "/api/v0/me",
    operation_id = "me",
    security(("session" = [])),
    responses(
        (status = 200, description = "The authenticated user", body = MeResponse),
        (status = "4XX", description = "Not authenticated", body = ErrorResponse),
        (status = "5XX", description = "Server error", body = ErrorResponse),
    ),
)]
#[instrument(skip_all, fields(user.id = user.id))]
pub async fn get(
    UserAuth(user): UserAuth,
) -> Result<Json<MeResponse>, ErrorResponseStatus<'static>> {
    Ok(Json(MeResponse {
        username: user.username,
    }))
}
