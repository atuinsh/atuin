use atuin_domain::record::{EncryptedData, HostId, Record, RecordIdx, RecordStatus, RecordTag};
use atuin_server_database::Database;
use axum::Json;
use axum::extract::{Query, State};
use axum::http::StatusCode;
use metrics::counter;
use serde::Deserialize;
use tracing::{error, instrument};

use crate::handlers::{ErrorResponse, ErrorResponseStatus, RespExt};
use crate::router::{AppState, UserAuth};

#[instrument(skip_all, err(level = "warn"), fields(user.id = user.id, record.count = records.len()))]
pub async fn post<DB: Database>(
    UserAuth(user): UserAuth,
    state: State<AppState<DB>>,
    Json(records): Json<Vec<Record<EncryptedData>>>,
) -> Result<(), ErrorResponseStatus<'static>> {
    let State(AppState {
        database, settings, ..
    }) = state;

    tracing::debug!(count = records.len(), user = user.username, "request to add records");

    counter!("atuin_record_uploaded").increment(records.len() as u64);

    let keep = records
        .iter()
        .all(|r| r.data.raw.len() <= settings.max_record_size || settings.max_record_size == 0);

    if !keep {
        counter!("atuin_record_too_large").increment(1);

        return Err(ErrorResponse::reply("could not add records; record too large")
            .with_status(StatusCode::BAD_REQUEST));
    }

    if let Err(e) = database.add_records(&user, &records).await {
        error!("failed to add record: {}", e);

        return Err(ErrorResponse::reply("failed to add record")
            .with_status(StatusCode::INTERNAL_SERVER_ERROR));
    };

    Ok(())
}

#[instrument(skip_all, err(level = "warn"), fields(user.id = user.id))]
pub async fn index<DB: Database>(
    UserAuth(user): UserAuth,
    state: State<AppState<DB>>,
) -> Result<Json<RecordStatus>, ErrorResponseStatus<'static>> {
    let State(AppState { database, .. }) = state;

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
    tag: RecordTag,
    start: Option<RecordIdx>,
    count: u64,
}

#[instrument(skip_all, err(level = "warn"), fields(user.id = user.id, host.id = %params.host, tag = params.tag.as_str(), count = params.count))]
pub async fn next<DB: Database>(
    params: Query<NextParams>,
    UserAuth(user): UserAuth,
    state: State<AppState<DB>>,
) -> Result<Json<Vec<Record<EncryptedData>>>, ErrorResponseStatus<'static>> {
    let State(AppState { database, .. }) = state;
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
