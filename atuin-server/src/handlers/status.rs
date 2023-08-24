use axum::{extract::State, Json};
use http::StatusCode;
use tracing::instrument;

use super::{ErrorResponse, ErrorResponseStatus, RespExt};
use crate::router::{AppState, UserAuth};
use atuin_server_database::Database;

use atuin_common::api::*;

const VERSION: &str = env!("CARGO_PKG_VERSION");

#[instrument(skip_all, fields(user.id = user.id))]
pub async fn status<DB: Database>(
    UserAuth(user): UserAuth,
    state: State<AppState<DB>>,
) -> Result<Json<StatusResponse>, ErrorResponseStatus<'static>> {
    let db = &state.0.database;

    let deleted = db.deleted_history(&user).await.unwrap_or(vec![]);

    let count = match db.count_history_cached(&user).await {
        // By default read out the cached value
        Ok(count) => count,

        // If that fails, fallback on a full COUNT. Cache is built on a POST
        // only
        Err(_) => match db.count_history(&user).await {
            Ok(count) => count,
            Err(_) => {
                return Err(ErrorResponse::reply("failed to query history count")
                    .with_status(StatusCode::INTERNAL_SERVER_ERROR))
            }
        },
    };

    Ok(Json(StatusResponse {
        count,
        deleted,
        username: user.username,
        version: VERSION.to_string(),
        page_size: state.settings.page_size,
    }))
}
