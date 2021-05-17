//! Traits to represent a database driver.
//!
//! # Support
//!
//! ## Tier 1
//!
//! Tier 1 support can be thought of as "guaranteed to work". Automated testing is setup to
//! ensure a high level of stability and functionality.
//!
//! | Database | Version | Driver |
//! | - | - | - |
//! | [MariaDB] | 10.1+ | [`mysql`] |
//! | [Microsoft SQL Server] | 2019 | [`mssql`] |
//! | [MySQL] | 5.6, 5.7, 8.0 | [`mysql`] |
//! | [PostgreSQL] | 9.5+ | [`postgres`] |
//! | [SQLite] | 3.20.1+ | [`sqlite`] |
//!
//! [MariaDB]: https://mariadb.com/
//! [MySQL]: https://www.mysql.com/
//! [Microsoft SQL Server]: https://www.microsoft.com/en-us/sql-server
//! [PostgreSQL]: https://www.postgresql.org/
//! [SQLite]: https://www.sqlite.org/
//!
//! [`mysql`]: crate::mysql
//! [`postgres`]: crate::postgres
//! [`mssql`]: crate::mssql
//! [`sqlite`]: crate::sqlite
//!
//! ## Tier 2
//!
//! Tier 2 support can be thought as "should work". No specific automated testing is done,
//! at this time, but there are efforts to ensure compatibility. Tier 2 support also includes
//! database distributions that provide protocols that closely match a database from Tier 1.
//!
//! _No databases are in tier 2 at this time._
//!
//! # `Any`
//!
//! Selecting a database driver is, by default, a compile-time decision. SQLx is designed this way
//! to take full advantage of the performance and type safety made available by Rust.
//!
//! We recognize that you may wish to make a runtime decision to decide the database driver. The
//! [`Any`](crate::any) driver is provided for that purpose.
//!
//! ## Example
//!
//! ```rust,ignore
//! // connect to SQLite
//! let conn = AnyConnection::connect("sqlite://file.db").await?;
//!
//! // connect to Postgres, no code change
//! // required, decided by the scheme of the URI
//! let conn = AnyConnection::connect("postgres://localhost/sqlx").await?;
//! ```

use std::fmt::Debug;

use crate::arguments::Arguments;
use crate::column::Column;
use crate::connection::Connection;
use crate::row::Row;
use crate::statement::Statement;
use crate::transaction::TransactionManager;
use crate::type_info::TypeInfo;
use crate::value::{Value, ValueRef};

/// A database driver.
///
/// This trait encapsulates a complete set of traits that implement a driver for a
/// specific database (e.g., MySQL, PostgreSQL).
pub trait Database:
    'static
    + Sized
    + Send
    + Debug
    + for<'r> HasValueRef<'r, Database = Self>
    + for<'q> HasArguments<'q, Database = Self>
    + for<'q> HasStatement<'q, Database = Self>
{
    /// The concrete `Connection` implementation for this database.
    type Connection: Connection<Database = Self>;

    /// The concrete `TransactionManager` implementation for this database.
    type TransactionManager: TransactionManager<Database = Self>;

    /// The concrete `Row` implementation for this database.
    type Row: Row<Database = Self>;

    /// The concrete `QueryResult` implementation for this database.
    type QueryResult: 'static + Sized + Send + Sync + Default + Extend<Self::QueryResult>;

    /// The concrete `Column` implementation for this database.
    type Column: Column<Database = Self>;

    /// The concrete `TypeInfo` implementation for this database.
    type TypeInfo: TypeInfo;

    /// The concrete type used to hold an owned copy of the not-yet-decoded value that was
    /// received from the database.
    type Value: Value<Database = Self> + 'static;
}

/// Associate [`Database`] with a [`ValueRef`](crate::value::ValueRef) of a generic lifetime.
///
/// ---
///
/// The upcoming Rust feature, [Generic Associated Types], should obviate
/// the need for this trait.
///
/// [Generic Associated Types]: https://github.com/rust-lang/rust/issues/44265
pub trait HasValueRef<'r> {
    type Database: Database;

    /// The concrete type used to hold a reference to the not-yet-decoded value that has just been
    /// received from the database.
    type ValueRef: ValueRef<'r, Database = Self::Database>;
}

/// Associate [`Database`] with an [`Arguments`](crate::arguments::Arguments) of a generic lifetime.
///
/// ---
///
/// The upcoming Rust feature, [Generic Associated Types], should obviate
/// the need for this trait.
///
/// [Generic Associated Types]: https://github.com/rust-lang/rust/issues/44265
pub trait HasArguments<'q> {
    type Database: Database;

    /// The concrete `Arguments` implementation for this database.
    type Arguments: Arguments<'q, Database = Self::Database>;

    /// The concrete type used as a buffer for arguments while encoding.
    type ArgumentBuffer;
}

/// Associate [`Database`] with a [`Statement`](crate::statement::Statement) of a generic lifetime.
///
/// ---
///
/// The upcoming Rust feature, [Generic Associated Types], should obviate
/// the need for this trait.
///
/// [Generic Associated Types]: https://github.com/rust-lang/rust/issues/44265
pub trait HasStatement<'q> {
    type Database: Database;

    /// The concrete `Statement` implementation for this database.
    type Statement: Statement<'q, Database = Self::Database>;
}

/// A [`Database`] that maintains a client-side cache of prepared statements.
pub trait HasStatementCache {}
