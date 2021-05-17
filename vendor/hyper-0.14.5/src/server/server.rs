use std::error::Error as StdError;
use std::fmt;
#[cfg(feature = "tcp")]
use std::net::{SocketAddr, TcpListener as StdTcpListener};

#[cfg(feature = "tcp")]
use std::time::Duration;

use pin_project::pin_project;
use tokio::io::{AsyncRead, AsyncWrite};

use super::accept::Accept;
use crate::body::{Body, HttpBody};
use crate::common::exec::{ConnStreamExec, Exec, NewSvcExec};
use crate::common::{task, Future, Pin, Poll, Unpin};
use crate::service::{HttpService, MakeServiceRef};
// Renamed `Http` as `Http_` for now so that people upgrading don't see an
// error that `hyper::server::Http` is private...
use super::conn::{Http as Http_, NoopWatcher, SpawnAll};
use super::shutdown::{Graceful, GracefulWatcher};
#[cfg(feature = "tcp")]
use super::tcp::AddrIncoming;

/// A listening HTTP server that accepts connections in both HTTP1 and HTTP2 by default.
///
/// `Server` is a `Future` mapping a bound listener with a set of service
/// handlers. It is built using the [`Builder`](Builder), and the future
/// completes when the server has been shutdown. It should be run by an
/// `Executor`.
#[pin_project]
pub struct Server<I, S, E = Exec> {
    #[pin]
    spawn_all: SpawnAll<I, S, E>,
}

/// A builder for a [`Server`](Server).
#[derive(Debug)]
pub struct Builder<I, E = Exec> {
    incoming: I,
    protocol: Http_<E>,
}

// ===== impl Server =====

impl<I> Server<I, ()> {
    /// Starts a [`Builder`](Builder) with the provided incoming stream.
    pub fn builder(incoming: I) -> Builder<I> {
        Builder {
            incoming,
            protocol: Http_::new(),
        }
    }
}

cfg_feature! {
    #![all(feature = "tcp")]

    impl Server<AddrIncoming, ()> {
        /// Binds to the provided address, and returns a [`Builder`](Builder).
        ///
        /// # Panics
        ///
        /// This method will panic if binding to the address fails. For a method
        /// to bind to an address and return a `Result`, see `Server::try_bind`.
        pub fn bind(addr: &SocketAddr) -> Builder<AddrIncoming> {
            let incoming = AddrIncoming::new(addr).unwrap_or_else(|e| {
                panic!("error binding to {}: {}", addr, e);
            });
            Server::builder(incoming)
        }

        /// Tries to bind to the provided address, and returns a [`Builder`](Builder).
        pub fn try_bind(addr: &SocketAddr) -> crate::Result<Builder<AddrIncoming>> {
            AddrIncoming::new(addr).map(Server::builder)
        }

        /// Create a new instance from a `std::net::TcpListener` instance.
        pub fn from_tcp(listener: StdTcpListener) -> Result<Builder<AddrIncoming>, crate::Error> {
            AddrIncoming::from_std(listener).map(Server::builder)
        }
    }
}

cfg_feature! {
    #![all(feature = "tcp")]

    impl<S, E> Server<AddrIncoming, S, E> {
        /// Returns the local address that this server is bound to.
        pub fn local_addr(&self) -> SocketAddr {
            self.spawn_all.local_addr()
        }
    }
}

