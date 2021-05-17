use futures_core::future::BoxFuture;

use crate::error::Error;
use crate::executor::Executor;
use crate::mysql::connection::Busy;
use crate::mysql::protocol::text::Query;
use crate::mysql::{MySql, MySqlConnection};
use crate::transaction::{
    begin_ansi_transaction_sql, commit_ansi_transaction_sql, rollback_ansi_transaction_sql,
    TransactionManager,
};

/// Implementation of [`TransactionManager`] for MySQL.
pub struct MySqlTransactionManager;

impl TransactionManager for MySqlTransactionManager {
    type Database = MySql;

    fn begin(conn: &mut MySqlConnection) -> BoxFuture<'_, Result<(), Error>> {
        Box::pin(async move {
            let depth = conn.transaction_depth;

            conn.execute(&*begin_ansi_transaction_sql(depth)).await?;
            conn.transaction_depth = depth + 1;

            Ok(())
        })
    }

    fn commit(conn: &mut MySqlConnection) -> BoxFuture<'_, Result<(), Error>> {
        Box::pin(async move {
            let depth = conn.transaction_depth;

            if depth > 0 {
                conn.execute(&*commit_ansi_transaction_sql(depth)).await?;
                conn.transaction_depth = depth - 1;
            }

            Ok(())
        })
    }

    fn rollback(conn: &mut MySqlConnection) -> BoxFuture<'_, Result<(), Error>> {
        Box::pin(async move {
            let depth = conn.transaction_depth;

            if depth > 0 {
                conn.execute(&*rollback_ansi_transaction_sql(depth)).await?;
                conn.transaction_depth = depth - 1;
            }

            Ok(())
        })
    }

    fn start_rollback(conn: &mut MySqlConnection) {
        let depth = conn.transaction_depth;

        if depth > 0 {
            conn.stream.busy = Busy::Result;
            conn.stream.sequence_id = 0;
            conn.stream
                .write_packet(Query(&*rollback_ansi_transaction_sql(depth)));

            conn.transaction_depth = depth - 1;
        }
    }
}
