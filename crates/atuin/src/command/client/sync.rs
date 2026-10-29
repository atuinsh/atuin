use std::sync::Arc;

use atuin_client::database::Sqlite;
use atuin_client::history::store::HistoryStore;
use atuin_client::record::sqlite_store::SqliteStore;
use atuin_client::record::sync::{ClientSource, SyncSession};
use atuin_client::settings::Settings;
use atuin_common::encryption::paseto_v4;
use atuin_common::userio;
use atuin_domain::record::{RecordId, RecordTag};
use clap::Subcommand;
use clap_couture::Couture;
use easy_cast::Conv;
use eyre::{Result, WrapErr};
use tracing::instrument;

mod status;

use crate::command::client::account;
#[cfg(feature = "daemon")]
use crate::command::client::daemon;
use crate::i18n::fl;

#[derive(Subcommand, Debug, Couture)]
#[command(infer_subcommands = true)]
#[couture(inherit = ["sync", "advanced"])]
pub enum Cmd {
    #[command(about = fl!("cmd-sync"))]
    #[couture(category = "sync")]
    Sync {
        #[arg(long, short, help = fl!("arg-sync-force"))]
        force: bool,
    },

    #[command(about = fl!("cmd-login"))]
    #[couture(category = "sync")]
    Login(account::login::Cmd),

    #[command(about = fl!("cmd-logout"))]
    #[couture(category = "sync")]
    Logout,

    #[command(about = fl!("cmd-register"))]
    #[couture(category = "sync")]
    Register(account::register::Cmd),

    #[command(about = fl!("cmd-key"))]
    #[couture(category = "advanced")]
    Key {
        #[arg(long, help = fl!("arg-key-base64"))]
        base64: bool,
    },

    #[command(about = fl!("cmd-status"))]
    #[couture(category = "sync")]
    Status,
}

impl Cmd {
    #[instrument(level = "trace", skip_all, err)]
    pub async fn run(
        self,
        settings: &Arc<Settings>,
        db: &Sqlite,
        store: SqliteStore,
    ) -> Result<()> {
        match self {
            Self::Sync { force } => run(settings, force, db, store).await,
            Self::Login(l) => l.run(settings, &store).await,
            Self::Logout => account::logout::run(settings).await,
            Self::Register(r) => r.run(settings, &store).await,
            Self::Status => status::run(settings).await,
            Self::Key { base64 } => {
                let key = paseto_v4::Key::try_load_from_path(&settings.key_path)
                    .wrap_err(fl!("sync-key-load-failed"))?;

                if base64 {
                    println!("{}", key.encode().dangerously_leak_secret());
                } else {
                    println!("{}", key.try_mnemonic().context(fl!("sync-key-invalid"))?);
                }
                Ok(())
            }
        }
    }
}

#[instrument(level = "trace", skip_all, fields(force), err)]
async fn run(settings: &Arc<Settings>, force: bool, db: &Sqlite, store: SqliteStore) -> Result<()> {
    let encryption_key = paseto_v4::Key::try_load_from_path(&settings.key_path)
        .context("could not load encryption key")?;

    let host_id = settings.host_id().await?;
    let history_store = HistoryStore::new(store.clone(), host_id, encryption_key.clone());

    // Build the session once and reuse it for both sync passes below. It owns a clone of the store
    // (a shared pool), so the second pass sees whatever the store-init writes locally.
    let session = SyncSession::builder()
        .store(store.clone())
        .client_source(ClientSource::FromSettings {
            settings,
            caps: None,
        })
        .build()
        .connect()
        .await?;

    let (uploaded, downloaded) = session.keyed(&encryption_key).sync().await?;

    build(settings, &store, db, &downloaded).await?;

    userio::log::info!("sync-up-down", uploaded = uploaded, downloaded = downloaded.len());

    let history_length = db.history_count(true).await?;
    let store_history_length = store.len_tag(&RecordTag::History).await?;

    if u64::conv(history_length) > store_history_length {
        userio::log::info!(
            "sync-history-mismatch",
            index = history_length,
            store = store_history_length
        );
        userio::log::info!("sync-store-init");

        // Internally we use the global filter mode, so this context is ignored.
        // don't recurse or loop here.
        history_store.init_store(db, settings).await?;

        userio::log::info!("sync-rerun");

        // we'll want to run sync once more, as there will now be stuff to upload -- re-key the same
        // session rather than reconnecting.
        let (uploaded, downloaded) = session.keyed(&encryption_key).sync().await?;

        build(settings, &store, db, &downloaded).await?;

        userio::log::info!("sync-up-down", uploaded = uploaded, downloaded = downloaded.len());
    }

    let count = db.history_count(true).await?;
    userio::log::info!("sync-complete", count = count, force = force.to_string());

    Ok(())
}

/// [`crate::sync::build`], then have a running daemon rebuild its search index and project the new
/// AI sessions, even if building the other stores failed: it owns those indexes, and otherwise sees
/// only what it syncs itself, or finds as it starts.
pub(super) async fn build(
    settings: &Settings,
    store: &SqliteStore,
    db: &Sqlite,
    downloaded: &[RecordId],
) -> Result<()> {
    let built = crate::sync::build(settings, store, db, Some(downloaded)).await;

    #[cfg(feature = "daemon")]
    if settings.daemon.enabled && !downloaded.is_empty() {
        if let Err(e) = daemon::rebuild_search_index(settings).await {
            userio::log::warn!("sync-rebuild-search-index-failed", error = e.to_string());
        }
        if let Err(e) = daemon::project_synced_ai_sessions(settings).await {
            userio::log::warn!("sync-build-threads-failed", error = e.to_string());
        }
    }

    built
}
