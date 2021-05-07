use warp::{http::StatusCode, Reply};

use crate::database::Database;
use crate::models::{NewHistory, User};
use atuin_common::api::*;
pub async fn count(
    user: User,
    db: impl Database + Clone + Send + Sync,
) -> JSONResponse<CountResponse> {
    db.count_history(&user).await.map_or(
        json_error(
            ErrorResponse::reply("failed to query history count")
                .with_status(StatusCode::INTERNAL_SERVER_ERROR),
        ),
        |count| json(CountResponse { count }),
    )
}

pub async fn list(
    req: SyncHistoryRequest,
    user: User,
    db: impl Database + Clone + Send + Sync,
) -> JSONResponse<SyncHistoryResponse> {
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
        return json_error(
            ErrorResponse::reply("failed to load history")
                .with_status(StatusCode::INTERNAL_SERVER_ERROR),
        );
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

    json(SyncHistoryResponse { history })
}

pub async fn add(
    req: Vec<AddHistoryRequest>,
    user: User,
    db: impl Database + Clone + Send + Sync,
) -> ReplyResponse<impl Reply> {
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

        return reply_error(
            ErrorResponse::reply("failed to add history")
                .with_status(StatusCode::INTERNAL_SERVER_ERROR),
        );
    };

    reply(warp::reply())
}
