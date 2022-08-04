use async_trait::async_trait;
use axum::{
    extract::{FromRequest, RequestParts},
    handler::Handler,
    response::IntoResponse,
    routing::{get, post},
    Extension, Router,
};
use eyre::Result;
use tower::ServiceBuilder;
use tower_http::trace::TraceLayer;

use super::{
    database::{DatabaseExtension, DatabaseWrapped},
    handlers,
};
use crate::{models::User, settings::Settings};

#[async_trait]
impl<B, T> FromRequest<B> for DatabaseWrapped<User, T>
where
    B: Send,
    T: DatabaseExtension,
{
    type Rejection = http::StatusCode;

    async fn from_request(req: &mut RequestParts<B>) -> Result<Self, Self::Rejection> {
        let database = req
            .extensions()
            .get::<T>()
            .ok_or(http::StatusCode::INTERNAL_SERVER_ERROR)?;

        let auth_header = req
            .headers()
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

        let user = database
            .get_session_user(token)
            .await
            .map_err(|_| http::StatusCode::FORBIDDEN)?;

        Ok(user.into())
    }
}

async fn teapot() -> impl IntoResponse {
    (http::StatusCode::IM_A_TEAPOT, "☕")
}
pub fn router<T: DatabaseExtension>(database: T, settings: Settings) -> Router {
    let routes = Router::new()
        .route("/", get(handlers::index))
        .route("/sync/count", get(handlers::history::count::<T>))
        .route("/sync/history", get(handlers::history::list::<T>))
        .route(
            "/sync/calendar/:focus",
            get(handlers::history::calendar::<T>),
        )
        .route("/history", post(handlers::history::add::<T>))
        .route("/user/:username", get(handlers::user::get::<T>))
        .route("/register", post(handlers::user::register::<T>))
        .route("/login", post(handlers::user::login::<T>));

    let path = settings.path.as_str();
    if path.is_empty() {
        routes
    } else {
        Router::new().nest(path, routes)
    }
    .fallback(teapot.into_service())
    .layer(
        ServiceBuilder::new()
            .layer(TraceLayer::new_for_http())
            .layer(Extension(database))
            .layer(Extension(settings)),
    )
}
