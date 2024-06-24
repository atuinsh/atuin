use std::{collections::HashMap, convert::TryFrom};

use axum::{
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode},
    Json,
};
use metrics::counter;
use time::{Month, UtcOffset};
use tracing::{debug, error, instrument};

use super::{ErrorResponse, ErrorResponseStatus, RespExt};
use crate::{
    router::{AppState, UserAuth},
    utils::client_version_min,
};
use atuin_server_database::{
    calendar::{TimePeriod, TimePeriodInfo},
    models::NewHistory,
    Database,
};

use atuin_common::api::*;

#[instrument(skip_all, fields(user.id = user.id))]
pub async fn count<DB: Database>(
    UserAuth(user): UserAuth,
    state: State<AppState<DB>>,
) -> Result<Json<CountResponse>, ErrorResponseStatus<'static>> {
    let db = &state.0.database;
    match db.count_history_cached(&user).await {
        // By default read out the cached value
        Ok(count) => Ok(Json(CountResponse { count })),

        // If that fails, fallback on a full COUNT. Cache is built on a POST
        // only
        Err(_) => match db.count_history(&user).await {
            Ok(count) => Ok(Json(CountResponse { count })),
            Err(_) => Err(ErrorResponse::reply("failed to query history count")
                .with_status(StatusCode::INTERNAL_SERVER_ERROR)),
        },
    }
}

#[instrument(skip_all, fields(user.id = user.id))]
pub async fn list<DB: Database>(
    req: Query<SyncHistoryRequest>,
    UserAuth(user): UserAuth,
    headers: HeaderMap,
    state: State<AppState<DB>>,
) -> Result<Json<SyncHistoryResponse>, ErrorResponseStatus<'static>> {
    let db = &state.0.database;

    let agent = headers
        .get("user-agent")
        .map_or("", |v| v.to_str().unwrap_or(""));

    let variable_page_size = client_version_min(agent, ">=15.0.0").unwrap_or(false);

    let page_size = if variable_page_size {
        state.settings.page_size
    } else {
        100
    };

    if req.sync_ts.unix_timestamp_nanos() < 0 || req.history_ts.unix_timestamp_nanos() < 0 {
        error!("client asked for history from < epoch 0");
        counter!("atuin_history_epoch_before_zero").increment(1);

        return Err(
            ErrorResponse::reply("asked for history from before epoch 0")
                .with_status(StatusCode::BAD_REQUEST),
        );
    }

    let history = db
        .list_history(&user, req.sync_ts, req.history_ts, &req.host, page_size)
        .await;

    if let Err(e) = history {
        error!("failed to load history: {}", e);
        return Err(ErrorResponse::reply("failed to load history")
            .with_status(StatusCode::INTERNAL_SERVER_ERROR));
    }

    let history: Vec<String> = history
        .unwrap()
        .iter()
        .map(|i| i.data.to_string())
        .collect();

    debug!(
        "loaded {} items of history for user {}",
        history.len(),
        user.id
    );

    counter!("atuin_history_returned").increment(history.len() as u64);

    Ok(Json(SyncHistoryResponse { history }))
}

#[instrument(skip_all, fields(user.id = user.id))]
pub async fn delete<DB: Database>(
    UserAuth(user): UserAuth,
    state: State<AppState<DB>>,
    Json(req): Json<DeleteHistoryRequest>,
) -> Result<Json<MessageResponse>, ErrorResponseStatus<'static>> {
    let db = &state.0.database;

    // user_id is the ID of the history, as set by the user (the server has its own ID)
    let deleted = db.delete_history(&user, req.client_id).await;

    if let Err(e) = deleted {
        error!("failed to delete history: {}", e);
        return Err(ErrorResponse::reply("failed to delete history")
            .with_status(StatusCode::INTERNAL_SERVER_ERROR));
    }

    Ok(Json(MessageResponse {
        message: String::from("deleted OK"),
    }))
}

#[instrument(skip_all, fields(user.id = user.id))]
pub async fn add<DB: Database>(
    UserAuth(user): UserAuth,
    state: State<AppState<DB>>,
    Json(req): Json<Vec<AddHistoryRequest>>,
) -> Result<(), ErrorResponseStatus<'static>> {
    let State(AppState { database, settings }) = state;

    debug!("request to add {} history items", req.len());
    counter!("atuin_history_uploaded").increment(req.len() as u64);

    let mut history: Vec<NewHistory> = req
        .into_iter()
        .map(|h| NewHistory {
            client_id: h.id,
            user_id: user.id,
            hostname: h.hostname,
            timestamp: h.timestamp,
            data: h.data,
        })
        .collect();

    history.retain(|h| {
        // keep if within limit, or limit is 0 (unlimited)
        let keep = h.data.len() <= settings.max_history_length || settings.max_history_length == 0;

        // Don't return an error here. We want to insert as much of the
        // history list as we can, so log the error and continue going.
        if !keep {
            counter!("atuin_history_too_long").increment(1);

            tracing::warn!(
                "history too long, got length {}, max {}",
                h.data.len(),
                settings.max_history_length
            );
        }

        keep
    });

    if let Err(e) = database.add_history(&history).await {
        error!("failed to add history: {}", e);

        return Err(ErrorResponse::reply("failed to add history")
            .with_status(StatusCode::INTERNAL_SERVER_ERROR));
    };

    Ok(())
}

#[derive(serde::Deserialize, Debug)]
pub struct CalendarQuery {
    #[serde(default = "serde_calendar::zero")]
    year: i32,
    #[serde(default = "serde_calendar::one")]
    month: u8,

    #[serde(default = "serde_calendar::utc")]
    tz: UtcOffset,
}

mod serde_calendar {
    use time::UtcOffset;

    pub fn zero() -> i32 {
        0
    }

    pub fn one() -> u8 {
        1
    }

    pub fn utc() -> UtcOffset {
        UtcOffset::UTC
    }
}

#[instrument(skip_all, fields(user.id = user.id))]
pub async fn calendar<DB: Database>(
    Path(focus): Path<String>,
    Query(params): Query<CalendarQuery>,
    UserAuth(user): UserAuth,
    state: State<AppState<DB>>,
) -> Result<Json<HashMap<u64, TimePeriodInfo>>, ErrorResponseStatus<'static>> {
    let focus = focus.as_str();

    let year = params.year;
    let month = Month::try_from(params.month).map_err(|e| ErrorResponseStatus {
        error: ErrorResponse {
            reason: e.to_string().into(),
        },
        status: StatusCode::BAD_REQUEST,
    })?;

    let period = match focus {
        "year" => TimePeriod::Year,
        "month" => TimePeriod::Month { year },
        "day" => TimePeriod::Day { year, month },
        _ => {
            return Err(ErrorResponse::reply("invalid focus: use year/month/day")
                .with_status(StatusCode::BAD_REQUEST))
        }
    };

    let db = &state.0.database;
    let focus = db.calendar(&user, period, params.tz).await.map_err(|_| {
        ErrorResponse::reply("failed to query calendar")
            .with_status(StatusCode::INTERNAL_SERVER_ERROR)
    })?;

    Ok(Json(focus))
}
