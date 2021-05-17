use crate::describe::Describe;
use crate::error::Error;
use crate::executor::{Execute, Executor};
use crate::logger::QueryLogger;
use crate::mssql::connection::prepare::prepare;
use crate::mssql::protocol::col_meta_data::Flags;
use crate::mssql::protocol::done::Status;
use crate::mssql::protocol::message::Message;
use crate::mssql::protocol::packet::PacketType;
use crate::mssql::protocol::rpc::{OptionFlags, Procedure, RpcRequest};
use crate::mssql::protocol::sql_batch::SqlBatch;
use crate::mssql::{
    Mssql, MssqlArguments, MssqlConnection, MssqlQueryResult, MssqlRow, MssqlStatement,
    MssqlTypeInfo,
};
use either::Either;
use futures_core::future::BoxFuture;
use futures_core::stream::BoxStream;
use futures_util::TryStreamExt;
use std::borrow::Cow;
use std::sync::Arc;

impl MssqlConnection {
    async fn run(&mut self, query: &str, arguments: Option<MssqlArguments>) -> Result<(), Error> {
        self.stream.wait_until_ready().await?;
        self.stream.pending_done_count += 1;

        if let Some(mut arguments) = arguments {
            let proc = Either::Right(Procedure::ExecuteSql);
            let mut proc_args = MssqlArguments::default();

            // SQL
            proc_args.add_unnamed(query);

            if !arguments.data.is_empty() {
                // Declarations
                //  NAME TYPE, NAME TYPE, ...
                proc_args.add_unnamed(&*arguments.declarations);

                // Add the list of SQL parameters _after_ our RPC parameters
                proc_args.append(&mut arguments);
            }

            self.stream.write_packet(
                PacketType::Rpc,
                RpcRequest {
                    transaction_descriptor: self.stream.transaction_descriptor,
                    arguments: &proc_args,
                    procedure: proc,
                    options: OptionFlags::empty(),
                },
            );
        } else {
            self.stream.write_packet(
                PacketType::SqlBatch,
                SqlBatch {
                    transaction_descriptor: self.stream.transaction_descriptor,
                    sql: query,
                },
            );
        }

        self.stream.flush().await?;

        Ok(())
    }
}

impl<'c> Executor<'c> for &'c mut MssqlConnection {
    type Database = Mssql;

    fn fetch_many<'e, 'q: 'e, E: 'q>(
        self,
        mut query: E,
    ) -> BoxStream<'e, Result<Either<MssqlQueryResult, MssqlRow>, Error>>
    where
        'c: 'e,
        E: Execute<'q, Self::Database>,
    {
        let sql = query.sql();
        let arguments = query.take_arguments();
        let mut logger = QueryLogger::new(sql, self.log_settings.clone());

        Box::pin(try_stream! {
            self.run(sql, arguments).await?;

            loop {
                let message = self.stream.recv_message().await?;

                match message {
                    Message::Row(row) => {
                        let columns = Arc::clone(&self.stream.columns);
                        let column_names = Arc::clone(&self.stream.column_names);

                        logger.increment_rows();

                        r#yield!(Either::Right(MssqlRow { row, column_names, columns }));
                    }

                    Message::Done(done) | Message::DoneProc(done) => {
                        if !done.status.contains(Status::DONE_MORE) {
                            self.stream.handle_done(&done);
                        }

                        if done.status.contains(Status::DONE_COUNT) {
                            r#yield!(Either::Left(MssqlQueryResult {
                                rows_affected: done.affected_rows,
                            }));
                        }

                        if !done.status.contains(Status::DONE_MORE) {
                            break;
                        }
                    }

                    Message::DoneInProc(done) => {
                        if done.status.contains(Status::DONE_COUNT) {
                            r#yield!(Either::Left(MssqlQueryResult {
                                rows_affected: done.affected_rows,
                            }));
                        }
                    }

                    _ => {}
                }
            }

            Ok(())
        })
    }

    fn fetch_optional<'e, 'q: 'e, E: 'q>(
        self,
        query: E,
    ) -> BoxFuture<'e, Result<Option<MssqlRow>, Error>>
    where
        'c: 'e,
        E: Execute<'q, Self::Database>,
    {
        let mut s = self.fetch_many(query);

        Box::pin(async move {
            while let Some(v) = s.try_next().await? {
                if let Either::Right(r) = v {
                    return Ok(Some(r));
                }
            }

            Ok(None)
        })
    }

    fn prepare_with<'e, 'q: 'e>(
        self,
        sql: &'q str,
        _parameters: &[MssqlTypeInfo],
    ) -> BoxFuture<'e, Result<MssqlStatement<'q>, Error>>
    where
        'c: 'e,
    {
        Box::pin(async move {
            let metadata = prepare(self, sql).await?;

            Ok(MssqlStatement {
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
            let metadata = prepare(self, sql).await?;

            let mut nullable = Vec::with_capacity(metadata.columns.len());

            for col in metadata.columns.iter() {
                nullable.push(Some(col.flags.contains(Flags::NULLABLE)));
            }

            Ok(Describe {
                nullable,
                columns: (metadata.columns).clone(),
                parameters: None,
            })
        })
    }
}
