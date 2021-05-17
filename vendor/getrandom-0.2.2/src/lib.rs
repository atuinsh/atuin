// Copyright 2019 Developers of the Rand project.
//
// Licensed under the Apache License, Version 2.0 <LICENSE-APACHE or
// https://www.apache.org/licenses/LICENSE-2.0> or the MIT license
// <LICENSE-MIT or https://opensource.org/licenses/MIT>, at your
// option. This file may not be copied, modified, or distributed
// except according to those terms.

//! Interface to the operating system's random number generator.
//!
//! # Supported targets
//!
//! | Target            | Target Triple      | Implementation
//! | ----------------- | ------------------ | --------------
//! | Linux, Android    | `*‑linux‑*`        | [`getrandom`][1] system call if available, otherwise [`/dev/urandom`][2] after successfully polling `/dev/random` |
//! | Windows           | `*‑windows‑*`      | [`BCryptGenRandom`][3] |
//! | macOS             | `*‑apple‑darwin`   | [`getentropy()`][19] if available, otherwise [`/dev/random`][20] (identical to `/dev/urandom`)
//! | iOS               | `*‑apple‑ios`      | [`SecRandomCopyBytes`][4]
//! | FreeBSD           | `*‑freebsd`        | [`getrandom()`][21] if available, otherwise [`kern.arandom`][5]
//! | OpenBSD           | `*‑openbsd`        | [`getentropy`][6]
//! | NetBSD            | `*‑netbsd`         | [`kern.arandom`][7]
//! | Dragonfly BSD     | `*‑dragonfly`      | [`/dev/random`][8]
//! | Solaris, illumos  | `*‑solaris`, `*‑illumos` | [`getrandom()`][9] if available, otherwise [`/dev/random`][10]
//! | Fuchsia OS        | `*‑fuchsia`        | [`cprng_draw`][11]
//! | Redox             | `*‑redox`          | [`rand:`][12]
//! | Haiku             | `*‑haiku`          | `/dev/random` (identical to `/dev/urandom`)
//! | SGX               | `x86_64‑*‑sgx`     | [RDRAND][18]
//! | VxWorks           | `*‑wrs‑vxworks‑*`  | `randABytes` after checking entropy pool initialization with `randSecure`
//! | Emscripten        | `*‑emscripten`     | `/dev/random` (identical to `/dev/urandom`)
//! | WASI              | `wasm32‑wasi`      | [`__wasi_random_get`][17]
//! | Web Browser       | `wasm32‑*‑unknown` | [`Crypto.getRandomValues()`][14], see [WebAssembly support][16]
//! | Node.js           | `wasm32‑*‑unknown` | [`crypto.randomBytes`][15], see [WebAssembly support][16]
//!
//! There is no blanket implementation on `unix` targets that reads from
//! `/dev/urandom`. This ensures all supported targets are using the recommended
//! interface and respect maximum buffer sizes.
//!
//! Pull Requests that add support for new targets to `getrandom` are always welcome.
//!
//! ## Unsupported targets
//!
//! By default, `getrandom` will not compile on unsupported targets, but certain
//! features allow a user to select a "fallback" implementation if no supported
//! implementation exists.
//!
//! All of the below mechanisms only affect unsupported
//! targets. Supported targets will _always_ use their supported implementations.
//! This prevents a crate from overriding a secure source of randomness
//! (either accidentally or intentionally).
//!
//! ### RDRAND on x86
//!
//! *If the `"rdrand"` Cargo feature is enabled*, `getrandom` will fallback to using
//! the [`RDRAND`][18] instruction to get randomness on `no_std` `x86`/`x86_64`
//! targets. This feature has no effect on other CPU architectures.
//!
//! ### WebAssembly support
//!
//! This crate fully supports the
//! [`wasm32-wasi`](https://github.com/CraneStation/wasi) and
//! [`wasm32-unknown-emscripten`](https://www.hellorust.com/setup/emscripten/)
//! targets. However, the `wasm32-unknown-unknown` target is not automatically
//! supported since, from the target name alone, we cannot deduce which
//! JavaScript interface is in use (or if JavaScript is available at all).
//!
//! Instead, *if the `"js"` Cargo feature is enabled*, this crate will assume
//! that you are building for an environment containing JavaScript, and will
//! call the appropriate methods. Both web browser (main window and Web Workers)
//! and Node.js environments are supported, invoking the methods
//! [described above](#supported-targets) using the
//! [wasm-bindgen](https://github.com/rust-lang/rust-bindgen) toolchain.
//!
//! This feature has no effect on targets other than `wasm32-unknown-unknown`.
//!
//! ### Custom implementations
//!
//! The [`register_custom_getrandom!`] macro allows a user to mark their own
//! function as the backing implementation for [`getrandom`]. See the macro's
//! documentation for more information about writing and registering your own
//! custom implementations.
//!
//! Note that registering a custom implementation only has an effect on targets
//! that would otherwise not compile. Any supported targets (including those
//! using `"rdrand"` and `"js"` Cargo features) continue using their normal
//! implementations even if a function is registered.
//!
//! ### Indirect Dependencies
//!
//! If `getrandom` is not a direct dependency of your crate, you can still
//! enable any of the above fallback behaviors by enabling the relevant
//! feature in your root crate's `Cargo.toml`:
//! ```toml
//! [dependencies]
//! getrandom = { version = "0.2", features = ["js"] }
//! ```
//!
//! ## Early boot
//!
//! Sometimes, early in the boot process, the OS has not collected enough
//! entropy to securely seed its RNG. This is especially common on virtual
//! machines, where standard "random" events are hard to come by.
//!
//! Some operating system interfaces always block until the RNG is securely
//! seeded. This can take anywhere from a few seconds to more than a minute.
//! A few (Linux, NetBSD and Solaris) offer a choice between blocking and
//! getting an error; in these cases, we always choose to block.
//!
//! On Linux (when the `getrandom` system call is not available), reading from
//! `/dev/urandom` never blocks, even when the OS hasn't collected enough
//! entropy yet. To avoid returning low-entropy bytes, we first poll
//! `/dev/random` and only switch to `/dev/urandom` once this has succeeded.
//!
//! ## Error handling
//!
//! We always choose failure over returning insecure "random" bytes. In general,
//! on supported platforms, failure is highly unlikely, though not impossible.
//! If an error does occur, then it is likely that it will occur on every call to
//! `getrandom`, hence after the first successful call one can be reasonably
//! confident that no errors will occur.
//!
//! [1]: http://man7.org/linux/man-pages/man2/getrandom.2.html
//! [2]: http://man7.org/linux/man-pages/man4/urandom.4.html
//! [3]: https://docs.microsoft.com/en-us/windows/win32/api/bcrypt/nf-bcrypt-bcryptgenrandom
//! [4]: https://developer.apple.com/documentation/security/1399291-secrandomcopybytes?language=objc
//! [5]: https://www.freebsd.org/cgi/man.cgi?query=random&sektion=4
//! [6]: https://man.openbsd.org/getentropy.2
//! [7]: https://netbsd.gw.com/cgi-bin/man-cgi?sysctl+7+NetBSD-8.0
//! [8]: https://leaf.dragonflybsd.org/cgi/web-man?command=random&section=4
//! [9]: https://docs.oracle.com/cd/E88353_01/html/E37841/getrandom-2.html
//! [10]: https://docs.oracle.com/cd/E86824_01/html/E54777/random-7d.html
//! [11]: https://fuchsia.dev/fuchsia-src/zircon/syscalls/cprng_draw
//! [12]: https://github.com/redox-os/randd/blob/master/src/main.rs
//! [14]: https://www.w3.org/TR/WebCryptoAPI/#Crypto-method-getRandomValues
//! [15]: https://nodejs.org/api/crypto.html#crypto_crypto_randombytes_size_callback
//! [16]: #webassembly-support
//! [17]: https://github.com/WebAssembly/WASI/blob/master/design/WASI-core.md#__wasi_random_get
//! [18]: https://software.intel.com/en-us/articles/intel-digital-random-number-generator-drng-software-implementation-guide
//! [19]: https://www.unix.com/man-page/mojave/2/getentropy/
//! [20]: https://www.unix.com/man-page/mojave/4/random/
//! [21]: https://www.freebsd.org/cgi/man.cgi?query=getrandom&manpath=FreeBSD+12.0-stable

