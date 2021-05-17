# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## 0.10.0 (2020-10-15)
### Changed
- Replace `block-cipher` crate with new `cipher` crate ([#337], [#338])

[#338]: https://github.com/RustCrypto/traits/pull/338
[#337]: https://github.com/RustCrypto/traits/pull/337

## 0.9.1 (2020-08-12)
### Added
- Re-export the `block-cipher` crate ([#257])

[#257]: https://github.com/RustCrypto/traits/pull/257

## 0.9.0 (2020-08-10)
### Added
- `FromBlockCipher` trait and blanket implementation of the `NewMac` trait
for it ([#217])

### Changed
- Updated test vectors storage to `blobby v0.3` ([#217])

### Removed
- `impl_write!` macro ([#217])

[#217]: https://github.com/RustCrypto/traits/pull/217

## 0.8.0 (2020-06-04)
### Added
- `impl_write!` macro ([#134])

### Changed
- Bump `generic-array` dependency to v0.14 ([#144])
- Split `Mac` initialization into `NewMac` trait ([#133])
- Rename `MacResult` => `Output`, `code` => `into_bytes` ([#114])
- Rename `Input::input` to `Update::update` ([#111])
- Update to 2018 edition ([#108])
- Bump `subtle` dependency from v1.0 to v2.0 ([#33])

[#144]: https://github.com/RustCrypto/traits/pull/95
[#134]: https://github.com/RustCrypto/traits/pull/134
[#133]: https://github.com/RustCrypto/traits/pull/133
[#114]: https://github.com/RustCrypto/traits/pull/114
[#111]: https://github.com/RustCrypto/traits/pull/111
[#108]: https://github.com/RustCrypto/traits/pull/108
[#33]: https://github.com/RustCrypto/traits/pull/33

## 0.7.0 (2018-10-01)

## 0.6.2 (2018-06-21)

## 0.6.1 (2018-06-20)

## 0.6.0 (2017-11-26)

## 0.5.2 (2017-11-20)

## 0.5.1 (2017-11-15)

## 0.5.0 (2017-11-14)

## 0.4.0 (2017-06-12)

## 0.3.0 (2017-05-14)

## 0.2.0 (2017-05-14)

## 0.1.0 (2016-10-14)
