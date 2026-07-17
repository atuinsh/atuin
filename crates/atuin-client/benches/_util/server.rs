use std::net::SocketAddr;
use std::time::Duration;

use atuin_client::api_client::{self, AuthToken, Client};
use atuin_common::utils::uuid_v7;
use atuin_server::settings::Metrics;
use atuin_server::{Settings as ServerSettings, make_router};
use atuin_server_database::DbSettings;
use atuin_server_sqlite::Sqlite;
use axum::extract::{Request, State};
use axum::middleware::{self, Next};
use axum::response::Response;
use tempfile::TempDir;
use tokio::net::TcpListener;
use tokio::task::JoinHandle;

/// Sleeps for one round-trip time before handling each request.
///
/// Loopback RTT is effectively zero, which is exactly the cost that a larger sync page size
/// removes — so without this, a localhost benchmark measures SQLite and serialization and reports
/// the paging change as noise. Delaying per *request* rather than per byte is deliberate: it
/// models one RTT per round-trip regardless of body size, whereas a byte-level delay would charge
/// a large page once per TCP segment and invert the result.
///
/// This models latency only. Bandwidth is still loopback-fast, and connection setup is not
/// delayed (reqwest keep-alive means that happens once per benchmark anyway).
async fn latency(State(rtt): State<Duration>, request: Request, next: Next) -> Response {
    if !rtt.is_zero() {
        tokio::time::sleep(rtt).await;
    }

    next.run(request).await
}

/// A real `atuin-server` running in-process, reachable over a real TCP socket on loopback.
pub struct BenchServer {
    address: String,
    // Holds the server's SQLite file. Dropped, and so deleted, with the server.
    _db_dir: TempDir,
    handle: JoinHandle<()>,
}

impl BenchServer {
    const CONNECT_TIMEOUT_S: u64 = 5;

    /// Generous: a page at a high injected RTT is slow, and a timeout mid-benchmark would show up
    /// as a confusing panic rather than a slow number.
    const TIMEOUT_S: u64 = 120;

    /// Start a server that sleeps `rtt` before handling each request.
    pub async fn start(rtt: Duration) -> Self {
        let db_dir = tempfile::tempdir().unwrap();
        let db_path = db_dir.path().join("server.db");

        let settings = ServerSettings {
            host: "127.0.0.1".to_owned(),
            port: 0,
            path: String::new(),
            open_registration: true,
            max_record_size: 1024 * 1024 * 1024,
            register_webhook_url: None,
            register_webhook_username: String::new(),
            db_settings: DbSettings {
                // Not `sqlite::memory:` — the server's pool would hand each connection its own
                // private database, so migrations and queries would disagree about the schema.
                db_uri: format!("sqlite://{}", db_path.display()),
                read_db_uri: None,
            },
            metrics: Metrics::default(),
            fake_version: None,
        };

        let router = make_router::<Sqlite>(settings)
            .await
            .unwrap()
            .layer(middleware::from_fn_with_state(rtt, latency));

        // Bind before spawning: the socket is listening the moment `start` returns, so callers
        // never need to sleep and hope the server came up.
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr: SocketAddr = listener.local_addr().unwrap();

        let handle = tokio::spawn(async move {
            axum::serve(listener, router.into_make_service())
                .await
                .expect("bench server failed");
        });

        Self {
            address: format!("http://{addr}"),
            _db_dir: db_dir,
            handle,
        }
    }

    /// Register a fresh user and return a client authenticated as them.
    pub async fn register(&self) -> Client<'_> {
        let username = uuid_v7().as_simple().to_string();
        let email = format!("{username}@example.com");
        let password = uuid_v7().as_simple().to_string();

        let registration = api_client::register(&self.address, &username, &email, &password)
            .await
            .unwrap();

        // The server rejects any scheme other than `Token` (atuin-server/src/router.rs:50).
        Client::new(
            &self.address,
            AuthToken::Token(registration.session),
            Self::CONNECT_TIMEOUT_S,
            Self::TIMEOUT_S,
        )
        .unwrap()
    }
}

impl Drop for BenchServer {
    fn drop(&mut self) {
        self.handle.abort();
    }
}
