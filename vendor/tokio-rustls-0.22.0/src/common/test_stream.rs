use super::Stream;
use futures_util::future::poll_fn;
use futures_util::task::noop_waker_ref;
use rustls::internal::pemfile::{certs, rsa_private_keys};
use rustls::{ClientConfig, ClientSession, NoClientAuth, ServerConfig, ServerSession, Session};
use std::io::{self, BufReader, Cursor, Read, Write};
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, ReadBuf};
use webpki::DNSNameRef;

struct Good<'a>(&'a mut dyn Session);

impl<'a> AsyncRead for Good<'a> {
    fn poll_read(
        mut self: Pin<&mut Self>,
        _cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let mut buf2 = buf.initialize_unfilled();

        Poll::Ready(match self.0.write_tls(buf2.by_ref()) {
            Ok(n) => {
                buf.advance(n);
                Ok(())
            }
            Err(err) => Err(err),
        })
    }
}

impl<'a> AsyncWrite for Good<'a> {
    fn poll_write(
        mut self: Pin<&mut Self>,
        _cx: &mut Context<'_>,
        mut buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        let len = self.0.read_tls(buf.by_ref())?;
        self.0
            .process_new_packets()
            .map_err(|err| io::Error::new(io::ErrorKind::InvalidData, err))?;
        Poll::Ready(Ok(len))
    }

    fn poll_flush(mut self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        self.0
            .process_new_packets()
            .map_err(|err| io::Error::new(io::ErrorKind::InvalidData, err))?;
        Poll::Ready(Ok(()))
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        self.0.send_close_notify();
        Poll::Ready(Ok(()))
    }
}

struct Pending;

impl AsyncRead for Pending {
    fn poll_read(
        self: Pin<&mut Self>,
        _cx: &mut Context<'_>,
        _: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        Poll::Pending
    }
}

impl AsyncWrite for Pending {
    fn poll_write(
        self: Pin<&mut Self>,
        _cx: &mut Context<'_>,
        _buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        Poll::Pending
    }

    fn poll_flush(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }

    fn poll_shutdown(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }
}

struct Eof;

impl AsyncRead for Eof {
    fn poll_read(
        self: Pin<&mut Self>,
        _cx: &mut Context<'_>,
        _: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }
}

impl AsyncWrite for Eof {
    fn poll_write(
        self: Pin<&mut Self>,
        _cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        Poll::Ready(Ok(buf.len()))
    }

    fn poll_flush(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }

    fn poll_shutdown(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }
}

#[tokio::test]
async fn stream_good() -> io::Result<()> {
    const FILE: &'static [u8] = include_bytes!("../../README.md");

    let (mut server, mut client) = make_pair();
    poll_fn(|cx| do_handshake(&mut client, &mut server, cx)).await?;
    io::copy(&mut Cursor::new(FILE), &mut server)?;

    {
        let mut good = Good(&mut server);
        let mut stream = Stream::new(&mut good, &mut client);

        let mut buf = Vec::new();
        stream.read_to_end(&mut buf).await?;
        assert_eq!(buf, FILE);
        stream.write_all(b"Hello World!").await?;
        stream.flush().await?;
    }

    let mut buf = String::new();
    server.read_to_string(&mut buf)?;
    assert_eq!(buf, "Hello World!");

    Ok(()) as io::Result<()>
}

#[tokio::test]
async fn stream_bad() -> io::Result<()> {
    let (mut server, mut client) = make_pair();
    poll_fn(|cx| do_handshake(&mut client, &mut server, cx)).await?;
    client.set_buffer_limit(1024);

    let mut bad = Pending;
    let mut stream = Stream::new(&mut bad, &mut client);
    assert_eq!(
        poll_fn(|cx| stream.as_mut_pin().poll_write(cx, &[0x42; 8])).await?,
        8
    );
    assert_eq!(
        poll_fn(|cx| stream.as_mut_pin().poll_write(cx, &[0x42; 8])).await?,
        8
    );
    let r = poll_fn(|cx| stream.as_mut_pin().poll_write(cx, &[0x00; 1024])).await?; // fill buffer
    assert!(r < 1024);

    let mut cx = Context::from_waker(noop_waker_ref());
    let ret = stream.as_mut_pin().poll_write(&mut cx, &[0x01]);
    assert!(ret.is_pending());

    Ok(()) as io::Result<()>
}

#[tokio::test]
async fn stream_handshake() -> io::Result<()> {
    let (mut server, mut client) = make_pair();

    {
        let mut good = Good(&mut server);
        let mut stream = Stream::new(&mut good, &mut client);
        let (r, w) = poll_fn(|cx| stream.handshake(cx)).await?;

        assert!(r > 0);
        assert!(w > 0);

        poll_fn(|cx| stream.handshake(cx)).await?; // finish server handshake
    }

    assert!(!server.is_handshaking());
    assert!(!client.is_handshaking());

    Ok(()) as io::Result<()>
}

#[tokio::test]
async fn stream_handshake_eof() -> io::Result<()> {
    let (_, mut client) = make_pair();

    let mut bad = Eof;
    let mut stream = Stream::new(&mut bad, &mut client);

    let mut cx = Context::from_waker(noop_waker_ref());
    let r = stream.handshake(&mut cx);
    assert_eq!(
        r.map_err(|err| err.kind()),
        Poll::Ready(Err(io::ErrorKind::UnexpectedEof))
    );

    Ok(()) as io::Result<()>
}

#[tokio::test]
async fn stream_eof() -> io::Result<()> {
    let (mut server, mut client) = make_pair();
    poll_fn(|cx| do_handshake(&mut client, &mut server, cx)).await?;

    let mut good = Good(&mut server);
    let mut stream = Stream::new(&mut good, &mut client).set_eof(true);

    let mut buf = Vec::new();
    stream.read_to_end(&mut buf).await?;
    assert_eq!(buf.len(), 0);

    Ok(()) as io::Result<()>
}

fn make_pair() -> (ServerSession, ClientSession) {
    const CERT: &str = include_str!("../../tests/end.cert");
    const CHAIN: &str = include_str!("../../tests/end.chain");
    const RSA: &str = include_str!("../../tests/end.rsa");

    let cert = certs(&mut BufReader::new(Cursor::new(CERT))).unwrap();
    let mut keys = rsa_private_keys(&mut BufReader::new(Cursor::new(RSA))).unwrap();
    let mut sconfig = ServerConfig::new(NoClientAuth::new());
    sconfig.set_single_cert(cert, keys.pop().unwrap()).unwrap();
    let server = ServerSession::new(&Arc::new(sconfig));

    let domain = DNSNameRef::try_from_ascii_str("localhost").unwrap();
    let mut cconfig = ClientConfig::new();
    let mut chain = BufReader::new(Cursor::new(CHAIN));
    cconfig.root_store.add_pem_file(&mut chain).unwrap();
    let client = ClientSession::new(&Arc::new(cconfig), domain);

    (server, client)
}

fn do_handshake(
    client: &mut ClientSession,
    server: &mut ServerSession,
    cx: &mut Context<'_>,
) -> Poll<io::Result<()>> {
    let mut good = Good(server);
    let mut stream = Stream::new(&mut good, client);

    while stream.session.is_handshaking() {
        ready!(stream.handshake(cx))?;
    }

    while stream.session.wants_write() {
        ready!(stream.write_io(cx))?;
    }

    Poll::Ready(Ok(()))
}
