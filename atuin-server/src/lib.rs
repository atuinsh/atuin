#![forbid(unsafe_code)]

use std::future::Future;
use std::net::SocketAddr;
use std::sync::Arc;

use atuin_server_database::Database;
use axum::{serve, Router};
use axum_server::Handle;
use eyre::{Context, Result};

mod handlers;
mod metrics;
mod router;
mod utils;

use rustls::ServerConfig;
pub use settings::example_config;
pub use settings::Settings;

pub mod settings;

use tokio::net::TcpListener;
use tokio::signal;

#[cfg(target_family = "unix")]
async fn shutdown_signal() {
    let mut term = signal::unix::signal(signal::unix::SignalKind::terminate())
        .expect("failed to register signal handler");
    let mut interrupt = signal::unix::signal(signal::unix::SignalKind::interrupt())
        .expect("failed to register signal handler");

    tokio::select! {
        _ = term.recv() => {},
        _ = interrupt.recv() => {},
    };
    eprintln!("Shutting down gracefully...");
}

#[cfg(target_family = "windows")]
async fn shutdown_signal() {
    signal::windows::ctrl_c()
        .expect("failed to register signal handler")
        .recv()
        .await;
    eprintln!("Shutting down gracefully...");
}

pub async fn launch<Db: Database>(
    settings: Settings<Db::Settings>,
    addr: SocketAddr,
) -> Result<()> {
    if settings.tls.enable {
        launch_with_tls::<Db>(settings, addr, shutdown_signal()).await
    } else {
        launch_with_tcp_listener::<Db>(
            settings,
            TcpListener::bind(addr)
                .await
                .context("could not connect to socket")?,
            shutdown_signal(),
        )
        .await
    }
}

pub async fn launch_with_tcp_listener<Db: Database>(
    settings: Settings<Db::Settings>,
    listener: TcpListener,
    shutdown: impl Future<Output = ()> + Send + 'static,
) -> Result<()> {
    let r = make_router::<Db>(settings).await?;

    serve(listener, r.into_make_service())
        .with_graceful_shutdown(shutdown)
        .await?;

    Ok(())
}

async fn launch_with_tls<Db: Database>(
    settings: Settings<Db::Settings>,
    addr: SocketAddr,
    shutdown: impl Future<Output = ()>,
) -> Result<()> {
    let certificates = settings.tls.certificates()?;
    let pkey = settings.tls.private_key()?;

    let server_config = ServerConfig::builder()
        .with_safe_defaults()
        .with_no_client_auth()
        .with_single_cert(certificates, pkey)?;

    let server_config = Arc::new(server_config);
    let rustls_config = axum_server::tls_rustls::RustlsConfig::from_config(server_config);

    let r = make_router::<Db>(settings).await?;

    let handle = Handle::new();

    let server = axum_server::bind_rustls(addr, rustls_config)
        .handle(handle.clone())
        .serve(r.into_make_service());

    tokio::select! {
        _ = server => {}
        _ = shutdown => {
            handle.graceful_shutdown(None);
        }
    }

    Ok(())
}

// The separate listener means it's much easier to ensure metrics are not accidentally exposed to
// the public.
pub async fn launch_metrics_server(host: String, port: u16) -> Result<()> {
    let listener = TcpListener::bind((host, port))
        .await
        .context("failed to bind metrics tcp")?;

    let recorder_handle = metrics::setup_metrics_recorder();

    let router = Router::new().route(
        "/metrics",
        axum::routing::get(move || std::future::ready(recorder_handle.render())),
    );

    serve(listener, router.into_make_service())
        .with_graceful_shutdown(shutdown_signal())
        .await?;

    Ok(())
}

async fn make_router<Db: Database>(
    settings: Settings<<Db as Database>::Settings>,
) -> Result<Router, eyre::Error> {
    let db = Db::new(&settings.db_settings)
        .await
        .wrap_err_with(|| format!("failed to connect to db: {:?}", settings.db_settings))?;
    let r = router::router(db, settings);
    Ok(r)
}
