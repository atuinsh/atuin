use crate::common::{Stream, TlsState};
use rustls::Session;
use std::future::Future;
use std::pin::Pin;
use std::task::{Context, Poll};
use std::{io, mem};
use tokio::io::{AsyncRead, AsyncWrite};

pub(crate) trait IoSession {
    type Io;
    type Session;

    fn skip_handshake(&self) -> bool;
    fn get_mut(&mut self) -> (&mut TlsState, &mut Self::Io, &mut Self::Session);
    fn into_io(self) -> Self::Io;
}

pub(crate) enum MidHandshake<IS> {
    Handshaking(IS),
    End,
}

impl<IS> Future for MidHandshake<IS>
where
    IS: IoSession + Unpin,
    IS::Io: AsyncRead + AsyncWrite + Unpin,
    IS::Session: Session + Unpin,
{
    type Output = Result<IS, (io::Error, IS::Io)>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.get_mut();

        let mut stream =
            if let MidHandshake::Handshaking(stream) = mem::replace(this, MidHandshake::End) {
                stream
            } else {
                panic!("unexpected polling after handshake")
            };

        if !stream.skip_handshake() {
            let (state, io, session) = stream.get_mut();
            let mut tls_stream = Stream::new(io, session).set_eof(!state.readable());

            macro_rules! try_poll {
                ( $e:expr ) => {
                    match $e {
                        Poll::Ready(Ok(_)) => (),
                        Poll::Ready(Err(err)) => return Poll::Ready(Err((err, stream.into_io()))),
                        Poll::Pending => {
                            *this = MidHandshake::Handshaking(stream);
                            return Poll::Pending;
                        }
                    }
                };
            }

            while tls_stream.session.is_handshaking() {
                try_poll!(tls_stream.handshake(cx));
            }

            while tls_stream.session.wants_write() {
                try_poll!(tls_stream.write_io(cx));
            }
        }

        Poll::Ready(Ok(stream))
    }
}
