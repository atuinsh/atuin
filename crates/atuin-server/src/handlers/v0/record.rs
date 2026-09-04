use atuin_domain::api::{FailedSyncRecord, ServerConfigSyncError, SyncResponse};
use atuin_domain::record::{
    EncryptedData, HostId, Record, RecordIdx, RecordSeriesKey, RecordStatus, RecordTag,
};
use axum::Json;
use axum::extract::{Query, State};
use axum::http::StatusCode;
use easy_cast::Conv;
use metrics::counter;
use serde::Deserialize;
use tracing::{error, instrument};

use crate::handlers::{ErrorResponse, ErrorResponseStatus, RespExt};
use crate::router::{AppState, UserAuth};

#[instrument(skip_all, err(level = "warn"), fields(user.id = user.id, record.count = records.len()))]
pub async fn post(
    UserAuth(user): UserAuth,
    state: State<AppState>,
    Json(records): Json<Vec<Record<EncryptedData>>>,
) -> Result<Json<SyncResponse>, ErrorResponseStatus<'static>> {
    let State(AppState {
        database, settings, ..
    }) = state;

    tracing::debug!(count = records.len(), user = user.username, "request to add records");

    counter!("atuin_record_uploaded").increment(u64::conv(records.len()));
    let (mut valid_len_commands, invalid_len_commands): (
        Vec<Record<EncryptedData>>,
        Vec<Record<EncryptedData>>,
    ) = records.into_iter().partition(|r| {
        r.data.raw.len() <= settings.max_record_size || settings.max_record_size == 0
    });

    for _ in 0..invalid_len_commands.len() {
        counter!("atuin_record_too_large").increment(1);
    }
    let failed_commands_ids = invalid_len_commands
        .iter()
        .map(|r| FailedSyncRecord {
            reason: ServerConfigSyncError::RequestTooLarge,
            record_id: r.id,
        })
        .collect();

    let mut transformed_invalid_command: Vec<Record<EncryptedData>> = invalid_len_commands
        .into_iter()
        .map(|command| {
            command.with_data(EncryptedData {
                raw: "b".to_string(),
                cek: "b".to_string(),
            })
        })
        .collect();
    valid_len_commands.append(&mut transformed_invalid_command);

    if let Err(e) = database.add_records(&user, &valid_len_commands).await {
        error!("failed to add record: {}", e);

        return Err(ErrorResponse::reply("failed to add record")
            .with_status(StatusCode::INTERNAL_SERVER_ERROR));
    };
    let res = SyncResponse {
        failed_commands: failed_commands_ids,
    };
    Ok(Json(res))
}

#[instrument(skip_all, err(level = "warn"), fields(user.id = user.id))]
pub async fn index(
    UserAuth(user): UserAuth,
    state: State<AppState>,
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
pub async fn next(
    params: Query<NextParams>,
    UserAuth(user): UserAuth,
    state: State<AppState>,
) -> Result<Json<Vec<Record<EncryptedData>>>, ErrorResponseStatus<'static>> {
    let State(AppState { database, .. }) = state;
    let params = params.0;
    let series = RecordSeriesKey::new(params.host, params.tag);

    let records = match database.next_records(&user, &series, params.start, params.count).await {
        Ok(records) => records,
        Err(e) => {
            error!("failed to get record index: {}", e);

            return Err(ErrorResponse::reply("failed to calculate record index")
                .with_status(StatusCode::INTERNAL_SERVER_ERROR));
        }
    };

    counter!("atuin_record_downloaded").increment(u64::conv(records.len()));

    Ok(Json(records))
}
