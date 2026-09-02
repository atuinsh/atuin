use atuin_client::database::Sqlite;
use atuin_client::history::store::HistoryStore;
use atuin_client::record::sqlite_store::SqliteStore;
use atuin_client::settings::Settings;
use atuin_common::encryption::paseto_v4;
use atuin_dotfiles::store::AliasStore;
use atuin_dotfiles::store::var::VarStore;
use atuin_scripts::store::ScriptStore;
use clap::Args;
use eyre::{Context as _, Result, bail};

#[cfg(feature = "daemon")]
use crate::command::client::daemon as daemon_cmd;

#[derive(Args, Debug)]
pub struct Rebuild {
    pub tag: String,
}

impl Rebuild {
    pub async fn run(
        &self,
        settings: &Settings,
        store: SqliteStore,
        database: &Sqlite,
    ) -> Result<()> {
        // keep it as a string and not an enum atm
        // would be super cool to build this dynamically in the future
        // eg register handles for rebuilding various tags without having to make this part of the
        // binary big
        match self.tag.as_str() {
            "history" => {
                self.rebuild_history(settings, store.clone(), database).await?;
            }

            "dotfiles" => {
                self.rebuild_dotfiles(settings, store.clone()).await?;
            }

            "scripts" => {
                self.rebuild_scripts(settings, store.clone()).await?;
            }

            tag => {
                bail!("unknown tag: {tag}");
            }
        }

        Ok(())
    }

    async fn rebuild_history(
        &self,
        settings: &Settings,
        store: SqliteStore,
        database: &Sqlite,
    ) -> Result<()> {
        let encryption_key = paseto_v4::Key::try_load_from_path(&settings.key_path)
            .context("could not load encryption key")?;

        let host_id = Settings::host_id().await?;
        let history_store = HistoryStore::new(store, host_id, encryption_key);

        history_store.build(database).await?;

        #[cfg(feature = "daemon")]
        daemon_cmd::emit_event(settings, atuin_daemon::DaemonEvent::HistoryRebuilt).await;

        Ok(())
    }

    async fn rebuild_dotfiles(&self, settings: &Settings, store: SqliteStore) -> Result<()> {
        let encryption_key = paseto_v4::Key::try_load_from_path(&settings.key_path)
            .context("could not load encryption key")?;

        let host_id = Settings::host_id().await?;

        let alias_store = AliasStore::new(store.clone(), host_id, encryption_key.clone());
        let var_store = VarStore::new(store.clone(), host_id, encryption_key);

        alias_store.build().await?;
        var_store.build().await?;

        Ok(())
    }

    async fn rebuild_scripts(&self, settings: &Settings, store: SqliteStore) -> Result<()> {
        let encryption_key = paseto_v4::Key::try_load_from_path(&settings.key_path)
            .context("could not load encryption key")?;
        let host_id = Settings::host_id().await?;
        let script_store = ScriptStore::new(store, host_id, encryption_key);
        let database = atuin_scripts::database::Database::new(
            settings.scripts.db_path.clone(),
            std::time::Duration::from_secs(1),
        )
        .await?;

        script_store.build(database).await?;

        Ok(())
    }
}
