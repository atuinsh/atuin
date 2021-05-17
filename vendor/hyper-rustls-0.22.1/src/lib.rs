//! # hyper-rustls
//!
//! A pure-Rust HTTPS connector for [hyper](https://hyper.rs), based on [Rustls](https://github.com/ctz/rustls).
//!
//! ## Example
//!
//! ```no_run
//! # #[cfg(all(feature = "rustls-native-certs", feature = "tokio-runtime"))]
//! # fn main() {
//! use hyper::{Body, Client, StatusCode, Uri};
//!
//! let mut rt = tokio::runtime::Runtime::new().unwrap();
//! let url = ("https://hyper.rs").parse().unwrap();
//! let https = hyper_rustls::HttpsConnector::with_native_roots();
//!
//! let client: Client<_, hyper::Body> = Client::builder().build(https);
//!
//! let res = rt.block_on(client.get(url)).unwrap();
//! assert_eq!(res.status(), StatusCode::OK);
//! # }
//! # #[cfg(not(all(feature = "rustls-native-certs", feature = "tokio-runtime")))]
//! # fn main() {}
//! ```

#![cfg_attr(docsrs, feature(doc_cfg))]

mod connector;
mod stream;

pub use crate::connector::HttpsConnector;
pub use crate::stream::MaybeHttpsStream;
