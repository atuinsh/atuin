//! Lower-level client connection API.
//!
//! The types in this module are to provide a lower-level API based around a
//! single connection. Connecting to a host, pooling connections, and the like
//! are not handled at this level. This module provides the building blocks to
//! customize those things externally.
//!
//! If don't have need to manage connections yourself, consider using the
//! higher-level [Client](super) API.
//!
//! ## Example
//! A simple example that uses the `SendRequest` struct to talk HTTP over a Tokio TCP stream
//! ```no_run
//! # #[cfg(all(feature = "client", feature = "http1", feature = "runtime"))]
//! # mod rt {
//! use http::{Request, StatusCode};
//! use hyper::{client::conn::Builder, Body};
//! use tokio::net::TcpStream;
//!
//! #[tokio::main]
//! async fn main() -> Result<(), Box<dyn std::error::Error>> {
//!     let target_stream = TcpStream::connect("example.com:80").await?;
//!
//!     let (mut request_sender, connection) = Builder::new()
//!         .handshake::<TcpStream, Body>(target_stream)
//!         .await?;
//!
//!     // spawn a task to poll the connection and drive the HTTP state
//!     tokio::spawn(async move {
//!         if let Err(e) = connection.await {
//!             eprintln!("Error in connection: {}", e);
//!         }
//!     });
//!
//!     let request = Request::builder()
//!     // We need to manually add the host header because SendRequest does not
//!         .header("Host", "example.com")
//!         .method("GET")
//!         .body(Body::from(""))?;
//!
//!     let response = request_sender.send_request(request).await?;
//!     assert!(response.status() == StatusCode::OK);
//!     Ok(())
//! }
//!
//! # }
//! ```

use std::error::Error as StdError;
use std::fmt;
#[cfg(feature = "http2")]
use std::marker::PhantomData;
use std::sync::Arc;
#[cfg(all(feature = "runtime", feature = "http2"))]
use std::time::Duration;

use bytes::Bytes;
use futures_util::future::{self, Either, FutureExt as _};
use pin_project::pin_project;
use tokio::io::{AsyncRead, AsyncWrite};
use tower_service::Service;

use super::dispatch;
use crate::body::HttpBody;
use crate::common::{
    exec::{BoxSendFuture, Exec},
    task, Future, Pin, Poll,
};
use crate::proto;
use crate::rt::Executor;
#[cfg(feature = "http1")]
use crate::upgrade::Upgraded;
use crate::{Body, Request, Response};

#[cfg(feature = "http1")]
type Http1Dispatcher<T, B, R> = proto::dispatch::Dispatcher<proto::dispatch::Client<B>, B, T, R>;