impl<I, IO, IE, S, E, B> Server<I, S, E>
where
    I: Accept<Conn = IO, Error = IE>,
    IE: Into<Box<dyn StdError + Send + Sync>>,
    IO: AsyncRead + AsyncWrite + Unpin + Send + 'static,
    S: MakeServiceRef<IO, Body, ResBody = B>,
    S::Error: Into<Box<dyn StdError + Send + Sync>>,
    B: HttpBody + Send + Sync + 'static,
    B::Error: Into<Box<dyn StdError + Send + Sync>>,
    E: ConnStreamExec<<S::Service as HttpService<Body>>::Future, B>,
    E: NewSvcExec<IO, S::Future, S::Service, E, GracefulWatcher>,
{
    /// Prepares a server to handle graceful shutdown when the provided future
    /// completes.
    ///
    /// # Example
    ///
    /// ```
    /// # fn main() {}
    /// # #[cfg(feature = "tcp")]
    /// # async fn run() {
    /// # use hyper::{Body, Response, Server, Error};
    /// # use hyper::service::{make_service_fn, service_fn};
    /// # let make_service = make_service_fn(|_| async {
    /// #     Ok::<_, Error>(service_fn(|_req| async {
    /// #         Ok::<_, Error>(Response::new(Body::from("Hello World")))
    /// #     }))
    /// # });
    /// // Make a server from the previous examples...
    /// let server = Server::bind(&([127, 0, 0, 1], 3000).into())
    ///     .serve(make_service);
    ///
    /// // Prepare some signal for when the server should start shutting down...
    /// let (tx, rx) = tokio::sync::oneshot::channel::<()>();
    /// let graceful = server
    ///     .with_graceful_shutdown(async {
    ///         rx.await.ok();
    ///     });
    ///
    /// // Await the `server` receiving the signal...
    /// if let Err(e) = graceful.await {
    ///     eprintln!("server error: {}", e);
    /// }
    ///
    /// // And later, trigger the signal by calling `tx.send(())`.
    /// let _ = tx.send(());
    /// # }
    /// ```
    pub fn with_graceful_shutdown<F>(self, signal: F) -> Graceful<I, S, F, E>
    where
        F: Future<Output = ()>,
    {
        Graceful::new(self.spawn_all, signal)
    }
}

impl<I, IO, IE, S, B, E> Future for Server<I, S, E>
where
    I: Accept<Conn = IO, Error = IE>,
    IE: Into<Box<dyn StdError + Send + Sync>>,
    IO: AsyncRead + AsyncWrite + Unpin + Send + 'static,
    S: MakeServiceRef<IO, Body, ResBody = B>,
    S::Error: Into<Box<dyn StdError + Send + Sync>>,
    B: HttpBody + 'static,
    B::Error: Into<Box<dyn StdError + Send + Sync>>,
    E: ConnStreamExec<<S::Service as HttpService<Body>>::Future, B>,
    E: NewSvcExec<IO, S::Future, S::Service, E, NoopWatcher>,
{
    type Output = crate::Result<()>;

    fn poll(self: Pin<&mut Self>, cx: &mut task::Context<'_>) -> Poll<Self::Output> {
        self.project().spawn_all.poll_watch(cx, &NoopWatcher)
    }
}

impl<I: fmt::Debug, S: fmt::Debug> fmt::Debug for Server<I, S> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Server")
            .field("listener", &self.spawn_all.incoming_ref())
            .finish()
    }
}

// ===== impl Builder =====

impl<I, E> Builder<I, E> {
    /// Start a new builder, wrapping an incoming stream and low-level options.
    ///
    /// For a more convenient constructor, see [`Server::bind`](Server::bind).
    pub fn new(incoming: I, protocol: Http_<E>) -> Self {
        Builder { incoming, protocol }
    }

    /// Sets whether to use keep-alive for HTTP/1 connections.
    ///
    /// Default is `true`.
    #[cfg(feature = "http1")]
    #[cfg_attr(docsrs, doc(cfg(feature = "http1")))]
    pub fn http1_keepalive(mut self, val: bool) -> Self {
        self.protocol.http1_keep_alive(val);
        self
    }

    /// Set whether HTTP/1 connections should support half-closures.
    ///
    /// Clients can chose to shutdown their write-side while waiting
    /// for the server to respond. Setting this to `true` will
    /// prevent closing the connection immediately if `read`
    /// detects an EOF in the middle of a request.
    ///
    /// Default is `false`.
    #[cfg(feature = "http1")]
    #[cfg_attr(docsrs, doc(cfg(feature = "http1")))]
    pub fn http1_half_close(mut self, val: bool) -> Self {
        self.protocol.http1_half_close(val);
        self
    }

    /// Set the maximum buffer size.
    ///
    /// Default is ~ 400kb.
    #[cfg(feature = "http1")]
    #[cfg_attr(docsrs, doc(cfg(feature = "http1")))]
    pub fn http1_max_buf_size(mut self, val: usize) -> Self {
        self.protocol.max_buf_size(val);
        self
    }

    // Sets whether to bunch up HTTP/1 writes until the read buffer is empty.
    //
    // This isn't really desirable in most cases, only really being useful in
    // silly pipeline benchmarks.
    #[doc(hidden)]
    #[cfg(feature = "http1")]
    pub fn http1_pipeline_flush(mut self, val: bool) -> Self {
        self.protocol.pipeline_flush(val);
        self
    }

