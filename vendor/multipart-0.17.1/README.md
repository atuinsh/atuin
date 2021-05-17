# Multipart [![Build Status](https://travis-ci.org/abonander/multipart.svg?branch=master)](https://travis-ci.org/abonander/multipart) [![On Crates.io](https://img.shields.io/crates/v/multipart.svg)](https://crates.io/crates/multipart)

Client- and server-side abstractions for HTTP file uploads (POST requests with  `Content-Type: multipart/form-data`).

Supports several different (**sync**hronous API) HTTP crates. 
**Async**hronous (i.e. `futures`-based) API support will be provided by [multipart-async].

##### Minimum supported Rust version: 1.33.0

##### Maintenance Status: Passive

As the web ecosystem in Rust moves towards asynchronous APIs, the need for this crate in synchronous
API form becomes dubious. This crate in its current form is usable enough, so as of June 2020 it
is now in passive maintenance mode; bug reports will be addressed as time permits and PRs will be
accepted but otherwise no new development of the existing API is taking place.

Look for a release of [multipart-async] soon which targets newer releases of Hyper.

### [Documentation](http://docs.rs/multipart/)

## Integrations

Example files demonstrating how to use `multipart` with these crates are available under [`examples/`](examples).

### [Hyper ![](https://img.shields.io/crates/v/hyper.svg)](https://crates.io/crates/hyper) 
via the `hyper` feature (enabled by default). 

**Note: Hyper 0.9, 0.10 (synchronous API) only**; support for asynchronous APIs will be provided by [multipart-async].
 
Client integration includes support for regular `hyper::client::Request` objects via `multipart::client::Multipart`, as well
as integration with the new `hyper::Client` API via `multipart::client::lazy::Multipart` (new in 0.5).

Server integration for `hyper::server::Request` via `multipart::server::Multipart`.

### [Iron ![](https://img.shields.io/crates/v/iron.svg)](https://crates.io/crates/iron) 
via the `iron` feature.

Provides regular server-side integration with `iron::Request` via `multipart::server::Multipart`, 
as well as a convenient `BeforeMiddleware` implementation in `multipart::server::iron::Intercept`.

### [Nickel ![](https://img.shields.io/crates/v/nickel.svg)](https://crates.io/crates/nickel) <sup>returning to `multipart` in 0.14!</sup>
via the `nickel` feature.

Provides server-side integration with `&mut nickel::Request` via `multipart::server::Multipart`. 

### [tiny_http ![](https://img.shields.io/crates/v/tiny_http.svg)](https://crates.io/crates/tiny_http)
via the `tiny_http` feature.

Provides server-side integration with `tiny_http::Request` via `multipart::server::Multipart`.

### [Rocket ![](https://img.shields.io/crates/v/rocket.svg)](https://crates.io/crates/rocket)

Direct integration is not provided as the Rocket folks seem to want to handle `multipart/form-data`
behind the scenes which would supercede any integration with `multipart`. However, an example is available
showing how to use `multipart` on a Rocket server: [examples/rocket.rs](examples/rocket.rs)

## ⚡ Powered By ⚡

### [buf_redux ![](https://img.shields.io/crates/v/buf_redux.svg)](https://crates.io/crates/buf_redux)

Customizable drop-in `std::io::BufReader` replacement, created to be used in this crate.
Needed because it can read more bytes into the buffer without the buffer being empty, necessary
when a boundary falls across two reads. (It was easier to author a new crate than try to get this added
to `std::io::BufReader`.)

### [httparse ![](https://img.shields.io/crates/v/httparse.svg)](https://crates.io/crates/httparse)

Fast, zero-copy HTTP header parsing, used to read field headers in `multipart/form-data` request bodies.

### [twoway ![](https://img.shields.io/crates/v/twoway.svg)](https://crates.io/crates/twoway)

Fast string and byte-string search. Used to find boundaries in the request body. Uses SIMD acceleration
when possible.

## License

Licensed under either of

 * Apache License, Version 2.0, ([LICENSE-APACHE](LICENSE-APACHE) or http://www.apache.org/licenses/LICENSE-2.0)
 * MIT license ([LICENSE-MIT](LICENSE-MIT) or http://opensource.org/licenses/MIT)

at your option.

## Contribution

Unless you explicitly state otherwise, any contribution intentionally submitted
for inclusion in the work by you, as defined in the Apache-2.0 license, shall be dual licensed as above, without any
additional terms or conditions.

[multipart-async]: https://github.com/abonander/multipart-async
