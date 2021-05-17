# Changelog
All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](http://keepachangelog.com/en/1.0.0/)
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [0.1.16] - 2020-12-31
### Changed
- Update `cfg-if` to v1.0. [#173]
- Implement `std::error::Error` for the `Error` type on additional targets. [#169]

### Fixed
- Multithreaded WASM support. [#171]

[#173]: https://github.com/rust-random/getrandom/pull/173
[#171]: https://github.com/rust-random/getrandom/pull/171
[#169]: https://github.com/rust-random/getrandom/pull/169

## [0.1.15] - 2020-09-10
### Changed
- Added support for Internet Explorer 11 [#139]
- Fix Webpack require warning with `wasm-bindgen` [#137]

[#137]: https://github.com/rust-random/getrandom/pull/137
[#139]: https://github.com/rust-random/getrandom/pull/139

## [0.1.14] - 2020-01-07
### Changed
- Remove use of spin-locks in the `use_file` module. [#125]
- Update `wasi` to v0.9. [#126]
- Do not read errno value on DragonFlyBSD to fix compilation failure. [#129]

[#125]: https://github.com/rust-random/getrandom/pull/125
[#126]: https://github.com/rust-random/getrandom/pull/126
[#129]: https://github.com/rust-random/getrandom/pull/129

## [0.1.13] - 2019-08-25
### Added
- VxWorks targets support. [#86]

### Changed
- If zero-length slice is passed to the `getrandom` function, always return
`Ok(())` immediately without doing any calls to the underlying operating
system. [#104]
- Use the `kern.arandom` sysctl on NetBSD. [#115]

### Fixed
- Bump `cfg-if` minimum version from 0.1.0 to 0.1.2. [#112]
- Typos and bad doc links. [#117]

[#86]: https://github.com/rust-random/getrandom/pull/86
[#104]: https://github.com/rust-random/getrandom/pull/104
[#112]: https://github.com/rust-random/getrandom/pull/112
[#115]: https://github.com/rust-random/getrandom/pull/115
[#117]: https://github.com/rust-random/getrandom/pull/117

## [0.1.12] - 2019-08-18
### Changed
- Update wasi dependency from v0.5 to v0.7. [#100]

[#100]: https://github.com/rust-random/getrandom/pull/100

## [0.1.11] - 2019-08-25
### Fixed
- Implement `std`-dependent traits for selected targets even if `std`
feature is disabled. (backward compatibility with v0.1.8) [#96]

[#96]: https://github.com/rust-random/getrandom/pull/96

## [0.1.10] - 2019-08-18 [YANKED]
### Changed
- Use the dummy implementation on `wasm32-unknown-unknown` even with the
disabled `dummy` feature. [#90]

### Fixed
- Fix CSP error for `wasm-bindgen`. [#92]

[#90]: https://github.com/rust-random/getrandom/pull/90
[#92]: https://github.com/rust-random/getrandom/pull/92

## [0.1.9] - 2019-08-14 [YANKED]
### Changed
- Remove `std` dependency for opening and reading files. [#58]
- Use `wasi` isntead of `libc` on WASI target. [#64]
- By default emit a compile-time error when built for an unsupported target.
This behaviour can be disabled by using the `dummy` feature. [#71]

### Added
- Add support for UWP targets. [#69]
- Add unstable `rustc-dep-of-std` feature. [#78]

[#58]: https://github.com/rust-random/getrandom/pull/58
[#64]: https://github.com/rust-random/getrandom/pull/64
[#69]: https://github.com/rust-random/getrandom/pull/69
[#71]: https://github.com/rust-random/getrandom/pull/71
[#78]: https://github.com/rust-random/getrandom/pull/78

## [0.1.8] - 2019-07-29
### Changed
- Explicitly specify types to arguments of 'libc::syscall'. [#74]

[#74]: https://github.com/rust-random/getrandom/pull/74

## [0.1.7] - 2019-07-29
### Added
- Support for hermit and l4re. [#61]
- `Error::raw_os_error` method, `Error::INTERNAL_START` and
`Error::CUSTOM_START` constants. Use `libc` for retrieving OS error descriptions. [#54]

### Changed
- Remove `lazy_static` dependency and use custom structures for lock-free
initialization. [#51] [#52]
- Try `getrandom()` first on FreeBSD. [#57]

### Removed
-  Bitrig support. [#56]

### Deprecated
- `Error::UNKNOWN`, `Error::UNAVAILABLE`. [#54]

[#51]: https://github.com/rust-random/getrandom/pull/51
[#52]: https://github.com/rust-random/getrandom/pull/52
[#54]: https://github.com/rust-random/getrandom/pull/54
[#56]: https://github.com/rust-random/getrandom/pull/56
[#57]: https://github.com/rust-random/getrandom/pull/57
[#61]: https://github.com/rust-random/getrandom/pull/61

## [0.1.6] - 2019-06-30
### Changed
- Minor change of RDRAND AMD bug handling. [#48]

[#48]: https://github.com/rust-random/getrandom/pull/48

## [0.1.5] - 2019-06-29
### Fixed
- Use shared `File` instead of shared file descriptor. [#44]
- Workaround for RDRAND hardware bug present on some AMD CPUs. [#43]

### Changed
- Try `getentropy` and then fallback to `/dev/random` on macOS. [#38]

[#38]: https://github.com/rust-random/getrandom/issues/38
[#43]: https://github.com/rust-random/getrandom/pull/43
[#44]: https://github.com/rust-random/getrandom/issues/44

## [0.1.4] - 2019-06-28
### Added
- Add support for `x86_64-unknown-uefi` target by using RDRAND with CPUID
feature detection. [#30]

### Fixed
- Fix long buffer issues on Windows and Linux. [#31] [#32]
- Check `EPERM` in addition to `ENOSYS` on Linux. [#37]

### Changed
- Improve efficiency by sharing file descriptor across threads. [#13]
- Remove `cloudabi`, `winapi`, and `fuchsia-cprng` dependencies. [#40]
- Improve RDRAND implementation. [#24]
- Don't block during syscall detection on Linux. [#26]
- Increase consistency with libc implementation on FreeBSD. [#36]
- Apply `rustfmt`. [#39]

[#30]: https://github.com/rust-random/getrandom/pull/30
[#13]: https://github.com/rust-random/getrandom/issues/13
[#40]: https://github.com/rust-random/getrandom/pull/40
[#26]: https://github.com/rust-random/getrandom/pull/26
[#24]: https://github.com/rust-random/getrandom/pull/24
[#39]: https://github.com/rust-random/getrandom/pull/39
[#36]: https://github.com/rust-random/getrandom/pull/36
[#31]: https://github.com/rust-random/getrandom/issues/31
[#32]: https://github.com/rust-random/getrandom/issues/32
[#37]: https://github.com/rust-random/getrandom/issues/37

## [0.1.3] - 2019-05-15
- Update for `wasm32-unknown-wasi` being renamed to `wasm32-wasi`, and for
  WASI being categorized as an OS.

## [0.1.2] - 2019-04-06
- Add support for `wasm32-unknown-wasi` target.

## [0.1.1] - 2019-04-05
- Enable std functionality for CloudABI by default.

## [0.1.0] - 2019-03-23
Publish initial implementation.

## [0.0.0] - 2019-01-19
Publish an empty template library.
