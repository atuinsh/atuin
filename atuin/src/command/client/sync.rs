use clap::Subcommand;
use eyre::{Result, WrapErr};

use atuin_client::{
    database::Database,
    encryption,
    history::store::HistoryStore,
    record::{sqlite_store::SqliteStore, sync},
    settings::Settings,
};

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

    Status,
}

impl Cmd {
    pub async fn run(
        self,
        settings: Settings,
        db: &impl Database,
        store: SqliteStore,
    ) -> Result<()> {
        match self {
            Self::Sync { force } => run(&settings, force, db, store).await,
            Self::Login(l) => l.run(&settings).await,
            Self::Logout => account::logout::run(&settings),
            Self::Register(r) => r.run(&settings).await,
            Self::Status => status::run(&settings, db).await,
            Self::Key { base64 } => {
                use atuin_client::encryption::{encode_key, load_key};
                let key = load_key(&settings).wrap_err("could not load encryption key")?;

                if base64 {
                    let encode = encode_key(&key).wrap_err("could not encode encryption key")?;
                    println!("{encode}");
                } else {
                    let mnemonic = bip39::Mnemonic::from_entropy(&key, bip39::Language::English)
                        .map_err(|_| eyre::eyre!("invalid key"))?;
                    println!("{mnemonic}");
                }
                Ok(())
            }
        }
    }
}

async fn run(
    settings: &Settings,
    force: bool,
    db: &impl Database,
    store: SqliteStore,
) -> Result<()> {
    if settings.sync.records {
        let (diff, _) = sync::diff(settings, &store).await?;
        let operations = sync::operations(diff, &store).await?;
        let (uploaded, downloaded) = sync::sync_remote(operations, &store, settings).await?;

        let encryption_key: [u8; 32] = encryption::load_key(settings)
            .context("could not load encryption key")?
            .into();

        let host_id = Settings::host_id().expect("failed to get host_id");
        let history_store = HistoryStore::new(store.clone(), host_id, encryption_key);

        history_store.incremental_build(db, &downloaded).await?;

        println!("{uploaded}/{} up/down to record store", downloaded.len());
    } else {
        atuin_client::sync::sync(settings, force, db).await?;
    }

    println!(
        "Sync complete! {} items in history database, force: {}",
        db.history_count(true).await?,
        force
    );

    Ok(())
}
