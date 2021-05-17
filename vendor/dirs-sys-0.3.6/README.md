[![crates.io](https://img.shields.io/crates/v/dirs-sys.svg)](https://crates.io/crates/dirs-sys)
[![API documentation](https://docs.rs/dirs-sys/badge.svg)](https://docs.rs/dirs-sys/)
![actively developed](https://img.shields.io/badge/maintenance-as--is-yellow.svg)
[![TravisCI status](https://img.shields.io/travis/dirs-dev/dirs-sys-rs/master.svg?label=Linux/macOS%20build)](https://travis-ci.org/dirs-dev/dirs-sys-rs)
[![AppVeyor status](https://img.shields.io/appveyor/ci/soc/dirs-sys-rs/master.svg?label=Windows%20build)](https://ci.appveyor.com/project/soc/dirs-sys-rs/branch/master)
![License: MIT/Apache-2.0](https://img.shields.io/badge/license-MIT%2FApache--2.0-orange.svg)

# `dirs-sys`

System-level helper functions for the [`dirs`](https://github.com/dirs-dev/dirs-rs)
and [`directories`](https://github.com/dirs-dev/directories-rs) crates.

_Do not use this library directly, use [`dirs`](https://github.com/dirs-dev/dirs-rs)
or [`directories`](https://github.com/dirs-dev/directories-rs)._

## Compatibility

This crate only exists to facilitate code sharing between [`dirs`](https://github.com/dirs-dev/dirs-rs)
and [`directories`](https://github.com/dirs-dev/directories-rs).

There are no compatibility guarantees whatsoever.
Functions may change or disappear without warning or any kind of deprecation period.  

## Platforms

This library is written in Rust, and supports Linux, Redox, macOS and Windows.
Other platforms are also supported; they use the Linux conventions.

The minimal required version of Rust is 1.13 except for Redox, where the minimum Rust version
depends on the [`redox_users`](https://crates.io/crates/redox_users) crate.

## Build

It's possible to cross-compile this library if the necessary toolchains are installed with rustup.
This is helpful to ensure a change has not broken compilation on a different platform.

The following commands will build this library on Linux, macOS and Windows:

```
cargo build --target=x86_64-unknown-linux-gnu
cargo build --target=x86_64-pc-windows-gnu
cargo build --target=x86_64-apple-darwin
cargo build --target=x86_64-unknown-redox
```

## License

Licensed under either of

 * Apache License, Version 2.0
   ([LICENSE-APACHE](LICENSE-APACHE) or http://www.apache.org/licenses/LICENSE-2.0)
 * MIT license
   ([LICENSE-MIT](LICENSE-MIT) or http://opensource.org/licenses/MIT)

at your option.

## Contribution

Unless you explicitly state otherwise, any contribution intentionally submitted
for inclusion in the work by you, as defined in the Apache-2.0 license, shall be
dual licensed as above, without any additional terms or conditions.
