use super::*;
use crate::common::IoSession;
use rustls::Session;

/// A wrapper around an underlying raw stream which implements the TLS or SSL
/// protocol.
#[derive(Debug)]
pub struct TlsStream<IO> {
    pub(crate) io: IO,
    pub(crate) session: ClientSession,
    pub(crate) state: TlsState,
}

impl<IO> TlsStream<IO> {
    #[inline]
    pub fn get_ref(&self) -> (&IO, &ClientSession) {
        (&self.io, &self.session)
    }

    #[inline]
    pub fn get_mut(&mut self) -> (&mut IO, &mut ClientSession) {
        (&mut self.io, &mut self.session)
    }

    #[inline]
    pub fn into_inner(self) -> (IO, ClientSession) {
        (self.io, self.session)
    }
}

impl<IO> IoSession for TlsStream<IO> {
    type Io = IO;
    type Session = ClientSession;

    #[inline]
    fn skip_handshake(&self) -> bool {
        self.state.is_early_data()
    }

    #[inline]
    fn get_mut(&mut self) -> (&mut TlsState, &mut Self::Io, &mut Self::Session) {
        (&mut self.state, &mut self.io, &mut self.session)
    }

    #[inline]
    fn into_io(self) -> Self::Io {
        self.io
    }
}

impl<IO> AsyncRead for TlsStream<IO>
where
    IO: AsyncRead + AsyncWrite + Unpin,
{
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        match self.state {
            #[cfg(feature = "early-data")]
            TlsState::EarlyData(..) => Poll::Pending,
            TlsState::Stream | TlsState::WriteShutdown => {
                let this = self.get_mut();
                let mut stream =
                    Stream::new(&mut this.io, &mut this.session).set_eof(!this.state.readable());
                let prev = buf.remaining();

                match stream.as_mut_pin().poll_read(cx, buf) {
                    Poll::Ready(Ok(())) => {
                        if prev == buf.remaining() {
                            this.state.shutdown_read();
                        }

                        Poll::Ready(Ok(()))
                    }
                    Poll::Ready(Err(ref e)) if e.kind() == io::ErrorKind::ConnectionAborted => {
                        this.state.shutdown_read();
                        Poll::Ready(Ok(()))
                    }
                    output => output,
                }
            }
            TlsState::ReadShutdown | TlsState::FullyShutdown => Poll::Ready(Ok(())),
        }
    }
}

impl<IO> AsyncWrite for TlsStream<IO>
where
    IO: AsyncRead + AsyncWrite + Unpin,
{
    /// Note: that it does not guarantee the final data to be sent.
    /// To be cautious, you must manually call `flush`.
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        let this = self.get_mut();
        let mut stream =
            Stream::new(&mut this.io, &mut this.session).set_eof(!this.state.readable());

        #[allow(clippy::match_single_binding)]
        match this.state {
            #[cfg(feature = "early-data")]
            TlsState::EarlyData(ref mut pos, ref mut data) => {
                use std::io::Write;

                // write early data
                if let Some(mut early_data) = stream.session.early_data() {
                    let len = match early_data.write(buf) {
                        Ok(n) => n,
                        Err(ref err) if err.kind() == io::ErrorKind::WouldBlock => {
                            return Poll::Pending
                        }
                        Err(err) => return Poll::Ready(Err(err)),
                    };
                    if len != 0 {
                        data.extend_from_slice(&buf[..len]);
                        return Poll::Ready(Ok(len));
                    }
                }

                // complete handshake
                while stream.session.is_handshaking() {
                    ready!(stream.handshake(cx))?;
                }

                // write early data (fallback)
                if !stream.session.is_early_data_accepted() {
                    while *pos < data.len() {
                        let len = ready!(stream.as_mut_pin().poll_write(cx, &data[*pos..]))?;
                        *pos += len;
                    }
                }

                // end
                this.state = TlsState::Stream;
                stream.as_mut_pin().poll_write(cx, buf)
            }
            _ => stream.as_mut_pin().poll_write(cx, buf),
        }
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        let this = self.get_mut();
        let mut stream =
            Stream::new(&mut this.io, &mut this.session).set_eof(!this.state.readable());

        #[cfg(feature = "early-data")]
        {
            if let TlsState::EarlyData(ref mut pos, ref mut data) = this.state {
                // complete handshake
                while stream.session.is_handshaking() {
                    ready!(stream.handshake(cx))?;
                }

                // write early data (fallback)
                if !stream.session.is_early_data_accepted() {
                    while *pos < data.len() {
                        let len = ready!(stream.as_mut_pin().poll_write(cx, &data[*pos..]))?;
                        *pos += len;
                    }
                }

                this.state = TlsState::Stream;
            }
        }

        stream.as_mut_pin().poll_flush(cx)
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        if self.state.writeable() {
            self.session.send_close_notify();
            self.state.shutdown_write();
        }

        #[cfg(feature = "early-data")]
        {
            // we skip the handshake
            if let TlsState::EarlyData(..) = self.state {
                return Pin::new(&mut self.io).poll_shutdown(cx);
            }
        }

        let this = self.get_mut();
        let mut stream =
            Stream::new(&mut this.io, &mut this.session).set_eof(!this.state.readable());
        stream.as_mut_pin().poll_shutdown(cx)
    }
}
