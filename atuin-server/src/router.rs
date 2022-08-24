use std::sync::Arc;

use async_trait::async_trait;
use axum::{
    extract::FromRequestParts,
    response::IntoResponse,
    routing::{get, post},
    Router,
};
use eyre::Result;
use http::request::Parts;
use tower_http::trace::TraceLayer;

use super::handlers;
use crate::database::Database;
use crate::{models::User, settings::Settings};

#[derive(Clone, Debug)]
pub struct AppState<DB> {
    pub database: DB,
    pub settings: Settings,
}

#[async_trait]
impl<DB> FromRequestParts<AppState<DB>> for User
where
    DB: Database,
{
    type Rejection = http::StatusCode;

    async fn from_request_parts(
        req: &mut Parts,
        state: &AppState<DB>,
    ) -> Result<Self, Self::Rejection> {
        let auth_header = req
            .headers
            .get(http::header::AUTHORIZATION)
            .ok_or(http::StatusCode::FORBIDDEN)?;
        let auth_header = auth_header
            .to_str()
            .map_err(|_| http::StatusCode::FORBIDDEN)?;
        let (typ, token) = auth_header
            .split_once(' ')
            .ok_or(http::StatusCode::FORBIDDEN)?;

        if typ != "Token" {
            return Err(http::StatusCode::FORBIDDEN);
        }

        let user = state
            .database
            .get_session_user(token)
            .await
            .map_err(|_| http::StatusCode::FORBIDDEN)?;

        Ok(user)
    }
}

async fn teapot() -> impl IntoResponse {
    (http::StatusCode::IM_A_TEAPOT, "☕")
}
pub fn router<DB: Database + 'static>(database: DB, settings: Settings) -> Router<AppState<DB>> {
    let path = settings.path.to_owned();

    let state = Arc::new(AppState { database, settings });
    let routes = Router::with_state_arc(state.clone())
        .route("/", get(handlers::index))
        .route("/sync/count", get(handlers::history::count::<DB>))
        .route("/sync/history", get(handlers::history::list::<DB>))
        .route(
            "/sync/calendar/:focus",
            get(handlers::history::calendar::<DB>),
        )
        .route("/history", post(handlers::history::add::<DB>))
        .route("/user/:username", get(handlers::user::get::<DB>))
        .route("/register", post(handlers::user::register::<DB>))
        .route("/login", post(handlers::user::login::<DB>));

    if path.is_empty() {
        routes
    } else {
        Router::with_state_arc(state).nest(&path, routes)
    }
    .fallback(teapot)
    .layer(TraceLayer::new_for_http())
}
