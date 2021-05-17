//! Asynchronous TLS/SSL streams for Tokio using [Rustls](https://github.com/ctz/rustls).

macro_rules! ready {
    ( $e:expr ) => {
        match $e {
            std::task::Poll::Ready(t) => t,
            std::task::Poll::Pending => return std::task::Poll::Pending,
        }
    };
}

pub mod client;
mod common;
pub mod server;

use common::{MidHandshake, Stream, TlsState};
use rustls::{ClientConfig, ClientSession, ServerConfig, ServerSession, Session};
use std::future::Future;
use std::io;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use webpki::DNSNameRef;

pub use rustls;
pub use webpki;

/// A wrapper around a `rustls::ClientConfig`, providing an async `connect` method.
#[derive(Clone)]
pub struct TlsConnector {
    inner: Arc<ClientConfig>,
    #[cfg(feature = "early-data")]
    early_data: bool,
}

/// A wrapper around a `rustls::ServerConfig`, providing an async `accept` method.
#[derive(Clone)]
pub struct TlsAcceptor {
    inner: Arc<ServerConfig>,
}

impl From<Arc<ClientConfig>> for TlsConnector {
    fn from(inner: Arc<ClientConfig>) -> TlsConnector {
        TlsConnector {
            inner,
            #[cfg(feature = "early-data")]
            early_data: false,
        }
    }
}

impl From<Arc<ServerConfig>> for TlsAcceptor {
    fn from(inner: Arc<ServerConfig>) -> TlsAcceptor {
        TlsAcceptor { inner }
    }
}

impl TlsConnector {
    /// Enable 0-RTT.
    ///
    /// If you want to use 0-RTT,
    /// You must also set `ClientConfig.enable_early_data` to `true`.
    #[cfg(feature = "early-data")]
    pub fn early_data(mut self, flag: bool) -> TlsConnector {
        self.early_data = flag;
        self
    }

    #[inline]
    pub fn connect<IO>(&self, domain: DNSNameRef, stream: IO) -> Connect<IO>
    where
        IO: AsyncRead + AsyncWrite + Unpin,
    {
        self.connect_with(domain, stream, |_| ())
    }

    pub fn connect_with<IO, F>(&self, domain: DNSNameRef, stream: IO, f: F) -> Connect<IO>
    where
        IO: AsyncRead + AsyncWrite + Unpin,
        F: FnOnce(&mut ClientSession),
    {
        let mut session = ClientSession::new(&self.inner, domain);
        f(&mut session);

        Connect(MidHandshake::Handshaking(client::TlsStream {
            io: stream,

            #[cfg(not(feature = "early-data"))]
            state: TlsState::Stream,

            #[cfg(feature = "early-data")]
            state: if self.early_data && session.early_data().is_some() {
                TlsState::EarlyData(0, Vec::new())
            } else {
                TlsState::Stream
            },

            session,
        }))
    }
}

impl TlsAcceptor {
    #[inline]
    pub fn accept<IO>(&self, stream: IO) -> Accept<IO>
    where
        IO: AsyncRead + AsyncWrite + Unpin,
    {
        self.accept_with(stream, |_| ())
    }

    pub fn accept_with<IO, F>(&self, stream: IO, f: F) -> Accept<IO>
    where
        IO: AsyncRead + AsyncWrite + Unpin,
        F: FnOnce(&mut ServerSession),
    {
        let mut session = ServerSession::new(&self.inner);
        f(&mut session);

        Accept(MidHandshake::Handshaking(server::TlsStream {
            session,
            io: stream,
            state: TlsState::Stream,
        }))
    }
}

/// Future returned from `TlsConnector::connect` which will resolve
/// once the connection handshake has finished.
pub struct Connect<IO>(MidHandshake<client::TlsStream<IO>>);

/// Future returned from `TlsAcceptor::accept` which will resolve
/// once the accept handshake has finished.
pub struct Accept<IO>(MidHandshake<server::TlsStream<IO>>);

/// Like [Connect], but returns `IO` on failure.
pub struct FailableConnect<IO>(MidHandshake<client::TlsStream<IO>>);

