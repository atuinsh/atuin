use axum::{extract::State, Json};
use http::StatusCode;
use tracing::instrument;

use super::{ErrorResponse, ErrorResponseStatus, RespExt};
use crate::{database::Database, models::User, router::AppState};

use atuin_common::api::*;

#[instrument(skip_all, fields(user.id = user.id))]
pub async fn status<DB: Database>(
    user: User,
    state: State<AppState<DB>>,
) -> Result<Json<StatusResponse>, ErrorResponseStatus<'static>> {
    let db = &state.0.database;

    let history_count = db.count_history_cached(&user).await;
    let deleted = db.deleted_history(&user).await;

    if history_count.is_err() || deleted.is_err() {
        return Err(ErrorResponse::reply("failed to query history count")
            .with_status(StatusCode::INTERNAL_SERVER_ERROR));
    }

    return Ok(Json(StatusResponse {
        count: history_count.unwrap(),
        deleted: deleted.unwrap(),
    }));
}
