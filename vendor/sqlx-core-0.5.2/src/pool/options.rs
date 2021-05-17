use crate::connection::Connection;
use crate::database::Database;
use crate::error::Error;
use crate::pool::inner::SharedPool;
use crate::pool::Pool;
use futures_core::future::BoxFuture;
use sqlx_rt::spawn;
use std::cmp;
use std::fmt::{self, Debug, Formatter};
use std::sync::Arc;
use std::time::{Duration, Instant};

pub struct PoolOptions<DB: Database> {
    pub(crate) test_before_acquire: bool,
    pub(crate) after_connect: Option<
        Box<
            dyn Fn(&mut DB::Connection) -> BoxFuture<'_, Result<(), Error>> + 'static + Send + Sync,
        >,
    >,
    pub(crate) before_acquire: Option<
        Box<
            dyn Fn(&mut DB::Connection) -> BoxFuture<'_, Result<bool, Error>>
                + 'static
                + Send
                + Sync,
        >,
    >,
    pub(crate) after_release:
        Option<Box<dyn Fn(&mut DB::Connection) -> bool + 'static + Send + Sync>>,
    pub(crate) max_connections: u32,
    pub(crate) connect_timeout: Duration,
    pub(crate) min_connections: u32,
    pub(crate) max_lifetime: Option<Duration>,
    pub(crate) idle_timeout: Option<Duration>,
    pub(crate) fair: bool,
}

impl<DB: Database> Default for PoolOptions<DB> {
    fn default() -> Self {
        Self::new()
    }
}

impl<DB: Database> PoolOptions<DB> {
    pub fn new() -> Self {
        Self {
            after_connect: None,
            test_before_acquire: true,
            before_acquire: None,
            after_release: None,
            max_connections: 10,
            min_connections: 0,
            connect_timeout: Duration::from_secs(30),
            idle_timeout: Some(Duration::from_secs(10 * 60)),
            max_lifetime: Some(Duration::from_secs(30 * 60)),
            fair: true,
        }
    }

    /// Set the maximum number of connections that this pool should maintain.
    pub fn max_connections(mut self, max: u32) -> Self {
        self.max_connections = max;
        self
    }

    /// Set the amount of time to attempt connecting to the database.
    ///
    /// If this timeout elapses, [`Pool::acquire`] will return an error.
    pub fn connect_timeout(mut self, timeout: Duration) -> Self {
        self.connect_timeout = timeout;
        self
    }

    /// Set the minimum number of connections to maintain at all times.
    ///
    /// When the pool is built, this many connections will be automatically spun up.
    ///
    /// If any connection is reaped by [`max_lifetime`] or [`idle_timeout`] and it brings
    /// the connection count below this amount, a new connection will be opened to replace it.
    ///
    /// [`max_lifetime`]: Self::max_lifetime
    /// [`idle_timeout`]: Self::idle_timeout
    pub fn min_connections(mut self, min: u32) -> Self {
        self.min_connections = min;
        self
    }

    /// Set the maximum lifetime of individual connections.
    ///
    /// Any connection with a lifetime greater than this will be closed.
    ///
    /// When set to `None`, all connections live until either reaped by [`idle_timeout`]
    /// or explicitly disconnected.
    ///
    /// Infinite connections are not recommended due to the unfortunate reality of memory/resource
    /// leaks on the database-side. It is better to retire connections periodically
    /// (even if only once daily) to allow the database the opportunity to clean up data structures
    /// (parse trees, query metadata caches, thread-local storage, etc.) that are associated with a
    /// session.
    ///
    /// [`idle_timeout`]: Self::idle_timeout
    pub fn max_lifetime(mut self, lifetime: impl Into<Option<Duration>>) -> Self {
        self.max_lifetime = lifetime.into();
        self
    }

    /// Set a maximum idle duration for individual connections.
    ///
    /// Any connection with an idle duration longer than this will be closed.
    ///
    /// For usage-based database server billing, this can be a cost saver.
    pub fn idle_timeout(mut self, timeout: impl Into<Option<Duration>>) -> Self {
        self.idle_timeout = timeout.into();
        self
    }

    /// If true, the health of a connection will be verified by a call to [`Connection::ping`]
    /// before returning the connection.
    ///
    /// Defaults to `true`.
    pub fn test_before_acquire(mut self, test: bool) -> Self {
        self.test_before_acquire = test;
        self
    }

