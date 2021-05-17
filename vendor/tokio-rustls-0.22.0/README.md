# tokio-rustls
[![github actions](https://github.com/tokio-rs/tls/workflows/Rust/badge.svg)](https://github.com/tokio-rs/tls/actions)
[![crates](https://img.shields.io/crates/v/tokio-rustls.svg)](https://crates.io/crates/tokio-rustls)
[![license](https://img.shields.io/badge/License-MIT-blue.svg)](https://github.com/tokio-rs/tls/blob/master/tokio-rustls/LICENSE-MIT)
[![license](https://img.shields.io/badge/License-Apache%202.0-blue.svg)](https://github.com/tokio-rs/tls/blob/master/tokio-rustls/LICENSE-APACHE)
[![docs.rs](https://docs.rs/tokio-rustls/badge.svg)](https://docs.rs/tokio-rustls/)

Asynchronous TLS/SSL streams for [Tokio](https://tokio.rs/) using
[Rustls](https://github.com/ctz/rustls).

### Basic Structure of a Client

```rust
use webpki::DNSNameRef;
use tokio_rustls::{ TlsConnector, rustls::ClientConfig };

// ...

let mut config = ClientConfig::new();
config.root_store.add_server_trust_anchors(&webpki_roots::TLS_SERVER_ROOTS);
let config = TlsConnector::from(Arc::new(config));
let dnsname = DNSNameRef::try_from_ascii_str("www.rust-lang.org").unwrap();

let stream = TcpStream::connect(&addr).await?;
let mut stream = config.connect(dnsname, stream).await?;

// ...
```

### Client Example Program

See [examples/client](examples/client/src/main.rs). You can run it with:

```sh
cd examples/client
cargo run -- hsts.badssl.com
```

### Server Example Program

See [examples/server](examples/server/src/main.rs). You can run it with:

```sh
cd examples/server
cargo run -- 127.0.0.1 --cert mycert.der --key mykey.der
```

### License & Origin

This project is licensed under either of

 * Apache License, Version 2.0, ([LICENSE-APACHE](LICENSE-APACHE) or
   http://www.apache.org/licenses/LICENSE-2.0)
 * MIT license ([LICENSE-MIT](LICENSE-MIT) or
   http://opensource.org/licenses/MIT)

at your option.

This started as a fork of [tokio-tls](https://github.com/tokio-rs/tokio-tls).

### Contribution

Unless you explicitly state otherwise, any contribution intentionally submitted
for inclusion in tokio-rustls by you, as defined in the Apache-2.0 license, shall be
dual licensed as above, without any additional terms or conditions.
