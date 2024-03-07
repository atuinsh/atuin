use eyre::{Result, WrapErr};
use tokio::time::{self, MissedTickBehavior};

use atuin_client::{
    encryption,
    history::store::HistoryStore,
    record::{sqlite_store::SqliteStore, sync},
    settings::Settings,
};

pub async fn worker(settings: Settings, store: SqliteStore) -> Result<()> {
    tracing::info!("booting sync worker");
    let mut ticker = time::interval(time::Duration::from_secs(5));

    // IMPORTANT: without this, if we miss ticks because a sync takes ages or is otherwise delayed,
    // we may end up running a lot of syncs in a hot loop. No bueno!
    ticker.set_missed_tick_behavior(MissedTickBehavior::Skip);

    loop {
        ticker.tick().await;
        tracing::info!("sync worker tick");

        let (uploaded, downloaded) = sync::sync(&settings, &store).await?;

        tracing::info!(
            uploaded = ?uploaded,
            downloaded = ?downloaded,
            "sync complete"
        );
    }
}