    /// If set to `true`, calls to `acquire()` are fair and connections  are issued
    /// in first-come-first-serve order. If `false`, "drive-by" tasks may steal idle connections
    /// ahead of tasks that have been waiting.
    ///
    /// According to `sqlx-bench/benches/pg_pool` this may slightly increase time
    /// to `acquire()` at low pool contention but at very high contention it helps
    /// avoid tasks at the head of the waiter queue getting repeatedly preempted by
    /// these "drive-by" tasks and tasks further back in the queue timing out because
    /// the queue isn't moving.
    ///
    /// Currently only exposed for benchmarking; `fair = true` seems to be the superior option
    /// in most cases.
    #[doc(hidden)]
    pub fn __fair(mut self, fair: bool) -> Self {
        self.fair = fair;
        self
    }

    /// Perform an action after connecting to the database.
    ///
    /// # Example
    ///
    /// ```no_run
    /// # async fn f() -> Result<(), Box<dyn std::error::Error>> {
    /// use sqlx_core::executor::Executor;
    /// use sqlx_core::postgres::PgPoolOptions;
    /// // PostgreSQL
    /// let pool = PgPoolOptions::new()
    ///     .after_connect(|conn| Box::pin(async move {
    ///        conn.execute("SET application_name = 'your_app';").await?;
    ///        conn.execute("SET search_path = 'my_schema';").await?;
    ///
    ///        Ok(())
    ///     }))
    ///     .connect("postgres:// …").await?;
    /// # Ok(())
    /// # }
    /// ```
    pub fn after_connect<F>(mut self, callback: F) -> Self
    where
        for<'c> F:
            Fn(&'c mut DB::Connection) -> BoxFuture<'c, Result<(), Error>> + 'static + Send + Sync,
    {
        self.after_connect = Some(Box::new(callback));
        self
    }

    pub fn before_acquire<F>(mut self, callback: F) -> Self
    where
        for<'c> F: Fn(&'c mut DB::Connection) -> BoxFuture<'c, Result<bool, Error>>
            + 'static
            + Send
            + Sync,
    {
        self.before_acquire = Some(Box::new(callback));
        self
    }

    pub fn after_release<F>(mut self, callback: F) -> Self
    where
        F: Fn(&mut DB::Connection) -> bool + 'static + Send + Sync,
    {
        self.after_release = Some(Box::new(callback));
        self
    }

    /// Creates a new pool from this configuration and immediately establishes one connection.
    pub async fn connect(self, uri: &str) -> Result<Pool<DB>, Error> {
        self.connect_with(uri.parse()?).await
    }

    /// Creates a new pool from this configuration and immediately establishes one connection.
    pub async fn connect_with(
        self,
        options: <DB::Connection as Connection>::Options,
    ) -> Result<Pool<DB>, Error> {
        let shared = SharedPool::new_arc(self, options);

        init_min_connections(&shared).await?;

        Ok(Pool(shared))
    }

    /// Creates a new pool from this configuration and will establish a connections as the pool
    /// starts to be used.
    pub fn connect_lazy(self, uri: &str) -> Result<Pool<DB>, Error> {
        Ok(self.connect_lazy_with(uri.parse()?))
    }

    /// Creates a new pool from this configuration and will establish a connections as the pool
    /// starts to be used.
    pub fn connect_lazy_with(self, options: <DB::Connection as Connection>::Options) -> Pool<DB> {
        let shared = SharedPool::new_arc(self, options);

        let _ = spawn({
            let shared = Arc::clone(&shared);
            async move {
                let _ = init_min_connections(&shared).await;
            }
        });

        Pool(shared)
    }
}

async fn init_min_connections<DB: Database>(pool: &SharedPool<DB>) -> Result<(), Error> {
    for _ in 0..cmp::max(pool.options.min_connections, 1) {
        let deadline = Instant::now() + pool.options.connect_timeout;

        // this guard will prevent us from exceeding `max_size`
        if let Some(guard) = pool.try_increment_size() {
            // [connect] will raise an error when past deadline
            let conn = pool.connection(deadline, guard).await?;
            let is_ok = pool
                .idle_conns
                .push(conn.into_idle().into_leakable())
                .is_ok();

            if !is_ok {
                panic!("BUG: connection queue overflow in init_min_connections");
            }
        }
    }

    Ok(())
}

impl<DB: Database> Debug for PoolOptions<DB> {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.debug_struct("PoolOptions")
            .field("max_connections", &self.max_connections)
            .field("min_connections", &self.min_connections)
            .field("connect_timeout", &self.connect_timeout)
            .field("max_lifetime", &self.max_lifetime)
            .field("idle_timeout", &self.idle_timeout)
            .field("test_before_acquire", &self.test_before_acquire)
            .finish()
    }
}
