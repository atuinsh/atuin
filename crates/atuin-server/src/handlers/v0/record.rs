use axum::{Json, extract::Query, extract::State, http::StatusCode};
use metrics::counter;
use serde::Deserialize;
use tracing::{error, instrument};

use crate::{
    handlers::{ErrorResponse, ErrorResponseStatus, RespExt},
    router::{AppState, UserAuth},
};
use atuin_server_database::Database;

use atuin_common::record::{EncryptedData, HostId, Record, RecordIdx, RecordStatus};

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

    counter!("atuin_record_uploaded").increment(records.len() as u64);

    let keep = records
        .iter()
        .all(|r| r.data.data.len() <= settings.max_record_size || settings.max_record_size == 0);

    if !keep {
        counter!("atuin_record_too_large").increment(1);

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

/// Replace the encrypted payload of existing records this user owns.
///
/// Used by `atuin store repair` to surgically fix records whose payload cannot be
/// decrypted with the current key (e.g. left over from a botched key rotation).
/// Only `data` and `cek` are updated - every other column is preserved so that
/// PASETO implicit assertions still verify. Records the user does not own are
/// silently skipped by the database layer.
#[instrument(skip_all, fields(user.id = user.id))]
pub async fn repair<DB: Database>(
    UserAuth(user): UserAuth,
    state: State<AppState<DB>>,
    Json(records): Json<Vec<Record<EncryptedData>>>,
) -> Result<(), ErrorResponseStatus<'static>> {
    let State(AppState { database, settings }) = state;

    tracing::debug!(
        count = records.len(),
        user = user.username,
        "request to repair records"
    );

    counter!("atuin_record_repair_requested").increment(records.len() as u64);

    let keep = records
        .iter()
        .all(|r| r.data.data.len() <= settings.max_record_size || settings.max_record_size == 0);

    if !keep {
        counter!("atuin_record_too_large").increment(1);

        return Err(
            ErrorResponse::reply("could not repair records; record too large")
                .with_status(StatusCode::BAD_REQUEST),
        );
    }

    if let Err(e) = database.repair_records(&user, &records).await {
        error!("failed to repair records: {}", e);

        return Err(ErrorResponse::reply("failed to repair records")
            .with_status(StatusCode::INTERNAL_SERVER_ERROR));
    };

    Ok(())
}

#[instrument(skip_all, fields(user.id = user.id))]
pub async fn index<DB: Database>(
    UserAuth(user): UserAuth,
    state: State<AppState<DB>>,
) -> Result<Json<RecordStatus>, ErrorResponseStatus<'static>> {
    let State(AppState {
        database,
        settings: _,
    }) = state;

    let record_index = match database.status(&user).await {
        Ok(index) => index,
        Err(e) => {
            error!("failed to get record index: {}", e);

            return Err(ErrorResponse::reply("failed to calculate record index")
                .with_status(StatusCode::INTERNAL_SERVER_ERROR));
        }
    };

    tracing::debug!(user = user.username, "record index request");

    Ok(Json(record_index))
}

#[derive(Deserialize)]
pub struct NextParams {
    host: HostId,
    tag: String,
    start: Option<RecordIdx>,
    count: u64,
}

#[instrument(skip_all, fields(user.id = user.id))]
pub async fn next<DB: Database>(
    params: Query<NextParams>,
    UserAuth(user): UserAuth,
    state: State<AppState<DB>>,
) -> Result<Json<Vec<Record<EncryptedData>>>, ErrorResponseStatus<'static>> {
    let State(AppState {
        database,
        settings: _,
    }) = state;
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

    counter!("atuin_record_downloaded").increment(records.len() as u64);

    Ok(Json(records))
}
