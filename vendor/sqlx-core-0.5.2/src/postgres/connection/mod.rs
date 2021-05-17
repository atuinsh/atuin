use std::fmt::{self, Debug, Formatter};
use std::sync::Arc;

use crate::HashMap;
use futures_core::future::BoxFuture;
use futures_util::{FutureExt, TryFutureExt};

use crate::common::StatementCache;
use crate::connection::{Connection, LogSettings};
use crate::error::Error;
use crate::executor::Executor;
use crate::ext::ustr::UStr;
use crate::io::Decode;
use crate::postgres::connection::stream::PgStream;
use crate::postgres::message::{
    Close, Message, MessageFormat, ReadyForQuery, Terminate, TransactionStatus,
};
use crate::postgres::statement::PgStatementMetadata;
use crate::postgres::{PgConnectOptions, PgTypeInfo, Postgres};
use crate::transaction::Transaction;

pub(crate) mod describe;
mod establish;
mod executor;
mod sasl;
mod stream;
mod tls;

/// A connection to a PostgreSQL database.
pub struct PgConnection {
    // underlying TCP or UDS stream,
    // wrapped in a potentially TLS stream,
    // wrapped in a buffered stream
    pub(crate) stream: PgStream,

    // process id of this backend
    // used to send cancel requests
    #[allow(dead_code)]
    process_id: u32,

    // secret key of this backend
    // used to send cancel requests
    #[allow(dead_code)]
    secret_key: u32,

    // sequence of statement IDs for use in preparing statements
    // in PostgreSQL, the statement is prepared to a user-supplied identifier
    next_statement_id: u32,

    // cache statement by query string to the id and columns
    cache_statement: StatementCache<(u32, Arc<PgStatementMetadata>)>,

    // cache user-defined types by id <-> info
    cache_type_info: HashMap<u32, PgTypeInfo>,
    cache_type_oid: HashMap<UStr, u32>,

    // number of ReadyForQuery messages that we are currently expecting
    pub(crate) pending_ready_for_query_count: usize,

    // current transaction status
    transaction_status: TransactionStatus,
    pub(crate) transaction_depth: usize,

    log_settings: LogSettings,
}

impl PgConnection {
    // will return when the connection is ready for another query
    async fn wait_until_ready(&mut self) -> Result<(), Error> {
        if !self.stream.wbuf.is_empty() {
            self.stream.flush().await?;
        }

        while self.pending_ready_for_query_count > 0 {
            let message = self.stream.recv().await?;

            if let MessageFormat::ReadyForQuery = message.format {
                self.handle_ready_for_query(message)?;
            }
        }

        Ok(())
    }

    async fn recv_ready_for_query(&mut self) -> Result<(), Error> {
        let r: ReadyForQuery = self
            .stream
            .recv_expect(MessageFormat::ReadyForQuery)
            .await?;

        self.pending_ready_for_query_count -= 1;
        self.transaction_status = r.transaction_status;

        Ok(())
    }

    fn handle_ready_for_query(&mut self, message: Message) -> Result<(), Error> {
        self.pending_ready_for_query_count -= 1;
        self.transaction_status = ReadyForQuery::decode(message.contents)?.transaction_status;

        Ok(())
    }
}

impl Debug for PgConnection {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.debug_struct("PgConnection").finish()
    }
}

impl Connection for PgConnection {
    type Database = Postgres;

    type Options = PgConnectOptions;

    fn close(mut self) -> BoxFuture<'static, Result<(), Error>> {
        // The normal, graceful termination procedure is that the frontend sends a Terminate
        // message and immediately closes the connection.

        // On receipt of this message, the backend closes the
        // connection and terminates.

        Box::pin(async move {
            self.stream.send(Terminate).await?;
            self.stream.shutdown().await?;

            Ok(())
        })
    }

    fn ping(&mut self) -> BoxFuture<'_, Result<(), Error>> {
        // By sending a comment we avoid an error if the connection was in the middle of a rowset
        self.execute("/* SQLx ping */").map_ok(|_| ()).boxed()
    }

    fn begin(&mut self) -> BoxFuture<'_, Result<Transaction<'_, Self::Database>, Error>>
    where
        Self: Sized,
    {
        Transaction::begin(self)
    }

    fn cached_statements_size(&self) -> usize {
        self.cache_statement.len()
    }

    fn clear_cached_statements(&mut self) -> BoxFuture<'_, Result<(), Error>> {
        Box::pin(async move {
            let mut cleared = 0_usize;

            self.wait_until_ready().await?;

            while let Some((id, _)) = self.cache_statement.remove_lru() {
                self.stream.write(Close::Statement(id));
                cleared += 1;
            }

            if cleared > 0 {
                self.write_sync();
                self.stream.flush().await?;

                self.wait_for_close_complete(cleared).await?;
                self.recv_ready_for_query().await?;
            }

            Ok(())
        })
    }

    #[doc(hidden)]
    fn flush(&mut self) -> BoxFuture<'_, Result<(), Error>> {
        self.wait_until_ready().boxed()
    }

    #[doc(hidden)]
    fn should_flush(&self) -> bool {
        !self.stream.wbuf.is_empty()
    }
}
