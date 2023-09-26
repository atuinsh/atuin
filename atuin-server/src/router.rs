use async_trait::async_trait;
use atuin_common::api::ErrorResponse;
use axum::{
    extract::FromRequestParts,
    response::IntoResponse,
    routing::{delete, get, post},
    Router,
};
use eyre::Result;
use http::request::Parts;
use tower::ServiceBuilder;
use tower_http::trace::TraceLayer;

use super::handlers;
use crate::{
    handlers::{ErrorResponseStatus, RespExt},
    settings::Settings,
};
use atuin_server_database::{models::User, Database, DbError};

pub struct UserAuth(pub User);

#[async_trait]
impl<DB: Send + Sync> FromRequestParts<AppState<DB>> for UserAuth
where
    DB: Database,
{
    type Rejection = ErrorResponseStatus<'static>;

    async fn from_request_parts(
        req: &mut Parts,
        state: &AppState<DB>,
    ) -> Result<Self, Self::Rejection> {
        let auth_header = req
            .headers
            .get(http::header::AUTHORIZATION)
            .ok_or_else(|| {
                ErrorResponse::reply("missing authorization header")
                    .with_status(http::StatusCode::BAD_REQUEST)
            })?;
        let auth_header = auth_header.to_str().map_err(|_| {
            ErrorResponse::reply("invalid authorization header encoding")
                .with_status(http::StatusCode::BAD_REQUEST)
        })?;
        let (typ, token) = auth_header.split_once(' ').ok_or_else(|| {
            ErrorResponse::reply("invalid authorization header encoding")
                .with_status(http::StatusCode::BAD_REQUEST)
        })?;

        if typ != "Token" {
            return Err(
                ErrorResponse::reply("invalid authorization header encoding")
                    .with_status(http::StatusCode::BAD_REQUEST),
            );
        }

        let user = state
            .database
            .get_session_user(token)
            .await
            .map_err(|e| match e {
                DbError::NotFound => ErrorResponse::reply("session not found")
                    .with_status(http::StatusCode::FORBIDDEN),
                DbError::Other(e) => {
                    tracing::error!(error = ?e, "could not query user session");
                    ErrorResponse::reply("could not query user session")
                        .with_status(http::StatusCode::INTERNAL_SERVER_ERROR)
                }
            })?;

        Ok(UserAuth(user))
    }
}

async fn teapot() -> impl IntoResponse {
    (http::StatusCode::IM_A_TEAPOT, "🫖")
}

#[derive(Clone)]
pub struct AppState<DB: Database> {
    pub database: DB,
    pub settings: Settings<DB::Settings>,
}

pub fn router<DB: Database>(database: DB, settings: Settings<DB::Settings>) -> Router {
    let routes = Router::new()
        .route("/", get(handlers::index))
        .route("/sync/count", get(handlers::history::count))
        .route("/sync/history", get(handlers::history::list))
        .route("/sync/calendar/:focus", get(handlers::history::calendar))
        .route("/sync/status", get(handlers::status::status))
        .route("/history", post(handlers::history::add))
        .route("/history", delete(handlers::history::delete))
        .route("/record", post(handlers::record::post))
        .route("/record", get(handlers::record::index))
        .route("/record/next", get(handlers::record::next))
        .route("/user/:username", get(handlers::user::get))
        .route("/account", delete(handlers::user::delete))
        .route("/register", post(handlers::user::register))
        .route("/login", post(handlers::user::login));

    let path = settings.path.as_str();
    if path.is_empty() {
        routes
    } else {
        Router::new().nest(path, routes)
    }
    .fallback(teapot)
    .with_state(AppState { database, settings })
    .layer(ServiceBuilder::new().layer(TraceLayer::new_for_http()))
}
