use clap::{Args, Subcommand};
use eyre::{bail, Result};

use atuin_client::{
    database::Database,
    encryption,
    history::store::HistoryStore,
    record::{sqlite_store::SqliteStore, store::Store},
    settings::Settings,
};
use time::OffsetDateTime;

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
}

#[derive(Subcommand, Debug)]
#[command(infer_subcommands = true)]
pub enum Cmd {
    Status,
    Rebuild(Rebuild),
}

impl Cmd {
    pub async fn run(
        &self,
        settings: &Settings,
        database: &dyn Database,
        store: SqliteStore,
    ) -> Result<()> {
        match self {
            Self::Status => self.status(store).await,
            Self::Rebuild(rebuild) => rebuild.run(settings, store, database).await,
        }
    }

    pub async fn status(&self, store: SqliteStore) -> Result<()> {
        let host_id = Settings::host_id().expect("failed to get host_id");

        let status = store.status().await?;

        // TODO: should probs build some data structure and then pretty-print it or smth
        for (host, st) in &status.hosts {
            let host_string = if host == &host_id {
                format!("host: {} <- CURRENT HOST", host.0.as_hyphenated())
            } else {
                format!("host: {}", host.0.as_hyphenated())
            };

            println!("{host_string}");

            for (tag, idx) in st {
                println!("\tstore: {tag}");

                let first = store.first(*host, tag).await?;
                let last = store.last(*host, tag).await?;

                println!("\t\tidx: {idx}");

                if let Some(first) = first {
                    println!("\t\tfirst: {}", first.id.0.as_hyphenated());

                    let time =
                        OffsetDateTime::from_unix_timestamp_nanos(i128::from(first.timestamp))?;
                    println!("\t\t\tcreated: {time}");
                }

                if let Some(last) = last {
                    println!("\t\tlast: {}", last.id.0.as_hyphenated());

                    let time =
                        OffsetDateTime::from_unix_timestamp_nanos(i128::from(last.timestamp))?;
                    println!("\t\t\tcreated: {time}");
                }
            }

            println!();
        }

        Ok(())
    }
}
