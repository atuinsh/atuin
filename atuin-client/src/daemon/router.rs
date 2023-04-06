use std::net::{IpAddr, SocketAddr};

use axum::{
    response::IntoResponse,
    routing::{get},
    Router,
    Server,
};
use eyre::{Result};

use crate::settings::Settings;

async fn teapot() -> impl IntoResponse {
    (http::StatusCode::IM_A_TEAPOT, "☕")
}

fn router(
    settings: Settings,
) -> Router {
    let routes = Router::new()
        .route("/", get(handlers::index));

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

pub async fn listen(settings: Settings, host: String, port: u16) -> Result<()> {
    let host = host.parse::<IpAddr>()?;

    let r = router(settings);

    Server::bind(&SocketAddr::new(host, port))
        .serve(r.into_make_service())
        .await?;

    Ok(())
}
