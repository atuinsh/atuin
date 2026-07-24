use atuin_server::{Settings, make_router, settings::Metrics};
use atuin_server_database::DbSettings;
use atuin_server_sqlite::Sqlite;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use tower::ServiceExt;

/// `make_router` is the seam benchmarks and tests use to compose middleware onto the server
/// before serving it. This proves it is reachable from outside the crate and that it produces a
/// working router against the SQLite backend.
#[tokio::test]
async fn make_router_builds_a_serviceable_router() {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("server.db");

    let settings = Settings {
        host: "127.0.0.1".to_owned(),
        port: 0,
        path: String::new(),
        open_registration: true,
        max_record_size: 1024 * 1024 * 1024,
        register_webhook_url: None,
        register_webhook_username: String::new(),
        db_settings: DbSettings {
            db_uri: format!("sqlite://{}", db_path.display()),
            read_db_uri: None,
        },
        metrics: Metrics::default(),
        fake_version: None,
    };

    let router = make_router::<Sqlite>(settings).await.unwrap();

    let response = router
        .oneshot(
            Request::builder()
                .uri("/healthz")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
}
