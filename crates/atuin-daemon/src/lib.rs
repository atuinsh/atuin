use atuin_client::database::Sqlite as HistoryDatabase;
use atuin_client::{record::sqlite_store::SqliteStore, settings::Settings};
use eyre::Result;

use crate::server::listen;

pub mod client;
pub mod events;
pub mod history;
pub mod search;
pub mod search_server;
pub mod server;

pub async fn boot(
    settings: Settings,
    store: SqliteStore,
    history_db: HistoryDatabase,
) -> Result<()> {
    listen(settings, store, history_db).await
}
