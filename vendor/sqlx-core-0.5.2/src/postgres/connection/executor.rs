use crate::describe::Describe;
use crate::error::Error;
use crate::executor::{Execute, Executor};
use crate::logger::QueryLogger;
use crate::postgres::message::{
    self, Bind, Close, CommandComplete, DataRow, MessageFormat, ParameterDescription, Parse, Query,
    RowDescription,
};
use crate::postgres::statement::PgStatementMetadata;
use crate::postgres::type_info::PgType;
use crate::postgres::{
    statement::PgStatement, PgArguments, PgConnection, PgQueryResult, PgRow, PgTypeInfo,
    PgValueFormat, Postgres,
};
use either::Either;
use futures_core::future::BoxFuture;
use futures_core::stream::BoxStream;
use futures_core::Stream;
use futures_util::{pin_mut, TryStreamExt};
use std::{borrow::Cow, sync::Arc};

async fn prepare(
    conn: &mut PgConnection,
    sql: &str,
    parameters: &[PgTypeInfo],
    metadata: Option<Arc<PgStatementMetadata>>,
) -> Result<(u32, Arc<PgStatementMetadata>), Error> {
    let id = conn.next_statement_id;
    conn.next_statement_id = conn.next_statement_id.wrapping_add(1);

    // build a list of type OIDs to send to the database in the PARSE command
    // we have not yet started the query sequence, so we are *safe* to cleanly make
    // additional queries here to get any missing OIDs

    let mut param_types = Vec::with_capacity(parameters.len());

    for ty in parameters {
        param_types.push(if let PgType::DeclareWithName(name) = &ty.0 {
            conn.fetch_type_id_by_name(name).await?
        } else {
            ty.0.oid()
        });
    }

    // flush and wait until we are re-ready
    conn.wait_until_ready().await?;

    // next we send the PARSE command to the server
    conn.stream.write(Parse {
        param_types: &*param_types,
        query: sql,
        statement: id,
    });

    if metadata.is_none() {
        // get the statement columns and parameters
        conn.stream.write(message::Describe::Statement(id));
    }

    // we ask for the server to immediately send us the result of the PARSE command
    conn.write_sync();
    conn.stream.flush().await?;

    // indicates that the SQL query string is now successfully parsed and has semantic validity
    let _ = conn
        .stream
        .recv_expect(MessageFormat::ParseComplete)
        .await?;

    let metadata = if let Some(metadata) = metadata {
        // each SYNC produces one READY FOR QUERY
        conn.recv_ready_for_query().await?;

        // we already have metadata
        metadata
    } else {
        let parameters = recv_desc_params(conn).await?;

        let rows = recv_desc_rows(conn).await?;

        // each SYNC produces one READY FOR QUERY
        conn.recv_ready_for_query().await?;

        let parameters = conn.handle_parameter_description(parameters).await?;

        let (columns, column_names) = conn.handle_row_description(rows, true).await?;

        // ensure that if we did fetch custom data, we wait until we are fully ready before
        // continuing
        conn.wait_until_ready().await?;

        Arc::new(PgStatementMetadata {
            parameters,
            columns,
            column_names,
        })
    };

    Ok((id, metadata))
}

async fn recv_desc_params(conn: &mut PgConnection) -> Result<ParameterDescription, Error> {
    conn.stream
        .recv_expect(MessageFormat::ParameterDescription)
        .await
}

async fn recv_desc_rows(conn: &mut PgConnection) -> Result<Option<RowDescription>, Error> {
    let rows: Option<RowDescription> = match conn.stream.recv().await? {
        // describes the rows that will be returned when the statement is eventually executed
        message if message.format == MessageFormat::RowDescription => Some(message.decode()?),

        // no data would be returned if this statement was executed
        message if message.format == MessageFormat::NoData => None,

        message => {
            return Err(err_protocol!(
                "expecting RowDescription or NoData but received {:?}",
                message.format
            ));
        }
    };

    Ok(rows)
}

