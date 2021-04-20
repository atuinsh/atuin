use std::convert::Infallible;

use warp::{http::StatusCode, reply::json};

use crate::database::Database;
use crate::models::{NewHistory, User};
use atuin_common::api::{
    AddHistoryRequest, CountResponse, ErrorResponse, SyncHistoryRequest, SyncHistoryResponse,
};

pub async fn count(
    user: User,
    db: impl Database + Clone + Send + Sync,
) -> Result<Box<dyn warp::Reply>, Infallible> {
    db.count_history(&user).await.map_or(
        Ok(Box::new(ErrorResponse::reply(
            "failed to query history count",
            StatusCode::INTERNAL_SERVER_ERROR,
        ))),
        |count| Ok(Box::new(json(&CountResponse { count }))),
    )
}

pub async fn list(
    req: SyncHistoryRequest,
    user: User,
    db: impl Database + Clone + Send + Sync,
) -> Result<Box<dyn warp::Reply>, Infallible> {
    let history = db
        .list_history(
            &user,
            req.sync_ts.naive_utc(),
            req.history_ts.naive_utc(),
            req.host,
        )
        .await;

    if let Err(e) = history {
        error!("failed to load history: {}", e);
        let resp =
            ErrorResponse::reply("failed to load history", StatusCode::INTERNAL_SERVER_ERROR);
        let resp = Box::new(resp);
        return Ok(resp);
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

    Ok(Box::new(json(&SyncHistoryResponse { history })))
}

pub async fn add(
    req: Vec<AddHistoryRequest>,
    user: User,
    db: impl Database + Clone + Send + Sync,
) -> Result<Box<dyn warp::Reply>, Infallible> {
    debug!("request to add {} history items", req.len());

    let history: Vec<NewHistory> = req
        .iter()
        .map(|h| NewHistory {
            client_id: h.id.as_str(),
            user_id: user.id,
            hostname: h.hostname.as_str(),
            timestamp: h.timestamp.naive_utc(),
            data: h.data.as_str(),
        })
        .collect();

    if let Err(e) = db.add_history(&history).await {
        error!("failed to add history: {}", e);

        return Ok(Box::new(ErrorResponse::reply(
            "failed to add history",
            StatusCode::INTERNAL_SERVER_ERROR,
        )));
    };

    Ok(Box::new(warp::reply()))
}