#[pin_project(project = ProtoClientProj)]
enum ProtoClient<T, B>
where
    B: HttpBody,
{
    #[cfg(feature = "http1")]
    H1(#[pin] Http1Dispatcher<T, B, proto::h1::ClientTransaction>),
    #[cfg(feature = "http2")]
    H2(#[pin] proto::h2::ClientTask<B>, PhantomData<fn(T)>),
}

/// Returns a handshake future over some IO.
///
/// This is a shortcut for `Builder::new().handshake(io)`.
pub async fn handshake<T>(
    io: T,
) -> crate::Result<(SendRequest<crate::Body>, Connection<T, crate::Body>)>
where
    T: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    Builder::new().handshake(io).await
}

/// The sender side of an established connection.
pub struct SendRequest<B> {
    dispatch: dispatch::Sender<Request<B>, Response<Body>>,
}

/// A future that processes all HTTP state for the IO object.
///
/// In most cases, this should just be spawned into an executor, so that it
/// can process incoming and outgoing messages, notice hangups, and the like.
#[must_use = "futures do nothing unless polled"]
pub struct Connection<T, B>
where
    T: AsyncRead + AsyncWrite + Send + 'static,
    B: HttpBody + 'static,
{
    inner: Option<ProtoClient<T, B>>,
}

/// A builder to configure an HTTP connection.
///
/// After setting options, the builder is used to create a handshake future.
#[derive(Clone, Debug)]
pub struct Builder {
    pub(super) exec: Exec,
    h09_responses: bool,
    h1_title_case_headers: bool,
    h1_read_buf_exact_size: Option<usize>,
    h1_max_buf_size: Option<usize>,
    #[cfg(feature = "http2")]
    h2_builder: proto::h2::client::Config,
    version: Proto,
}

#[derive(Clone, Debug)]
enum Proto {
    #[cfg(feature = "http1")]
    Http1,
    #[cfg(feature = "http2")]
    Http2,
}

/// A future returned by `SendRequest::send_request`.
///
/// Yields a `Response` if successful.
#[must_use = "futures do nothing unless polled"]
pub struct ResponseFuture {
    inner: ResponseFutureState,
}

enum ResponseFutureState {
    Waiting(dispatch::Promise<Response<Body>>),
    // Option is to be able to `take()` it in `poll`
    Error(Option<crate::Error>),
}

/// Deconstructed parts of a `Connection`.
///
/// This allows taking apart a `Connection` at a later time, in order to
/// reclaim the IO object, and additional related pieces.
#[derive(Debug)]
pub struct Parts<T> {
    /// The original IO object used in the handshake.
    pub io: T,
    /// A buffer of bytes that have been read but not processed as HTTP.
    ///
    /// For instance, if the `Connection` is used for an HTTP upgrade request,
    /// it is possible the server sent back the first bytes of the new protocol
    /// along with the response upgrade.
    ///
    /// You will want to check for any existing bytes if you plan to continue
    /// communicating on the IO object.
    pub read_buf: Bytes,
    _inner: (),
}

// ========== internal client api

// A `SendRequest` that can be cloned to send HTTP2 requests.
// private for now, probably not a great idea of a type...
#[must_use = "futures do nothing unless polled"]
#[cfg(feature = "http2")]
pub(super) struct Http2SendRequest<B> {
    dispatch: dispatch::UnboundedSender<Request<B>, Response<Body>>,
}

// ===== impl SendRequest

impl<B> SendRequest<B> {
    /// Polls to determine whether this sender can be used yet for a request.
    ///
    /// If the associated connection is closed, this returns an Error.
    pub fn poll_ready(&mut self, cx: &mut task::Context<'_>) -> Poll<crate::Result<()>> {
        self.dispatch.poll_ready(cx)
    }

    pub(super) async fn when_ready(self) -> crate::Result<Self> {
        let mut me = Some(self);
        future::poll_fn(move |cx| {
            ready!(me.as_mut().unwrap().poll_ready(cx))?;
            Poll::Ready(Ok(me.take().unwrap()))
        })
        .await
    }

    pub(super) fn is_ready(&self) -> bool {
        self.dispatch.is_ready()
    }

    pub(super) fn is_closed(&self) -> bool {
        self.dispatch.is_closed()
    }

    #[cfg(feature = "http2")]
    pub(super) fn into_http2(self) -> Http2SendRequest<B> {
        Http2SendRequest {
            dispatch: self.dispatch.unbound(),
        }
    }
}

impl<B> SendRequest<B>
where
    B: HttpBody + 'static,
{
    /// Sends a `Request` on the associated connection.
    ///
    /// Returns a future that if successful, yields the `Response`.
    ///
    /// # Note
    ///
    /// There are some key differences in what automatic things the `Client`
    /// does for you that will not be done here:
    ///
    /// - `Client` requires absolute-form `Uri`s, since the scheme and
    ///   authority are needed to connect. They aren't required here.
    /// - Since the `Client` requires absolute-form `Uri`s, it can add
    ///   the `Host` header based on it. You must add a `Host` header yourself
    ///   before calling this method.
    /// - Since absolute-form `Uri`s are not required, if received, they will
    ///   be serialized as-is.
    ///
    /// # Example
    ///
    /// ```
    /// # use http::header::HOST;
    /// # use hyper::client::conn::SendRequest;
    /// # use hyper::Body;
    /// use hyper::Request;
    ///
    /// # async fn doc(mut tx: SendRequest<Body>) -> hyper::Result<()> {
    /// // build a Request
    /// let req = Request::builder()
    ///     .uri("/foo/bar")
    ///     .header(HOST, "hyper.rs")
    ///     .body(Body::empty())
    ///     .unwrap();
    ///
    /// // send it and await a Response
    /// let res = tx.send_request(req).await?;
    /// // assert the Response
    /// assert!(res.status().is_success());
    /// # Ok(())
    /// # }
    /// # fn main() {}
    /// ```
    pub fn send_request(&mut self, req: Request<B>) -> ResponseFuture {
        let inner = match self.dispatch.send(req) {
            Ok(rx) => ResponseFutureState::Waiting(rx),
            Err(_req) => {
                debug!("connection was not ready");
                let err = crate::Error::new_canceled().with("connection was not ready");
                ResponseFutureState::Error(Some(err))
            }
        };

        ResponseFuture { inner }
    }

    pub(super) fn send_request_retryable(
        &mut self,
        req: Request<B>,
    ) -> impl Future<Output = Result<Response<Body>, (crate::Error, Option<Request<B>>)>> + Unpin
    where
        B: Send,
    {
        match self.dispatch.try_send(req) {
            Ok(rx) => {
                Either::Left(rx.then(move |res| {
                    match res {
                        Ok(Ok(res)) => future::ok(res),
                        Ok(Err(err)) => future::err(err),
                        // this is definite bug if it happens, but it shouldn't happen!
                        Err(_) => panic!("dispatch dropped without returning error"),
                    }
                }))
            }
            Err(req) => {
                debug!("connection was not ready");
                let err = crate::Error::new_canceled().with("connection was not ready");
                Either::Right(future::err((err, Some(req))))
            }
        }
    }
}

impl<B> Service<Request<B>> for SendRequest<B>
where
    B: HttpBody + 'static,
{
    type Response = Response<Body>;
    type Error = crate::Error;
    type Future = ResponseFuture;

    fn poll_ready(&mut self, cx: &mut task::Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.poll_ready(cx)
    }

    fn call(&mut self, req: Request<B>) -> Self::Future {
        self.send_request(req)
    }
}

impl<B> fmt::Debug for SendRequest<B> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SendRequest").finish()
    }
}

