use warp::{http::StatusCode, Reply};

use crate::database::Database;
use crate::models::{NewHistory, User};
use atuin_common::api::*;
pub async fn count(
    user: User,
    db: impl Database + Clone + Send + Sync,
) -> JSONResult<ErrorResponseStatus<'static>> {
    db.count_history(&user).await.map_or(
        reply_error(
            ErrorResponse::reply("failed to query history count")
                .with_status(StatusCode::INTERNAL_SERVER_ERROR),
        ),
        |count| reply_json(CountResponse { count }),
    )
}

pub async fn list(
    req: SyncHistoryRequest<'_>,
    user: User,
    db: impl Database + Clone + Send + Sync,
) -> JSONResult<ErrorResponseStatus<'static>> {
    let history = db
        .list_history(
            &user,
            req.sync_ts.naive_utc(),
            req.history_ts.naive_utc(),
            &req.host,
        )
        .await;

    let history = match history {
        Err(e) => {
            error!("failed to load history: {}", e);
            return reply_error(
                ErrorResponse::reply("failed to load history")
                    .with_status(StatusCode::INTERNAL_SERVER_ERROR),
            );
        }
        Ok(h) => h,
    };

    let history: Vec<String> = history.into_iter().map(|i| i.data).collect();

    debug!(
        "loaded {} items of history for user {}",
        history.len(),
        user.id
    );

    reply_json(SyncHistoryResponse { history })
}

pub async fn add(
    req: Vec<AddHistoryRequest<'_, String>>,
    user: User,
    db: impl Database + Clone + Send + Sync,
) -> ReplyResult<impl Reply, ErrorResponseStatus<'_>> {
    debug!("request to add {} history items", req.len());

    let history: Vec<NewHistory> = req
        .into_iter()
        .map(|h| NewHistory {
            client_id: h.id,
            user_id: user.id,
            hostname: h.hostname,
            timestamp: h.timestamp.naive_utc(),
            data: h.data.into(),
        })
        .collect();

    if let Err(e) = db.add_history(&history).await {
        error!("failed to add history: {}", e);

        return reply_error(
            ErrorResponse::reply("failed to add history")
                .with_status(StatusCode::INTERNAL_SERVER_ERROR),
        );
    };

    reply(warp::reply())
}
