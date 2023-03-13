// This is very similar to the history handler. So why not de-dupe it?
// Well... it's different enough to be very awkward. I don't want to tie the two
// implementations together and would rather make it easier to evolve this one
// than spend time abstracting away common functionality.
// As soon as this code is released and active, the history sync is locked in
// place - it will never change, and will one day be deprecated. So let's just
// move forward :)

use axum::{
    extract::{Path, Query, State},
    Json,
};
use http::StatusCode;
use tracing::{debug, error, instrument};

use super::{ErrorResponse, ErrorResponseStatus, RespExt};
use crate::{
    calendar::{TimePeriod, TimePeriodInfo},
    database::Database,
    models::{NewEvent, User},
    router::AppState,
};

use atuin_common::api::*;

#[instrument(skip_all, fields(user.id = user.id))]
pub async fn count<DB: Database>(
    user: User,
    state: State<AppState<DB>>,
) -> Result<Json<CountResponse>, ErrorResponseStatus<'static>> {
    let db = &state.0.database;
    match db.count_event_cached(&user).await {
        // By default read out the cached value
        Ok(count) => Ok(Json(CountResponse { count })),

        // If that fails, fallback on a full COUNT. Cache is built on a POST
        // only
        Err(_) => match db.count_event(&user).await {
            Ok(count) => Ok(Json(CountResponse { count })),
            Err(_) => Err(ErrorResponse::reply("failed to query event count")
                .with_status(StatusCode::INTERNAL_SERVER_ERROR)),
        },
    }
}

#[instrument(skip_all, fields(user.id = user.id))]
pub async fn list<DB: Database>(
    req: Query<SyncEventRequest>,
    user: User,
    state: State<AppState<DB>>,
) -> Result<Json<SyncEventResponse>, ErrorResponseStatus<'static>> {
    let db = &state.0.database;
    let events = db
        .list_events(
            &user,
            req.sync_ts.naive_utc(),
            req.event_ts.naive_utc(),
            &req.host,
        )
        .await;

    if let Err(e) = events {
        error!("failed to load events: {}", e);
        return Err(ErrorResponse::reply("failed to load events")
            .with_status(StatusCode::INTERNAL_SERVER_ERROR));
    }

    let events: Vec<String> = events
        .unwrap()
        .iter()
        .map(|i| i.data.to_string())
        .collect();

    debug!(
        "loaded {} events for user {}",
        events.len(),
        user.id
    );

    Ok(Json(SyncEventResponse { events }))
}

#[instrument(skip_all, fields(user.id = user.id))]
pub async fn add<DB: Database>(
    user: User,
    state: State<AppState<DB>>,
    Json(req): Json<Vec<AddEventRequest>>,
) -> Result<(), ErrorResponseStatus<'static>> {
    debug!("request to add {} events", req.len());

    let events: Vec<NewEvent> = req
        .into_iter()
        .map(|h| NewEvent{
            client_id: h.id,
            user_id: user.id,
            hostname: h.hostname,
            timestamp: h.timestamp.naive_utc(),
            event_type: h.event_type,
            data: h.data,
        })
        .collect();

    let db = &state.0.database;
    if let Err(e) = db.add_events(&events).await {
        error!("failed to add events: {}", e);

        return Err(ErrorResponse::reply("failed to add events")
            .with_status(StatusCode::INTERNAL_SERVER_ERROR));
    };

    Ok(())
}