/// Like [Accept], but returns `IO` on failure.
pub struct FailableAccept<IO>(MidHandshake<server::TlsStream<IO>>);

impl<IO> Connect<IO> {
    #[inline]
    pub fn into_failable(self) -> FailableConnect<IO> {
        FailableConnect(self.0)
    }
}

impl<IO> Accept<IO> {
    #[inline]
    pub fn into_failable(self) -> FailableAccept<IO> {
        FailableAccept(self.0)
    }
}

impl<IO: AsyncRead + AsyncWrite + Unpin> Future for Connect<IO> {
    type Output = io::Result<client::TlsStream<IO>>;

    #[inline]
    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        Pin::new(&mut self.0).poll(cx).map_err(|(err, _)| err)
    }
}

impl<IO: AsyncRead + AsyncWrite + Unpin> Future for Accept<IO> {
    type Output = io::Result<server::TlsStream<IO>>;

    #[inline]
    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        Pin::new(&mut self.0).poll(cx).map_err(|(err, _)| err)
    }
}

impl<IO: AsyncRead + AsyncWrite + Unpin> Future for FailableConnect<IO> {
    type Output = Result<client::TlsStream<IO>, (io::Error, IO)>;

    #[inline]
    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        Pin::new(&mut self.0).poll(cx)
    }
}

impl<IO: AsyncRead + AsyncWrite + Unpin> Future for FailableAccept<IO> {
    type Output = Result<server::TlsStream<IO>, (io::Error, IO)>;

    #[inline]
    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        Pin::new(&mut self.0).poll(cx)
    }
}

/// Unified TLS stream type
///
/// This abstracts over the inner `client::TlsStream` and `server::TlsStream`, so you can use
/// a single type to keep both client- and server-initiated TLS-encrypted connections.
#[derive(Debug)]
pub enum TlsStream<T> {
    Client(client::TlsStream<T>),
    Server(server::TlsStream<T>),
}

impl<T> TlsStream<T> {
    pub fn get_ref(&self) -> (&T, &dyn Session) {
        use TlsStream::*;
        match self {
            Client(io) => {
                let (io, session) = io.get_ref();
                (io, &*session)
            }
            Server(io) => {
                let (io, session) = io.get_ref();
                (io, &*session)
            }
        }
    }

    pub fn get_mut(&mut self) -> (&mut T, &mut dyn Session) {
        use TlsStream::*;
        match self {
            Client(io) => {
                let (io, session) = io.get_mut();
                (io, &mut *session)
            }
            Server(io) => {
                let (io, session) = io.get_mut();
                (io, &mut *session)
            }
        }
    }
}

impl<T> From<client::TlsStream<T>> for TlsStream<T> {
    fn from(s: client::TlsStream<T>) -> Self {
        Self::Client(s)
    }
}

impl<T> From<server::TlsStream<T>> for TlsStream<T> {
    fn from(s: server::TlsStream<T>) -> Self {
        Self::Server(s)
    }
}

impl<T> AsyncRead for TlsStream<T>
where
    T: AsyncRead + AsyncWrite + Unpin,
{
    #[inline]
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        match self.get_mut() {
            TlsStream::Client(x) => Pin::new(x).poll_read(cx, buf),
            TlsStream::Server(x) => Pin::new(x).poll_read(cx, buf),
        }
    }
}

impl<T> AsyncWrite for TlsStream<T>
where
    T: AsyncRead + AsyncWrite + Unpin,
{
    #[inline]
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        match self.get_mut() {
            TlsStream::Client(x) => Pin::new(x).poll_write(cx, buf),
            TlsStream::Server(x) => Pin::new(x).poll_write(cx, buf),
        }
    }

    #[inline]
    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        match self.get_mut() {
            TlsStream::Client(x) => Pin::new(x).poll_flush(cx),
            TlsStream::Server(x) => Pin::new(x).poll_flush(cx),
        }
    }

    #[inline]
    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        match self.get_mut() {
            TlsStream::Client(x) => Pin::new(x).poll_shutdown(cx),
            TlsStream::Server(x) => Pin::new(x).poll_shutdown(cx),
        }
    }
}
