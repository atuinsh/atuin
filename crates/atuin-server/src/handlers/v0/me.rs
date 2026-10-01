use atuin_api_client::types::MeResponse;
use axum::Json;
use tracing::instrument;

use crate::handlers::ErrorResponseStatus;
use crate::router::UserAuth;

#[instrument(skip_all, err(level = "warn"), fields(user.id = user.id))]
pub async fn get(
    UserAuth(user): UserAuth,
) -> Result<Json<MeResponse>, ErrorResponseStatus> {
    Ok(Json(MeResponse {
        username: Some(user.username),
    }))
}
