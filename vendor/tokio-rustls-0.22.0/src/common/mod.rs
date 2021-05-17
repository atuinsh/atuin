mod handshake;

pub(crate) use handshake::{IoSession, MidHandshake};
use rustls::Session;
use std::io::{self, IoSlice, Read, Write};
use std::pin::Pin;
use std::task::{Context, Poll};
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};

#[derive(Debug)]
pub enum TlsState {
    #[cfg(feature = "early-data")]
    EarlyData(usize, Vec<u8>),
    Stream,
    ReadShutdown,
    WriteShutdown,
    FullyShutdown,
}

impl TlsState {
    #[inline]
    pub fn shutdown_read(&mut self) {
        match *self {
            TlsState::WriteShutdown | TlsState::FullyShutdown => *self = TlsState::FullyShutdown,
            _ => *self = TlsState::ReadShutdown,
        }
    }

    #[inline]
    pub fn shutdown_write(&mut self) {
        match *self {
            TlsState::ReadShutdown | TlsState::FullyShutdown => *self = TlsState::FullyShutdown,
            _ => *self = TlsState::WriteShutdown,
        }
    }

    #[inline]
    pub fn writeable(&self) -> bool {
        !matches!(*self, TlsState::WriteShutdown | TlsState::FullyShutdown)
    }

    #[inline]
    pub fn readable(&self) -> bool {
        !matches!(*self, TlsState::ReadShutdown | TlsState::FullyShutdown)
    }

    #[inline]
    #[cfg(feature = "early-data")]
    pub fn is_early_data(&self) -> bool {
        matches!(self, TlsState::EarlyData(..))
    }

    #[inline]
    #[cfg(not(feature = "early-data"))]
    pub const fn is_early_data(&self) -> bool {
        false
    }
}

pub struct Stream<'a, IO, S> {
    pub io: &'a mut IO,
    pub session: &'a mut S,
    pub eof: bool,
}

impl<'a, IO: AsyncRead + AsyncWrite + Unpin, S: Session> Stream<'a, IO, S> {
    pub fn new(io: &'a mut IO, session: &'a mut S) -> Self {
        Stream {
            io,
            session,
            // The state so far is only used to detect EOF, so either Stream
            // or EarlyData state should both be all right.
            eof: false,
        }
    }

    pub fn set_eof(mut self, eof: bool) -> Self {
        self.eof = eof;
        self
    }

    pub fn as_mut_pin(&mut self) -> Pin<&mut Self> {
        Pin::new(self)
    }

    pub fn read_io(&mut self, cx: &mut Context) -> Poll<io::Result<usize>> {
        struct Reader<'a, 'b, T> {
            io: &'a mut T,
            cx: &'a mut Context<'b>,
        }

        impl<'a, 'b, T: AsyncRead + Unpin> Read for Reader<'a, 'b, T> {
            #[inline]
            fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
                let mut buf = ReadBuf::new(buf);
                match Pin::new(&mut self.io).poll_read(self.cx, &mut buf) {
                    Poll::Ready(Ok(())) => Ok(buf.filled().len()),
                    Poll::Ready(Err(err)) => Err(err),
                    Poll::Pending => Err(io::ErrorKind::WouldBlock.into()),
                }
            }
        }

        let mut reader = Reader { io: self.io, cx };

        let n = match self.session.read_tls(&mut reader) {
            Ok(n) => n,
            Err(ref err) if err.kind() == io::ErrorKind::WouldBlock => return Poll::Pending,
            Err(err) => return Poll::Ready(Err(err)),
        };

        self.session.process_new_packets().map_err(|err| {
            // In case we have an alert to send describing this error,
            // try a last-gasp write -- but don't predate the primary
            // error.
            let _ = self.write_io(cx);

            io::Error::new(io::ErrorKind::InvalidData, err)
        })?;