impl PgConnection {
    // wait for CloseComplete to indicate a statement was closed
    pub(super) async fn wait_for_close_complete(&mut self, mut count: usize) -> Result<(), Error> {
        // we need to wait for the [CloseComplete] to be returned from the server
        while count > 0 {
            match self.stream.recv().await? {
                message if message.format == MessageFormat::PortalSuspended => {
                    // there was an open portal
                    // this can happen if the last time a statement was used it was not fully executed
                    // such as in [fetch_one]
                }

                message if message.format == MessageFormat::CloseComplete => {
                    // successfully closed the statement (and freed up the server resources)
                    count -= 1;
                }

                message => {
                    return Err(err_protocol!(
                        "expecting PortalSuspended or CloseComplete but received {:?}",
                        message.format
                    ));
                }
            }
        }

        Ok(())
    }

    pub(crate) fn write_sync(&mut self) {
        self.stream.write(message::Sync);

        // all SYNC messages will return a ReadyForQuery
        self.pending_ready_for_query_count += 1;
    }

    async fn get_or_prepare<'a>(
        &mut self,
        sql: &str,
        parameters: &[PgTypeInfo],
        // should we store the result of this prepare to the cache
        store_to_cache: bool,
        // optional metadata that was provided by the user, this means they are reusing
        // a statement object
        metadata: Option<Arc<PgStatementMetadata>>,
    ) -> Result<(u32, Arc<PgStatementMetadata>), Error> {
        if let Some(statement) = self.cache_statement.get_mut(sql) {
            return Ok((*statement).clone());
        }

        let statement = prepare(self, sql, parameters, metadata).await?;

        if store_to_cache && self.cache_statement.is_enabled() {
            if let Some((id, _)) = self.cache_statement.insert(sql, statement.clone()) {
                self.stream.write(Close::Statement(id));
                self.write_sync();

                self.stream.flush().await?;

                self.wait_for_close_complete(1).await?;
                self.recv_ready_for_query().await?;
            }
        }

        Ok(statement)
    }

    async fn run<'e, 'c: 'e, 'q: 'e>(
        &'c mut self,
        query: &'q str,
        arguments: Option<PgArguments>,
        limit: u8,
        persistent: bool,
        metadata_opt: Option<Arc<PgStatementMetadata>>,
    ) -> Result<impl Stream<Item = Result<Either<PgQueryResult, PgRow>, Error>> + 'e, Error> {
        let mut logger = QueryLogger::new(query, self.log_settings.clone());

        // before we continue, wait until we are "ready" to accept more queries
        self.wait_until_ready().await?;

        let mut metadata: Arc<PgStatementMetadata>;

        let format = if let Some(mut arguments) = arguments {
            // prepare the statement if this our first time executing it
            // always return the statement ID here
            let (statement, metadata_) = self
                .get_or_prepare(query, &arguments.types, persistent, metadata_opt)
                .await?;

            metadata = metadata_;

            // patch holes created during encoding
            arguments.apply_patches(self, &metadata.parameters).await?;

            // bind to attach the arguments to the statement and create a portal
            self.stream.write(Bind {
                portal: None,
                statement,
                formats: &[PgValueFormat::Binary],
                num_params: arguments.types.len() as i16,
                params: &*arguments.buffer,
                result_formats: &[PgValueFormat::Binary],
            });

            // executes the portal up to the passed limit
            // the protocol-level limit acts nearly identically to the `LIMIT` in SQL
            self.stream.write(message::Execute {
                portal: None,
                limit: limit.into(),
            });

            // finally, [Sync] asks postgres to process the messages that we sent and respond with
            // a [ReadyForQuery] message when it's completely done. Theoretically, we could send
            // dozens of queries before a [Sync] and postgres can handle that. Execution on the server
            // is still serial but it would reduce round-trips. Some kind of builder pattern that is
            // termed batching might suit this.
            self.write_sync();

            // prepared statements are binary
            PgValueFormat::Binary
        } else {
            // Query will trigger a ReadyForQuery
            self.stream.write(Query(query));
            self.pending_ready_for_query_count += 1;

            // metadata starts out as "nothing"
            metadata = Arc::new(PgStatementMetadata::default());

            // and unprepared statements are text
            PgValueFormat::Text
        };

        self.stream.flush().await?;

        Ok(try_stream! {
            loop {
                let message = self.stream.recv().await?;

                match message.format {
                    MessageFormat::BindComplete
                    | MessageFormat::ParseComplete
                    | MessageFormat::ParameterDescription
                    | MessageFormat::NoData => {
                        // harmless messages to ignore
                    }

                    MessageFormat::CommandComplete => {
                        // a SQL command completed normally
                        let cc: CommandComplete = message.decode()?;

                        r#yield!(Either::Left(PgQueryResult {
                            rows_affected: cc.rows_affected(),
                        }));
                    }

                    MessageFormat::EmptyQueryResponse => {
                        // empty query string passed to an unprepared execute
                    }

                    MessageFormat::RowDescription => {
                        // indicates that a *new* set of rows are about to be returned
                        let (columns, column_names) = self
                            .handle_row_description(Some(message.decode()?), false)
                            .await?;

                        metadata = Arc::new(PgStatementMetadata {
                            column_names,
                            columns,
                            parameters: Vec::default(),
                        });
                    }

                    MessageFormat::DataRow => {
                        logger.increment_rows();

                        // one of the set of rows returned by a SELECT, FETCH, etc query
                        let data: DataRow = message.decode()?;
                        let row = PgRow {
                            data,
                            format,
                            metadata: Arc::clone(&metadata),
                        };

                        r#yield!(Either::Right(row));
                    }

                    MessageFormat::ReadyForQuery => {
                        // processing of the query string is complete
                        self.handle_ready_for_query(message)?;
                        break;
                    }

                    _ => {
                        return Err(err_protocol!(
                            "execute: unexpected message: {:?}",
                            message.format
                        ));
                    }
                }
            }

            Ok(())
        })
    }
}

