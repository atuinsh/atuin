use crate::describe::Describe;
use crate::error::Error;
use crate::executor::{Execute, Executor};
use crate::pool::PoolOptions;
use crate::pool::{Pool, PoolConnection};
use crate::postgres::message::{MessageFormat, Notification};
use crate::postgres::{PgConnection, PgQueryResult, PgRow, PgStatement, PgTypeInfo, Postgres};
use either::Either;
use futures_channel::mpsc;
use futures_core::future::BoxFuture;
use futures_core::stream::{BoxStream, Stream};
use std::fmt::{self, Debug};
use std::io;
use std::str::from_utf8;

/// A stream of asynchronous notifications from Postgres.
///
/// This listener will auto-reconnect. If the active
/// connection being used ever dies, this listener will detect that event, create a
/// new connection, will re-subscribe to all of the originally specified channels, and will resume
/// operations as normal.
pub struct PgListener {
    pool: Pool<Postgres>,
    connection: Option<PoolConnection<Postgres>>,
    buffer_rx: mpsc::UnboundedReceiver<Notification>,
    buffer_tx: Option<mpsc::UnboundedSender<Notification>>,
    channels: Vec<String>,
}

/// An asynchronous notification from Postgres.
pub struct PgNotification(Notification);

impl PgListener {
    pub async fn connect(uri: &str) -> Result<Self, Error> {
        // Create a pool of 1 without timeouts (as they don't apply here)
        // We only use the pool to handle re-connections
        let pool = PoolOptions::<Postgres>::new()
            .max_connections(1)
            .max_lifetime(None)
            .idle_timeout(None)
            .connect(uri)
            .await?;

        Self::connect_with(&pool).await
    }

    pub async fn connect_with(pool: &Pool<Postgres>) -> Result<Self, Error> {
        // Pull out an initial connection
        let mut connection = pool.acquire().await?;

        // Setup a notification buffer
        let (sender, receiver) = mpsc::unbounded();
        connection.stream.notifications = Some(sender);

        Ok(Self {
            pool: pool.clone(),
            connection: Some(connection),
            buffer_rx: receiver,
            buffer_tx: None,
            channels: Vec::new(),
        })
    }

