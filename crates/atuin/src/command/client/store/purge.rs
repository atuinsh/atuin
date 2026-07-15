use clap::Args;
use eyre::Result;

use atuin_client::{
    encryption::load_key,
    record::store::{ArcStore, Store},
    settings::Settings,
};

#[derive(Args, Debug)]
pub struct Purge {}

impl Purge {
    pub async fn run(&self, settings: &Settings, store: ArcStore) -> Result<()> {
        println!("Purging local records that cannot be decrypted");

        let key = load_key(settings)?;

        match store.purge(&key.into()).await {
            Ok(()) => println!("Local store purge completed OK"),
            Err(e) => println!("Failed to purge local store: {e:?}"),
        }

        Ok(())
    }
}