impl<'c> Executor<'c> for &'c mut PgConnection {
    type Database = Postgres;

    fn fetch_many<'e, 'q: 'e, E: 'q>(
        self,
        mut query: E,
    ) -> BoxStream<'e, Result<Either<PgQueryResult, PgRow>, Error>>
    where
        'c: 'e,
        E: Execute<'q, Self::Database>,
    {
        let sql = query.sql();
        let metadata = query.statement().map(|s| Arc::clone(&s.metadata));
        let arguments = query.take_arguments();
        let persistent = query.persistent();

        Box::pin(try_stream! {
            let s = self.run(sql, arguments, 0, persistent, metadata).await?;
            pin_mut!(s);

            while let Some(v) = s.try_next().await? {
                r#yield!(v);
            }

            Ok(())
        })
    }

    fn fetch_optional<'e, 'q: 'e, E: 'q>(
        self,
        mut query: E,
    ) -> BoxFuture<'e, Result<Option<PgRow>, Error>>
    where
        'c: 'e,
        E: Execute<'q, Self::Database>,
    {
        let sql = query.sql();
        let metadata = query.statement().map(|s| Arc::clone(&s.metadata));
        let arguments = query.take_arguments();
        let persistent = query.persistent();

        Box::pin(async move {
            let s = self.run(sql, arguments, 1, persistent, metadata).await?;
            pin_mut!(s);

            while let Some(s) = s.try_next().await? {
                if let Either::Right(r) = s {
                    return Ok(Some(r));
                }
            }

            Ok(None)
        })
    }

    fn prepare_with<'e, 'q: 'e>(
        self,
        sql: &'q str,
        parameters: &'e [PgTypeInfo],
    ) -> BoxFuture<'e, Result<PgStatement<'q>, Error>>
    where
        'c: 'e,
    {
        Box::pin(async move {
            self.wait_until_ready().await?;

            let (_, metadata) = self.get_or_prepare(sql, parameters, true, None).await?;

            Ok(PgStatement {
                sql: Cow::Borrowed(sql),
                metadata,
            })
        })
    }

    fn describe<'e, 'q: 'e>(
        self,
        sql: &'q str,
    ) -> BoxFuture<'e, Result<Describe<Self::Database>, Error>>
    where
        'c: 'e,
    {
        Box::pin(async move {
            self.wait_until_ready().await?;

            let (stmt_id, metadata) = self.get_or_prepare(sql, &[], true, None).await?;

            let nullable = self.get_nullable_for_columns(stmt_id, &metadata).await?;

            Ok(Describe {
                columns: metadata.columns.clone(),
                nullable,
                parameters: Some(Either::Left(metadata.parameters.clone())),
            })
        })
    }
}
