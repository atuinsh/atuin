# Rand

[![Build Status](https://travis-ci.org/rust-random/rand.svg?branch=master)](https://travis-ci.org/rust-random/rand)
[![Build Status](https://ci.appveyor.com/api/projects/status/github/rust-random/rand?svg=true)](https://ci.appveyor.com/project/rust-random/rand)
[![Crate](https://img.shields.io/crates/v/rand.svg)](https://crates.io/crates/rand)
[![Book](https://img.shields.io/badge/book-master-yellow.svg)](https://rust-random.github.io/book/)
[![API](https://img.shields.io/badge/api-master-yellow.svg)](https://rust-random.github.io/rand)
[![API](https://docs.rs/rand/badge.svg)](https://docs.rs/rand)
[![Minimum rustc version](https://img.shields.io/badge/rustc-1.32+-lightgray.svg)](https://github.com/rust-random/rand#rust-version-requirements)

A Rust library for random number generation.

Rand provides utilities to generate random numbers, to convert them to useful
types and distributions, and some randomness-related algorithms.

The core random number generation traits of Rand live in the [rand_core](
https://crates.io/crates/rand_core) crate but are also exposed here; RNG
implementations should prefer to use `rand_core` while most other users should
depend on `rand`.

Documentation:
-   [The Rust Rand Book](https://rust-random.github.io/book)
-   [API reference (master)](https://rust-random.github.io/rand)
-   [API reference (docs.rs)](https://docs.rs/rand)


## Usage

Add this to your `Cargo.toml`:

```toml
[dependencies]
rand = "0.7"
```

To get started using Rand, see [The Book](https://rust-random.github.io/book).


## Versions

Rand libs have inter-dependencies and make use of the
[semver trick](https://github.com/dtolnay/semver-trick/) in order to make traits
compatible across crate versions. (This is especially important for `RngCore`
and `SeedableRng`.) A few crate releases are thus compatibility shims,
depending on the *next* lib version (e.g. `rand_core` versions `0.2.2` and
`0.3.1`). This means, for example, that `rand_core_0_4_0::SeedableRng` and
`rand_core_0_3_0::SeedableRng` are distinct, incompatible traits, which can
cause build errors. Usually, running `cargo update` is enough to fix any issues.

The Rand lib is not yet stable, however we are careful to limit breaking changes
and warn via deprecation wherever possible. Patch versions never introduce
breaking changes. The following minor versions are supported:

-   Version 0.7 was released in June 2019, moving most non-uniform distributions
    to an external crate, moving `from_entropy` to `SeedableRng`, and many small
    changes and fixes.
-   Version 0.6 was released in November 2018, redesigning the `seq` module,
    moving most PRNGs to external crates, and many small changes.
-   Version 0.5 was released in May 2018, as a major reorganisation
    (introducing `RngCore` and `rand_core`, and deprecating `Rand` and the
    previous distribution traits).
-   Version 0.4 was released in December 2017, but contained almost no breaking
    changes from the 0.3 series.

A detailed [changelog](CHANGELOG.md) is available.

When upgrading to the next minor series (especially 0.4 → 0.5), we recommend
reading the [Upgrade Guide](https://rust-random.github.io/book/update.html).

### Yanked versions

Some versions of Rand crates have been yanked ("unreleased"). Where this occurs,
the crate's CHANGELOG *should* be updated with a rationale, and a search on the
issue tracker with the keyword `yank` *should* uncover the motivation.

### Rust version requirements

Since version 0.7, Rand requires **Rustc version 1.32 or greater**.
Rand 0.5 requires Rustc 1.22 or greater while versions
0.4 and 0.3 (since approx. June 2017) require Rustc version 1.15 or
greater. Subsets of the Rand code may work with older Rust versions, but this
is not supported.

Travis CI always has a build with a pinned version of Rustc matching the oldest
supported Rust release. The current policy is that this can be updated in any
Rand release if required, but the change must be noted in the changelog.

## Crate Features

Rand is built with these features enabled by default:

-   `std` enables functionality dependent on the `std` lib
-   `alloc` (implied by `std`) enables functionality requiring an allocator (when using this feature in `no_std`, Rand requires Rustc version 1.36 or greater)
-   `getrandom` (implied by `std`) is an optional dependency providing the code
    behind `rngs::OsRng`

Optionally, the following dependencies can be enabled:

-   `log` enables logging via the `log` crate
-   `stdweb` implies `getrandom/stdweb` to enable
    `getrandom` support on `wasm32-unknown-unknown`
    (will be removed in rand 0.8; activate via `getrandom` crate instead)
-   `wasm-bindgen` implies `getrandom/wasm-bindgen` to enable
    `getrandom` support on `wasm32-unknown-unknown`
    (will be removed in rand 0.8; activate via `getrandom` crate instead)

Additionally, these features configure Rand:

-   `small_rng` enables inclusion of the `SmallRng` PRNG
-   `nightly` enables all experimental features
-   `simd_support` (experimental) enables sampling of SIMD values
    (uniformly random SIMD integers and floats)

Rand supports limited functionality in `no_std` mode (enabled via
`default-features = false`). In this case, `OsRng` and `from_entropy` are
unavailable (unless `getrandom` is enabled), large parts of `seq` are
unavailable (unless `alloc` is enabled), and `thread_rng` and `random` are
unavailable.

# License

Rand is distributed under the terms of both the MIT license and the
Apache License (Version 2.0).

See [LICENSE-APACHE](LICENSE-APACHE) and [LICENSE-MIT](LICENSE-MIT), and
[COPYRIGHT](COPYRIGHT) for details.
