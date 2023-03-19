use std::sync::Arc;

use async_trait::async_trait;
use atuin_client::{
    database::Database,
    history::History,
    settings::FilterMode,
    tantivy::{index, schema, HistorySchema},
};
use chrono::{TimeZone, Utc};
use eyre::Result;
use tantivy::{
    collector::TopDocs,
    query::{BooleanQuery, ConstScoreQuery, FuzzyTermQuery, Occur, Query, TermQuery},
    schema::Value,
    Searcher, Term,
};

use super::{HistoryWrapper, SearchEngine, SearchState};

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
    async fn full_query(
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
