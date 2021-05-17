# Change Log
All notable changes to this project will be documented in this file.
This project adheres to [Semantic Versioning](http://semver.org/).

## [Unreleased][unreleased]
### Added:
- Generic `decode::from_read_ref` function that allows to deserialize a borrowed byte-array into the specified type.
- Add `Ext` trait for `Serializer` that allows to wrap a serializer with another one, that overrides exactly one serialization policy. For example using `with_struct_map` method it is possible to serialize structs as a MessagePack map with field names, overriding default serialization policy, which emits structs as a tuple.
- Add `UnderlyingWrite` trait for `Serializer` and its wrappers to be able to obtain the underlying writer.
- Add missing `Debug` implementations.
- More `decode::Error` conversions.
- Support for serializing and deserializing 128-bit values in serde.
- Support for serializing sequences and maps with unknown length, that enables the use of `#[serde(flatten)]` attribute (#196).

### Changed:
- (Breaking) Serialize newtype structs by serializing its inner type without wrapping into a tuple.
- (Breaking) Enums are now encoded as a map `{tag: data}` rather than as a list `[tag, data]`. (#149)
- Function `encode::to_vec_named` now accepts unsized values.
- Renamed `decode::Read` trait to `decode::ReadSlice` to avoid clashing with `std::io::Read` and to specify more precisely what it does.
- Support reading encoded integers as floats when safe (#204)

### Removed:
- Type parameter `VariantWriter` is no longer a type member of `Serializer`. Instead a `Serializer` can be wrapped by another serializer using `with_struct_map`, `with_struct_tuple` etc. methods.

### Fixed:
- Fix error decoding `Some(enum)` (#185)
- Fix error decoding unit structs which were encoded as `[]` (#181)
- Fix `Display` implementations for errors not including all relevant information (#199)
- Fix deserialization of nested `Option`s (#245)

## 0.13.7 - 2017-09-13
### Changed:
- `Raw` and `RawRef` are now serializable.
- Allow to construct `Raw` and `RawRef` from string or from a byte array.

## 0.13.6 - 2017-08-04
### Added:
- Serialize struct as a map (#140).

## 0.13.5 - 2017-07-21
### Changed
- Switch to using `char::encode_utf8`.
  In Rust 1.15, the function `char::encode_utf8` was stabilized. Assuming that `rmp` follows the `serde` standard of supporting the last 3 stable releases, this function is now safe to use. I believe this removes the last allocation required on the serialization path.

## 0.13.4 - 2017-07-11
### Fixed
- Fixed build on nightly rustc (#135).

## 0.13.3 - 2017-05-27
### Fixed
- Fixed build on nightly rustc (#131).

## 0.13.2 - 2017-04-30
### Changed
- Fixed `rmps::decode::from_read` signature by marking that it can only deserialize into `DeserializeOwned`. The previous signature let try to deserialize, for example `&str` and other borrow types and it failed at runtime instead of catching it at compile time.

## 0.13.1 - 2017-04-25
### Added
- Add helper `RawRef` struct that allows to deserialize borrowed strings even if they contain invalid UTF-8. This can be when deserializing frames from older MessagePack spec.

## 0.13.0 - 2017-04-24
### Added
- Zero-copy deserialization from `&[u8]`.

### Changed
- Adapt with serde 1.0.

## 0.12.4 - 2017-03-26
### Fixed
- Fix compilation on rustc 1.13.

## 0.12.3 - 2017-03-26
### Added
- Add helper `Raw` struct that allows to deserialize strings even if they contain invalid UTF-8. This can be when deserializing frames from older MessagePack spec.
- Serializer can now return back its underlying writer by reference, mutable reference and by value.

## 0.12.2 - 2017-02-17
### Added
- Added `write`, `to_vec` and `from_read` functions to reduce boilerplate for serializing and deserializing custom types that implement `Serialize` or `Deserialize`.

## 0.12.1 - 2017-02-11
### Added
- Allow `Deserializer` to return number of bytes read in case of using Cursor as an underlying reader.

## 0.12.0 - 2017-02-08
### Changed
- Adapt with serde 0.9.

## 0.11.0 - 2017-01-05
### Changed
- Adapt with RMP core 0.8.
- The `Serializer` now encodes integers using the most effective representation.
- The `Deserializer` now properly decodes integer values that fit in the expected type.
- Default stack protector depth is now 1024 instead of 1000.
- Internal buffer in the `Deserializer` now have some capacity preallocated.

## 0.10.0 - 2016-10-06
### Changed
- Update serde dependency to 0.8.

## 0.9.6 - 2016-08-05
### Fixed
- Switch unit structs to using the same serialization mechanism as other structs (#76).

## 0.9.5 - 2016-07-28
### Added
- Added a wrapper over `rmp::Value` to be able to serialize it.

## 0.9.4 - 2016-07-11
### Fixed
- Reading binary should no longer trigger unexpected EOF error on valid read.

## 0.9.3 - 2016-07-11
### Changed
- Reuse deserializer buffer on every read for string and binary deserialization without unnecessary intermediate buffer creation.
  This change increases the string and binary deserialization performance (many thanks to Fedor Gogolev <knsd@knsd.net>).

## 0.9.2 - 2016-07-03
### Added
- Implement `size_hint()` function for `SeqVisitor` and `MapVisitor`, so it can be possible to preallocate things, increasing the performance greatly.

## 0.9.1 - 2016-06-24
### Fixed
- Serializer should no longer panic with unimplemented error on struct variant serialization ([#64]).

## 0.9.0 - 2016-03-28
### Changed
- Adapt code to be compilable with Serde v0.7.

## 0.8.2 - 2015-11-10
### Changed
- Fixed stack overflow when unpacking recursive data structures.

## 0.8.1 - 2015-10-03
### Changed
- Upper limit for serde version.

### Fixed
- Use the most effective int encoding
  Even if the value is explicitly marked as i64 it must be encoded using
  the most effective bytes representation despite of signed it or
  unsigned.

## 0.8.0 - 2015-09-11
### Changed
- Serializer can now be extended with custom struct encoding policy.
- Improved error types and its messages for serialization part.
    - New error type introduced - UnknownLength. Returned on attempt to serialize struct, map or serquence with unknown
    length (Serde allows this).
    - The new type is returned if necessary.

### Fixed
- Deserializer now properly works with enums.
- Options with default values (that can be initialized using unit marker) deserialization.
  This fix also forbids the following Option deserialization cases:
    - Option<()>.
    - Option<Option<...>>.
  It's impossible to properly deserialize the listed cases without explicit option marker in protocol.
- Serializer now properly serializes unit structs.
  Previously it was serialized as a unit (nil), now there is just an empty array ([]).

[#64]: (https://github.com/3Hren/msgpack-rust/pull/64)
[#76]: (https://github.com/3Hren/msgpack-rust/pull/76)
