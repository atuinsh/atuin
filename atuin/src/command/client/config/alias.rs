use std::{
    fmt::{self, Display},
    io::{self, IsTerminal, Write},
    time::Duration,
};

use atuin_common::utils::{self, Escapable as _};
use clap::Subcommand;
use eyre::{Context, Result};
use runtime_format::{FormatKey, FormatKeyError, ParseSegment, ParsedFmt};

use atuin_client::{
    database::{current_context, Database},
    encryption,
    history::{store::HistoryStore, History},
    record::sqlite_store::SqliteStore,
    settings::{
        FilterMode::{Directory, Global, Session},
        Settings, Timezone,
    },
};

use log::{debug, warn};
use time::{macros::format_description, OffsetDateTime};

use atuin_config::store::AliasStore;

#[derive(Subcommand, Debug)]
#[command(infer_subcommands = true)]
pub enum Cmd {
    Set { name: String, value: String },
}

impl Cmd {
    async fn set(
        &self,
        _settings: &Settings,
        store: AliasStore,
        name: String,
        value: String,
    ) -> Result<()> {
        store.set(&name, &value).await?;

        Ok(())
    }

    pub async fn run(&self, settings: &Settings, store: SqliteStore) -> Result<()> {
        let encryption_key: [u8; 32] = encryption::load_key(settings)
            .context("could not load encryption key")?
            .into();
        let host_id = Settings::host_id().expect("failed to get host_id");

        let alias_store = AliasStore::new(store, host_id, encryption_key);

        match self {
            Self::Set { name, value } => {
                self.set(settings, alias_store, name.clone(), value.clone())
                    .await
            }
        }
    }
}
