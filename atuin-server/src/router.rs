use async_trait::async_trait;
use axum::{
    extract::FromRequestParts,
    response::IntoResponse,
    routing::{get, post},
    Router,
};
use eyre::Result;
use http::request::Parts;
use tower::ServiceBuilder;
use tower_http::trace::TraceLayer;

use super::{database::Database, handlers};
use crate::{models::User, settings::Settings};

#[async_trait]
impl<DB: Send + Sync> FromRequestParts<AppState<DB>> for User
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

#[derive(Clone)]
pub struct AppState<DB> {
    pub database: DB,
    pub settings: Settings,
}

pub fn router<DB: Database + Clone + Send + Sync + 'static>(
    database: DB,
    settings: Settings,
) -> Router {
    let routes = Router::new()
        .route("/", get(handlers::index))
        .route("/history", post(handlers::history::add))
        .route("/user/:username", get(handlers::user::get))
        .route("/register", post(handlers::user::register))
        .route("/login", post(handlers::user::login))
        .route("/sync/count", get(handlers::history::count))
        .route("/sync/history", get(handlers::history::list))
        .route("/sync/calendar/:focus", get(handlers::history::calendar))
        .route("/event/count", get(handlers::event::count))
        .route("/event/sync", get(handlers::event::list))
        .route("/event/sync", post(handlers::event::add));

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
