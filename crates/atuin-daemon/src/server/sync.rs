use eyre::Result;
use rand::Rng;
use tokio::time::{self, MissedTickBehavior};

use atuin_client::{
    record::{sqlite_store::SqliteStore, sync},
    settings::Settings,
};

pub async fn worker(settings: Settings, store: SqliteStore) -> Result<()> {
    tracing::info!("booting sync worker");

    let mut ticker = time::interval(time::Duration::from_secs(settings.daemon.sync_frequency));

    // IMPORTANT: without this, if we miss ticks because a sync takes ages or is otherwise delayed,
    // we may end up running a lot of syncs in a hot loop. No bueno!
    ticker.set_missed_tick_behavior(MissedTickBehavior::Skip);

    loop {
        ticker.tick().await;
        tracing::info!("sync worker tick");

        let res = sync::sync(&settings, &store).await;

        if let Err(e) = res {
            tracing::error!("sync tick failed with {e}");
            let mut rng = rand::thread_rng();

            let new_interval = ticker.period().as_secs_f64() * rng.gen_range(2.0..2.2);

            // Don't backoff by more than 30 mins
            if new_interval > 60.0 * 30.0 {
                continue;
            }

            ticker = time::interval(time::Duration::from_secs(new_interval as u64));
            ticker.reset_after(time::Duration::from_secs(new_interval as u64));

            tracing::error!("backing off, next sync tick in {new_interval}");
        } else {
            let (uploaded, downloaded) = res.unwrap();

            tracing::info!(
                uploaded = ?uploaded,
                downloaded = ?downloaded,
                "sync complete"
            );

            // Reset backoff on success
            if ticker.period().as_secs() != settings.daemon.sync_frequency {
                ticker = time::interval(time::Duration::from_secs(settings.daemon.sync_frequency));
            }
        }
    }
}
