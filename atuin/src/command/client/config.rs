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

use super::search::format_duration_into;

mod alias;

#[derive(Subcommand, Debug)]
#[command(infer_subcommands = true)]
pub enum Cmd {
    #[command(subcommand)]
    Alias(alias::Cmd),
}

impl Cmd {
    pub async fn run(self, settings: &Settings, store: SqliteStore) -> Result<()> {
        match self {
            Self::Alias(cmd) => cmd.run(settings, store).await,
        }
    }
}
