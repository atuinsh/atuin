//! A todo store.
//!
//! * `tag` = "todo"
//! * `version`s:
//!   - "v0"
//!
//! ## Encryption schemes
//!
//! ### v0
//!
//! [`PASETO_V4`]
//!
//! ## Encoding schemes
//!
//! ### v0
//!
//! Message pack encoding of
//!
//! ```text
//! [
//!     state,
//!     text,
//!     [tag],
//!     updates,
//! ]
//! ```

use atuin_common::record::{DecryptedData, EncryptedData, Record, RecordId};
use eyre::{bail, ensure, eyre, Context, ContextCompat, Result};

use atuin_client::record::encryption::paseto_v4::PASETO_V4;
use atuin_client::record::store::Store;
use atuin_client::settings::Settings;
use serde::Serialize;
use tantivy::{collector::TopDocs, query::QueryParser, Index};
use uuid::Uuid;

use crate::{
    search::{self, TodoSchema},
    TodoId,
};

const TODO_VERSION: &str = "v0";
const TODO_TAG: &str = "todo";

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TodoRecord {
    pub state: String,
    pub text: String,
    pub tags: Vec<String>,
    pub updates: TodoId,
}

impl TodoRecord {
    pub fn serialize(&self) -> Result<DecryptedData> {
        use rmp::encode;

        let mut output = vec![];

        // INFO: ensure this is updated when adding new fields
        encode::write_array_len(&mut output, 4)?;

        encode::write_str(&mut output, &self.state)?;
        encode::write_str(&mut output, &self.text)?;

        encode::write_array_len(&mut output, self.tags.len() as u32)?;
        for tag in &self.tags {
            encode::write_str(&mut output, tag)?;
        }

        encode::write_bin(&mut output, self.updates.uuid().as_bytes())?;

        Ok(DecryptedData(output))
    }

    pub fn deserialize(data: &DecryptedData, version: &str) -> Result<Self> {
        use rmp::decode;

        fn error_report<E: std::fmt::Debug>(err: E) -> eyre::Report {
            eyre!("{err:?}")
        }

        match version {
            TODO_VERSION => {
                // let mut rd = decode::Bytes::new(&data.0);
                let mut data = &*data.0;

                let nfields = decode::read_array_len(&mut data)?;
                ensure!(
                    nfields == 4,
                    "incorrect number of entries in v0 todo record"
                );

                let (state, data) = decode::read_str_from_slice(data).map_err(error_report)?;
                let (text, mut data) = decode::read_str_from_slice(data).map_err(error_report)?;

                let ntags = decode::read_array_len(&mut data)?;
                let mut tags = Vec::with_capacity(ntags as usize);
                for _ in 0..ntags {
                    let (value, b) = decode::read_str_from_slice(data).map_err(error_report)?;
                    data = b;
                    tags.push(value.to_owned())
                }

                let updates_len = decode::read_bin_len(&mut data)?;
                ensure!(updates_len == 16, "incorrect UUID encoding in todo record");
                let updates: [u8; 16] = data
                    .try_into()
                    .context("incorrect UUID encoding in todo record")?;

                let updates = TodoId::from_uuid(Uuid::from_bytes(updates));

                Ok(TodoRecord {
                    state: state.to_owned(),
                    text: text.to_owned(),
                    tags,
                    updates,
                })
            }
            _ => {
                bail!("unknown version {version:?}")
            }
        }
    }
}

pub struct TodoStore {
    schema: TodoSchema,
    index: Index,
}

impl Default for TodoStore {
    fn default() -> Self {
        Self::new()
    }
}

impl TodoStore {
    // will want to init the actual kv store when that is done
    pub fn new() -> TodoStore {
        let (ts, schema) = search::schema();
        let index = search::index(schema).unwrap();
        TodoStore { schema: ts, index }
    }

    pub async fn create_item(
        &self,
        store: &mut (impl Store + Send + Sync),
        encryption_key: &[u8; 32],
        record: TodoRecord,
    ) -> Result<Record<EncryptedData>> {
        let host_id = Settings::host_id().expect("failed to get host_id");

        let parent = store.tail(host_id, TODO_TAG).await?.map(|entry| entry.id);

        let record = Record::builder()
            .host(host_id)
            .version(TODO_VERSION.to_string())
            .tag(TODO_TAG.to_owned())
            .parent(parent)
            .data(record)
            .build();

        let mut writer = self.index.writer(3_000_000)?;
        search::write_record(&mut writer, &self.schema, &record)?;
        tokio::task::spawn_blocking(|| {
            writer.commit()?;
            writer.wait_merging_threads()
        })
        .await??;

        let record = record.try_map(|s| s.serialize())?;
        let record = record.encrypt::<PASETO_V4>(encryption_key);
        store.push(&record).await?;

        Ok(record)
    }

    pub async fn get(
        &self,
        store: &mut (impl Store + Send + Sync),
        encryption_key: &[u8; 32],
        id: TodoId,
    ) -> Result<Record<TodoRecord>> {
        let record = store.get(RecordId(id.uuid())).await?;
        match &*record.version {
            "v0" => {
                let record = record.decrypt::<PASETO_V4>(encryption_key)?;
                let record = record.try_map(|s| TodoRecord::deserialize(&s, "v0"))?;
                Ok(record)
            }
            _ => bail!("unsupported todo record version"),
        }
    }

    pub async fn search(&self, query: &str, limit: usize) -> Result<Vec<TodoId>> {
        let query_parser = QueryParser::new(
            self.index.schema(),
            vec![self.schema.text, self.schema.tag],
            self.index.tokenizers().clone(),
        );
        let query = query_parser.parse_query(query)?;
        let searcher = self.index.reader()?.searcher();

        let mut output = vec![];

        let docs = searcher.search(&query, &TopDocs::with_limit(limit))?;
        for (_, doc) in docs {
            let doc = searcher.doc(doc)?;
            let id = doc.get_first(self.schema.id);
            output.push(
                id.context("missing id")?
                    .as_text()
                    .context("invalid id")?
                    .parse()?,
            )
        }

        Ok(output)
    }
}

#[cfg(test)]
mod tests {
    use uuid::Uuid;

    use crate::TodoId;

    use super::{TodoRecord, TODO_VERSION};

    #[test]
    fn encode_decode() {
        let kv = TodoRecord {
            state: "todo".to_owned(),
            text: "implement todo".to_owned(),
            tags: vec!["atuin".to_owned(), "rust".to_owned()],
            updates: TodoId::from_uuid(Uuid::nil()),
        };

        let encoded = kv.serialize().unwrap();
        let decoded = TodoRecord::deserialize(&encoded, TODO_VERSION).unwrap();

        assert_eq!(decoded, kv);
    }
}
