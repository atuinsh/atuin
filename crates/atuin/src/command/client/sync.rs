use std::sync::Arc;

use atuin_client::database::Sqlite;
use atuin_client::history::store::HistoryStore;
use atuin_client::record::sqlite_store::SqliteStore;
use atuin_client::record::sync::{ClientSource, SyncSession};
use atuin_client::settings::Settings;
use atuin_common::encryption::paseto_v4;
use atuin_domain::record::{RecordId, RecordTag};
use clap::Subcommand;
use easy_cast::Conv;
use eyre::{Result, WrapErr};
use tracing::instrument;

mod status;

use crate::command::client::account;
#[cfg(feature = "daemon")]
use crate::command::client::daemon;
use crate::i18n::fl;

#[derive(Subcommand, Debug)]
#[command(infer_subcommands = true)]
pub enum Cmd {
    #[command(about = fl!("cmd-sync"))]
    Sync {
        #[arg(long, short, help = fl!("arg-sync-force"))]
        force: bool,
    },

    #[command(about = fl!("cmd-login"))]
    Login(account::login::Cmd),

    #[command(about = fl!("cmd-logout"))]
    Logout,

    #[command(about = fl!("cmd-register"))]
    Register(account::register::Cmd),

    #[command(about = fl!("cmd-key"))]
    Key {
        #[arg(long, help = fl!("arg-key-base64"))]
        base64: bool,
    },

    #[command(about = fl!("cmd-status"))]
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
        .await
        .map_err(crate::print_error::format_sync_error)?;

    let (uploaded, downloaded) = session
        .keyed(&encryption_key)
        .sync()
        .await
        .map_err(crate::print_error::format_sync_error)?;

    build(settings, &store, db, &downloaded).await?;

    println!("{}", fl!("sync-up-down", uploaded = uploaded, downloaded = downloaded.len()));

    let history_length = db.history_count(true).await?;
    let store_history_length = store.len_tag(&RecordTag::History).await?;

    if u64::conv(history_length) > store_history_length {
        println!(
            "{}",
            fl!("sync-history-mismatch", index = history_length, store = store_history_length)
        );
        println!("{}", fl!("sync-store-init"));

        // Internally we use the global filter mode, so this context is ignored.
        // don't recurse or loop here.
        history_store.init_store(db, settings).await?;

        println!("{}", fl!("sync-rerun"));

        // we'll want to run sync once more, as there will now be stuff to upload -- re-key the same
        // session rather than reconnecting.
        let (uploaded, downloaded) = session
            .keyed(&encryption_key)
            .sync()
            .await
            .map_err(crate::print_error::format_sync_error)?;

        build(settings, &store, db, &downloaded).await?;

        println!("{}", fl!("sync-up-down", uploaded = uploaded, downloaded = downloaded.len()));
    }

    println!(
        "{}",
        fl!("sync-complete", count = db.history_count(true).await?, force = force.to_string())
    );

    Ok(())
}

/// [`crate::sync::build`], then have a running daemon project the AI sessions `downloaded` brought,
/// even if building the other stores failed: it owns their index, and otherwise projects only what
/// it syncs itself, or finds as it starts.
pub(super) async fn build(
    settings: &Settings,
    store: &SqliteStore,
    db: &Sqlite,
    downloaded: &[RecordId],
) -> Result<()> {
    let built = crate::sync::build(settings, store, db, Some(downloaded)).await;

    #[cfg(feature = "daemon")]
    if settings.daemon.enabled
        && !downloaded.is_empty()
        && let Err(e) = daemon::project_synced_ai_sessions(settings).await
    {
        eprintln!("Warning: failed to build threads: {e}");
    }

    built
}
