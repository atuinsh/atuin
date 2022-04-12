use axum::extract::Query;
use axum::{Extension, Json};
use http::StatusCode;

use crate::database::{Database, Postgres};
use crate::models::{NewHistory, User};
use atuin_common::api::*;

pub async fn count(
    user: User,
    db: Extension<Postgres>,
) -> Result<Json<CountResponse>, ErrorResponseStatus<'static>> {
    match db.count_history(&user).await {
        Ok(count) => Ok(Json(CountResponse { count })),
        Err(_) => Err(ErrorResponse::reply("failed to query history count")
            .with_status(StatusCode::INTERNAL_SERVER_ERROR)),
    }
}

pub async fn list(
    req: Query<SyncHistoryRequest>,
    user: User,
    db: Extension<Postgres>,
) -> Result<Json<SyncHistoryResponse>, ErrorResponseStatus<'static>> {
    let history = db
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

pub async fn add(
    Json(req): Json<Vec<AddHistoryRequest>>,
    user: User,
    db: Extension<Postgres>,
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

    if let Err(e) = db.add_history(&history).await {
        error!("failed to add history: {}", e);

        return Err(ErrorResponse::reply("failed to add history")
            .with_status(StatusCode::INTERNAL_SERVER_ERROR));
    };

    Ok(())
}
