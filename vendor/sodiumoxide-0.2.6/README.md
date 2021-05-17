# sodiumoxide

|Crate|Documentation|Linux/OS X|Windows|Coverage|Gitter|
|:---:|:-----------:|:--------:|:-----:|:------:|:----:|
|[![Crates.io][crates-badge]][crates-url]|[![Docs][doc-badge]][doc-url]|[![TravisCI][travis-badge]][travis-url]|[![AppveyorCI][appveyor-badge]][appveyor-url]|[![Coverage Status][coverage-badge]][coverage-url]|[![Gitter][gitter-badge]][gitter-url]|

[crates-badge]: https://img.shields.io/crates/v/sodiumoxide.svg
[crates-url]: https://crates.io/crates/sodiumoxide
[doc-badge]: https://docs.rs/sodiumoxide/badge.svg
[doc-url]: https://docs.rs/sodiumoxide
[travis-badge]: https://travis-ci.org/sodiumoxide/sodiumoxide.svg?branch=master
[travis-url]: https://travis-ci.org/sodiumoxide/sodiumoxide/branches
[appveyor-badge]: https://ci.appveyor.com/api/projects/status/u05iy6wufw9ncdi7/branch/master?svg=true
[appveyor-url]: https://ci.appveyor.com/project/Dylan-DPC/sodiumoxide/branch/master
[coverage-badge]: https://coveralls.io/repos/github/sodiumoxide/sodiumoxide/badge.svg
[coverage-url]: https://coveralls.io/github/sodiumoxide/sodiumoxide
[gitter-badge]: https://badges.gitter.im/rust-sodiumoxide/Lobby.svg
[gitter-url]: https://gitter.im/rust-sodiumoxide/Lobby

