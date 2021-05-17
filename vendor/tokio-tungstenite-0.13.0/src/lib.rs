//! Async WebSocket usage.
//!
//! This library is an implementation of WebSocket handshakes and streams. It
//! is based on the crate which implements all required WebSocket protocol
//! logic. So this crate basically just brings tokio support / tokio integration
//! to it.
//!
//! Each WebSocket stream implements the required `Stream` and `Sink` traits,
//! so the socket is just a stream of messages coming in and going out.

#![deny(missing_docs, unused_must_use, unused_mut, unused_imports, unused_import_braces)]

pub use tungstenite;

mod compat;
#[cfg(feature = "connect")]
mod connect;
mod handshake;
#[cfg(feature = "stream")]
pub mod stream;

use std::io::{Read, Write};

use compat::{cvt, AllowStd, ContextWaker};
use futures_util::{
    sink::{Sink, SinkExt},
    stream::Stream,
};
use log::*;
use std::{
    pin::Pin,
    task::{Context, Poll},
};
use tokio::io::{AsyncRead, AsyncWrite};

use tungstenite::{
    client::IntoClientRequest,
    error::Error as WsError,
    handshake::{
        client::{ClientHandshake, Request, Response},
        server::{Callback, NoCallback},
        HandshakeError,
    },
    protocol::{Message, Role, WebSocket, WebSocketConfig},
    server,
};

#[cfg(feature = "connect")]
pub use connect::{
    client_async_tls, client_async_tls_with_config, connect_async, connect_async_with_config,
    TlsConnector,
};

#[cfg(all(feature = "connect", feature = "tls"))]
pub use connect::MaybeTlsStream;
use tungstenite::protocol::CloseFrame;

/// Creates a WebSocket handshake from a request and a stream.
/// For convenience, the user may call this with a url string, a URL,
/// or a `Request`. Calling with `Request` allows the user to add
/// a WebSocket protocol or other custom headers.
///
/// Internally, this custom creates a handshake representation and returns
/// a future representing the resolution of the WebSocket handshake. The
/// returned future will resolve to either `WebSocketStream<S>` or `Error`
/// depending on whether the handshake is successful.
///
/// This is typically used for clients who have already established, for
/// example, a TCP connection to the remote server.
pub async fn client_async<'a, R, S>(
    request: R,
    stream: S,
) -> Result<(WebSocketStream<S>, Response), WsError>
where
    R: IntoClientRequest + Unpin,
    S: AsyncRead + AsyncWrite + Unpin,
{
    client_async_with_config(request, stream, None).await
}

/// The same as `client_async()` but the one can specify a websocket configuration.
/// Please refer to `client_async()` for more details.
pub async fn client_async_with_config<'a, R, S>(
    request: R,
    stream: S,
    config: Option<WebSocketConfig>,
) -> Result<(WebSocketStream<S>, Response), WsError>
where
    R: IntoClientRequest + Unpin,
    S: AsyncRead + AsyncWrite + Unpin,
{
    let f = handshake::client_handshake(stream, move |allow_std| {
        let request = request.into_client_request()?;
        let cli_handshake = ClientHandshake::start(allow_std, request, config)?;
        cli_handshake.handshake()
    });
    f.await.map_err(|e| match e {
        HandshakeError::Failure(e) => e,
        e => WsError::Io(std::io::Error::new(std::io::ErrorKind::Other, e.to_string())),
    })
}

/// Accepts a new WebSocket connection with the provided stream.
///
/// This function will internally call `server::accept` to create a
/// handshake representation and returns a future representing the
/// resolution of the WebSocket handshake. The returned future will resolve
/// to either `WebSocketStream<S>` or `Error` depending if it's successful
/// or not.
///
/// This is typically used after a socket has been accepted from a
/// `TcpListener`. That socket is then passed to this function to perform
/// the server half of the accepting a client's websocket connection.
pub async fn accept_async<S>(stream: S) -> Result<WebSocketStream<S>, WsError>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    accept_hdr_async(stream, NoCallback).await
}

