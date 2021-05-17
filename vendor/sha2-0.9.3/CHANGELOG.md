# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## 0.9.3 (2021-01-30)
### Changed
- Use the SHA extension backend with enabled `asm` feature. ([#224])

[#224]: https://github.com/RustCrypto/hashes/pull/224

## 0.9.2 (2020-11-04)
### Added
- `force-soft` feature to enforce use of software implementation. ([#203])

### Changed
- `cfg-if` dependency updated to v1.0. ([#197])

[#197]: https://github.com/RustCrypto/hashes/pull/197
[#203]: https://github.com/RustCrypto/hashes/pull/203

## 0.9.1 (2020-06-24)
### Added
- x86 hardware acceleration of SHA-256 via SHA extension instrinsics. ([#167])

[#167]: https://github.com/RustCrypto/hashes/pull/167

## 0.9.0 (2020-06-09)
### Changed
- Update to `digest` v0.9 release; MSRV 1.41+ ([#155])
- Use new `*Dirty` traits from the `digest` crate ([#153])
- Bump `block-buffer` to v0.8 release ([#151])
- Rename `*result*` to `finalize` ([#148])
- Upgrade to Rust 2018 edition ([#133])

[#155]: https://github.com/RustCrypto/hashes/pull/155
[#153]: https://github.com/RustCrypto/hashes/pull/153
[#151]: https://github.com/RustCrypto/hashes/pull/151
[#148]: https://github.com/RustCrypto/hashes/pull/148
[#133]: https://github.com/RustCrypto/hashes/pull/133

## 0.8.2 (2020-05-23)
### Added
- Expose compression function under the `compress` feature flag ([#108])

### Changed
- Use `libc` crate for `aarch64` consts ([#109])
- Minor code cleanups ([#94])

[#109]: https://github.com/RustCrypto/hashes/pull/109
[#108]: https://github.com/RustCrypto/hashes/pull/108
[#94]: https://github.com/RustCrypto/hashes/pull/94

## 0.8.1 (2020-01-05)

## 0.8.0 (2018-10-02)

## 0.7.1 (2018-04-27)

## 0.6.0 (2017-06-12)

## 0.5.3 (2017-06-03)

## 0.5.2 (2017-05-08)

## 0.5.1 (2017-05-01)

## 0.5.0 (2017-04-06)

## 0.4.2 (2017-01-23)

## 0.4.1 (2017-01-20)

## 0.4.0 (2016-12-24)

## 0.3.0 (2016-11-17)

## 0.2.0 (2016-10-26)

## 0.1.2 (2016-05-06)

## 0.1.1 (2016-05-06)

## 0.1.0 (2016-05-06)
