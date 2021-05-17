# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## 0.9.4 (2021-02-16)
### Added
- Expose compression function under the `compress` feature flag. ([#238])

[#238]: https://github.com/RustCrypto/hashes/pull/238

## 0.9.3 (2021-02-01)
### Changed
- Use SHA1 intrinsics when `asm` feature is enabled. ([#225])

[#225]: https://github.com/RustCrypto/hashes/pull/225

## 0.9.2 (2020-11-04)
### Added
- `force-soft` feature to enforce use of software implementation. ([#203])

### Changed
- `cfg-if` dependency updated to v1.0. ([#197])

[#197]: https://github.com/RustCrypto/hashes/pull/197
[#203]: https://github.com/RustCrypto/hashes/pull/203

## 0.9.1 (2020-06-24)
### Added
- x86 hardware acceleration via SHA extension instrinsics. ([#167])

[#167]: https://github.com/RustCrypto/hashes/pull/167

## 0.9.0 (2020-06-09)
### Changed
- Update to `digest` v0.9 release; MSRV 1.41+ ([#155])
- Use new `*Dirty` traits from the `digest` crate ([#153])
- Bump `block-buffer` to v0.8 release ([#151])
- Rename `*result*` to `finalize` ([#148])
- Upgrade to Rust 2018 edition ([#132])
- Use `libc` for `aarch64` consts ([#94])
- Allow compile-time detection of crypto on `aarch64` ([#94])

[#155]: https://github.com/RustCrypto/hashes/pull/155
[#153]: https://github.com/RustCrypto/hashes/pull/153
[#151]: https://github.com/RustCrypto/hashes/pull/151
[#148]: https://github.com/RustCrypto/hashes/pull/148
[#132]: https://github.com/RustCrypto/hashes/pull/132
[#94]: https://github.com/RustCrypto/hashes/pull/94

## 0.8.2 (2020-01-06)

## 0.8.1 (2018-11-14)

## 0.8.0 (2018-10-02)

## 0.7.0 (2017-11-15)

## 0.4.1 (2017-06-13)

## 0.4.0 (2017-06-12)

## 0.3.4 (2017-06-04)

## 0.3.3 (2017-05-09)

## 0.3.2 (2017-05-02)

## 0.3.1 (2017-04-18)

## 0.3.0 (2017-04-06)
