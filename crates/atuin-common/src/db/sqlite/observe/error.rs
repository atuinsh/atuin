#[derive(Debug, thiserror::Error)]
pub enum ObserveError {
    #[error("failed to open the observer connection: {0}")]
    Connect(#[source] sqlx::Error),
    #[error("the initial replay/seed scan failed: {0}")]
    Seed(#[source] sqlx::Error),
    #[error("a query against the observed database failed: {0}")]
    Query(#[source] sqlx::Error),
}
