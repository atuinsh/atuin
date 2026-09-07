use atuin_client::database::Sqlite;
use atuin_client::history::store::HistoryStore;
use atuin_client::record::sqlite_store::SqliteStore;
use atuin_client::record::sync::{ClientSource, SyncEngine};
use atuin_client::settings::Settings;
use atuin_common::encryption::paseto_v4;
use atuin_domain::record::RecordTag;
use clap::Subcommand;
use easy_cast::Conv;
use eyre::{Result, WrapErr};
use tracing::instrument;

mod status;

use crate::command::client::account;

#[derive(Subcommand, Debug)]
#[command(infer_subcommands = true)]
pub enum Cmd {
    /// Sync with the configured server
    Sync {
        /// Force re-download everything
        #[arg(long, short)]
        force: bool,
    },

    /// Login to the configured server
    Login(account::login::Cmd),

    /// Log out
    Logout,

    /// Register with the configured server
    Register(account::register::Cmd),

    /// Print the encryption key for transfer to another machine
    Key {
        /// Switch to base64 output of the key
        #[arg(long)]
        base64: bool,
    },

    /// Display the sync status
    Status,
}

impl Cmd {
    #[instrument(level = "trace", skip_all, err)]
    pub async fn run(self, settings: Settings, db: &Sqlite, store: SqliteStore) -> Result<()> {
        match self {
            Self::Sync { force } => run(&settings, force, db, store).await,
            Self::Login(l) => l.run(&settings, &store).await,
            Self::Logout => account::logout::run().await,
            Self::Register(r) => r.run(&settings, &store).await,
            Self::Status => status::run(&settings).await,
            Self::Key { base64 } => {
                let key = paseto_v4::Key::try_load_from_path(&settings.key_path)
                    .wrap_err("could not load encryption key")?;

                if base64 {
                    println!("{}", key.encode().dangerously_leak_secret());
                } else {
                    println!("{}", key.try_mnemonic().context("invalid key")?);
                }
                Ok(())
            }
        }
    }
}

#[instrument(level = "trace", skip_all, fields(force), err)]
async fn run(settings: &Settings, force: bool, db: &Sqlite, store: SqliteStore) -> Result<()> {
    let encryption_key = paseto_v4::Key::try_load_from_path(&settings.key_path)
        .context("could not load encryption key")?;

    let host_id = Settings::host_id().await?;
    let history_store = HistoryStore::new(store.clone(), host_id, encryption_key.clone());

    // Build the engine once and reuse it for both sync passes below. It owns a clone of the store
    // (a shared pool), so the second pass sees whatever the store-init writes locally.
    let engine = SyncEngine::builder()
        .store(store.clone())
        .client_source(ClientSource::FromSettings {
            settings,
            caps: None,
        })
        .build()
        .connect()
        .await
        .map_err(crate::print_error::format_sync_error)?;

    let (uploaded, downloaded) = engine
        .keyed(&encryption_key)
        .sync()
        .await
        .map_err(crate::print_error::format_sync_error)?;

    crate::sync::build(settings, &store, db, Some(&downloaded)).await?;

    println!("{uploaded}/{} up/down to record store", downloaded.len());

    let history_length = db.history_count(true).await?;
    let store_history_length = store.len_tag(&RecordTag::History).await?;

    if u64::conv(history_length) > store_history_length {
        println!("{history_length} in history index, but {store_history_length} in history store");
        println!("Running automatic history store init...");

        // Internally we use the global filter mode, so this context is ignored.
        // don't recurse or loop here.
        history_store.init_store(db).await?;

        println!("Re-running sync due to new records locally");

        // we'll want to run sync once more, as there will now be stuff to upload -- re-key the same
        // engine rather than reconnecting.
        let (uploaded, downloaded) = engine
            .keyed(&encryption_key)
            .sync()
            .await
            .map_err(crate::print_error::format_sync_error)?;

        crate::sync::build(settings, &store, db, Some(&downloaded)).await?;

        println!("{uploaded}/{} up/down to record store", downloaded.len());
    }

    println!(
        "Sync complete! {} items in history database, force: {}",
        db.history_count(true).await?,
        force
    );

    Ok(())
}
