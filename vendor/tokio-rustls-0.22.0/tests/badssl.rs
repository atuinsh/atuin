use rustls::ClientConfig;
use std::io;
use std::net::ToSocketAddrs;
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio_rustls::{client::TlsStream, TlsConnector};

async fn get(
    config: Arc<ClientConfig>,
    domain: &str,
    port: u16,
) -> io::Result<(TlsStream<TcpStream>, String)> {
    let connector = TlsConnector::from(config);
    let input = format!("GET / HTTP/1.0\r\nHost: {}\r\n\r\n", domain);

    let addr = (domain, port).to_socket_addrs()?.next().unwrap();
    let domain = webpki::DNSNameRef::try_from_ascii_str(&domain).unwrap();
    let mut buf = Vec::new();

    let stream = TcpStream::connect(&addr).await?;
    let mut stream = connector.connect(domain, stream).await?;
    stream.write_all(input.as_bytes()).await?;
    stream.flush().await?;
    stream.read_to_end(&mut buf).await?;

    Ok((stream, String::from_utf8(buf).unwrap()))
}

#[tokio::test]
async fn test_tls12() -> io::Result<()> {
    let mut config = ClientConfig::new();
    config
        .root_store
        .add_server_trust_anchors(&webpki_roots::TLS_SERVER_ROOTS);
    config.versions = vec![rustls::ProtocolVersion::TLSv1_2];
    let config = Arc::new(config);
    let domain = "tls-v1-2.badssl.com";

    let (_, output) = get(config.clone(), domain, 1012).await?;
    assert!(output.contains("<title>tls-v1-2.badssl.com</title>"));

    Ok(())
}

#[ignore]
#[should_panic]
#[test]
fn test_tls13() {
    unimplemented!("todo https://github.com/chromium/badssl.com/pull/373");
}

#[tokio::test]
async fn test_modern() -> io::Result<()> {
    let mut config = ClientConfig::new();
    config
        .root_store
        .add_server_trust_anchors(&webpki_roots::TLS_SERVER_ROOTS);
    let config = Arc::new(config);
    let domain = "mozilla-modern.badssl.com";

    let (_, output) = get(config.clone(), domain, 443).await?;
    assert!(output.contains("<title>mozilla-modern.badssl.com</title>"));

    Ok(())
}
