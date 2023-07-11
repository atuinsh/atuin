use std::path::PathBuf;

use atuin_client::{
    record::{
        encodings::key::{EncryptionKey, KeyStore},
        store::sqlite::SqliteStore,
    },
    settings::Settings,
};
use atuin_common::record::HostId;
use clap::Parser;
use eyre::{bail, Context, Result};
use record::TodoStore;

mod record;
mod search;

#[derive(clap::Parser)]
#[command(author = "Conrad Ludgate <conradludgate@gmail.com>")]
enum Cmd {
    Push {
        /// The state this todo item is in. eg "todo", "in progress", etc
        #[arg(short = 's', long = "state")]
        state: String,

        /// tags for this todo item
        #[arg(short = 't', long = "tag")]
        tags: Vec<String>,

        /// todo text
        text: String,
    },
    Search {
        #[arg(long, default_value = "20")]
        limit: usize,

        /// search query
        query: String,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    let cmd = Cmd::parse();

    let settings = Settings::new().wrap_err("could not load client settings")?;
    let record_store_path = PathBuf::from(settings.record_store_path.as_str());
    let mut store = SqliteStore::new(record_store_path).await?;

    let key_store = KeyStore::new();
    // ensure this encryption key is the latest registered key before encrypting anything new.
    let encryption_key = match key_store
        .validate_encryption_key(&mut store, &settings)
        .await?
    {
        EncryptionKey::Valid { encryption_key } => encryption_key,
        EncryptionKey::Invalid {
            kid,
            host_id: HostId(host_id),
        } => {
            bail!("A new encryption key [id:{kid}] has been set by [host:{host_id}]. You must update to this encryption key to continue")
        }
    };

    let todo_store = TodoStore::new();

    match cmd {
        Cmd::Push { state, tags, text } => {
            todo_store
                .create_item(&mut store, &encryption_key, state, text, tags)
                .await?;
        }
        Cmd::Search { limit, query } => {
            let ids = todo_store.search(&query, limit).await?;
            for id in ids {
                let record = todo_store.get(&mut store, &encryption_key, id).await?;
                println!("{}", serde_json::to_string(&record).unwrap());
            }
        }
    }

    Ok(())
}
