use std::net::{IpAddr, SocketAddr};

use axum::{routing::get, Router, Server};
use eyre::Result;

use crate::daemon::handlers;
use crate::settings::Settings;

fn router(settings: Settings) -> Router {
    Router::new().route("/", get(handlers::index))
}

pub async fn listen(settings: Settings, host: String, port: u16) -> Result<()> {
    let host = host.parse::<IpAddr>()?;

    let r = router(settings);

    Server::bind(&SocketAddr::new(host, port))
        .serve(r.into_make_service())
        .await?;

    Ok(())
}
