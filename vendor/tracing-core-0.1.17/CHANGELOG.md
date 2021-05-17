# 0.1.17 (September 28, 2020)

### Fixed

- Incorrect inlining of `Event::dispatch` and `Event::child_of`, which could
  result in `dispatcher::get_default` being inlined at the callsite ([#994])

### Added

- `Copy` implementations for `Level` and `LevelFilter` ([#992])

Thanks to new contributors @jyn514 and @TaKO8Ki for contributing to this 
release!

[#994]: https://github.com/tokio-rs/tracing/pull/994
[#992]: https://github.com/tokio-rs/tracing/pull/992

# 0.1.16 (September 8, 2020)

### Fixed

- Added a conversion from `Option<Level>` to `LevelFilter`. This resolves a
  previously unreported regression where `Option<Level>` was no longer
  a valid LevelFilter. ([#966](https://github.com/tokio-rs/tracing/pull/966))

# 0.1.15 (August 22, 2020)

### Fixed

- When combining `Interest` from multiple subscribers, if the interests differ,
  the current subscriber is now always asked if a callsite should be enabled
  (#927)

## Added

- Internal API changes to support optimizations in the `tracing` crate (#943)
- **docs**: Multiple fixes and improvements (#913, #941)

# 0.1.14 (August 10, 2020)

### Fixed

- Incorrect calculation of global max level filter which could result in fast
  filtering paths not being taken (#908)
  
# 0.1.13 (August 4, 2020)

### Fixed

- Missing `fmt::Display` impl for `field::DisplayValue` causing a compilation
  failure when the "log" feature is enabled (#887)
  
Thanks to @d-e-s-o for contributing to this release!

# 0.1.12 (July 31, 2020)

### Added

- `LevelFilter` type and `LevelFilter::current()` for returning the highest level
  that any subscriber will enable (#853)
- `Subscriber::max_level_hint` optional trait method, for setting the value
  returned by `LevelFilter::current()` (#853)
  
### Fixed

- **docs**: Removed outdated reference to a Tokio API that no longer exists
  (#857)

Thanks to new contributor @dignati for contributing to this release!

# 0.1.11 (June 8, 2020)

### Changed

- Replaced use of `inner_local_macros` with `$crate::` (#729)

### Added

- `must_use` warning to guards returned by `dispatcher::set_default` (#686)
- `fmt::Debug` impl to `dyn Value`s (#696) 
- Functions to convert between `span::Id` and `NonZeroU64` (#770)
- More obvious warnings in documentation (#769)

### Fixed

- Compiler error when `tracing-core/std` feature is enabled but `tracing/std` is
  not (#760)
- Clippy warning on vtable address comparison in `callsite::Identifier` (#749)
- Documentation formatting issues (#715, #771)

Thanks to @bkchr, @majecty, @taiki-e, @nagisa, and @nvzqz for contributing to
this release!

# 0.1.10 (January 24, 2020)

### Added

- `field::Empty` type for declaring empty fields whose values will be recorded
  later (#548)
- `field::Value` implementations for `Wrapping` and `NonZero*` numbers (#538)

### Fixed

- Broken and unresolvable links in RustDoc (#595)

Thanks to @oli-cosmian for contributing to this release!

# 0.1.9 (January 10, 2020)

### Added

- API docs now show what feature flags are required to enable each item (#523)

### Fixed

- A panic when the current default subscriber subscriber calls
  `dispatcher::with_default` as it is being dropped (#522)
- Incorrect documentation for `Subscriber::drop_span` (#524)

# 0.1.8 (December 20, 2019)

### Added

- `Default` impl for `Dispatch` (#411)

### Fixed

- Removed duplicate `lazy_static` dependencies (#424)
- Fixed no-std dependencies being enabled even when `std` feature flag is set
  (#424)
- Broken link to `Metadata` in `Event` docs (#461)

# 0.1.7 (October 18, 2019)

### Added

- Added `dispatcher::set_default` API which returns a drop guard (#388)

### Fixed

- Added missing `Value` impl for `u8` (#392)
- Broken links in docs.

# 0.1.7 (October 18, 2019)

### Added

- Added `dispatcher::set_default` API which returns a drop guard (#388)

### Fixed

- Added missing `Value` impl for `u8` (#392)
- Broken links in docs.

# 0.1.6 (September 12, 2019)

### Added

- Internal APIs to support performance optimizations (#326)

### Fixed

- Clarified wording in `field::display` documentation (#340)

# 0.1.6 (August 16, 2019)

### Added

- `std::error::Error` as a new primitive `Value` type (#277)
- `Event::new` and `Event::new_child_of` to manually construct `Event`s (#281)

# 0.1.4 (August 9, 2019)

### Added

- Support for `no-std` + `liballoc` (#256)

### Fixed

- Broken links in RustDoc (#259)

# 0.1.3 (August 8, 2019)

### Added

- `std::fmt::Display` implementation for `Level` (#194)
- `std::str::FromStr` implementation for `Level` (#195)

# 0.1.2 (July 10, 2019)

### Deprecated

- `Subscriber::drop_span` in favor of new `Subscriber::try_close` (#168)

### Added

- `Into<Option<&Id>>`, `Into<Option<Id>>`, and
  `Into<Option<&'static Metadata<'static>>>` impls for `span::Current` (#170)
- `Subscriber::try_close` method (#153)
- Improved documentation for `dispatcher` (#171)

# 0.1.1 (July 6, 2019)

### Added

- `Subscriber::current_span` API to return the current span (#148).
- `span::Current` type, representing the `Subscriber`'s view of the current
  span (#148).

### Fixed

- Typos and broken links in documentation (#123, #124, #128, #154)

# 0.1.0 (June 27, 2019)

- Initial release
