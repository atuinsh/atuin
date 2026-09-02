use std::sync::Arc;

use atuin_domain::api::{ATUIN_CARGO_VERSION, ATUIN_HEADER_VERSION, ErrorResponse};
use atuin_domain::caps::axum::{CapabilitiesRouterExt, get as capabilities_endpoint};
use atuin_domain::caps::{CapServer, CapabilitiesCap, PageSizeCap};
use axum::Router;
use axum::extract::{FromRequestParts, Request};
use axum::http::request::Parts;
use axum::http::{self};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, patch, post};
use eyre::Result;
use tower::ServiceBuilder;
use tower_http::trace::TraceLayer;

use super::handlers;
use crate::db::models::User;
use crate::db::{DbError, DynDatabase};
use crate::handlers::{ErrorResponseStatus, RespExt};
use crate::metrics;
use crate::settings::Settings;

pub struct UserAuth(pub User);

impl FromRequestParts<AppState> for UserAuth {
    type Rejection = ErrorResponseStatus<'static>;

    #[tracing::instrument(name = "auth", skip_all)]
    async fn from_request_parts(
        req: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        let auth_header = req.headers.get(http::header::AUTHORIZATION).ok_or_else(|| {
            tracing::debug!("request is missing the authorization header");
            ErrorResponse::reply("missing authorization header")
                .with_status(http::StatusCode::BAD_REQUEST)
        })?;
        let auth_header = auth_header.to_str().map_err(|_| {
            tracing::debug!("authorization header is not valid ascii");
            ErrorResponse::reply("invalid authorization header encoding")
                .with_status(http::StatusCode::BAD_REQUEST)
        })?;
        let (typ, token) = auth_header.split_once(' ').ok_or_else(|| {
            tracing::debug!("authorization header is not a space-separated pair");
            ErrorResponse::reply("invalid authorization header encoding")
                .with_status(http::StatusCode::BAD_REQUEST)
        })?;

        if typ != "Token" {
            tracing::debug!(scheme = typ, "unsupported authorization scheme");
            return Err(ErrorResponse::reply("invalid authorization header encoding")
                .with_status(http::StatusCode::BAD_REQUEST));
        }

        let user = state.database.get_session_user(token).await.map_err(|e| match e {
            DbError::NotFound => {
                tracing::warn!("presented session token was not recognised");
                ErrorResponse::reply("session not found").with_status(http::StatusCode::FORBIDDEN)
            }
            DbError::Other(e) => {
                tracing::error!(error = ?e, "could not query user session");
                ErrorResponse::reply("could not query user session")
                    .with_status(http::StatusCode::INTERNAL_SERVER_ERROR)
            }
        })?;

        tracing::debug!(user.id = user.id, user.username = %user.username, "request authenticated");

        Ok(Self(user))
    }
}

async fn teapot() -> impl IntoResponse {
    // This used to return 418: 🫖
    // Much as it was fun, it wasn't as useful or informative as it should be
    (http::StatusCode::NOT_FOUND, "404 not found")
}

async fn clacks_overhead(request: Request, next: Next) -> Response {
    let mut response = next.run(request).await;

    let gnu_terry_value = "GNU Terry Pratchett, Kris Nova";
    let gnu_terry_header = "X-Clacks-Overhead";

    response.headers_mut().insert(gnu_terry_header, gnu_terry_value.parse().unwrap());
    response
}

/// Ensure that we only try and sync with clients on the same major version
async fn semver(request: Request, next: Next) -> Response {
    let mut response = next.run(request).await;
    response.headers_mut().insert(ATUIN_HEADER_VERSION, ATUIN_CARGO_VERSION.parse().unwrap());

    response
}

#[derive(Clone)]
pub struct AppState {
    pub database: Arc<dyn DynDatabase>,
    pub settings: Settings,
}

fn capabilities() -> CapServer {
    CapServer::new()
        .add(CapabilitiesCap { version: 1 })
        .expect("CapabilitiesCap is registered exactly once")
        .add(PageSizeCap {
            version: 1,
            page_size: 100,
        })
        .expect("PageSizeCap is registered exactly once")
}

pub fn router(database: Arc<dyn DynDatabase>, settings: Settings) -> Router {
    // Advertise the self-referential capabilities capability, so every server that speaks the
    // protocol carries at least one concrete capability a client can observe.
    let caps = Arc::new(capabilities());

    let negotiated = Router::new()
        .route("/", get(handlers::index))
        .route("/user/{username}", get(handlers::user::get))
        .route("/account", delete(handlers::user::delete))
        .route("/account/password", patch(handlers::user::change_password))
        .route("/register", post(handlers::user::register))
        .route("/login", post(handlers::user::login))
        .route("/api/v0/me", get(handlers::v0::me::get))
        .route("/api/v0/record", post(handlers::v0::record::post))
        .route("/api/v0/record", get(handlers::v0::record::index))
        .route("/api/v0/record/next", get(handlers::v0::record::next))
        .route("/api/v0/store", delete(handlers::v0::store::delete))
        .negotiate_capabilities(caps.clone());

    let unnegotiated = Router::new()
        .route("/api/v0/capabilities", get(capabilities_endpoint))
        .route("/healthz", get(handlers::health::health_check))
        .with_state(caps);

    let routes = unnegotiated.merge(negotiated);

    let path = settings.path.as_str();
    let routes = if path.is_empty() {
        routes
    } else {
        Router::new().nest(path, routes)
    };

    routes.fallback(teapot).with_state(AppState { database, settings }).layer(
        ServiceBuilder::new()
            .layer(axum::middleware::from_fn(clacks_overhead))
            .layer(TraceLayer::new_for_http().make_span_with(crate::trace::make_request_span))
            .layer(axum::middleware::from_fn(metrics::track_metrics))
            .layer(axum::middleware::from_fn(semver)),
    )
}
