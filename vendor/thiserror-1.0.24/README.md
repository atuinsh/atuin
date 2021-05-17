derive(Error)
=============

[<img alt="github" src="https://img.shields.io/badge/github-dtolnay/thiserror-8da0cb?style=for-the-badge&labelColor=555555&logo=github" height="20">](https://github.com/dtolnay/thiserror)
[<img alt="crates.io" src="https://img.shields.io/crates/v/thiserror.svg?style=for-the-badge&color=fc8d62&logo=rust" height="20">](https://crates.io/crates/thiserror)
[<img alt="docs.rs" src="https://img.shields.io/badge/docs.rs-thiserror-66c2a5?style=for-the-badge&labelColor=555555&logoColor=white&logo=data:image/svg+xml;base64,PHN2ZyByb2xlPSJpbWciIHhtbG5zPSJodHRwOi8vd3d3LnczLm9yZy8yMDAwL3N2ZyIgdmlld0JveD0iMCAwIDUxMiA1MTIiPjxwYXRoIGZpbGw9IiNmNWY1ZjUiIGQ9Ik00ODguNiAyNTAuMkwzOTIgMjE0VjEwNS41YzAtMTUtOS4zLTI4LjQtMjMuNC0zMy43bC0xMDAtMzcuNWMtOC4xLTMuMS0xNy4xLTMuMS0yNS4zIDBsLTEwMCAzNy41Yy0xNC4xIDUuMy0yMy40IDE4LjctMjMuNCAzMy43VjIxNGwtOTYuNiAzNi4yQzkuMyAyNTUuNSAwIDI2OC45IDAgMjgzLjlWMzk0YzAgMTMuNiA3LjcgMjYuMSAxOS45IDMyLjJsMTAwIDUwYzEwLjEgNS4xIDIyLjEgNS4xIDMyLjIgMGwxMDMuOS01MiAxMDMuOSA1MmMxMC4xIDUuMSAyMi4xIDUuMSAzMi4yIDBsMTAwLTUwYzEyLjItNi4xIDE5LjktMTguNiAxOS45LTMyLjJWMjgzLjljMC0xNS05LjMtMjguNC0yMy40LTMzLjd6TTM1OCAyMTQuOGwtODUgMzEuOXYtNjguMmw4NS0zN3Y3My4zek0xNTQgMTA0LjFsMTAyLTM4LjIgMTAyIDM4LjJ2LjZsLTEwMiA0MS40LTEwMi00MS40di0uNnptODQgMjkxLjFsLTg1IDQyLjV2LTc5LjFsODUtMzguOHY3NS40em0wLTExMmwtMTAyIDQxLjQtMTAyLTQxLjR2LS42bDEwMi0zOC4yIDEwMiAzOC4ydi42em0yNDAgMTEybC04NSA0Mi41di03OS4xbDg1LTM4Ljh2NzUuNHptMC0xMTJsLTEwMiA0MS40LTEwMi00MS40di0uNmwxMDItMzguMiAxMDIgMzguMnYuNnoiPjwvcGF0aD48L3N2Zz4K" height="20">](https://docs.rs/thiserror)
[<img alt="build status" src="https://img.shields.io/github/workflow/status/dtolnay/thiserror/CI/master?style=for-the-badge" height="20">](https://github.com/dtolnay/thiserror/actions?query=branch%3Amaster)

This library provides a convenient derive macro for the standard library's
[`std::error::Error`] trait.

[`std::error::Error`]: https://doc.rust-lang.org/std/error/trait.Error.html

```toml
[dependencies]
thiserror = "1.0"
```

*Compiler support: requires rustc 1.31+*

<br>

## Example

```rust
use thiserror::Error;

#[derive(Error, Debug)]
pub enum DataStoreError {
    #[error("data store disconnected")]
    Disconnect(#[from] io::Error),
    #[error("the data for key `{0}` is not available")]
    Redaction(String),
    #[error("invalid header (expected {expected:?}, found {found:?})")]
    InvalidHeader {
        expected: String,
        found: String,
    },
    #[error("unknown data store error")]
    Unknown,
}
```

<br>

## Details

- Thiserror deliberately does not appear in your public API. You get the same
  thing as if you had written an implementation of `std::error::Error` by hand,
  and switching from handwritten impls to thiserror or vice versa is not a
  breaking change.

- Errors may be enums, structs with named fields, tuple structs, or unit
  structs.

- A `Display` impl is generated for your error if you provide `#[error("...")]`
  messages on the struct or each variant of your enum, as shown above in the
  example.

  The messages support a shorthand for interpolating fields from the error.

    - `#[error("{var}")]`&ensp;⟶&ensp;`write!("{}", self.var)`
    - `#[error("{0}")]`&ensp;⟶&ensp;`write!("{}", self.0)`
    - `#[error("{var:?}")]`&ensp;⟶&ensp;`write!("{:?}", self.var)`
    - `#[error("{0:?}")]`&ensp;⟶&ensp;`write!("{:?}", self.0)`

  These shorthands can be used together with any additional format args, which
  may be arbitrary expressions. For example:

  ```rust
  #[derive(Error, Debug)]
  pub enum Error {
      #[error("invalid rdo_lookahead_frames {0} (expected < {})", i32::MAX)]
      InvalidLookahead(u32),
  }
  ```

  If one of the additional expression arguments needs to refer to a field of the
  struct or enum, then refer to named fields as `.var` and tuple fields as `.0`.

  ```rust
  #[derive(Error, Debug)]
  pub enum Error {
      #[error("first letter must be lowercase but was {:?}", first_char(.0))]
      WrongCase(String),
      #[error("invalid index {idx}, expected at least {} and at most {}", .limits.lo, .limits.hi)]
      OutOfBounds { idx: usize, limits: Limits },
  }
  ```

- A `From` impl is generated for each variant containing a `#[from]` attribute.

  Note that the variant must not contain any other fields beyond the source
  error and possibly a backtrace. A backtrace is captured from within the `From`
  impl if there is a field for it.

  ```rust
  #[derive(Error, Debug)]
  pub enum MyError {
      Io {
          #[from]
          source: io::Error,
          backtrace: Backtrace,
      },
  }
  ```

- The Error trait's `source()` method is implemented to return whichever field
  has a `#[source]` attribute or is named `source`, if any. This is for
  identifying the underlying lower level error that caused your error.

  The `#[from]` attribute always implies that the same field is `#[source]`, so
  you don't ever need to specify both attributes.

  Any error type that implements `std::error::Error` or dereferences to `dyn
  std::error::Error` will work as a source.

  ```rust
  #[derive(Error, Debug)]
  pub struct MyError {
      msg: String,
      #[source]  // optional if field name is `source`
      source: anyhow::Error,
  }
  ```

- The Error trait's `backtrace()` method is implemented to return whichever
  field has a type named `Backtrace`, if any.

  ```rust
  use std::backtrace::Backtrace;

  #[derive(Error, Debug)]
  pub struct MyError {
      msg: String,
      backtrace: Backtrace,  // automatically detected
  }
  ```

- Errors may use `error(transparent)` to forward the source and Display methods
  straight through to an underlying error without adding an additional message.
  This would be appropriate for enums that need an "anything else" variant.

  ```rust
  #[derive(Error, Debug)]
  pub enum MyError {
      ...

      #[error(transparent)]
      Other(#[from] anyhow::Error),  // source and Display delegate to anyhow::Error
  }
  ```

- See also the [`anyhow`] library for a convenient single error type to use in
  application code.

  [`anyhow`]: https://github.com/dtolnay/anyhow

<br>

## Comparison to anyhow

Use thiserror if you care about designing your own dedicated error type(s) so
that the caller receives exactly the information that you choose in the event of
failure. This most often applies to library-like code. Use [Anyhow] if you don't
care what error type your functions return, you just want it to be easy. This is
common in application-like code.

[Anyhow]: https://github.com/dtolnay/anyhow

<br>

#### License

<sup>
Licensed under either of <a href="LICENSE-APACHE">Apache License, Version
2.0</a> or <a href="LICENSE-MIT">MIT license</a> at your option.
</sup>

<br>

<sub>
Unless you explicitly state otherwise, any contribution intentionally submitted
for inclusion in this crate by you, as defined in the Apache-2.0 license, shall
be dual licensed as above, without any additional terms or conditions.
</sub>
