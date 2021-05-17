# hyper-rustls
This is an integration between the [rustls TLS stack](https://github.com/ctz/rustls)
and the [hyper HTTP library](https://github.com/hyperium/hyper).

[![Build Status](https://github.com/ctz/hyper-rustls/workflows/hyper-rustls/badge.svg)](https://github.com/ctz/hyper-rustls/actions)
[![Build Status](https://dev.azure.com/ctz99/ctz/_apis/build/status/ctz.hyper-rustls?branchName=master)](https://dev.azure.com/ctz99/ctz/_build/latest?definitionId=4&branchName=master)
[![Crate](https://img.shields.io/crates/v/hyper-rustls.svg)](https://crates.io/crates/hyper-rustls)
[![Documentation](https://docs.rs/hyper-rustls/badge.svg)](https://docs.rs/hyper-rustls/)

# Release history
- 0.22.1 (2020-12-27):
  * Fixing docs.rs build; no other changes.
- 0.22.0 (2020-12-26):
  * Use tokio 1.0, hyper 0.14, and rustls 0.19. Thanks to @paolobarbolini and @messense.
  * Rework how the certificate store is chosen: now by an explicit API rather than
    implicitly by crate features. Thanks to @djc.
- 0.21.0 (2020-07-05):
  * Update dependencies.
- 0.20.0 (2020-02-24):
  * Use newer rustls-native-certs which works in presence of invalid certificates.
  * Update dependencies.
- 0.19.1 (2020-01-19):
  * Remove dependency on hyper's tcp feature.
- 0.19.0 (2019-12-17):
  * First release with async/await support.  Many thanks to @CryZe, @alex, @markuskobler and @dbcfd.
- 0.18.0 (2019-11-23)
  * Uses [rustls-native-certs](https://crates.io/crates/rustls-native-certs)
    instead of compiled-in root certificates.
- 0.17.1 (2019-08-19)
  * Fix accidental use of sync read/write.
- 0.17.0 (2019-08-11)
  * Update dependencies.

# License
hyper-rustls is distributed under the following three licenses:

- Apache License version 2.0.
- MIT license.
- ISC license.

These are included as LICENSE-APACHE, LICENSE-MIT and LICENSE-ISC
respectively.  You may use this software under the terms of any
of these licenses, at your option.

