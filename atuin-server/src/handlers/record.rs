use axum::{extract::Query, extract::State, Json};
use http::StatusCode;
use serde::Deserialize;
use tracing::{error, instrument};
use uuid::Uuid;

use super::{ErrorResponse, ErrorResponseStatus, RespExt};
use crate::router::{AppState, UserAuth};
use atuin_server_database::Database;

use atuin_common::record::{EncryptedData, Record, RecordIndex};

#[instrument(skip_all, fields(user.id = user.id))]
pub async fn post<DB: Database>(
    UserAuth(user): UserAuth,
    state: State<AppState<DB>>,
    Json(records): Json<Vec<Record<EncryptedData>>>,
) -> Result<(), ErrorResponseStatus<'static>> {
    let State(AppState { database, settings }) = state;

    tracing::debug!(
        count = records.len(),
        user = user.username,
        "request to add records"
    );

    let too_big = records
        .iter()
        .any(|r| r.data.data.len() >= settings.max_record_size || settings.max_record_size == 0);

    if too_big {
        return Err(
            ErrorResponse::reply("could not add records; record too large")
                .with_status(StatusCode::BAD_REQUEST),
        );
    }

    if let Err(e) = database.add_records(&user, &records).await {
        error!("failed to add record: {}", e);

        return Err(ErrorResponse::reply("failed to add record")
            .with_status(StatusCode::INTERNAL_SERVER_ERROR));
    };

    Ok(())
}

#[instrument(skip_all, fields(user.id = user.id))]
pub async fn index<DB: Database>(
    UserAuth(user): UserAuth,
    state: State<AppState<DB>>,
) -> Result<Json<RecordIndex>, ErrorResponseStatus<'static>> {
    let State(AppState { database, settings: _ }) = state;

    let index = match database.tail_records(&user).await {
        Ok(index) => index,
        Err(e) => {
            error!("failed to get record index: {}", e);

            return Err(ErrorResponse::reply("failed to calculate record index")
                .with_status(StatusCode::INTERNAL_SERVER_ERROR));
        }
    };

    let mut record_index = RecordIndex::new();

    for row in index {
        record_index.set_raw(row.0, row.1, row.2);
    }

    Ok(Json(record_index))
}

#[derive(Deserialize)]
pub struct NextParams {
    host: Uuid,
    tag: String,
    start: Option<Uuid>,
    count: u64,
}

#[instrument(skip_all, fields(user.id = user.id))]
pub async fn next<DB: Database>(
    params: Query<NextParams>,
    UserAuth(user): UserAuth,
    state: State<AppState<DB>>,
) -> Result<Json<Vec<Record<EncryptedData>>>, ErrorResponseStatus<'static>> {
    let State(AppState { database, settings: _ }) = state;
    let params = params.0;

    let records = match database
        .next_records(&user, params.host, params.tag, params.start, params.count)
        .await
    {
        Ok(records) => records,
        Err(e) => {
            error!("failed to get record index: {}", e);

            return Err(ErrorResponse::reply("failed to calculate record index")
                .with_status(StatusCode::INTERNAL_SERVER_ERROR));
        }
    };

    Ok(Json(records))
}
