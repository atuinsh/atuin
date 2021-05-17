uuid
---------

[![Latest Version](https://img.shields.io/crates/v/uuid.svg)](https://crates.io/crates/uuid)
[![Join the chat at https://gitter.im/uuid-rs/Lobby](https://badges.gitter.im/Join%20Chat.svg)](https://gitter.im/uuid-rs/Lobby?utm_source=badge&utm_medium=badge&utm_content=badge)
![Minimum rustc version](https://img.shields.io/badge/rustc-1.34.0+-yellow.svg)
[![Build Status](https://ci.appveyor.com/api/projects/status/github/uuid-rs/uuid?branch=master&svg=true)](https://ci.appveyor.com/project/uuid-rs/uuid/branch/master)
[![Build Status](https://travis-ci.org/uuid-rs/uuid.svg?branch=master)](https://travis-ci.org/uuid-rs/uuid)
[![Average time to resolve an issue](https://isitmaintained.com/badge/resolution/uuid-rs/uuid.svg)](https://isitmaintained.com/project/uuid-rs/uuid "Average time to resolve an issue")
[![Percentage of issues still open](https://isitmaintained.com/badge/open/uuid-rs/uuid.svg)](https://isitmaintained.com/project/uuid-rs/uuid "Percentage of issues still open")
[![FOSSA Status](https://app.fossa.com/api/projects/git%2Bgithub.com%2Fuuid-rs%2Fuuid.svg?type=shield)](https://app.fossa.com/projects/git%2Bgithub.com%2Fuuid-rs%2Fuuid?ref=badge_shield)

---

Generate and parse UUIDs.

Provides support for Universally Unique Identifiers (UUIDs). A UUID is a
unique 128-bit number, stored as 16 octets. UUIDs are used to  assign
unique identifiers to entities without requiring a central allocating
authority.

They are particularly useful in distributed systems, though they can be used in
disparate areas, such as databases and network protocols.  Typically a UUID
is displayed in a readable string form as a sequence of hexadecimal digits,
separated into groups by hyphens.

The uniqueness property is not strictly guaranteed, however for all
practical purposes, it can be assumed that an unintentional collision would
be extremely unlikely.

## Dependencies

By default, this crate depends on nothing but `std` and cannot generate
[`Uuid`]s. You need to enable the following Cargo features to enable
various pieces of functionality:

* `v1` - adds the `Uuid::new_v1` function and the ability to create a V1
  using an implementation of `uuid::v1::ClockSequence` (usually
`uuid::v1::Context`) and a timestamp from `time::timespec`.
* `v3` - adds the `Uuid::new_v3` function and the ability to create a V3
  UUID based on the MD5 hash of some data.
* `v4` - adds the `Uuid::new_v4` function and the ability to randomly
  generate a `Uuid`.
* `v5` - adds the `Uuid::new_v5` function and the ability to create a V5
  UUID based on the SHA1 hash of some data.
* `serde` - adds the ability to serialize and deserialize a `Uuid` using the
  `serde` crate.

You need to enable one of the following Cargo features together with
`v3`, `v4` or `v5` feature if you're targeting `wasm32-unknown-unknown` target:

* `stdweb` - enables support for `OsRng` on `wasm32-unknown-unknown` via
  `stdweb` combined with `cargo-web`
* `wasm-bindgen` - `wasm-bindgen` enables support for `OsRng` on
  `wasm32-unknown-unknown` via [`wasm-bindgen`]

By default, `uuid` can be depended on with:

```toml
[dependencies]
uuid = "0.8"
```

To activate various features, use syntax like:

```toml
[dependencies]
uuid = { version = "0.8", features = ["serde", "v4"] }
```

You can disable default features with:

```toml
[dependencies]
uuid = { version = "0.8", default-features = false }
```

## Examples

To parse a UUID given in the simple format and print it as a urn:

```rust
use uuid::Uuid;

fn main() -> Result<(), uuid::Error> {
    let my_uuid =
        Uuid::parse_str("936DA01F9ABD4d9d80C702AF85C822A8")?;
    println!("{}", my_uuid.to_urn());
    Ok(())
}
```

To create a new random (V4) UUID and print it out in hexadecimal form:

```rust
// Note that this requires the `v4` feature enabled in the uuid crate.

use uuid::Uuid;

fn main() {
    let my_uuid = Uuid::new_v4();
    println!("{}", my_uuid);
    Ok(())
}
```

## Strings

Examples of string representations:

* simple: `936DA01F9ABD4d9d80C702AF85C822A8`
* hyphenated: `550e8400-e29b-41d4-a716-446655440000`
* urn: `urn:uuid:F9168C5E-CEB2-4faa-B6BF-329BF39FA1E4`

## References

* [Wikipedia: Universally Unique Identifier](     http://en.wikipedia.org/wiki/Universally_unique_identifier)
* [RFC4122: A Universally Unique IDentifier (UUID) URN Namespace](     http://tools.ietf.org/html/rfc4122)

[`wasm-bindgen`]: https://github.com/rustwasm/wasm-bindgen

[`Uuid`]: https://docs.rs/uuid/0.8.2/uuid/struct.Uuid.html

---
# License

Licensed under either of

* Apache License, Version 2.0, (LICENSE-APACHE or https://www.apache.org/licenses/LICENSE-2.0)
* MIT license (LICENSE-MIT or https://opensource.org/licenses/MIT)

at your option.


[![FOSSA Status](https://app.fossa.com/api/projects/git%2Bgithub.com%2Fuuid-rs%2Fuuid.svg?type=large)](https://app.fossa.com/projects/git%2Bgithub.com%2Fuuid-rs%2Fuuid?ref=badge_large)

## Contribution

Unless you explicitly state otherwise, any contribution intentionally submitted
for inclusion in the work by you, as defined in the Apache-2.0 license, shall
be dual licensed as above, without any additional terms or conditions.