/// The same as `accept_async()` but the one can specify a websocket configuration.
/// Please refer to `accept_async()` for more details.
pub async fn accept_async_with_config<S>(
    stream: S,
    config: Option<WebSocketConfig>,
) -> Result<WebSocketStream<S>, WsError>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    accept_hdr_async_with_config(stream, NoCallback, config).await
}

/// Accepts a new WebSocket connection with the provided stream.
///
/// This function does the same as `accept_async()` but accepts an extra callback
/// for header processing. The callback receives headers of the incoming
/// requests and is able to add extra headers to the reply.
pub async fn accept_hdr_async<S, C>(stream: S, callback: C) -> Result<WebSocketStream<S>, WsError>
where
    S: AsyncRead + AsyncWrite + Unpin,
    C: Callback + Unpin,
{
    accept_hdr_async_with_config(stream, callback, None).await
}

/// The same as `accept_hdr_async()` but the one can specify a websocket configuration.
/// Please refer to `accept_hdr_async()` for more details.
pub async fn accept_hdr_async_with_config<S, C>(
    stream: S,
    callback: C,
    config: Option<WebSocketConfig>,
) -> Result<WebSocketStream<S>, WsError>
where
    S: AsyncRead + AsyncWrite + Unpin,
    C: Callback + Unpin,
{
    let f = handshake::server_handshake(stream, move |allow_std| {
        server::accept_hdr_with_config(allow_std, callback, config)
    });
    f.await.map_err(|e| match e {
        HandshakeError::Failure(e) => e,
        e => WsError::Io(std::io::Error::new(std::io::ErrorKind::Other, e.to_string())),
    })
}

/// A wrapper around an underlying raw stream which implements the WebSocket
/// protocol.
///
/// A `WebSocketStream<S>` represents a handshake that has been completed
/// successfully and both the server and the client are ready for receiving
/// and sending data. Message from a `WebSocketStream<S>` are accessible
/// through the respective `Stream` and `Sink`. Check more information about
/// them in `futures-rs` crate documentation or have a look on the examples
/// and unit tests for this crate.
#[derive(Debug)]
pub struct WebSocketStream<S> {
    inner: WebSocket<AllowStd<S>>,
}

impl<S> WebSocketStream<S> {
    /// Convert a raw socket into a WebSocketStream without performing a
    /// handshake.
    pub async fn from_raw_socket(stream: S, role: Role, config: Option<WebSocketConfig>) -> Self
    where
        S: AsyncRead + AsyncWrite + Unpin,
    {
        handshake::without_handshake(stream, move |allow_std| {
            WebSocket::from_raw_socket(allow_std, role, config)
        })
        .await
    }

    /// Convert a raw socket into a WebSocketStream without performing a
    /// handshake.
    pub async fn from_partially_read(
        stream: S,
        part: Vec<u8>,
        role: Role,
        config: Option<WebSocketConfig>,
    ) -> Self
    where
        S: AsyncRead + AsyncWrite + Unpin,
    {
        handshake::without_handshake(stream, move |allow_std| {
            WebSocket::from_partially_read(allow_std, part, role, config)
        })
        .await
    }

    pub(crate) fn new(ws: WebSocket<AllowStd<S>>) -> Self {
        WebSocketStream { inner: ws }
    }

