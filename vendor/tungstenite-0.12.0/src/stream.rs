//! Convenience wrapper for streams to switch between plain TCP and TLS at runtime.
//!
//!  There is no dependency on actual TLS implementations. Everything like
//! `native_tls` or `openssl` will work as long as there is a TLS stream supporting standard
//! `Read + Write` traits.

use std::io::{Read, Result as IoResult, Write};

use std::net::TcpStream;

#[cfg(feature = "tls")]
use native_tls::TlsStream;

/// Stream mode, either plain TCP or TLS.
#[derive(Clone, Copy, Debug)]
pub enum Mode {
    /// Plain mode (`ws://` URL).
    Plain,
    /// TLS mode (`wss://` URL).
    Tls,
}

/// Trait to switch TCP_NODELAY.
pub trait NoDelay {
    /// Set the TCP_NODELAY option to the given value.
    fn set_nodelay(&mut self, nodelay: bool) -> IoResult<()>;
}

impl NoDelay for TcpStream {
    fn set_nodelay(&mut self, nodelay: bool) -> IoResult<()> {
        TcpStream::set_nodelay(self, nodelay)
    }
}

#[cfg(feature = "tls")]
impl<S: Read + Write + NoDelay> NoDelay for TlsStream<S> {
    fn set_nodelay(&mut self, nodelay: bool) -> IoResult<()> {
        self.get_mut().set_nodelay(nodelay)
    }
}

/// Stream, either plain TCP or TLS.
#[derive(Debug)]
pub enum Stream<S, T> {
    /// Unencrypted socket stream.
    Plain(S),
    /// Encrypted socket stream.
    Tls(T),
}

impl<S: Read, T: Read> Read for Stream<S, T> {
    fn read(&mut self, buf: &mut [u8]) -> IoResult<usize> {
        match *self {
            Stream::Plain(ref mut s) => s.read(buf),
            Stream::Tls(ref mut s) => s.read(buf),
        }
    }
}

impl<S: Write, T: Write> Write for Stream<S, T> {
    fn write(&mut self, buf: &[u8]) -> IoResult<usize> {
        match *self {
            Stream::Plain(ref mut s) => s.write(buf),
            Stream::Tls(ref mut s) => s.write(buf),
        }
    }
    fn flush(&mut self) -> IoResult<()> {
        match *self {
            Stream::Plain(ref mut s) => s.flush(),
            Stream::Tls(ref mut s) => s.flush(),
        }
    }
}

impl<S: NoDelay, T: NoDelay> NoDelay for Stream<S, T> {
    fn set_nodelay(&mut self, nodelay: bool) -> IoResult<()> {
        match *self {
            Stream::Plain(ref mut s) => s.set_nodelay(nodelay),
            Stream::Tls(ref mut s) => s.set_nodelay(nodelay),
        }
    }
}
