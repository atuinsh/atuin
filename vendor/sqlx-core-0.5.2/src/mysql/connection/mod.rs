use crate::common::StatementCache;
use crate::connection::{Connection, LogSettings};
use crate::error::Error;
use crate::mysql::protocol::statement::StmtClose;
use crate::mysql::protocol::text::{Ping, Quit};
use crate::mysql::statement::MySqlStatementMetadata;
use crate::mysql::{MySql, MySqlConnectOptions};
use crate::transaction::Transaction;
use futures_core::future::BoxFuture;
use futures_util::FutureExt;
use std::fmt::{self, Debug, Formatter};

mod auth;
mod establish;
mod executor;
mod stream;
mod tls;

pub(crate) use stream::{Busy, MySqlStream};

const MAX_PACKET_SIZE: u32 = 1024;

/// A connection to a MySQL database.
pub struct MySqlConnection {
    // underlying TCP stream,
    // wrapped in a potentially TLS stream,
    // wrapped in a buffered stream
    pub(crate) stream: MySqlStream,

    // transaction status
    pub(crate) transaction_depth: usize,

    // cache by query string to the statement id and metadata
    cache_statement: StatementCache<(u32, MySqlStatementMetadata)>,

    log_settings: LogSettings,
}

impl Debug for MySqlConnection {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.debug_struct("MySqlConnection").finish()
    }
}

impl Connection for MySqlConnection {
    type Database = MySql;

    type Options = MySqlConnectOptions;

    fn close(mut self) -> BoxFuture<'static, Result<(), Error>> {
        Box::pin(async move {
            self.stream.send_packet(Quit).await?;
            self.stream.shutdown().await?;

            Ok(())
        })
    }

    fn ping(&mut self) -> BoxFuture<'_, Result<(), Error>> {
        Box::pin(async move {
            self.stream.wait_until_ready().await?;
            self.stream.send_packet(Ping).await?;
            self.stream.recv_ok().await?;

            Ok(())
        })
    }

    #[doc(hidden)]
    fn flush(&mut self) -> BoxFuture<'_, Result<(), Error>> {
        self.stream.wait_until_ready().boxed()
    }

    fn cached_statements_size(&self) -> usize {
        self.cache_statement.len()
    }

    fn clear_cached_statements(&mut self) -> BoxFuture<'_, Result<(), Error>> {
        Box::pin(async move {
            while let Some((statement_id, _)) = self.cache_statement.remove_lru() {
                self.stream
                    .send_packet(StmtClose {
                        statement: statement_id,
                    })
                    .await?;
            }

            Ok(())
        })
    }

    #[doc(hidden)]
    fn should_flush(&self) -> bool {
        !self.stream.wbuf.is_empty()
    }

    fn begin(&mut self) -> BoxFuture<'_, Result<Transaction<'_, Self::Database>, Error>>
    where
        Self: Sized,
    {
        Transaction::begin(self)
    }
}
