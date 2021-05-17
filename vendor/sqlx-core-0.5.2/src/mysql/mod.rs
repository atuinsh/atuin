//! **MySQL** database driver.

mod arguments;
mod collation;
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

#[cfg(feature = "migrate")]
mod migrate;

pub use arguments::MySqlArguments;
pub use column::MySqlColumn;
pub use connection::MySqlConnection;
pub use database::MySql;
pub use error::MySqlDatabaseError;
pub use options::{MySqlConnectOptions, MySqlSslMode};
pub use query_result::MySqlQueryResult;
pub use row::MySqlRow;
pub use statement::MySqlStatement;
pub use transaction::MySqlTransactionManager;
pub use type_info::MySqlTypeInfo;
pub use value::{MySqlValue, MySqlValueFormat, MySqlValueRef};

/// An alias for [`Pool`][crate::pool::Pool], specialized for MySQL.
pub type MySqlPool = crate::pool::Pool<MySql>;

/// An alias for [`PoolOptions`][crate::pool::PoolOptions], specialized for MySQL.
pub type MySqlPoolOptions = crate::pool::PoolOptions<MySql>;

// NOTE: required due to the lack of lazy normalization
impl_into_arguments_for_arguments!(MySqlArguments);
impl_executor_for_pool_connection!(MySql, MySqlConnection, MySqlRow);
impl_executor_for_transaction!(MySql, MySqlRow);
impl_acquire!(MySql, MySqlConnection);
impl_column_index_for_row!(MySqlRow);
impl_column_index_for_statement!(MySqlStatement);
impl_into_maybe_pool!(MySql, MySqlConnection);

// required because some databases have a different handling of NULL
impl_encode_for_option!(MySql);
