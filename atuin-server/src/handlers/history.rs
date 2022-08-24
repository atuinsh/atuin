use std::collections::HashMap;

use axum::{
    extract::{Path, Query, State},
    Json,
};
use http::StatusCode;
use tracing::{debug, error, instrument};

use super::{ErrorResponse, ErrorResponseStatus};
use crate::{
    calendar::{TimePeriod, TimePeriodInfo},
    database::Database,
    models::{NewHistory, User},
    router::AppState,
};

use atuin_common::api::*;

#[instrument(skip_all, fields(user.id = user.id))]
pub async fn count<DB: Database>(
    user: User,
    State(state): State<AppState<DB>>,
) -> Result<Json<CountResponse>, ErrorResponseStatus<'static>> {
    match state.database.count_history_cached(&user).await {
        // By default read out the cached value
        Ok(count) => Ok(Json(CountResponse { count })),

        // If that fails, fallback on a full COUNT. Cache is built on a POST
        // only
        Err(_) => match state.database.count_history(&user).await {
            Ok(count) => Ok(Json(CountResponse { count })),
            Err(_) => Err(ErrorResponse::reply("failed to query history count")
                .with_status(StatusCode::INTERNAL_SERVER_ERROR)),
        },
    }
}

#[instrument(skip_all, fields(user.id = user.id))]
pub async fn list<DB: Database>(
    req: Query<SyncHistoryRequest>,
    user: User,
    State(state): State<AppState<DB>>,
) -> Result<Json<SyncHistoryResponse>, ErrorResponseStatus<'static>> {
    let history = state
        .database
        .list_history(
            &user,
            req.sync_ts.naive_utc(),
            req.history_ts.naive_utc(),
            &req.host,
        )
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

    Ok(Json(SyncHistoryResponse { history }))
}

#[instrument(skip_all, fields(user.id = user.id))]
pub async fn add<DB: Database>(
    user: User,
    State(state): State<AppState<DB>>,
    Json(req): Json<Vec<AddHistoryRequest>>,
) -> Result<(), ErrorResponseStatus<'static>> {
    debug!("request to add {} history items", req.len());

    let history: Vec<NewHistory> = req
        .into_iter()
        .map(|h| NewHistory {
            client_id: h.id,
            user_id: user.id,
            hostname: h.hostname,
            timestamp: h.timestamp.naive_utc(),
            data: h.data,
        })
        .collect();

    if let Err(e) = state.database.add_history(&history).await {
        error!("failed to add history: {}", e);

        return Err(ErrorResponse::reply("failed to add history")
            .with_status(StatusCode::INTERNAL_SERVER_ERROR));
    };

    Ok(())
}

#[instrument(skip_all, fields(user.id = user.id))]
pub async fn calendar<DB: Database>(
    Path(focus): Path<String>,
    Query(params): Query<HashMap<String, u64>>,
    user: User,
    State(state): State<AppState<DB>>,
) -> Result<Json<HashMap<u64, TimePeriodInfo>>, ErrorResponseStatus<'static>> {
    let focus = focus.as_str();

    let year = params.get("year").unwrap_or(&0);
    let month = params.get("month").unwrap_or(&1);

    let focus = match focus {
        "year" => state
            .database
            .calendar(&user, TimePeriod::YEAR, *year, *month)
            .await
            .map_err(|_| {
                ErrorResponse::reply("failed to query calendar")
                    .with_status(StatusCode::INTERNAL_SERVER_ERROR)
            }),

        "month" => state
            .database
            .calendar(&user, TimePeriod::MONTH, *year, *month)
            .await
            .map_err(|_| {
                ErrorResponse::reply("failed to query calendar")
                    .with_status(StatusCode::INTERNAL_SERVER_ERROR)
            }),

        "day" => state
            .database
            .calendar(&user, TimePeriod::DAY, *year, *month)
            .await
            .map_err(|_| {
                ErrorResponse::reply("failed to query calendar")
                    .with_status(StatusCode::INTERNAL_SERVER_ERROR)
            }),

        _ => Err(ErrorResponse::reply("invalid focus: use year/month/day")
            .with_status(StatusCode::BAD_REQUEST)),
    }?;

    Ok(Json(focus))
}
