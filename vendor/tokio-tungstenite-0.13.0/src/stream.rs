//! Convenience wrapper for streams to switch between plain TCP and TLS at runtime.
//!
//!  There is no dependency on actual TLS implementations. Everything like
//! `native_tls` or `openssl` will work as long as there is a TLS stream supporting standard
//! `Read + Write` traits.
use pin_project::pin_project;
use std::{
    pin::Pin,
    task::{Context, Poll},
};

use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};

/// Stream, either plain TCP or TLS.
#[pin_project(project = StreamProj)]
pub enum Stream<S, T> {
    /// Unencrypted socket stream.
    Plain(#[pin] S),
    /// Encrypted socket stream.
    Tls(#[pin] T),
}

impl<S: AsyncRead + Unpin, T: AsyncRead + Unpin> AsyncRead for Stream<S, T> {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        match self.project() {
            StreamProj::Plain(ref mut s) => Pin::new(s).poll_read(cx, buf),
            StreamProj::Tls(ref mut s) => Pin::new(s).poll_read(cx, buf),
        }
    }
}

impl<S: AsyncWrite + Unpin, T: AsyncWrite + Unpin> AsyncWrite for Stream<S, T> {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<Result<usize, std::io::Error>> {
        match self.project() {
            StreamProj::Plain(ref mut s) => Pin::new(s).poll_write(cx, buf),
            StreamProj::Tls(ref mut s) => Pin::new(s).poll_write(cx, buf),
        }
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), std::io::Error>> {
        match self.project() {
            StreamProj::Plain(ref mut s) => Pin::new(s).poll_flush(cx),
            StreamProj::Tls(ref mut s) => Pin::new(s).poll_flush(cx),
        }
    }

    fn poll_shutdown(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Result<(), std::io::Error>> {
        match self.project() {
            StreamProj::Plain(ref mut s) => Pin::new(s).poll_shutdown(cx),
            StreamProj::Tls(ref mut s) => Pin::new(s).poll_shutdown(cx),
        }
    }
}
