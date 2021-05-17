use std::sync::Arc;

use std::io::{stdout, Read, Write};
use std::net::TcpStream;

use env_logger;
use rustls;
use webpki;
use webpki_roots;

fn start_session(config: &Arc<rustls::ClientConfig>, domain_name: &str) {
    let dns_name = webpki::DNSNameRef::try_from_ascii_str(domain_name).unwrap();
    let mut sess = rustls::ClientSession::new(config, dns_name);
    let mut sock = TcpStream::connect(format!("{}:443", domain_name)).unwrap();
    sock.set_nodelay(true).unwrap();
    let request = format!(
        "GET / HTTP/1.1\r\n\
         Host: {}\r\n\
         Connection: close\r\n\
         Accept-Encoding: identity\r\n\
         \r\n",
        domain_name
    );

    // If early data is available with this server, then early_data()
    // will yield Some(WriteEarlyData) and WriteEarlyData implements
    // io::Write.  Use this to send the request.
    if let Some(mut early_data) = sess.early_data() {
        early_data
            .write(request.as_bytes())
            .unwrap();
    }

    let mut stream = rustls::Stream::new(&mut sess, &mut sock);

    // Complete handshake.
    stream.flush().unwrap();

    // If we didn't send early data, or the server didn't accept it,
    // then send the request as normal.
    if !stream.sess.is_early_data_accepted() {
        stream
            .write_all(request.as_bytes())
            .unwrap();
    }

    let mut plaintext = Vec::new();
    stream
        .read_to_end(&mut plaintext)
        .unwrap();
    stdout().write_all(&plaintext).unwrap();
}

fn main() {
    env_logger::init();
    let mut config = rustls::ClientConfig::new();

    // Enable early data.
    config.enable_early_data = true;
    config
        .root_store
        .add_server_trust_anchors(&webpki_roots::TLS_SERVER_ROOTS);
    let config = Arc::new(config);

    // Do two sessions. The first will be a normal request, the
    // second will use early data if the server supports it.
    start_session(&config, "mesalink.io");
    start_session(&config, "mesalink.io");
}