    /// Sets whether HTTP/1 is required.
    ///
    /// Default is `false`.
    #[cfg(feature = "http1")]
    #[cfg_attr(docsrs, doc(cfg(feature = "http1")))]
    pub fn http1_only(mut self, val: bool) -> Self {
        self.protocol.http1_only(val);
        self
    }

    /// Sets whether HTTP/2 is required.
    ///
    /// Default is `false`.
    #[cfg(feature = "http2")]
    #[cfg_attr(docsrs, doc(cfg(feature = "http2")))]
    pub fn http2_only(mut self, val: bool) -> Self {
        self.protocol.http2_only(val);
        self
    }

    /// Sets the [`SETTINGS_INITIAL_WINDOW_SIZE`][spec] option for HTTP2
    /// stream-level flow control.
    ///
    /// Passing `None` will do nothing.
    ///
    /// If not set, hyper will use a default.
    ///
    /// [spec]: https://http2.github.io/http2-spec/#SETTINGS_INITIAL_WINDOW_SIZE
    #[cfg(feature = "http2")]
    #[cfg_attr(docsrs, doc(cfg(feature = "http2")))]
    pub fn http2_initial_stream_window_size(mut self, sz: impl Into<Option<u32>>) -> Self {
        self.protocol.http2_initial_stream_window_size(sz.into());
        self
    }

    /// Sets the max connection-level flow control for HTTP2
    ///
    /// Passing `None` will do nothing.
    ///
    /// If not set, hyper will use a default.
    #[cfg(feature = "http2")]
    #[cfg_attr(docsrs, doc(cfg(feature = "http2")))]
    pub fn http2_initial_connection_window_size(mut self, sz: impl Into<Option<u32>>) -> Self {
        self.protocol
            .http2_initial_connection_window_size(sz.into());
        self
    }

    /// Sets whether to use an adaptive flow control.
    ///
    /// Enabling this will override the limits set in
    /// `http2_initial_stream_window_size` and
    /// `http2_initial_connection_window_size`.
    #[cfg(feature = "http2")]
    #[cfg_attr(docsrs, doc(cfg(feature = "http2")))]
    pub fn http2_adaptive_window(mut self, enabled: bool) -> Self {
        self.protocol.http2_adaptive_window(enabled);
        self
    }

    /// Sets the maximum frame size to use for HTTP2.
    ///
    /// Passing `None` will do nothing.
    ///
    /// If not set, hyper will use a default.
    #[cfg(feature = "http2")]
    #[cfg_attr(docsrs, doc(cfg(feature = "http2")))]
    pub fn http2_max_frame_size(mut self, sz: impl Into<Option<u32>>) -> Self {
        self.protocol.http2_max_frame_size(sz);
        self
    }

    /// Sets the [`SETTINGS_MAX_CONCURRENT_STREAMS`][spec] option for HTTP2
    /// connections.
    ///
    /// Default is no limit (`std::u32::MAX`). Passing `None` will do nothing.
    ///
    /// [spec]: https://http2.github.io/http2-spec/#SETTINGS_MAX_CONCURRENT_STREAMS
    #[cfg(feature = "http2")]
    #[cfg_attr(docsrs, doc(cfg(feature = "http2")))]
    pub fn http2_max_concurrent_streams(mut self, max: impl Into<Option<u32>>) -> Self {
        self.protocol.http2_max_concurrent_streams(max.into());
        self
    }

    /// Sets an interval for HTTP2 Ping frames should be sent to keep a
    /// connection alive.
    ///
    /// Pass `None` to disable HTTP2 keep-alive.
    ///
    /// Default is currently disabled.
    ///
    /// # Cargo Feature
    ///
    /// Requires the `runtime` cargo feature to be enabled.
    #[cfg(feature = "runtime")]
    #[cfg(feature = "http2")]
    #[cfg_attr(docsrs, doc(cfg(feature = "http2")))]
    pub fn http2_keep_alive_interval(mut self, interval: impl Into<Option<Duration>>) -> Self {
        self.protocol.http2_keep_alive_interval(interval);
        self
    }

