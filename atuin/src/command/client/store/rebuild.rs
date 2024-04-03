use atuin_dotfiles::store::AliasStore;
use clap::Args;
use eyre::{bail, Result};

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
        let alias_store = AliasStore::new(store, host_id, encryption_key);

        alias_store.build().await?;

        Ok(())
    }
}
