itoa
====

[<img alt="github" src="https://img.shields.io/badge/github-dtolnay/itoa-8da0cb?style=for-the-badge&labelColor=555555&logo=github" height="20">](https://github.com/dtolnay/itoa)
[<img alt="crates.io" src="https://img.shields.io/crates/v/itoa.svg?style=for-the-badge&color=fc8d62&logo=rust" height="20">](https://crates.io/crates/itoa)
[<img alt="docs.rs" src="https://img.shields.io/badge/docs.rs-itoa-66c2a5?style=for-the-badge&labelColor=555555&logoColor=white&logo=data:image/svg+xml;base64,PHN2ZyByb2xlPSJpbWciIHhtbG5zPSJodHRwOi8vd3d3LnczLm9yZy8yMDAwL3N2ZyIgdmlld0JveD0iMCAwIDUxMiA1MTIiPjxwYXRoIGZpbGw9IiNmNWY1ZjUiIGQ9Ik00ODguNiAyNTAuMkwzOTIgMjE0VjEwNS41YzAtMTUtOS4zLTI4LjQtMjMuNC0zMy43bC0xMDAtMzcuNWMtOC4xLTMuMS0xNy4xLTMuMS0yNS4zIDBsLTEwMCAzNy41Yy0xNC4xIDUuMy0yMy40IDE4LjctMjMuNCAzMy43VjIxNGwtOTYuNiAzNi4yQzkuMyAyNTUuNSAwIDI2OC45IDAgMjgzLjlWMzk0YzAgMTMuNiA3LjcgMjYuMSAxOS45IDMyLjJsMTAwIDUwYzEwLjEgNS4xIDIyLjEgNS4xIDMyLjIgMGwxMDMuOS01MiAxMDMuOSA1MmMxMC4xIDUuMSAyMi4xIDUuMSAzMi4yIDBsMTAwLTUwYzEyLjItNi4xIDE5LjktMTguNiAxOS45LTMyLjJWMjgzLjljMC0xNS05LjMtMjguNC0yMy40LTMzLjd6TTM1OCAyMTQuOGwtODUgMzEuOXYtNjguMmw4NS0zN3Y3My4zek0xNTQgMTA0LjFsMTAyLTM4LjIgMTAyIDM4LjJ2LjZsLTEwMiA0MS40LTEwMi00MS40di0uNnptODQgMjkxLjFsLTg1IDQyLjV2LTc5LjFsODUtMzguOHY3NS40em0wLTExMmwtMTAyIDQxLjQtMTAyLTQxLjR2LS42bDEwMi0zOC4yIDEwMiAzOC4ydi42em0yNDAgMTEybC04NSA0Mi41di03OS4xbDg1LTM4Ljh2NzUuNHptMC0xMTJsLTEwMiA0MS40LTEwMi00MS40di0uNmwxMDItMzguMiAxMDIgMzguMnYuNnoiPjwvcGF0aD48L3N2Zz4K" height="20">](https://docs.rs/itoa)
[<img alt="build status" src="https://img.shields.io/github/workflow/status/dtolnay/itoa/CI/master?style=for-the-badge" height="20">](https://github.com/dtolnay/itoa/actions?query=branch%3Amaster)

This crate provides fast functions for printing integer primitives to an
[`io::Write`] or a [`fmt::Write`]. The implementation comes straight from
[libcore] but avoids the performance penalty of going through
[`fmt::Formatter`].

See also [`dtoa`] for printing floating point primitives.

*Version requirement: rustc 1.0+*

[`io::Write`]: https://doc.rust-lang.org/std/io/trait.Write.html
[`fmt::Write`]: https://doc.rust-lang.org/core/fmt/trait.Write.html
[libcore]: https://github.com/rust-lang/rust/blob/b8214dc6c6fc20d0a660fb5700dca9ebf51ebe89/src/libcore/fmt/num.rs#L201-L254
[`fmt::Formatter`]: https://doc.rust-lang.org/std/fmt/struct.Formatter.html
[`dtoa`]: https://github.com/dtolnay/dtoa

```toml
[dependencies]
itoa = "0.4"
```

<br>

## Performance (lower is better)

![performance](https://raw.githubusercontent.com/dtolnay/itoa/master/performance.png)

<br>

## Examples

```rust
use std::{fmt, io};

fn demo_itoa_write() -> io::Result<()> {
    // Write to a vector or other io::Write.
    let mut buf = Vec::new();
    itoa::write(&mut buf, 128u64)?;
    println!("{:?}", buf);

    // Write to a stack buffer.
    let mut bytes = [0u8; 20];
    let n = itoa::write(&mut bytes[..], 128u64)?;
    println!("{:?}", &bytes[..n]);

    Ok(())
}

fn demo_itoa_fmt() -> fmt::Result {
    // Write to a string.
    let mut s = String::new();
    itoa::fmt(&mut s, 128u64)?;
    println!("{}", s);

    Ok(())
}
```

The function signatures are:

```rust
fn write<W: io::Write, V: itoa::Integer>(writer: W, value: V) -> io::Result<usize>;

fn fmt<W: fmt::Write, V: itoa::Integer>(writer: W, value: V) -> fmt::Result;
```

where `itoa::Integer` is implemented for i8, u8, i16, u16, i32, u32, i64, u64,
i128, u128, isize and usize. 128-bit integer support requires rustc 1.26+ and
the `i128` feature of this crate enabled.

The `write` function is only available when the `std` feature is enabled
(default is enabled). The return value gives the number of bytes written.

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