    /// Sets a timeout for receiving an acknowledgement of the keep-alive ping.
    ///
    /// If the ping is not acknowledged within the timeout, the connection will
    /// be closed. Does nothing if `http2_keep_alive_interval` is disabled.
    ///
    /// Default is 20 seconds.
    ///
    /// # Cargo Feature
    ///
    /// Requires the `runtime` cargo feature to be enabled.
    #[cfg(feature = "runtime")]
    #[cfg(feature = "http2")]
    #[cfg_attr(docsrs, doc(cfg(feature = "http2")))]
    pub fn http2_keep_alive_timeout(mut self, timeout: Duration) -> Self {
        self.protocol.http2_keep_alive_timeout(timeout);
        self
    }

    /// Sets the `Executor` to deal with connection tasks.
    ///
    /// Default is `tokio::spawn`.
    pub fn executor<E2>(self, executor: E2) -> Builder<I, E2> {
        Builder {
            incoming: self.incoming,
            protocol: self.protocol.with_executor(executor),
        }
    }

    /// Consume this `Builder`, creating a [`Server`](Server).
    ///
    /// # Example
    ///
    /// ```
    /// # #[cfg(feature = "tcp")]
    /// # async fn run() {
    /// use hyper::{Body, Error, Response, Server};
    /// use hyper::service::{make_service_fn, service_fn};
    ///
    /// // Construct our SocketAddr to listen on...
    /// let addr = ([127, 0, 0, 1], 3000).into();
    ///
    /// // And a MakeService to handle each connection...
    /// let make_svc = make_service_fn(|_| async {
    ///     Ok::<_, Error>(service_fn(|_req| async {
    ///         Ok::<_, Error>(Response::new(Body::from("Hello World")))
    ///     }))
    /// });
    ///
    /// // Then bind and serve...
    /// let server = Server::bind(&addr)
    ///     .serve(make_svc);
    ///
    /// // Run forever-ish...
    /// if let Err(err) = server.await {
    ///     eprintln!("server error: {}", err);
    /// }
    /// # }
    /// ```
    pub fn serve<S, B>(self, new_service: S) -> Server<I, S, E>
    where
        I: Accept,
        I::Error: Into<Box<dyn StdError + Send + Sync>>,
        I::Conn: AsyncRead + AsyncWrite + Unpin + Send + 'static,
        S: MakeServiceRef<I::Conn, Body, ResBody = B>,
        S::Error: Into<Box<dyn StdError + Send + Sync>>,
        B: HttpBody + 'static,
        B::Error: Into<Box<dyn StdError + Send + Sync>>,
        E: NewSvcExec<I::Conn, S::Future, S::Service, E, NoopWatcher>,
        E: ConnStreamExec<<S::Service as HttpService<Body>>::Future, B>,
    {
        let serve = self.protocol.serve(self.incoming, new_service);
        let spawn_all = serve.spawn_all();
        Server { spawn_all }
    }
}

#[cfg(feature = "tcp")]
impl<E> Builder<AddrIncoming, E> {
    /// Set whether TCP keepalive messages are enabled on accepted connections.
    ///
    /// If `None` is specified, keepalive is disabled, otherwise the duration
    /// specified will be the time to remain idle before sending TCP keepalive
    /// probes.
    pub fn tcp_keepalive(mut self, keepalive: Option<Duration>) -> Self {
        self.incoming.set_keepalive(keepalive);
        self
    }

    /// Set the value of `TCP_NODELAY` option for accepted connections.
    pub fn tcp_nodelay(mut self, enabled: bool) -> Self {
        self.incoming.set_nodelay(enabled);
        self
    }

    /// Set whether to sleep on accept errors.
    ///
    /// A possible scenario is that the process has hit the max open files
    /// allowed, and so trying to accept a new connection will fail with
    /// EMFILE. In some cases, it's preferable to just wait for some time, if
    /// the application will likely close some files (or connections), and try
    /// to accept the connection again. If this option is true, the error will
    /// be logged at the error level, since it is still a big deal, and then
    /// the listener will sleep for 1 second.
    ///
    /// In other cases, hitting the max open files should be treat similarly
    /// to being out-of-memory, and simply error (and shutdown). Setting this
    /// option to false will allow that.
    ///
    /// For more details see [`AddrIncoming::set_sleep_on_errors`]
    pub fn tcp_sleep_on_accept_errors(mut self, val: bool) -> Self {
        self.incoming.set_sleep_on_errors(val);
        self
    }
}
