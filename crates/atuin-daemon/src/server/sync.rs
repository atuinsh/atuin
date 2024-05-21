use eyre::Result;
use rand::Rng;
use tokio::time::{self, MissedTickBehavior};

use atuin_client::database::Sqlite as HistoryDatabase;
use atuin_client::{
    encryption,
    history::store::HistoryStore,
    record::{sqlite_store::SqliteStore, sync},
    settings::Settings,
};

use atuin_dotfiles::store::{var::VarStore, AliasStore};

pub async fn worker(
    settings: Settings,
    store: SqliteStore,
    history_store: HistoryStore,
    history_db: HistoryDatabase,
) -> Result<()> {
    tracing::info!("booting sync worker");

    let encryption_key: [u8; 32] = encryption::load_key(&settings)?.into();
    let host_id = Settings::host_id().expect("failed to get host_id");
    let alias_store = AliasStore::new(store.clone(), host_id, encryption_key);
    let var_store = VarStore::new(store.clone(), host_id, encryption_key);

    // Don't backoff by more than 30 mins (with a random jitter of up to 1 min)
    let max_interval: f64 = 60.0 * 30.0 + rand::thread_rng().gen_range(0.0..60.0);

    let mut ticker = time::interval(time::Duration::from_secs(settings.daemon.sync_frequency));

    // IMPORTANT: without this, if we miss ticks because a sync takes ages or is otherwise delayed,
    // we may end up running a lot of syncs in a hot loop. No bueno!
    ticker.set_missed_tick_behavior(MissedTickBehavior::Skip);

    loop {
        ticker.tick().await;
        tracing::info!("sync worker tick");

        if !settings.logged_in() {
            tracing::debug!("not logged in, skipping sync tick");
            continue;
        }

        let res = sync::sync(&settings, &store).await;

        if let Err(e) = res {
            tracing::error!("sync tick failed with {e}");

            let mut rng = rand::thread_rng();

            let mut new_interval = ticker.period().as_secs_f64() * rng.gen_range(2.0..2.2);

            if new_interval > max_interval {
                new_interval = max_interval;
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

            history_store
                .incremental_build(&history_db, &downloaded)
                .await?;

            alias_store.build().await?;
            var_store.build().await?;

            // Reset backoff on success
            if ticker.period().as_secs() != settings.daemon.sync_frequency {
                ticker = time::interval(time::Duration::from_secs(settings.daemon.sync_frequency));
            }

            // store sync time
            tokio::task::spawn_blocking(Settings::save_sync_time).await??;
        }
    }
}
