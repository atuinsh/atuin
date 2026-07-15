use clap::Args;
use eyre::Result;

use atuin_client::{
    encryption::load_key,
    record::store::{ArcStore, Store},
    settings::Settings,
};

#[derive(Args, Debug)]
pub struct Verify {}

impl Verify {
    pub async fn run(&self, settings: &Settings, store: ArcStore) -> Result<()> {
        println!("Verifying local store can be decrypted with the current key");

        let key = load_key(settings)?;

        match store.verify(&key.into()).await {
            Ok(()) => println!("Local store encryption verified OK"),
            Err(e) => println!("Failed to verify local store encryption: {e:?}"),
        }

        Ok(())
    }
}
