//! Provides the connection pool for asynchronous SQLx connections.
//!
//! Opening a database connection for each and every operation to the database can quickly
//! become expensive. Furthermore, sharing a database connection between threads and functions
//! can be difficult to express in Rust.
//!
//! A connection pool is a standard technique that can manage opening and re-using connections.
//! Normally it also enforces a maximum number of connections as these are an expensive resource
//! on the database server.
//!
//! SQLx provides a canonical connection pool implementation intended to satisfy the majority
//! of use cases.
//!
//! See [Pool][crate::pool::Pool] for details.
//!
//! Type aliases are provided for each database to make it easier to sprinkle `Pool` through
//! your codebase:
//!
//! * [MssqlPool][crate::mssql::MssqlPool] (MSSQL)
//! * [MySqlPool][crate::mysql::MySqlPool] (MySQL)
//! * [PgPool][crate::postgres::PgPool] (PostgreSQL)
//! * [SqlitePool][crate::sqlite::SqlitePool] (SQLite)
//!
//! # Opening a connection pool
//!
//! A new connection pool with a default configuration can be created by supplying `Pool`
//! with the database driver and a connection string.
//!
//! ```rust,ignore
//! use sqlx::Pool;
//! use sqlx::postgres::Postgres;
//!
//! let pool = Pool::<Postgres>::connect("postgres://").await?;
//! ```
//!
//! For convenience, database-specific type aliases are provided:
//!
//! ```rust,ignore
//! use sqlx::mssql::MssqlPool;
//!
//! let pool = MssqlPool::connect("mssql://").await?;
//! ```
//!
//! # Using a connection pool
//!
//! A connection pool implements [`Executor`][crate::executor::Executor] and can be used directly
//! when executing a query. Notice that only an immutable reference (`&Pool`) is needed.
//!
//! ```rust,ignore
//! sqlx::query("DELETE FROM articles").execute(&pool).await?;
//! ```
//!
//! A connection or transaction may also be manually acquired with
//! [`Pool::acquire`] or
//! [`Pool::begin`].

use self::inner::SharedPool;
use crate::connection::Connection;
use crate::database::Database;
use crate::error::Error;
use crate::transaction::Transaction;
use std::fmt;
use std::future::Future;
use std::sync::Arc;
use std::time::{Duration, Instant};

#[macro_use]
mod executor;

#[macro_use]
mod maybe;

mod connection;
mod inner;
mod options;

pub use self::connection::PoolConnection;
pub(crate) use self::maybe::MaybePoolConnection;
pub use self::options::PoolOptions;

