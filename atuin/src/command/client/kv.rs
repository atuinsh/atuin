use clap::Subcommand;
use eyre::Result;
use serde::{Deserialize, Serialize};

use atuin_client::{kv::KvRecord, record::store::Store, settings::Settings};

#[derive(Subcommand)]
#[command(infer_subcommands = true)]
pub enum Cmd {
    // atuin kv set foo bar bar
    Set {
        #[arg(long, short)]
        key: String,

        value: String,
    },

    // atuin kv get foo => bar baz
    Get {
        key: String,
    },
}

impl Cmd {
    pub async fn run(&self, settings: &Settings, store: &mut impl Store) -> Result<()> {
        let kv_version = "v0";
        let kv_tag = "kv";
        let host_id = Settings::host_id().expect("failed to get host_id");

        match self {
            Self::Set { key, value } => {
                let record = KvRecord {
                    key: key.to_string(),
                    value: value.to_string(),
                };

                let bytes = record.serialize()?;
                let record = atuin_common::record::Record::new(
                    host_id,
                    kv_version.to_string(),
                    kv_tag.to_string(),
                    bytes,
                );

                store.push(record).await?;

                Ok(())
            }

            Self::Get { key } => Ok(()),
        }
    }
}
