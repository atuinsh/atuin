use h2::client;
use http::{Method, Request};
use tokio::net::TcpStream;
use tokio_rustls::TlsConnector;

use rustls::Session;
use webpki::DNSNameRef;

use std::error::Error;
use std::net::ToSocketAddrs;

const ALPN_H2: &str = "h2";

#[tokio::main]
pub async fn main() -> Result<(), Box<dyn Error>> {
    let _ = env_logger::try_init();

    let tls_client_config = std::sync::Arc::new({
        let mut c = rustls::ClientConfig::new();
        c.root_store
            .add_server_trust_anchors(&webpki_roots::TLS_SERVER_ROOTS);
        c.alpn_protocols.push(ALPN_H2.as_bytes().to_owned());
        c
    });

    // Sync DNS resolution.
    let addr = "http2.akamai.com:443"
        .to_socket_addrs()
        .unwrap()
        .next()
        .unwrap();

    println!("ADDR: {:?}", addr);

    let tcp = TcpStream::connect(&addr).await?;
    let dns_name = DNSNameRef::try_from_ascii_str("http2.akamai.com").unwrap();
    let connector = TlsConnector::from(tls_client_config);
    let res = connector.connect(dns_name, tcp).await;
    let tls = res.unwrap();
    {
        let (_, session) = tls.get_ref();
        let negotiated_protocol = session.get_alpn_protocol();
        assert_eq!(
            Some(ALPN_H2.as_bytes()),
            negotiated_protocol.as_ref().map(|x| &**x)
        );
    }

    println!("Starting client handshake");
    let (mut client, h2) = client::handshake(tls).await?;

    println!("building request");
    let request = Request::builder()
        .method(Method::GET)
        .uri("https://http2.akamai.com/")
        .body(())
        .unwrap();

    println!("sending request");
    let (response, other) = client.send_request(request, true).unwrap();

    tokio::spawn(async move {
        if let Err(e) = h2.await {
            println!("GOT ERR={:?}", e);
        }
    });

    println!("waiting on response : {:?}", other);
    let (_, mut body) = response.await?.into_parts();
    println!("processing body");
    while let Some(chunk) = body.data().await {
        println!("RX: {:?}", chunk?);
    }
    Ok(())
}