/// An asynchronous pool of SQLx database connections.
///
/// Create a pool with [Pool::connect] or [Pool::connect_with] and then call [Pool::acquire]
/// to get a connection from the pool; when the connection is dropped it will return to the pool
/// so it can be reused.
///
/// You can also pass `&Pool` directly anywhere an `Executor` is required; this will automatically
/// checkout a connection for you.
///
/// See [the module documentation](crate::pool) for examples.
///
/// The pool has a maximum connection limit that it will not exceed; if `acquire()` is called
/// when at this limit and all connections are checked out, the task will be made to wait until
/// a connection becomes available.
///
/// You can configure the connection limit, and other parameters, using [PoolOptions][crate::pool::PoolOptions].
///
/// Calls to `acquire()` are fair, i.e. fulfilled on a first-come, first-serve basis.
///
/// `Pool` is `Send`, `Sync` and `Clone`, so it should be created once at the start of your
/// application/daemon/web server/etc. and then shared with all tasks throughout its lifetime. How
/// best to accomplish this depends on your program architecture.
///
/// In Actix-Web, you can share a single pool with all request handlers using [web::Data].
///
/// Type aliases are provided for each database to make it easier to sprinkle `Pool` through
/// your codebase:
///
/// * [MssqlPool][crate::mssql::MssqlPool] (MSSQL)
/// * [MySqlPool][crate::mysql::MySqlPool] (MySQL)
/// * [PgPool][crate::postgres::PgPool] (PostgreSQL)
/// * [SqlitePool][crate::sqlite::SqlitePool] (SQLite)
///
/// [web::Data]: https://docs.rs/actix-web/2.0.0/actix_web/web/struct.Data.html
///
/// ### Why Use a Pool?
///
/// A single database connection (in general) cannot be used by multiple threads simultaneously
/// for various reasons, but an application or web server will typically need to execute numerous
/// queries or commands concurrently (think of concurrent requests against a web server; many or all
/// of them will probably need to hit the database).
///
/// You could place the connection in a `Mutex` but this will make it a huge bottleneck.
///
/// Naively, you might also think to just open a new connection per request, but this
/// has a number of other caveats, generally due to the high overhead involved in working with
/// a fresh connection. Examples to follow.
///
/// Connection pools facilitate reuse of connections to _amortize_ these costs, helping to ensure
/// that you're not paying for them each time you need a connection.
///
/// ##### 1. Overhead of Opening a Connection
/// Opening a database connection is not exactly a cheap operation.
///
/// For SQLite, it means numerous requests to the filesystem and memory allocations, while for
/// server-based databases it involves performing DNS resolution, opening a new TCP connection and
/// allocating buffers.
///
/// Each connection involves a nontrivial allocation of resources for the database server, usually
/// including spawning a new thread or process specifically to handle the connection, both for
/// concurrency and isolation of faults.
///
/// Additionally, database connections typically involve a complex handshake including
/// authentication, negotiation regarding connection parameters (default character sets, timezones,
/// locales, supported features) and upgrades to encrypted tunnels.
///
/// If `acquire()` is called on a pool with all connections checked out but it is not yet at its
/// connection limit (see next section), then a new connection is immediately opened, so this pool
/// does not _automatically_ save you from the overhead of creating a new connection.
///
/// However, because this pool by design enforces _reuse_ of connections, this overhead cost
/// is not paid each and every time you need a connection. In fact you set the `min_connections`
/// option in [PoolOptions], the pool will create that many connections up-front so that they are
/// ready to go when a request comes in.
///
/// ##### 2. Connection Limits (MySQL, MSSQL, Postgres)
/// Database servers usually place hard limits on the number of connections that it allows open at
/// any given time, to maintain performance targets and prevent excessive allocation of resources,
/// namely RAM.
///
/// These limits have different defaults per database flavor, and may vary between different
/// distributions of the same database, but are typically configurable on server start;
/// if you're paying for managed database hosting then the connection limit will typically vary with
/// your pricing tier.
///
/// In MySQL, the default limit is typically 150, plus 1 which is reserved for a user with the
/// `CONNECTION_ADMIN` privilege so you can still access the server to diagnose problems even
/// with all connections being used.
///
/// In MSSQL the only documentation for the default maximum limit is that it depends on the version
/// and server configuration.
///
/// In Postgres, the default limit is typically 100, minus 3 which are reserved for superusers
/// (putting the default limit for unprivileged users at 97 connections).
///
/// In any case, exceeding these limits results in an error when opening a new connection, which
/// in a web server context will turn into a `500 Internal Server Error` if not handled, but should
/// be turned into either `403 Forbidden` or `429 Too Many Requests` depending on your rate-limiting
/// scheme. However, in a web context, telling a client "go away, maybe try again later" results in
/// a sub-optimal user experience.
///
/// Instead with a connection pool, clients are made to wait in a fair queue for a connection to
/// become available; by using a single connection pool for your whole application, you can ensure
/// that you don't exceed the connection limit of your database server while allowing response
/// time to degrade gracefully at high load.
///
/// Of course, if multiple applications are connecting to the same database server, then you
/// should ensure that the connection limits for all applications add up to your server's maximum
/// connections or less.
///
/// ##### 3. Resource Reuse
/// The first time you execute a query against your database, the database engine must first turn
/// the SQL into an actionable _query plan_ which it may then execute against the database. This
/// involves parsing the SQL query, validating and analyzing it, and in the case of Postgres 12+ and
/// SQLite, generating code to execute the query plan (native or bytecode, respectively).
///
/// These database servers provide a way to amortize this overhead by _preparing_ the query,
/// associating it with an object ID and placing its query plan in a cache to be referenced when
/// it is later executed.
///
/// Prepared statements have other features, like bind parameters, which make them safer and more
/// ergonomic to use as well. By design, SQLx pushes you towards using prepared queries/statements
/// via the [Query][crate::query::Query] API _et al._ and the `query!()` macro _et al._, for
/// reasons of safety, ergonomics, and efficiency.
///
/// However, because database connections are typically isolated from each other in the database
/// server (either by threads or separate processes entirely), they don't typically share prepared
/// statements between connections so this work must be redone _for each connection_.
///
/// As with section 1, by facilitating reuse of connections, `Pool` helps to ensure their prepared
/// statements (and thus cached query plans) can be reused as much as possible, thus amortizing
/// the overhead involved.
///
/// Depending on the database server, a connection will have caches for all kinds of other data as
/// well and queries will generally benefit from these caches being "warm" (populated with data).
pub struct Pool<DB: Database>(pub(crate) Arc<SharedPool<DB>>);

