use std::time::Duration;

use atuin_server_database::Database;
use axum::{Json, extract::State, http, response::IntoResponse};
use serde::Serialize;

use crate::router::AppState;

/// How long to wait for the database health check before declaring it
/// unhealthy. Keeps probe requests from piling up behind a hung database.
const DB_HEALTH_CHECK_TIMEOUT: Duration = Duration::from_secs(3);

#[derive(Serialize)]
pub struct HealthResponse {
    pub status: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub checks: Option<Checks>,
}

#[derive(Serialize)]
pub struct Checks {
    pub database: &'static str,
}

pub async fn health_check<DB: Database>(state: State<AppState<DB>>) -> impl IntoResponse {
    // When the DB check is disabled, preserve the original behavior exactly:
    // a flat 200 {"status":"healthy"} with no checks object and no DB access.
    if !state.settings.db_health_check {
        return (
            http::StatusCode::OK,
            Json(HealthResponse {
                status: "healthy",
                checks: None,
            }),
        );
    }

    // Run the DB check against the existing pool, with a fail-fast timeout.
    let db_healthy =
        match tokio::time::timeout(DB_HEALTH_CHECK_TIMEOUT, state.database.health_check()).await {
            Ok(Ok(())) => true,
            Ok(Err(e)) => {
                tracing::error!(error = ?e, "healthz: database check failed");
                false
            }
            Err(_) => {
                tracing::warn!(
                    timeout = ?DB_HEALTH_CHECK_TIMEOUT,
                    "healthz: database check timed out"
                );
                false
            }
        };

    if db_healthy {
        (
            http::StatusCode::OK,
            Json(HealthResponse {
                status: "healthy",
                checks: Some(Checks {
                    database: "healthy",
                }),
            }),
        )
    } else {
        (
            http::StatusCode::SERVICE_UNAVAILABLE,
            Json(HealthResponse {
                status: "unhealthy",
                checks: Some(Checks {
                    database: "unreachable",
                }),
            }),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn healthy_without_checks_serializes_flat() {
        let body = HealthResponse {
            status: "healthy",
            checks: None,
        };
        let json = serde_json::to_string(&body).unwrap();
        assert_eq!(json, r#"{"status":"healthy"}"#);
    }

    #[test]
    fn unhealthy_with_checks_serializes_nested() {
        let body = HealthResponse {
            status: "unhealthy",
            checks: Some(Checks {
                database: "unreachable",
            }),
        };
        let json = serde_json::to_string(&body).unwrap();
        assert_eq!(
            json,
            r#"{"status":"unhealthy","checks":{"database":"unreachable"}}"#
        );
    }
}
