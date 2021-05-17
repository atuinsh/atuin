//! Microsoft SQL (MSSQL) database driver.

mod arguments;
mod column;
mod connection;
mod database;
mod error;
mod io;
mod options;
mod protocol;
mod query_result;
mod row;
mod statement;
mod transaction;
mod type_info;
pub mod types;
mod value;

pub use arguments::MssqlArguments;
pub use column::MssqlColumn;
pub use connection::MssqlConnection;
pub use database::Mssql;
pub use error::MssqlDatabaseError;
pub use options::MssqlConnectOptions;
pub use query_result::MssqlQueryResult;
pub use row::MssqlRow;
pub use statement::MssqlStatement;
pub use transaction::MssqlTransactionManager;
pub use type_info::MssqlTypeInfo;
pub use value::{MssqlValue, MssqlValueRef};

/// An alias for [`Pool`][crate::pool::Pool], specialized for MSSQL.
pub type MssqlPool = crate::pool::Pool<Mssql>;

// NOTE: required due to the lack of lazy normalization
impl_into_arguments_for_arguments!(MssqlArguments);
impl_executor_for_pool_connection!(Mssql, MssqlConnection, MssqlRow);
impl_executor_for_transaction!(Mssql, MssqlRow);
impl_acquire!(Mssql, MssqlConnection);
impl_column_index_for_row!(MssqlRow);
impl_column_index_for_statement!(MssqlStatement);
impl_into_maybe_pool!(Mssql, MssqlConnection);