#![doc(
    html_logo_url = "https://www.rust-lang.org/logos/rust-logo-128x128-blk.png",
    html_favicon_url = "https://www.rust-lang.org/favicon.ico",
    html_root_url = "https://docs.rs/getrandom/0.2.2"
)]
#![no_std]
#![warn(rust_2018_idioms, unused_lifetimes, missing_docs)]
#![cfg_attr(docsrs, feature(doc_cfg))]

#[macro_use]
extern crate cfg_if;

mod error;
mod util;
// To prevent a breaking change when targets are added, we always export the
// register_custom_getrandom macro, so old Custom RNG crates continue to build.
#[cfg(feature = "custom")]
mod custom;
#[cfg(feature = "std")]
mod error_impls;

pub use crate::error::Error;

// System-specific implementations.
//
// These should all provide getrandom_inner with the same signature as getrandom.
cfg_if! {
    if #[cfg(any(target_os = "dragonfly", target_os = "emscripten",
                 target_os = "haiku",     target_os = "redox"))] {
        mod util_libc;
        #[path = "use_file.rs"] mod imp;
    } else if #[cfg(any(target_os = "android", target_os = "linux"))] {
        mod util_libc;
        mod use_file;
        #[path = "linux_android.rs"] mod imp;
    } else if #[cfg(any(target_os = "illumos", target_os = "solaris"))] {
        mod util_libc;
        mod use_file;
        #[path = "solaris_illumos.rs"] mod imp;
    } else if #[cfg(any(target_os = "freebsd", target_os = "netbsd"))] {
        mod util_libc;
        #[path = "bsd_arandom.rs"] mod imp;
    } else if #[cfg(target_os = "fuchsia")] {
        #[path = "fuchsia.rs"] mod imp;
    } else if #[cfg(target_os = "ios")] {
        #[path = "ios.rs"] mod imp;
    } else if #[cfg(target_os = "macos")] {
        mod util_libc;
        mod use_file;
        #[path = "macos.rs"] mod imp;
    } else if #[cfg(target_os = "openbsd")] {
        mod util_libc;
        #[path = "openbsd.rs"] mod imp;
    } else if #[cfg(target_os = "wasi")] {
        #[path = "wasi.rs"] mod imp;
    } else if #[cfg(target_os = "vxworks")] {
        mod util_libc;
        #[path = "vxworks.rs"] mod imp;
    } else if #[cfg(windows)] {
        #[path = "windows.rs"] mod imp;
    } else if #[cfg(all(target_arch = "x86_64", target_env = "sgx"))] {
        #[path = "rdrand.rs"] mod imp;
    } else if #[cfg(all(feature = "rdrand",
                        any(target_arch = "x86_64", target_arch = "x86")))] {
        #[path = "rdrand.rs"] mod imp;
    } else if #[cfg(all(feature = "js",
                        target_arch = "wasm32", target_os = "unknown"))] {
        #[path = "js.rs"] mod imp;
    } else if #[cfg(feature = "custom")] {
        use custom as imp;
    } else {
        compile_error!("target is not supported, for more information see: \
                        https://docs.rs/getrandom/#unsupported-targets");
    }
}

/// Fill `dest` with random bytes from the system's preferred random number
/// source.
///
/// This function returns an error on any failure, including partial reads. We
/// make no guarantees regarding the contents of `dest` on error. If `dest` is
/// empty, `getrandom` immediately returns success, making no calls to the
/// underlying operating system.
///
/// Blocking is possible, at least during early boot; see module documentation.
///
/// In general, `getrandom` will be fast enough for interactive usage, though
/// significantly slower than a user-space CSPRNG; for the latter consider
/// [`rand::thread_rng`](https://docs.rs/rand/*/rand/fn.thread_rng.html).
pub fn getrandom(dest: &mut [u8]) -> Result<(), Error> {
    if dest.is_empty() {
        return Ok(());
    }
    imp::getrandom_inner(dest)
}