// ===== impl Http2SendRequest

#[cfg(feature = "http2")]
impl<B> Http2SendRequest<B> {
    pub(super) fn is_ready(&self) -> bool {
        self.dispatch.is_ready()
    }

    pub(super) fn is_closed(&self) -> bool {
        self.dispatch.is_closed()
    }
}

#[cfg(feature = "http2")]
impl<B> Http2SendRequest<B>
where
    B: HttpBody + 'static,
{
    pub(super) fn send_request_retryable(
        &mut self,
        req: Request<B>,
    ) -> impl Future<Output = Result<Response<Body>, (crate::Error, Option<Request<B>>)>>
    where
        B: Send,
    {
        match self.dispatch.try_send(req) {
            Ok(rx) => {
                Either::Left(rx.then(move |res| {
                    match res {
                        Ok(Ok(res)) => future::ok(res),
                        Ok(Err(err)) => future::err(err),
                        // this is definite bug if it happens, but it shouldn't happen!
                        Err(_) => panic!("dispatch dropped without returning error"),
                    }
                }))
            }
            Err(req) => {
                debug!("connection was not ready");
                let err = crate::Error::new_canceled().with("connection was not ready");
                Either::Right(future::err((err, Some(req))))
            }
        }
    }
}

#[cfg(feature = "http2")]
impl<B> fmt::Debug for Http2SendRequest<B> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Http2SendRequest").finish()
    }
}

#[cfg(feature = "http2")]
impl<B> Clone for Http2SendRequest<B> {
    fn clone(&self) -> Self {
        Http2SendRequest {
            dispatch: self.dispatch.clone(),
        }
    }
}

// ===== impl Connection

