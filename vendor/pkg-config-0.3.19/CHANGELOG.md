# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](http://keepachangelog.com/en/1.0.0/)
and this project adheres to [Semantic Versioning](http://semver.org/spec/v2.0.0.html).

## [0.3.19] - 2020-10-13

### Added

- Add `README.md` to be displayed on crates.io (#111).

- Support for `-isystem`, `-iquote` and `-idirafter` include flags (#115).

### Changed

- Improve documentation for cross-compilation (#113).

- Allow overriding system root via the `PKG_CONFIG_SYSROOT_DIR` or `SYSROOT`
  environment variable (#82).

## [0.3.18] - 2020-07-11

### Fixed

- Use `env::var_os()` almost everywhere to handle non-UTF8 paths in
  environment variables, and also improve error handling around environment
  variable handling (#106).

### Changed

- Default the `env_metadata` build parameter to `true` instead of `false`.
  Whenever a pkg-config related environment variable changes it would make
  sense to rebuild crates that use pkg-config, or otherwise changes might not
  be picked up. As such the previous default didn't make much sense (#105).

## [0.3.17] - 2019-11-02

### Fixed

- Fix support for multiple version number constraints (#95)

## [0.3.16] - 2019-09-09

### Changed
- Stop using deprecated functions and require Rust 1.30 (#84)

### Fixed
- Fix repository URL in README.md
- Fix various clippy warnings

### Added
- Run `cargo fmt` as part of the CI (#89)
- Derive `Clone` for `Library` and `Debug` for `Config (#91)
- Add support for `PKG_CONFIG_ALLOW_SYSTEM_CFLAGS` and enable by default (#93)

## [0.3.15] - 2019-07-25

### Changed
- Changes minimum documented rust version to 1.28 (#76)

### Fixed
- Fix Travis CI badge url (#78)
- Fix project name in README.md (#81)

### Added
- Support specifying range of versions (#75)
- Allow cross-compilation if pkg-config is customized (#44, #86)

## [0.3.14] - 2018-08-28

### Fixed
- Don't append .lib suffix on MSVC builds (#72)

## [0.3.13] - 2018-08-06

### Fixed
- Fix MSVC support to actually work and consider library paths too (#71)

## [0.3.12] - 2018-06-18

### Added
- Support for MSVC (#70)
- Document and test Rust 1.13 as minimally supported version (#66)

## [0.3.11] - 2018-04-24

### Fixed
- Re-added AsciiExt import (#65)

## [0.3.10] - 2018-04-23

### Added
- Allow static linking of /usr/ on macOS (#42)
- Add support for parsing `-Wl,` style framework flags (#48)
- Parse defines in `pkg-config` output (#49)
- Rerun on `PKG_CONFIG_PATH` changes (#50)
- Introduce target-scoped variables (#58)
- Respect pkg-config escaping rules used with --cflags and --libs (#61)

### Changed
- Use `?` instead of `try!()` in the codebase (#63)
