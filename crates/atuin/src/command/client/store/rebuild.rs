use atuin_dotfiles::store::{AliasStore, var::VarStore};
use atuin_scripts::store::ScriptStore;
use clap::Args;
use eyre::{Result, bail};

use atuin_client::{
    database::Database, encryption, history::store::HistoryStore,
    record::sqlite_store::SqliteStore, settings::Settings,
};

#[derive(Args, Debug)]
pub struct Rebuild {
    pub tag: String,
}

impl Rebuild {
    pub async fn run(
        &self,
        settings: &Settings,
        store: SqliteStore,
        database: &dyn Database,
    ) -> Result<()> {
        // keep it as a string and not an enum atm
        // would be super cool to build this dynamically in the future
        // eg register handles for rebuilding various tags without having to make this part of the
        // binary big
        match self.tag.as_str() {
            "history" => {
                self.rebuild_history(settings, store.clone(), database)
                    .await?;
            }

            "dotfiles" => {
                self.rebuild_dotfiles(settings, store.clone()).await?;
            }

            "scripts" => {
                self.rebuild_scripts(settings, store.clone()).await?;
            }

            tag => bail!("unknown tag: {tag}"),
        }

        Ok(())
    }

    async fn rebuild_history(
        &self,
        settings: &Settings,
        store: SqliteStore,
        database: &dyn Database,
    ) -> Result<()> {
        let encryption_key: [u8; 32] = encryption::load_key(settings)?.into();

        let host_id = Settings::host_id().expect("failed to get host_id");
        let history_store = HistoryStore::new(store, host_id, encryption_key);

        history_store.build(database).await?;

        Ok(())
    }

    async fn rebuild_dotfiles(&self, settings: &Settings, store: SqliteStore) -> Result<()> {
        let encryption_key: [u8; 32] = encryption::load_key(settings)?.into();

        let host_id = Settings::host_id().expect("failed to get host_id");

        let alias_store = AliasStore::new(store.clone(), host_id, encryption_key);
        let var_store = VarStore::new(store.clone(), host_id, encryption_key);

        alias_store.build().await?;
        var_store.build().await?;

        Ok(())
    }

    async fn rebuild_scripts(&self, settings: &Settings, store: SqliteStore) -> Result<()> {
        let encryption_key: [u8; 32] = encryption::load_key(settings)?.into();
        let host_id = Settings::host_id().expect("failed to get host_id");
        let script_store = ScriptStore::new(store, host_id, encryption_key);
        let database =
            atuin_scripts::database::Database::new(settings.scripts.database_path.clone(), 1.0)
                .await?;

        script_store.build(database).await?;

        Ok(())
    }
}
