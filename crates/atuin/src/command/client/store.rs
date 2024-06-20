use clap::Subcommand;
use eyre::Result;

use atuin_client::{
    database::Database,
    record::{sqlite_store::SqliteStore, store::Store},
    settings::Settings,
};
use time::OffsetDateTime;

#[cfg(feature = "sync")]
mod push;

#[cfg(feature = "sync")]
mod pull;

mod purge;
mod rebuild;
mod rekey;
mod verify;

#[derive(Subcommand, Debug)]
#[command(infer_subcommands = true)]
pub enum Cmd {
    /// Print the current status of the record store
    Status,

    /// Rebuild a store (eg atuin store rebuild history)
    Rebuild(rebuild::Rebuild),

    /// Re-encrypt the store with a new key (potential for data loss!)
    Rekey(rekey::Rekey),

    /// Delete all records in the store that cannot be decrypted with the current key
    Purge(purge::Purge),

    /// Verify that all records in the store can be decrypted with the current key
    Verify(verify::Verify),

    /// Push all records to the remote sync server (one way sync)
    #[cfg(feature = "sync")]
    Push(push::Push),

    /// Pull records from the remote sync server (one way sync)
    #[cfg(feature = "sync")]
    Pull(pull::Pull),
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
            Self::Rekey(rekey) => rekey.run(settings, store).await,
            Self::Verify(verify) => verify.run(settings, store).await,
            Self::Purge(purge) => purge.run(settings, store).await,

            #[cfg(feature = "sync")]
            Self::Push(push) => push.run(settings, store).await,

            #[cfg(feature = "sync")]
            Self::Pull(pull) => pull.run(settings, store, database).await,
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