> [NaCl](http://nacl.cr.yp.to) (pronounced "salt") is a new easy-to-use high-speed software library for network communication, encryption, decryption, signatures, etc. NaCl's goal is to provide all of the core operations needed to build higher-level cryptographic tools.
> Of course, other libraries already exist for these core operations. NaCl advances the state of the art by improving security, by improving usability, and by improving speed.

> [Sodium](https://github.com/jedisct1/libsodium) is a portable, cross-compilable, installable, packageable fork of NaCl (based on the latest released upstream version nacl-20110221), with a compatible API.

This package aims to provide a type-safe and efficient Rust binding that's just
as easy to use.
Rust >= 1.36.0 is required because of mem::MaybeUninit.

## Basic usage

### Cloning
```
git clone https://github.com/sodiumoxide/sodiumoxide.git
cd sodiumoxide
git submodule update --init --recursive
```

### Building
```
cargo build
```

### Testing
```
cargo test
```

### Documentation
```
cargo doc
```

Documentation will be generated in target/doc/...

Most documentation is taken from NaCl, with minor modification where the API
differs between the C and Rust versions.

## Dependencies

C compiler (`cc`, `clang`, ...) must be installed in order to build libsodium from source.

## Extended usage

This project contains a snapshot of libsodium and builds it by default, favouring a statically-built, fixed version of the native library.

Although it is highly recommended to use the default way with the pinned version, there are several ways you may want to use this crate:
* link it against the library installed on your system
* link it against a precompiled library that you built on your own

You can do this by setting environment variables.

|Name|Description|Example value|Notes|
| :- | :-------- | :---------- | :-- |
|`SODIUM_LIB_DIR`|Where to find a precompiled library|`/usr/lib/x86_64-linux-gnu/`|The value should be set to the directory containing `.so`,`.a`,`.la`,`.dll` or `.lib`|
|`SODIUM_SHARED`|Tell `rustc` to link the library dynamically|`1`|Works only with `SODIUM_LIB_DIR`. We check only the presence|
|`SODIUM_USE_PKG_CONFIG`|Tell build.rs to find system library using pkg-config|`1`|We check only the presence|
|`SODIUM_DISABLE_PIE`|Build with `--disable-pie`|`1`|Certain situations may require building libsodium configured with `--disable-pie`. Useful for !Windows only and when building libsodium from source. We check only the presence|

### Examples on *nix

#### Using pkg-config

(Ubuntu: `apt install pkg-config`, OSX: `brew install pkg-config`, ...)

```
export SODIUM_USE_PKG_CONFIG=1
cargo build
```

#### Using precompiled library

See https://download.libsodium.org/doc/installation.

```
export SODIUM_LIB_DIR=/home/user/libsodium-1.0.18/release/lib/
export SODIUM_SHARED=1
cargo build
```

## Optional features

Several [optional features](http://doc.crates.io/manifest.html#usage-in-end-products) are available:

* `std` (default: **enabled**). When this feature is disabled,
  sodiumoxide builds using `#![no_std]`. Some functionality may be lost.
  Requires a nightly build of Rust.

* `serde` (default: **enabled**). Allows serialization and deserialization of
  keys, authentication tags, etc. using the
  [serde library](https://crates.io/crates/serde).

* `benchmarks` (default: **disabled**). Compile benchmark tests. Requires a
  nightly build of Rust.

## Cross-Compiling

### Cross-Compiling for armv7-unknown-linux-gnueabihf

1. Install dependencies and toolchain:

```
sudo apt update
sudo apt install build-essential gcc-arm-linux-gnueabihf libc6-armhf-cross libc6-dev-armhf-cross -y
rustup target add armv7-unknown-linux-gnueabihf
```

2. Add the following to a [.cargo/config file](http://doc.crates.io/config.html):

```
[target.armv7-unknown-linux-gnueabihf]
linker = "arm-linux-gnueabihf-gcc"
```

3. Build by running:

```
cargo build --release --target armv7-unknown-linux-gnueabihf
```

### Cross-Compiling for armv7-unknown-linux-musleabihf via docker

1. cargo.config:

```
[target.armv7-unknown-linux-musleabihf]
linker = "arm-buildroot-linux-musleabihf-gcc"
```

2. Dockerfile:

```
FROM rust:1.36.0

ENV TARGET="armv7-unknown-linux-musleabihf"

ARG TOOLCHAIN_ARM7="armv7-eabihf--musl--stable-2018.02-2"
ARG TC_ARM7_URL="https://toolchains.bootlin.com/downloads/releases/toolchains/armv7-eabihf/tarballs/${TOOLCHAIN_ARM7}.tar.bz2"

RUN rustup target add ${TARGET}
COPY cargo.config "${CARGO_HOME}/config"

WORKDIR /opt
RUN curl -o- ${TC_ARM7_URL} | tar -xjf -

ENV PATH="${PATH}:/opt/${TOOLCHAIN_ARM7}/bin"
ENV CC_armv7_unknown_linux_musleabihf=arm-buildroot-linux-musleabihf-gcc
ENV CXX_armv7_unknown_linux_musleabihf=arm-buildroot-linux-musleabihf-g++
ENV LD_armv7_unknown_linux_musleabihf=arm-buildroot-linux-musleabihf-ld

WORKDIR /work
RUN git clone https://github.com/sodiumoxide/sodiumoxide

WORKDIR /work/sodiumoxide
RUN cargo build --target=${TARGET}
```

### Cross-Compiling for 32-bit Linux

1. Install dependencies and toolchain:

```
sudo apt update
sudo apt install build-essential gcc-multilib -y
rustup target add i686-unknown-linux-gnu
```

2. Build by running:

```
cargo build --release --target i686-unknown-linux-gnu
```

## Examples

TBD

## Platform Compatibiility

Sodiumoxide has been tested on:

- Linux: Yes
- Windows: Yes (MSVC)
- Mac OS: Yes
- IOS: TODO
- Android: TODO


# Join in

File bugs in the issue tracker

Master git repository

    git clone https://github.com/sodiumoxide/sodiumoxide.git

## License

Licensed under either of

* Apache License, Version 2.0, ([LICENSE-APACHE](LICENSE-APACHE) or http://www.apache.org/licenses/LICENSE-2.0)
* MIT license ([LICENSE-MIT](LICENSE-MIT) or http://opensource.org/licenses/MIT)

at your option.

### Contribution

Go through the [CONTRIBUTING.md](https://github.com/sodiumoxide/sodiumoxide/blob/master/CONTRIBUTING.md) document to know more about how to contribute to this project.

Unless you explicitly state otherwise, any contribution intentionally
submitted for inclusion in the work by you, as defined in the Apache-2.0
license, shall be dual licensed as above, without any additional terms or
conditions.

### Code of Conduct

We believe in creating an enabling community for developers and have laid out a general [code of conduct](https://github.com/sodiumoxide/sodiumoxide/blob/master/CODE_OF_CONDUCT.md). Please read and adopt it to help us achieve and maintain the desired community standards.
