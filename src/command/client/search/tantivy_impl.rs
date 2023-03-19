use std::sync::Arc;

use async_trait::async_trait;
use atuin_client::{database::Database, history::History, settings::FilterMode};
use chrono::{TimeZone, Utc};
use clap::Parser;
use eyre::Result;
use tantivy::{
    collector::TopDocs,
    directory::MmapDirectory,
    doc,
    query::{BooleanQuery, ConstScoreQuery, FuzzyTermQuery, Occur, Query, TermQuery},
    schema::{Field, Schema, Value, FAST, STORED, STRING, TEXT},
    DateTime, Index, IndexWriter, Searcher, Term,
};

use super::interactive::{HistoryWrapper, SearchEngine, SearchState};

fn schema() -> (HistorySchema, Schema) {
    let mut schema_builder = Schema::builder();

    (
        HistorySchema {
            id: schema_builder.add_text_field("id", STRING),
            command: schema_builder.add_text_field("command", TEXT | STORED),
            cwd: schema_builder.add_text_field("cwd", STRING | FAST),
            session: schema_builder.add_text_field("session", STRING | FAST),
            hostname: schema_builder.add_text_field("hostname", STRING | FAST),
            timestamp: schema_builder.add_date_field("timestamp", STORED),
            duration: schema_builder.add_i64_field("duration", STORED),
            exit: schema_builder.add_i64_field("exit", STORED),
        },
        schema_builder.build(),
    )
}

struct HistorySchema {
    id: Field,
    command: Field,
    cwd: Field,
    session: Field,
    hostname: Field,
    timestamp: Field,
    duration: Field,
    exit: Field,
}

fn index(schema: Schema) -> Result<Index> {
    let data_dir = atuin_common::utils::data_dir();
    let tantivy_dir = data_dir.join("tantivy");

    fs_err::create_dir_all(&tantivy_dir)?;
    let dir = MmapDirectory::open(tantivy_dir)?;

    Ok(Index::open_or_create(dir, schema)?)
}

pub fn write_history(h: impl IntoIterator<Item = History>) -> Result<()> {
    let (hs, schema) = schema();
    let index = index(schema)?;
    let mut writer = index.writer(3_000_000)?;

    for h in h {
        write_single_history(&mut writer, &hs, h)?;
    }

    writer.commit()?;

    Ok(())
}

pub struct Search {
    schema: HistorySchema,
    searcher: Searcher,
}

impl Search {
    pub fn new() -> Result<Self> {
        let (hs, schema) = schema();
        let index = index(schema)?;

        let reader = index.reader()?;
        let searcher = reader.searcher();

        Ok(Self {
            schema: hs,
            searcher,
        })
    }
}

#[async_trait]
impl SearchEngine for Search {
    async fn query(
        &mut self,
        state: &SearchState,
        _: &mut dyn Database,
    ) -> Result<Vec<Arc<HistoryWrapper>>> {
        let mut queries = Vec::<(Occur, Box<dyn Query>)>::new();
        for term in state.input.as_str().split_whitespace() {
            let command =
                FuzzyTermQuery::new(Term::from_field_text(self.schema.command, term), 2, true);
            queries.push((Occur::Should, Box::new(command)));
        }

        match state.filter_mode {
            FilterMode::Global => {}
            FilterMode::Directory => {
                let cwd = TermQuery::new(
                    Term::from_field_text(self.schema.cwd, &state.context.cwd),
                    tantivy::schema::IndexRecordOption::Basic,
                );
                queries.push((
                    Occur::Must,
                    Box::new(ConstScoreQuery::new(Box::new(cwd), 1.0)),
                ));
            }
            FilterMode::Host => {
                let host = TermQuery::new(
                    Term::from_field_text(self.schema.hostname, &state.context.hostname),
                    tantivy::schema::IndexRecordOption::Basic,
                );
                queries.push((
                    Occur::Must,
                    Box::new(ConstScoreQuery::new(Box::new(host), 1.0)),
                ));
            }
            FilterMode::Session => {
                let session = TermQuery::new(
                    Term::from_field_text(self.schema.session, &state.context.session),
                    tantivy::schema::IndexRecordOption::Basic,
                );
                queries.push((
                    Occur::Must,
                    Box::new(ConstScoreQuery::new(Box::new(session), 1.0)),
                ));
            }
        }

        let query = BooleanQuery::new(queries);

        let top_docs = self.searcher.search(&query, &TopDocs::with_limit(200))?;

        let mut output = Vec::with_capacity(top_docs.len());

        for (_score, doc_address) in top_docs {
            let retrieved_doc = self.searcher.doc(doc_address)?;
            output.push(Arc::new(HistoryWrapper {
                history: History {
                    id: String::new(),
                    command: retrieved_doc
                        .get_all(self.schema.command)
                        .next()
                        .and_then(Value::as_text)
                        .unwrap()
                        .to_string(),
                    cwd: String::new(),
                    session: String::new(),
                    hostname: String::new(),
                    timestamp: retrieved_doc
                        .get_all(self.schema.timestamp)
                        .next()
                        .and_then(Value::as_date)
                        .and_then(|d| Utc.timestamp_millis_opt(d.into_timestamp_millis()).single())
                        .unwrap(),
                    duration: retrieved_doc
                        .get_all(self.schema.duration)
                        .next()
                        .and_then(Value::as_i64)
                        .unwrap(),
                    exit: retrieved_doc
                        .get_all(self.schema.exit)
                        .next()
                        .and_then(Value::as_i64)
                        .unwrap(),
                    deleted_at: None,
                },
                count: 1,
            }));
        }

        Ok(output)
    }
}

fn write_single_history(
    writer: &mut IndexWriter,
    schema: &HistorySchema,
    h: History,
) -> Result<()> {
    let timestamp = DateTime::from_timestamp_millis(h.timestamp.timestamp_millis());
    writer.add_document(doc!(
        schema.id => h.id,
        schema.command => h.command,
        schema.cwd => h.cwd,
        schema.session => h.session,
        schema.hostname => h.hostname,
        schema.timestamp => timestamp,
        schema.duration => h.duration,
        schema.exit => h.exit,
    ))?;

    Ok(())
}

#[allow(clippy::struct_excessive_bools)]
#[derive(Parser)]
pub struct Cmd {}

impl Cmd {
    pub async fn run(self, db: &mut impl Database) -> Result<()> {
        let history = db.all_with_count().await?;

        // delete the index
        let data_dir = atuin_common::utils::data_dir();
        let tantivy_dir = dbg!(data_dir.join("tantivy"));
        fs_err::remove_dir_all(tantivy_dir)?;

        tokio::task::spawn_blocking(|| write_history(history.into_iter().map(|(h, _)| h)))
            .await??;

        Ok(())
    }
}
