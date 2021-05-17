`multipart` Examples
===========================

These example files show how to use `multipart` with the various crates it integrates with.

These files carry the same licenses as [`multipart` itself](https://github.com/abonander/multipart#license), though this may be lightened to a copyright-free license in the near future.

## Client

Examples for the client-side integrations of `multipart`'s API.

[`hyper_client`](hyper_client.rs)
---------------------------------
Author: [abonander]

This example showcases usage of `multipart` with the `hyper::client::Request` API.

```
$ cargo run --example hyper_client
```

[`hyper_reqbuilder`](hyper_reqbuilder.rs)
-----------------------------------------
Author: [abonander]

This example showcases usage of `multipart` with Hyper's new `Client` API,
via the lazy-writing capabilities of `multipart::client::lazy`.

```
$ cargo run --example hyper_reqbuilder
```

## Server

[`hyper_server`](hyper_server.rs)
---------------------------------
Author: [Puhrez]

This example shows how to use `multipart` with a [`hyper::Server`] (http://hyper.rs/) to intercept multipart requests.

```
$ cargo run --example hyper_server
```

[`iron`](iron.rs)
-----------------
Author: [White-Oak]

This example shows how to use `multipart` with the [Iron web application framework](http://ironframework.io/), via `multipart`'s support
for the `iron::Request` type.

To run:

```
$ cargo run --features iron --example iron
```

[`iron_intercept`](iron_intercept.rs)
-------------------------------------
Author: [abonander]

This example shows how to use `multipart`'s specialized `Intercept` middleware with Iron, which reads out all fields and
files to local storage so they can be accessed arbitrarily.

```
$ cargo run --features iron --example iron_intercept
```

[`tiny_http`](tiny_http.rs)
---------------------------
Author: [White-Oak]

This example shows how to use `multipart` with the [`tiny_http` crate](https://crates.io/crates/tiny_http), via `multipart`'s support for the `tiny_http::Request` type.

```
$ cargo run --features tiny_http --example tiny_http
```

[`hyper_server`](hyper_server.rs)
---------------------------------
Author: [Puhrez]

This example shows how to use `multipart` with a [`hyper::Server`] (http://hyper.rs/) to intercept multipart requests.

```
$ cargo run --example hyper_server
```

[`nickel`](nickel.rs)
---------------------
Author: [iamsebastian]

This example shows how to use `multipart` to handle multipart uploads in [nickel.rs](https://nickel.rs).

```
$ cargo run --example nickel --features nickel
```

[Rocket](rocket.rs)
-------------------
Author: [abonander]

This example shows how `multipart`'s server API can be used with [Rocket](https://rocket.rs) without
explicit support (the Rocket folks seem to want to handle `multipart/form-data` behind the scenes
but haven't gotten around to implementing it yet; this would supercede any integration from `multipart`). 

```
$ cargo run --example rocket --features "rocket"
```

[iamsebastian]: https://github.com/iamsebastian
[Puhrez]: https://github.com/puhrez
[White-Oak]: https://github.com/white-oak
[abonander]: https://github.com/abonander