        Poll::Ready(Ok(n))
    }

    pub fn write_io(&mut self, cx: &mut Context) -> Poll<io::Result<usize>> {
        struct Writer<'a, 'b, T> {
            io: &'a mut T,
            cx: &'a mut Context<'b>,
        }

        impl<'a, 'b, T: Unpin> Writer<'a, 'b, T> {
            #[inline]
            fn poll_with<U>(
                &mut self,
                f: impl FnOnce(Pin<&mut T>, &mut Context<'_>) -> Poll<io::Result<U>>,
            ) -> io::Result<U> {
                match f(Pin::new(&mut self.io), self.cx) {
                    Poll::Ready(result) => result,
                    Poll::Pending => Err(io::ErrorKind::WouldBlock.into()),
                }
            }
        }

        impl<'a, 'b, T: AsyncWrite + Unpin> Write for Writer<'a, 'b, T> {
            #[inline]
            fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
                self.poll_with(|io, cx| io.poll_write(cx, buf))
            }

            #[inline]
            fn write_vectored(&mut self, bufs: &[IoSlice<'_>]) -> io::Result<usize> {
                self.poll_with(|io, cx| io.poll_write_vectored(cx, bufs))
            }

            fn flush(&mut self) -> io::Result<()> {
                self.poll_with(|io, cx| io.poll_flush(cx))
            }
        }

        let mut writer = Writer { io: self.io, cx };

        match self.session.write_tls(&mut writer) {
            Err(ref err) if err.kind() == io::ErrorKind::WouldBlock => Poll::Pending,
            result => Poll::Ready(result),
        }
    }

    pub fn handshake(&mut self, cx: &mut Context) -> Poll<io::Result<(usize, usize)>> {
        let mut wrlen = 0;
        let mut rdlen = 0;

        loop {
            let mut write_would_block = false;
            let mut read_would_block = false;

            while self.session.wants_write() {
                match self.write_io(cx) {
                    Poll::Ready(Ok(n)) => wrlen += n,
                    Poll::Pending => {
                        write_would_block = true;
                        break;
                    }
                    Poll::Ready(Err(err)) => return Poll::Ready(Err(err)),
                }
            }

            while !self.eof && self.session.wants_read() {
                match self.read_io(cx) {
                    Poll::Ready(Ok(0)) => self.eof = true,
                    Poll::Ready(Ok(n)) => rdlen += n,
                    Poll::Pending => {
                        read_would_block = true;
                        break;
                    }
                    Poll::Ready(Err(err)) => return Poll::Ready(Err(err)),
                }
            }

            return match (self.eof, self.session.is_handshaking()) {
                (true, true) => {
                    let err = io::Error::new(io::ErrorKind::UnexpectedEof, "tls handshake eof");
                    Poll::Ready(Err(err))
                }
                (_, false) => Poll::Ready(Ok((rdlen, wrlen))),
                (_, true) if write_would_block || read_would_block => {
                    if rdlen != 0 || wrlen != 0 {
                        Poll::Ready(Ok((rdlen, wrlen)))
                    } else {
                        Poll::Pending
                    }
                }
                (..) => continue,
            };
        }
    }
}

impl<'a, IO: AsyncRead + AsyncWrite + Unpin, S: Session> AsyncRead for Stream<'a, IO, S> {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let prev = buf.remaining();

        while buf.remaining() != 0 {
            let mut would_block = false;

            // read a packet
            while self.session.wants_read() {
                match self.read_io(cx) {
                    Poll::Ready(Ok(0)) => {
                        self.eof = true;
                        break;
                    }
                    Poll::Ready(Ok(_)) => (),
                    Poll::Pending => {
                        would_block = true;
                        break;
                    }
                    Poll::Ready(Err(err)) => return Poll::Ready(Err(err)),
                }
            }

            return match self.session.read(buf.initialize_unfilled()) {
                Ok(0) if prev == buf.remaining() && would_block => Poll::Pending,
                Ok(n) => {
                    buf.advance(n);

                    if self.eof || would_block {
                        break;
                    } else {
                        continue;
                    }
                }
                Err(ref err)
                    if err.kind() == io::ErrorKind::ConnectionAborted
                        && prev != buf.remaining() =>
                {
                    break
                }
                Err(err) => Poll::Ready(Err(err)),
            };
        }

        Poll::Ready(Ok(()))
    }
}

impl<'a, IO: AsyncRead + AsyncWrite + Unpin, S: Session> AsyncWrite for Stream<'a, IO, S> {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        let mut pos = 0;

        while pos != buf.len() {
            let mut would_block = false;

            match self.session.write(&buf[pos..]) {
                Ok(n) => pos += n,
                Err(err) => return Poll::Ready(Err(err)),
            };

            while self.session.wants_write() {
                match self.write_io(cx) {
                    Poll::Ready(Ok(0)) | Poll::Pending => {
                        would_block = true;
                        break;
                    }
                    Poll::Ready(Ok(_)) => (),
                    Poll::Ready(Err(err)) => return Poll::Ready(Err(err)),
                }
            }

            return match (pos, would_block) {
                (0, true) => Poll::Pending,
                (n, true) => Poll::Ready(Ok(n)),
                (_, false) => continue,
            };
        }

        Poll::Ready(Ok(pos))
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context) -> Poll<io::Result<()>> {
        self.session.flush()?;
        while self.session.wants_write() {
            ready!(self.write_io(cx))?;
        }
        Pin::new(&mut self.io).poll_flush(cx)
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        while self.session.wants_write() {
            ready!(self.write_io(cx))?;
        }
        Pin::new(&mut self.io).poll_shutdown(cx)
    }
}

#[cfg(test)]
mod test_stream;
