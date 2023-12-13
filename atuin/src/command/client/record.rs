use clap::Subcommand;
use eyre::Result;

use atuin_client::{record::store::Store, settings::Settings};
use time::OffsetDateTime;

#[derive(Subcommand, Debug)]
#[command(infer_subcommands = true)]
pub enum Cmd {
    Status,
}

impl Cmd {
    pub async fn run(
        &self,
        _settings: &Settings,
        store: &(impl Store + Send + Sync),
    ) -> Result<()> {
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
