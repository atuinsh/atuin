use async_trait::async_trait;
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
use crate::settings::Settings;
use atuin_server_database::{models::User, Database};

pub struct UserAuth(pub User);

#[async_trait]
impl<DB: Send + Sync> FromRequestParts<AppState<DB>> for UserAuth
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

        Ok(UserAuth(user))
    }
}

async fn teapot() -> impl IntoResponse {
    (http::StatusCode::IM_A_TEAPOT, "☕")
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
