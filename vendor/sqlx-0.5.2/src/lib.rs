#![cfg_attr(docsrs, feature(doc_cfg))]

#[cfg(any(
    feature = "runtime-actix",
    feature = "runtime-async-std",
    feature = "runtime-tokio"
))]
compile_error!(
    "the features 'runtime-actix', 'runtime-async-std' and 'runtime-tokio' have been removed in
     favor of new features 'runtime-{rt}-{tls}' where rt is one of 'actix', 'async-std' and 'tokio'
     and 'tls' is one of 'native-tls' and 'rustls'."
);

pub use sqlx_core::acquire::Acquire;
pub use sqlx_core::arguments::{Arguments, IntoArguments};
pub use sqlx_core::column::Column;
pub use sqlx_core::column::ColumnIndex;
pub use sqlx_core::connection::{ConnectOptions, Connection};
pub use sqlx_core::database::{self, Database};
pub use sqlx_core::describe::Describe;
pub use sqlx_core::executor::{Execute, Executor};
pub use sqlx_core::from_row::FromRow;
pub use sqlx_core::pool::{self, Pool};
pub use sqlx_core::query::{query, query_with};
pub use sqlx_core::query_as::{query_as, query_as_with};
pub use sqlx_core::query_scalar::{query_scalar, query_scalar_with};
pub use sqlx_core::row::Row;
pub use sqlx_core::statement::Statement;
pub use sqlx_core::transaction::{Transaction, TransactionManager};
pub use sqlx_core::type_info::TypeInfo;
pub use sqlx_core::types::Type;
pub use sqlx_core::value::{Value, ValueRef};

#[doc(inline)]
pub use sqlx_core::error::{self, Error, Result};

#[cfg(feature = "migrate")]
pub use sqlx_core::migrate;

#[cfg(all(
    any(
        feature = "mysql",
        feature = "sqlite",
        feature = "postgres",
        feature = "mssql"
    ),
    feature = "any"
))]
pub use sqlx_core::any::{self, Any, AnyConnection, AnyPool};

#[cfg(feature = "mysql")]
#[cfg_attr(docsrs, doc(cfg(feature = "mysql")))]
pub use sqlx_core::mysql::{self, MySql, MySqlConnection, MySqlPool};

#[cfg(feature = "mssql")]
#[cfg_attr(docsrs, doc(cfg(feature = "mssql")))]
pub use sqlx_core::mssql::{self, Mssql, MssqlConnection, MssqlPool};

#[cfg(feature = "postgres")]
#[cfg_attr(docsrs, doc(cfg(feature = "postgres")))]
pub use sqlx_core::postgres::{self, PgConnection, PgPool, Postgres};

#[cfg(feature = "sqlite")]
#[cfg_attr(docsrs, doc(cfg(feature = "sqlite")))]
pub use sqlx_core::sqlite::{self, Sqlite, SqliteConnection, SqlitePool};

#[cfg(feature = "macros")]
#[doc(hidden)]
pub extern crate sqlx_macros;

// derives
#[cfg(feature = "macros")]
#[doc(hidden)]
pub use sqlx_macros::{FromRow, Type};

#[cfg(feature = "macros")]
mod macros;

// macro support
#[cfg(feature = "macros")]
#[doc(hidden)]
pub mod ty_match;

/// Conversions between Rust and SQL types.
///
/// To see how each SQL type maps to a Rust type, see the corresponding `types` module for each
/// database:
///
///  * Postgres: [postgres::types]
///  * MySQL: [mysql::types]
///  * SQLite: [sqlite::types]
///  * MSSQL: [mssql::types]
///
/// Any external types that have had [`Type`] implemented for, are re-exported in this module
/// for convenience as downstream users need to use a compatible version of the external crate
/// to take advantage of the implementation.
///
/// [`Type`]: types::Type
pub mod types {
    pub use sqlx_core::types::*;

    #[cfg(feature = "macros")]
    #[doc(hidden)]
    pub use sqlx_macros::Type;
}

/// Provides [`Encode`](encode::Encode) for encoding values for the database.
pub mod encode {
    pub use sqlx_core::encode::{Encode, IsNull};

    #[cfg(feature = "macros")]
    #[doc(hidden)]
    pub use sqlx_macros::Encode;
}

pub use self::encode::Encode;

/// Provides [`Decode`](decode::Decode) for decoding values from the database.
pub mod decode {
    pub use sqlx_core::decode::Decode;

    #[cfg(feature = "macros")]
    #[doc(hidden)]
    pub use sqlx_macros::Decode;
}

pub use self::decode::Decode;

/// Types and traits for the `query` family of functions and macros.
pub mod query {
    pub use sqlx_core::query::{Map, Query};
    pub use sqlx_core::query_as::QueryAs;
    pub use sqlx_core::query_scalar::QueryScalar;
}

/// Convenience re-export of common traits.
pub mod prelude {
    pub use super::Acquire;
    pub use super::ConnectOptions;
    pub use super::Connection;
    pub use super::Decode;
    pub use super::Encode;
    pub use super::Executor;
    pub use super::FromRow;
    pub use super::IntoArguments;
    pub use super::Row;
    pub use super::Statement;
    pub use super::Type;
}

#[doc(hidden)]
#[inline(always)]
#[deprecated = "`#[sqlx(rename = \"...\")]` is now `#[sqlx(type_name = \"...\")`"]
pub fn _rename() {}