impl<T, B> Connection<T, B>
where
    T: AsyncRead + AsyncWrite + Unpin + Send + 'static,
    B: HttpBody + Unpin + Send + 'static,
    B::Data: Send,
    B::Error: Into<Box<dyn StdError + Send + Sync>>,
{
    /// Return the inner IO object, and additional information.
    ///
    /// Only works for HTTP/1 connections. HTTP/2 connections will panic.
    pub fn into_parts(self) -> Parts<T> {
        match self.inner.expect("already upgraded") {
            #[cfg(feature = "http1")]
            ProtoClient::H1(h1) => {
                let (io, read_buf, _) = h1.into_inner();
                Parts {
                    io,
                    read_buf,
                    _inner: (),
                }
            }
            #[cfg(feature = "http2")]
            ProtoClient::H2(..) => {
                panic!("http2 cannot into_inner");
            }
        }
    }

    /// Poll the connection for completion, but without calling `shutdown`
    /// on the underlying IO.
    ///
    /// This is useful to allow running a connection while doing an HTTP
    /// upgrade. Once the upgrade is completed, the connection would be "done",
    /// but it is not desired to actually shutdown the IO object. Instead you
    /// would take it back using `into_parts`.
    ///
    /// Use [`poll_fn`](https://docs.rs/futures/0.1.25/futures/future/fn.poll_fn.html)
    /// and [`try_ready!`](https://docs.rs/futures/0.1.25/futures/macro.try_ready.html)
    /// to work with this function; or use the `without_shutdown` wrapper.
    pub fn poll_without_shutdown(&mut self, cx: &mut task::Context<'_>) -> Poll<crate::Result<()>> {
        match *self.inner.as_mut().expect("already upgraded") {
            #[cfg(feature = "http1")]
            ProtoClient::H1(ref mut h1) => h1.poll_without_shutdown(cx),
            #[cfg(feature = "http2")]
            ProtoClient::H2(ref mut h2, _) => Pin::new(h2).poll(cx).map_ok(|_| ()),
        }
    }

    /// Prevent shutdown of the underlying IO object at the end of service the request,
    /// instead run `into_parts`. This is a convenience wrapper over `poll_without_shutdown`.
    pub fn without_shutdown(self) -> impl Future<Output = crate::Result<Parts<T>>> {
        let mut conn = Some(self);
        future::poll_fn(move |cx| -> Poll<crate::Result<Parts<T>>> {
            ready!(conn.as_mut().unwrap().poll_without_shutdown(cx))?;
            Poll::Ready(Ok(conn.take().unwrap().into_parts()))
        })
    }
}

impl<T, B> Future for Connection<T, B>
where
    T: AsyncRead + AsyncWrite + Unpin + Send + 'static,
    B: HttpBody + Send + 'static,
    B::Data: Send,
    B::Error: Into<Box<dyn StdError + Send + Sync>>,
{
    type Output = crate::Result<()>;

    fn poll(mut self: Pin<&mut Self>, cx: &mut task::Context<'_>) -> Poll<Self::Output> {
        match ready!(Pin::new(self.inner.as_mut().unwrap()).poll(cx))? {
            proto::Dispatched::Shutdown => Poll::Ready(Ok(())),
            #[cfg(feature = "http1")]
            proto::Dispatched::Upgrade(pending) => match self.inner.take() {
                Some(ProtoClient::H1(h1)) => {
                    let (io, buf, _) = h1.into_inner();
                    pending.fulfill(Upgraded::new(io, buf));
                    Poll::Ready(Ok(()))
                }
                _ => {
                    drop(pending);
                    unreachable!("Upgrade expects h1");
                }
            },
        }
    }
}

impl<T, B> fmt::Debug for Connection<T, B>
where
    T: AsyncRead + AsyncWrite + fmt::Debug + Send + 'static,
    B: HttpBody + 'static,
{
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Connection").finish()
    }
}

// ===== impl Builder

impl Builder {
    /// Creates a new connection builder.
    #[inline]
    pub fn new() -> Builder {
        Builder {
            exec: Exec::Default,
            h09_responses: false,
            h1_read_buf_exact_size: None,
            h1_title_case_headers: false,
            h1_max_buf_size: None,
            #[cfg(feature = "http2")]
            h2_builder: Default::default(),
            #[cfg(feature = "http1")]
            version: Proto::Http1,
            #[cfg(not(feature = "http1"))]
            version: Proto::Http2,
        }
    }

    /// Provide an executor to execute background HTTP2 tasks.
    pub fn executor<E>(&mut self, exec: E) -> &mut Builder
    where
        E: Executor<BoxSendFuture> + Send + Sync + 'static,
    {
        self.exec = Exec::Executor(Arc::new(exec));
        self
    }

    pub(super) fn h09_responses(&mut self, enabled: bool) -> &mut Builder {
        self.h09_responses = enabled;
        self
    }

    pub(super) fn h1_title_case_headers(&mut self, enabled: bool) -> &mut Builder {
        self.h1_title_case_headers = enabled;
        self
    }

