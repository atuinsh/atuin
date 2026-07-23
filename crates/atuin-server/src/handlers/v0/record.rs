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

#[utoipa::path(
    post,
    path = "/api/v0/record",
    operation_id = "post_records",
    security(("session" = [])),
    request_body = Vec<RecordEncrypted>,
    responses(
        (status = 200, description = "Records stored (empty body)"),
        (status = "4XX", description = "Not authenticated, or a record was too large", body = ErrorResponse),
        (status = "5XX", description = "Server error", body = ErrorResponse),
    ),
)]
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

#[utoipa::path(
    get,
    path = "/api/v0/record",
    operation_id = "record_status",
    security(("session" = [])),
    responses(
        (status = 200, description = "Max record index per host and tag", body = RecordStatus),
        (status = "4XX", description = "Not authenticated", body = ErrorResponse),
        (status = "5XX", description = "Server error", body = ErrorResponse),
    ),
)]
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

#[utoipa::path(
    get,
    path = "/api/v0/record/next",
    operation_id = "next_records",
    security(("session" = [])),
    params(
        ("host" = String, Query, format = Uuid, description = "Host ID (hyphenated lowercase UUID string)"),
        ("tag" = String, Query, description = "Store tag, e.g. \"history\", \"kv\""),
        ("start" = Option<u64>, Query, format = "uint64", description = "Record index to start from; omitted means from the beginning"),
        ("count" = u64, Query, format = "uint64", description = "Maximum number of records to return"),
    ),
    responses(
        (status = 200, description = "A batch of records (bare JSON array)", body = Vec<RecordEncrypted>),
        (status = "4XX", description = "Not authenticated", body = ErrorResponse),
        (status = "5XX", description = "Server error", body = ErrorResponse),
    ),
)]
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
