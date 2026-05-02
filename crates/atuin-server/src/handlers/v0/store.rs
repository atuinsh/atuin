use axum::{extract::Query, extract::State, http::StatusCode};
use metrics::counter;
use serde::Deserialize;
use tracing::{error, instrument};

use crate::{
    handlers::{ErrorResponse, ErrorResponseStatus, RespExt},
    router::{AppState, UserAuth},
};
use atuin_server_database::Database;

#[derive(Deserialize)]
pub struct DeleteParams {}

#[instrument(skip_all, fields(user.id = user.id))]
pub async fn delete<DB: Database>(
    _params: Query<DeleteParams>,
    UserAuth(user): UserAuth,
    state: State<AppState<DB>>,
) -> Result<(), ErrorResponseStatus<'static>> {
    let State(AppState {
        database,
        settings: _,
    }) = state;

    if let Err(e) = database.delete_store(&user).await {
        counter!("atuin_store_delete_failed").increment(1);
        error!("failed to delete store {e:?}");

        return Err(ErrorResponse::reply("failed to delete store")
            .with_status(StatusCode::INTERNAL_SERVER_ERROR));
    }

    counter!("atuin_store_deleted").increment(1);

    Ok(())
}
