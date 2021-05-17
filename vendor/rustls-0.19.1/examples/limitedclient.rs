/// limitedclient: This example demonstrates usage of only ClientConfig::new_custom
/// so that unused cryptography in rustls can be discarded by the linker.  You can
/// observe using `nm` that the binary of this program does not contain any AES code.
use std::sync::Arc;

use std::io::{stdout, Read, Write};
use std::net::TcpStream;

use rustls;
use webpki;
use webpki_roots;

use rustls::Session;

fn main() {
    let mut config = rustls::ClientConfig::with_ciphersuites(&[
        &rustls::ciphersuite::TLS13_CHACHA20_POLY1305_SHA256,
    ]);
    config
        .root_store
        .add_server_trust_anchors(&webpki_roots::TLS_SERVER_ROOTS);

    let dns_name = webpki::DNSNameRef::try_from_ascii_str("google.com").unwrap();
    let mut sess = rustls::ClientSession::new(&Arc::new(config), dns_name);
    let mut sock = TcpStream::connect("google.com:443").unwrap();
    let mut tls = rustls::Stream::new(&mut sess, &mut sock);
    tls.write(
        concat!(
            "GET / HTTP/1.1\r\n",
            "Host: google.com\r\n",
            "Connection: close\r\n",
            "Accept-Encoding: identity\r\n",
            "\r\n"
        )
        .as_bytes(),
    )
    .unwrap();
    let ciphersuite = tls
        .sess
        .get_negotiated_ciphersuite()
        .unwrap();
    writeln!(
        &mut std::io::stderr(),
        "Current ciphersuite: {:?}",
        ciphersuite.suite
    )
    .unwrap();
    let mut plaintext = Vec::new();
    tls.read_to_end(&mut plaintext).unwrap();
    stdout().write_all(&plaintext).unwrap();
}