    fn with_context<F, R>(&mut self, ctx: Option<(ContextWaker, &mut Context<'_>)>, f: F) -> R
    where
        S: Unpin,
        F: FnOnce(&mut WebSocket<AllowStd<S>>) -> R,
        AllowStd<S>: Read + Write,
    {
        trace!("{}:{} WebSocketStream.with_context", file!(), line!());
        if let Some((kind, ctx)) = ctx {
            self.inner.get_mut().set_waker(kind, &ctx.waker());
        }
        f(&mut self.inner)
    }

    /// Returns a shared reference to the inner stream.
    pub fn get_ref(&self) -> &S
    where
        S: AsyncRead + AsyncWrite + Unpin,
    {
        &self.inner.get_ref().get_ref()
    }

    /// Returns a mutable reference to the inner stream.
    pub fn get_mut(&mut self) -> &mut S
    where
        S: AsyncRead + AsyncWrite + Unpin,
    {
        self.inner.get_mut().get_mut()
    }

    /// Returns a reference to the configuration of the tungstenite stream.
    pub fn get_config(&self) -> &WebSocketConfig {
        self.inner.get_config()
    }

    /// Close the underlying web socket
    pub async fn close(&mut self, msg: Option<CloseFrame<'_>>) -> Result<(), WsError>
    where
        S: AsyncRead + AsyncWrite + Unpin,
    {
        let msg = msg.map(|msg| msg.into_owned());
        self.send(Message::Close(msg)).await
    }
}

impl<T> Stream for WebSocketStream<T>
where
    T: AsyncRead + AsyncWrite + Unpin,
{
    type Item = Result<Message, WsError>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        trace!("{}:{} Stream.poll_next", file!(), line!());
        match futures_util::ready!(self.with_context(Some((ContextWaker::Read, cx)), |s| {
            trace!("{}:{} Stream.with_context poll_next -> read_message()", file!(), line!());
            cvt(s.read_message())
        })) {
            Ok(v) => Poll::Ready(Some(Ok(v))),
            Err(WsError::AlreadyClosed) | Err(WsError::ConnectionClosed) => Poll::Ready(None),
            Err(e) => Poll::Ready(Some(Err(e))),
        }
    }
}

impl<T> Sink<Message> for WebSocketStream<T>
where
    T: AsyncRead + AsyncWrite + Unpin,
{
    type Error = WsError;

    fn poll_ready(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        (*self).with_context(Some((ContextWaker::Write, cx)), |s| cvt(s.write_pending()))
    }

    fn start_send(mut self: Pin<&mut Self>, item: Message) -> Result<(), Self::Error> {
        match (*self).with_context(None, |s| s.write_message(item)) {
            Ok(()) => Ok(()),
            Err(::tungstenite::Error::Io(ref err))
                if err.kind() == std::io::ErrorKind::WouldBlock =>
            {
                // the message was accepted and queued
                // isn't an error.
                Ok(())
            }
            Err(e) => {
                debug!("websocket start_send error: {}", e);
                Err(e)
            }
        }
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        (*self).with_context(Some((ContextWaker::Write, cx)), |s| cvt(s.write_pending()))
    }

    fn poll_close(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        match (*self).with_context(Some((ContextWaker::Write, cx)), |s| s.close(None)) {
            Ok(()) => Poll::Ready(Ok(())),
            Err(::tungstenite::Error::ConnectionClosed) => Poll::Ready(Ok(())),
            Err(err) => {
                debug!("websocket close error: {}", err);
                Poll::Ready(Err(err))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    #[cfg(feature = "connect")]
    use crate::connect::encryption::AutoStream;
    use crate::{compat::AllowStd, WebSocketStream};
    use std::io::{Read, Write};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    fn is_read<T: Read>() {}
    fn is_write<T: Write>() {}
    fn is_async_read<T: AsyncReadExt>() {}
    fn is_async_write<T: AsyncWriteExt>() {}
    fn is_unpin<T: Unpin>() {}

    #[test]
    fn web_socket_stream_has_traits() {
        is_read::<AllowStd<tokio::net::TcpStream>>();
        is_write::<AllowStd<tokio::net::TcpStream>>();

        #[cfg(feature = "connect")]
        is_async_read::<AutoStream<tokio::net::TcpStream>>();
        #[cfg(feature = "connect")]
        is_async_write::<AutoStream<tokio::net::TcpStream>>();

        is_unpin::<WebSocketStream<tokio::net::TcpStream>>();
        #[cfg(feature = "connect")]
        is_unpin::<WebSocketStream<AutoStream<tokio::net::TcpStream>>>();
    }
}