    pub(super) fn h1_read_buf_exact_size(&mut self, sz: Option<usize>) -> &mut Builder {
        self.h1_read_buf_exact_size = sz;
        self.h1_max_buf_size = None;
        self
    }

    #[cfg(feature = "http1")]
    pub(super) fn h1_max_buf_size(&mut self, max: usize) -> &mut Self {
        assert!(
            max >= proto::h1::MINIMUM_MAX_BUFFER_SIZE,
            "the max_buf_size cannot be smaller than the minimum that h1 specifies."
        );

        self.h1_max_buf_size = Some(max);
        self.h1_read_buf_exact_size = None;
        self
    }

    /// Sets whether HTTP2 is required.
    ///
    /// Default is false.
    #[cfg(feature = "http2")]
    #[cfg_attr(docsrs, doc(cfg(feature = "http2")))]
    pub fn http2_only(&mut self, enabled: bool) -> &mut Builder {
        if enabled {
            self.version = Proto::Http2
        }
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
    pub fn http2_initial_stream_window_size(&mut self, sz: impl Into<Option<u32>>) -> &mut Self {
        if let Some(sz) = sz.into() {
            self.h2_builder.adaptive_window = false;
            self.h2_builder.initial_stream_window_size = sz;
        }
        self
    }

    /// Sets the max connection-level flow control for HTTP2
    ///
    /// Passing `None` will do nothing.
    ///
    /// If not set, hyper will use a default.
    #[cfg(feature = "http2")]
    #[cfg_attr(docsrs, doc(cfg(feature = "http2")))]
    pub fn http2_initial_connection_window_size(
        &mut self,
        sz: impl Into<Option<u32>>,
    ) -> &mut Self {
        if let Some(sz) = sz.into() {
            self.h2_builder.adaptive_window = false;
            self.h2_builder.initial_conn_window_size = sz;
        }
        self
    }

    /// Sets whether to use an adaptive flow control.
    ///
    /// Enabling this will override the limits set in
    /// `http2_initial_stream_window_size` and
    /// `http2_initial_connection_window_size`.
    #[cfg(feature = "http2")]
    #[cfg_attr(docsrs, doc(cfg(feature = "http2")))]
    pub fn http2_adaptive_window(&mut self, enabled: bool) -> &mut Self {
        use proto::h2::SPEC_WINDOW_SIZE;

        self.h2_builder.adaptive_window = enabled;
        if enabled {
            self.h2_builder.initial_conn_window_size = SPEC_WINDOW_SIZE;
            self.h2_builder.initial_stream_window_size = SPEC_WINDOW_SIZE;
        }
        self
    }

    /// Sets the maximum frame size to use for HTTP2.
    ///
    /// Passing `None` will do nothing.
    ///
    /// If not set, hyper will use a default.
    #[cfg(feature = "http2")]
    #[cfg_attr(docsrs, doc(cfg(feature = "http2")))]
    pub fn http2_max_frame_size(&mut self, sz: impl Into<Option<u32>>) -> &mut Self {
        if let Some(sz) = sz.into() {
            self.h2_builder.max_frame_size = sz;
        }
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
    pub fn http2_keep_alive_interval(
        &mut self,
        interval: impl Into<Option<Duration>>,
    ) -> &mut Self {
        self.h2_builder.keep_alive_interval = interval.into();
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
    pub fn http2_keep_alive_timeout(&mut self, timeout: Duration) -> &mut Self {
        self.h2_builder.keep_alive_timeout = timeout;
        self
    }

    /// Sets whether HTTP2 keep-alive should apply while the connection is idle.
    ///
    /// If disabled, keep-alive pings are only sent while there are open
    /// request/responses streams. If enabled, pings are also sent when no
    /// streams are active. Does nothing if `http2_keep_alive_interval` is
    /// disabled.
    ///
    /// Default is `false`.
    ///
    /// # Cargo Feature
    ///
    /// Requires the `runtime` cargo feature to be enabled.
    #[cfg(feature = "runtime")]
    #[cfg(feature = "http2")]
    #[cfg_attr(docsrs, doc(cfg(feature = "http2")))]
    pub fn http2_keep_alive_while_idle(&mut self, enabled: bool) -> &mut Self {
        self.h2_builder.keep_alive_while_idle = enabled;
        self
    }

    /// Constructs a connection with the configured options and IO.
    pub fn handshake<T, B>(
        &self,
        io: T,
    ) -> impl Future<Output = crate::Result<(SendRequest<B>, Connection<T, B>)>>
    where
        T: AsyncRead + AsyncWrite + Unpin + Send + 'static,
        B: HttpBody + 'static,
        B::Data: Send,
        B::Error: Into<Box<dyn StdError + Send + Sync>>,
    {
        let opts = self.clone();

        async move {
            trace!("client handshake {:?}", opts.version);

            let (tx, rx) = dispatch::channel();
            let proto = match opts.version {
                #[cfg(feature = "http1")]
                Proto::Http1 => {
                    let mut conn = proto::Conn::new(io);
                    if opts.h1_title_case_headers {
                        conn.set_title_case_headers();
                    }
                    if opts.h09_responses {
                        conn.set_h09_responses();
                    }
                    if let Some(sz) = opts.h1_read_buf_exact_size {
                        conn.set_read_buf_exact_size(sz);
                    }
                    if let Some(max) = opts.h1_max_buf_size {
                        conn.set_max_buf_size(max);
                    }
                    let cd = proto::h1::dispatch::Client::new(rx);
                    let dispatch = proto::h1::Dispatcher::new(cd, conn);
                    ProtoClient::H1(dispatch)
                }
                #[cfg(feature = "http2")]
                Proto::Http2 => {
                    let h2 =
                        proto::h2::client::handshake(io, rx, &opts.h2_builder, opts.exec.clone())
                            .await?;
                    ProtoClient::H2(h2, PhantomData)
                }
            };

            Ok((
                SendRequest { dispatch: tx },
                Connection { inner: Some(proto) },
            ))
        }
    }
}

// ===== impl ResponseFuture

impl Future for ResponseFuture {
    type Output = crate::Result<Response<Body>>;

    fn poll(mut self: Pin<&mut Self>, cx: &mut task::Context<'_>) -> Poll<Self::Output> {
        match self.inner {
            ResponseFutureState::Waiting(ref mut rx) => {
                Pin::new(rx).poll(cx).map(|res| match res {
                    Ok(Ok(resp)) => Ok(resp),
                    Ok(Err(err)) => Err(err),
                    // this is definite bug if it happens, but it shouldn't happen!
                    Err(_canceled) => panic!("dispatch dropped without returning error"),
                })
            }
            ResponseFutureState::Error(ref mut err) => {
                Poll::Ready(Err(err.take().expect("polled after ready")))
            }
        }
    }
}

impl fmt::Debug for ResponseFuture {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ResponseFuture").finish()
    }
}

// ===== impl ProtoClient

impl<T, B> Future for ProtoClient<T, B>
where
    T: AsyncRead + AsyncWrite + Send + Unpin + 'static,
    B: HttpBody + Send + 'static,
    B::Data: Send,
    B::Error: Into<Box<dyn StdError + Send + Sync>>,
{
    type Output = crate::Result<proto::Dispatched>;

    fn poll(self: Pin<&mut Self>, cx: &mut task::Context<'_>) -> Poll<Self::Output> {
        match self.project() {
            #[cfg(feature = "http1")]
            ProtoClientProj::H1(c) => c.poll(cx),
            #[cfg(feature = "http2")]
            ProtoClientProj::H2(c, _) => c.poll(cx),
        }
    }
}

// assert trait markers

trait AssertSend: Send {}
trait AssertSendSync: Send + Sync {}

#[doc(hidden)]
impl<B: Send> AssertSendSync for SendRequest<B> {}

#[doc(hidden)]
impl<T: Send, B: Send> AssertSend for Connection<T, B>
where
    T: AsyncRead + AsyncWrite + Send + 'static,
    B: HttpBody + 'static,
    B::Data: Send,
{
}

#[doc(hidden)]
impl<T: Send + Sync, B: Send + Sync> AssertSendSync for Connection<T, B>
where
    T: AsyncRead + AsyncWrite + Send + 'static,
    B: HttpBody + 'static,
    B::Data: Send + Sync + 'static,
{
}

#[doc(hidden)]
impl AssertSendSync for Builder {}

#[doc(hidden)]
impl AssertSend for ResponseFuture {}
