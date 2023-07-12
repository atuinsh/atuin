use std::path::PathBuf;

use atuin_client::{
    record::{
        encodings::key::{EncryptionKey, KeyStore},
        store::sqlite::SqliteStore,
    },
    settings::Settings,
};
use atuin_common::record::HostId;
use chrono::{Local, TimeZone};
use clap::Parser;
use eyre::{bail, Context, Result};
use record::{TodoRecord, TodoStore};
use type_safe_id::{StaticType, TypeSafeId};
use uuid::Uuid;

mod record;
mod search;

#[derive(Default, Copy, Clone, PartialEq, Eq)]
pub struct Todo;
impl StaticType for Todo {
    const TYPE: &'static str = "todo";
}
pub type TodoId = TypeSafeId<Todo>;

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
    Update {
        #[arg(long)]
        id: TodoId,

        /// The state this todo item is in. eg "todo", "in progress", etc
        #[arg(short = 's', long = "state")]
        state: Option<String>,

        /// Whether to append the new tags or overwrite the tags
        #[arg(short, long)]
        append: bool,

        /// tags for this todo item
        #[arg(short = 't', long = "tag")]
        tags: Vec<String>,

        /// todo text
        text: Option<String>,
    },
    Search {
        #[arg(long, default_value = "20")]
        limit: usize,

        /// search query
        query: String,
    },
    View {
        #[arg(long)]
        id: TodoId,
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
            let record = TodoRecord {
                tags,
                state,
                text,
                updates: TodoId::from_uuid(Uuid::nil()),
            };
            let record = todo_store
                .create_item(&mut store, &encryption_key, record)
                .await?;

            println!("created {}", TodoId::from_uuid(record.id.0));
        }
        Cmd::Update {
            id,
            state,
            append,
            mut tags,
            text,
        } => {
            let mut record = todo_store.get(&mut store, &encryption_key, id).await?.data;
            record.updates = id;
            if append {
                record.tags.append(&mut tags)
            } else if !tags.is_empty() {
                record.tags = tags;
            }
            if let Some(state) = state {
                record.state = state;
            }
            if let Some(text) = text {
                record.text = text;
            }

            let record = todo_store
                .create_item(&mut store, &encryption_key, record)
                .await?;

            println!("updated from {} to {}", id, TodoId::from_uuid(record.id.0));
        }
        Cmd::Search { limit, query } => {
            let ids = todo_store.search(&query, limit).await?;
            for id in ids {
                let record = todo_store.get(&mut store, &encryption_key, id).await?;
                println!("{id} - [{}] {}", record.data.state, record.data.text);
                if !record.data.tags.is_empty() {
                    print!("\t");
                    for tag in record.data.tags {
                        print!("#{tag} ")
                    }
                    println!()
                }
            }
        }
        Cmd::View { mut id } => {
            while !id.uuid().is_nil() {
                let record = todo_store.get(&mut store, &encryption_key, id).await?;
                id = record.data.updates;
                let ts = Local
                    .timestamp_nanos(record.timestamp as i64)
                    .format("%Y %B %-d %R");
                println!("{ts} - [{}] {}", record.data.state, record.data.text);
                if !record.data.tags.is_empty() {
                    print!("\t");
                    for tag in record.data.tags {
                        print!("#{tag} ")
                    }
                    println!()
                }
            }
        }
    }

    Ok(())
}