    /// Starts listening for notifications on a channel.
    /// The channel name is quoted here to ensure case sensitivity.
    pub async fn listen(&mut self, channel: &str) -> Result<(), Error> {
        self.connection()
            .execute(&*format!(r#"LISTEN "{}""#, ident(channel)))
            .await?;

        self.channels.push(channel.to_owned());

        Ok(())
    }

    /// Starts listening for notifications on all channels.
    pub async fn listen_all(
        &mut self,
        channels: impl IntoIterator<Item = &str>,
    ) -> Result<(), Error> {
        let beg = self.channels.len();
        self.channels.extend(channels.into_iter().map(|s| s.into()));

        self.connection
            .as_mut()
            .unwrap()
            .execute(&*build_listen_all_query(&self.channels[beg..]))
            .await?;

        Ok(())
    }

    /// Stops listening for notifications on a channel.
    /// The channel name is quoted here to ensure case sensitivity.
    pub async fn unlisten(&mut self, channel: &str) -> Result<(), Error> {
        self.connection()
            .execute(&*format!(r#"UNLISTEN "{}""#, ident(channel)))
            .await?;

        if let Some(pos) = self.channels.iter().position(|s| s == channel) {
            self.channels.remove(pos);
        }

        Ok(())
    }

    /// Stops listening for notifications on all channels.
    pub async fn unlisten_all(&mut self) -> Result<(), Error> {
        self.connection().execute("UNLISTEN *").await?;

        self.channels.clear();

        Ok(())
    }

    #[inline]
    async fn connect_if_needed(&mut self) -> Result<(), Error> {
        if self.connection.is_none() {
            let mut connection = self.pool.acquire().await?;
            connection.stream.notifications = self.buffer_tx.take();

            connection
                .execute(&*build_listen_all_query(&self.channels))
                .await?;

            self.connection = Some(connection);
        }

        Ok(())
    }

    #[inline]
    fn connection(&mut self) -> &mut PgConnection {
        self.connection.as_mut().unwrap()
    }

    /// Receives the next notification available from any of the subscribed channels.
    ///
    /// If the connection to PostgreSQL is lost, it is automatically reconnected on the next
    /// call to `recv()`, and should be entirely transparent (as long as it was just an
    /// intermittent network failure or long-lived connection reaper).
    ///
    /// As notifications are transient, any received while the connection was lost, will not
    /// be returned. If you'd prefer the reconnection to be explicit and have a chance to
    /// do something before, please see [`try_recv`](Self::try_recv).
    ///
    /// # Example
    ///
    /// ```rust,no_run
    /// # use sqlx_core::postgres::PgListener;
    /// # use sqlx_core::error::Error;
    /// #
    /// # #[cfg(feature = "_rt-async-std")]
    /// # sqlx_rt::block_on::<_, Result<(), Error>>(async move {
    /// # let mut listener = PgListener::connect("postgres:// ...").await?;
    /// loop {
    ///     // ask for next notification, re-connecting (transparently) if needed
    ///     let notification = listener.recv().await?;
    ///
    ///     // handle notification, do something interesting
    /// }
    /// # Ok(())
    /// # }).unwrap();
    /// ```
    pub async fn recv(&mut self) -> Result<PgNotification, Error> {
        loop {
            if let Some(notification) = self.try_recv().await? {
                return Ok(notification);
            }
        }
    }

    /// Receives the next notification available from any of the subscribed channels.
    ///
    /// If the connection to PostgreSQL is lost, `None` is returned, and the connection is
    /// reconnected on the next call to `try_recv()`.
    ///
    /// # Example
    ///
    /// ```rust,no_run
    /// # use sqlx_core::postgres::PgListener;
    /// # use sqlx_core::error::Error;
    /// #
    /// # #[cfg(feature = "_rt-async-std")]
    /// # sqlx_rt::block_on::<_, Result<(), Error>>(async move {
    /// # let mut listener = PgListener::connect("postgres:// ...").await?;
    /// loop {
    ///     // start handling notifications, connecting if needed
    ///     while let Some(notification) = listener.try_recv().await? {
    ///         // handle notification
    ///     }
    ///
    ///     // connection lost, do something interesting
    /// }
    /// # Ok(())
    /// # }).unwrap();
    /// ```
    pub async fn try_recv(&mut self) -> Result<Option<PgNotification>, Error> {
        // Flush the buffer first, if anything
        // This would only fill up if this listener is used as a connection
        if let Ok(Some(notification)) = self.buffer_rx.try_next() {
            return Ok(Some(PgNotification(notification)));
        }

        loop {
            // Ensure we have an active connection to work with.
            self.connect_if_needed().await?;

            let message = match self.connection().stream.recv_unchecked().await {
                Ok(message) => message,

                // The connection is dead, ensure that it is dropped,
                // update self state, and loop to try again.
                Err(Error::Io(err)) if err.kind() == io::ErrorKind::ConnectionAborted => {
                    self.buffer_tx = self.connection().stream.notifications.take();
                    self.connection = None;

                    // lost connection
                    return Ok(None);
                }

                // Forward other errors
                Err(error) => {
                    return Err(error);
                }
            };

            match message.format {
                // We've received an async notification, return it.
                MessageFormat::NotificationResponse => {
                    return Ok(Some(PgNotification(message.decode()?)));
                }

                // Mark the connection as ready for another query
                MessageFormat::ReadyForQuery => {
                    self.connection().pending_ready_for_query_count -= 1;
                }

                // Ignore unexpected messages
                _ => {}
            }
        }
    }

    /// Consume this listener, returning a `Stream` of notifications.
    ///
    /// The backing connection will be automatically reconnected should it be lost.
    ///
    /// This has the same potential drawbacks as [`recv`](PgListener::recv).
    ///
    pub fn into_stream(mut self) -> impl Stream<Item = Result<PgNotification, Error>> + Unpin {
        Box::pin(try_stream! {
            loop {
                r#yield!(self.recv().await?);
            }
        })
    }
}

impl Drop for PgListener {
    fn drop(&mut self) {
        if let Some(mut conn) = self.connection.take() {
            // Unregister any listeners before returning the connection to the pool.
            sqlx_rt::spawn(async move {
                let _ = conn.execute("UNLISTEN *").await;
            });
        }
    }
}

impl<'c> Executor<'c> for &'c mut PgListener {
    type Database = Postgres;

    fn fetch_many<'e, 'q: 'e, E: 'q>(
        self,
        query: E,
    ) -> BoxStream<'e, Result<Either<PgQueryResult, PgRow>, Error>>
    where
        'c: 'e,
        E: Execute<'q, Self::Database>,
    {
        self.connection().fetch_many(query)
    }

    fn fetch_optional<'e, 'q: 'e, E: 'q>(
        self,
        query: E,
    ) -> BoxFuture<'e, Result<Option<PgRow>, Error>>
    where
        'c: 'e,
        E: Execute<'q, Self::Database>,
    {
        self.connection().fetch_optional(query)
    }

    fn prepare_with<'e, 'q: 'e>(
        self,
        query: &'q str,
        parameters: &'e [PgTypeInfo],
    ) -> BoxFuture<'e, Result<PgStatement<'q>, Error>>
    where
        'c: 'e,
    {
        self.connection().prepare_with(query, parameters)
    }

    #[doc(hidden)]
    fn describe<'e, 'q: 'e>(
        self,
        query: &'q str,
    ) -> BoxFuture<'e, Result<Describe<Self::Database>, Error>>
    where
        'c: 'e,
    {
        self.connection().describe(query)
    }
}

impl PgNotification {
    /// The process ID of the notifying backend process.
    #[inline]
    pub fn process_id(&self) -> u32 {
        self.0.process_id
    }

