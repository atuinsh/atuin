use clap::Subcommand;
use eyre::Result;

use atuin_client::{record::store::Store, settings::Settings};

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

        for (host, store) in &status.hosts {
            let host_string = if host == &host_id {
                format!("host: {} <- CURRENT HOST", host.0.as_hyphenated())
            } else {
                format!("host: {}", host.0.as_hyphenated())
            };

            println!("{host_string}");

            for (tag, idx) in store {
                println!("\tstore: {tag} at {idx}");
            }

            println!();
        }

        Ok(())
    }
}