impl<DB: Database> Pool<DB> {
    /// Creates a new connection pool with a default pool configuration and
    /// the given connection URI; and, immediately establishes one connection.
    pub async fn connect(uri: &str) -> Result<Self, Error> {
        PoolOptions::<DB>::new().connect(uri).await
    }

    /// Creates a new connection pool with a default pool configuration and
    /// the given connection options; and, immediately establishes one connection.
    pub async fn connect_with(
        options: <DB::Connection as Connection>::Options,
    ) -> Result<Self, Error> {
        PoolOptions::<DB>::new().connect_with(options).await
    }

    /// Creates a new connection pool with a default pool configuration and
    /// the given connection URI; and, will establish a connections as the pool
    /// starts to be used.
    pub fn connect_lazy(uri: &str) -> Result<Self, Error> {
        PoolOptions::<DB>::new().connect_lazy(uri)
    }

    /// Creates a new connection pool with a default pool configuration and
    /// the given connection options; and, will establish a connections as the pool
    /// starts to be used.
    pub fn connect_lazy_with(options: <DB::Connection as Connection>::Options) -> Self {
        PoolOptions::<DB>::new().connect_lazy_with(options)
    }

    /// Retrieves a connection from the pool.
    ///
    /// Waits for at most the configured connection timeout before returning an error.
    pub fn acquire(&self) -> impl Future<Output = Result<PoolConnection<DB>, Error>> + 'static {
        let shared = self.0.clone();
        async move { shared.acquire().await.map(|conn| conn.attach(&shared)) }
    }

    /// Attempts to retrieve a connection from the pool if there is one available.
    ///
    /// Returns `None` immediately if there are no idle connections available in the pool.
    pub fn try_acquire(&self) -> Option<PoolConnection<DB>> {
        self.0.try_acquire().map(|conn| conn.attach(&self.0))
    }

    /// Retrieves a new connection and immediately begins a new transaction.
    pub async fn begin(&self) -> Result<Transaction<'static, DB>, Error> {
        Ok(Transaction::begin(MaybePoolConnection::PoolConnection(self.acquire().await?)).await?)
    }

    /// Attempts to retrieve a new connection and immediately begins a new transaction if there
    /// is one available.
    pub async fn try_begin(&self) -> Result<Option<Transaction<'static, DB>>, Error> {
        match self.try_acquire() {
            Some(conn) => Transaction::begin(MaybePoolConnection::PoolConnection(conn))
                .await
                .map(Some),

            None => Ok(None),
        }
    }

    /// Ends the use of a connection pool. Prevents any new connections
    /// and will close all active connections when they are returned to the pool.
    ///
    /// Does not resolve until all connections are closed.
    pub async fn close(&self) {
        self.0.close().await;
    }

    /// Returns `true` if [`.close()`][Pool::close] has been called on the pool, `false` otherwise.
    pub fn is_closed(&self) -> bool {
        self.0.is_closed()
    }

    /// Returns the number of connections currently active. This includes idle connections.
    pub fn size(&self) -> u32 {
        self.0.size()
    }

    /// Returns the number of connections active and idle (not in use).
    ///
    /// This will block until the number of connections stops changing for at
    /// least 2 atomic accesses in a row. If the number of idle connections is
    /// changing rapidly, this may run indefinitely.
    pub fn num_idle(&self) -> usize {
        self.0.num_idle()
    }
}

/// Returns a new [Pool] tied to the same shared connection pool.
impl<DB: Database> Clone for Pool<DB> {
    fn clone(&self) -> Self {
        Self(Arc::clone(&self.0))
    }
}

impl<DB: Database> fmt::Debug for Pool<DB> {
    fn fmt(&self, fmt: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt.debug_struct("Pool")
            .field("size", &self.0.size())
            .field("num_idle", &self.0.num_idle())
            .field("is_closed", &self.0.is_closed())
            .field("options", &self.0.options)
            .finish()
    }
}

/// get the time between the deadline and now and use that as our timeout
///
/// returns `Error::PoolTimedOut` if the deadline is in the past
fn deadline_as_timeout<DB: Database>(deadline: Instant) -> Result<Duration, Error> {
    deadline
        .checked_duration_since(Instant::now())
        .ok_or(Error::PoolTimedOut)
}

#[test]
#[allow(dead_code)]
fn assert_pool_traits() {
    fn assert_send_sync<T: Send + Sync>() {}
    fn assert_clone<T: Clone>() {}

    fn assert_pool<DB: Database>() {
        assert_send_sync::<Pool<DB>>();
        assert_clone::<Pool<DB>>();
    }
}