    /// The channel that the notify has been raised on. This can be thought
    /// of as the message topic.
    #[inline]
    pub fn channel(&self) -> &str {
        from_utf8(&self.0.channel).unwrap()
    }

    /// The payload of the notification. An empty payload is received as an
    /// empty string.
    #[inline]
    pub fn payload(&self) -> &str {
        from_utf8(&self.0.payload).unwrap()
    }
}

impl Debug for PgListener {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PgListener").finish()
    }
}

impl Debug for PgNotification {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PgNotification")
            .field("process_id", &self.process_id())
            .field("channel", &self.channel())
            .field("payload", &self.payload())
            .finish()
    }
}

fn ident(mut name: &str) -> String {
    // If the input string contains a NUL byte, we should truncate the
    // identifier.
    if let Some(index) = name.find('\0') {
        name = &name[..index];
    }

    // Any double quotes must be escaped
    name.replace('"', "\"\"")
}

fn build_listen_all_query(channels: impl IntoIterator<Item = impl AsRef<str>>) -> String {
    channels.into_iter().fold(String::new(), |mut acc, chan| {
        acc.push_str(r#"LISTEN ""#);
        acc.push_str(&ident(chan.as_ref()));
        acc.push_str(r#"";"#);
        acc
    })
}

#[test]
fn test_build_listen_all_query_with_single_channel() {
    let output = build_listen_all_query(&["test"]);
    assert_eq!(output.as_str(), r#"LISTEN "test";"#);
}

#[test]
fn test_build_listen_all_query_with_multiple_channels() {
    let output = build_listen_all_query(&["channel.0", "channel.1"]);
    assert_eq!(output.as_str(), r#"LISTEN "channel.0";LISTEN "channel.1";"#);
}
