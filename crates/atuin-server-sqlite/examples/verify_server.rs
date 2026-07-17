// Throwaway harness: runs the real atuin server on the sqlite backend, so a sync flow can be
// driven end to end without a postgres instance. Not part of the build; delete when done.
use atuin_server::{Settings, launch_with_tcp_listener, settings::Metrics};
use atuin_server_database::DbSettings;
use atuin_server_sqlite::Sqlite;
use tokio::net::TcpListener;

#[tokio::main]
async fn main() -> eyre::Result<()> {
    let db = std::env::args().nth(1).expect("usage: verify_server <db-path> <port>");
    let port: u16 = std::env::args().nth(2).expect("need port").parse()?;

    let settings = Settings {
        host: "127.0.0.1".to_owned(),
        port,
        path: String::new(),
        open_registration: true,
        max_record_size: 1024 * 1024 * 1024,
        register_webhook_url: None,
        register_webhook_username: String::new(),
        db_settings: DbSettings { db_uri: format!("sqlite://{db}?mode=rwc"), read_db_uri: None },
        metrics: Metrics::default(),
        fake_version: None,
    };

    let listener = TcpListener::bind(("127.0.0.1", port)).await?;
    eprintln!("VERIFY_SERVER listening on {}", listener.local_addr()?);
    launch_with_tcp_listener::<Sqlite>(settings, listener, async { std::future::pending().await }).await
}
