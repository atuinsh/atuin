Procedural macros in expression position
========================================

[<img alt="github" src="https://img.shields.io/badge/github-dtolnay/proc--macro--hack-8da0cb?style=for-the-badge&labelColor=555555&logo=github" height="20">](https://github.com/dtolnay/proc-macro-hack)
[<img alt="crates.io" src="https://img.shields.io/crates/v/proc-macro-hack.svg?style=for-the-badge&color=fc8d62&logo=rust" height="20">](https://crates.io/crates/proc-macro-hack)
[<img alt="docs.rs" src="https://img.shields.io/badge/docs.rs-proc--macro--hack-66c2a5?style=for-the-badge&labelColor=555555&logoColor=white&logo=data:image/svg+xml;base64,PHN2ZyByb2xlPSJpbWciIHhtbG5zPSJodHRwOi8vd3d3LnczLm9yZy8yMDAwL3N2ZyIgdmlld0JveD0iMCAwIDUxMiA1MTIiPjxwYXRoIGZpbGw9IiNmNWY1ZjUiIGQ9Ik00ODguNiAyNTAuMkwzOTIgMjE0VjEwNS41YzAtMTUtOS4zLTI4LjQtMjMuNC0zMy43bC0xMDAtMzcuNWMtOC4xLTMuMS0xNy4xLTMuMS0yNS4zIDBsLTEwMCAzNy41Yy0xNC4xIDUuMy0yMy40IDE4LjctMjMuNCAzMy43VjIxNGwtOTYuNiAzNi4yQzkuMyAyNTUuNSAwIDI2OC45IDAgMjgzLjlWMzk0YzAgMTMuNiA3LjcgMjYuMSAxOS45IDMyLjJsMTAwIDUwYzEwLjEgNS4xIDIyLjEgNS4xIDMyLjIgMGwxMDMuOS01MiAxMDMuOSA1MmMxMC4xIDUuMSAyMi4xIDUuMSAzMi4yIDBsMTAwLTUwYzEyLjItNi4xIDE5LjktMTguNiAxOS45LTMyLjJWMjgzLjljMC0xNS05LjMtMjguNC0yMy40LTMzLjd6TTM1OCAyMTQuOGwtODUgMzEuOXYtNjguMmw4NS0zN3Y3My4zek0xNTQgMTA0LjFsMTAyLTM4LjIgMTAyIDM4LjJ2LjZsLTEwMiA0MS40LTEwMi00MS40di0uNnptODQgMjkxLjFsLTg1IDQyLjV2LTc5LjFsODUtMzguOHY3NS40em0wLTExMmwtMTAyIDQxLjQtMTAyLTQxLjR2LS42bDEwMi0zOC4yIDEwMiAzOC4ydi42em0yNDAgMTEybC04NSA0Mi41di03OS4xbDg1LTM4Ljh2NzUuNHptMC0xMTJsLTEwMiA0MS40LTEwMi00MS40di0uNmwxMDItMzguMiAxMDIgMzguMnYuNnoiPjwvcGF0aD48L3N2Zz4K" height="20">](https://docs.rs/proc-macro-hack)
[<img alt="build status" src="https://img.shields.io/github/workflow/status/dtolnay/proc-macro-hack/CI/master?style=for-the-badge" height="20">](https://github.com/dtolnay/proc-macro-hack/actions?query=branch%3Amaster)

<table><tr><td><hr>
<b>Note:</b> <i>As of Rust 1.45 this crate is superseded by native support for
#[proc_macro] in expression position. Only consider using this crate if you care
about supporting compilers between 1.31 and 1.45.</i>
<hr></td></tr></table>

Since Rust 1.30, the language supports user-defined function-like procedural
macros. However these can only be invoked in item position, not in statements or
expressions.

This crate implements an alternative type of procedural macro that can be
invoked in statement or expression position.

This approach works with any Rust version 1.31+.

## Defining procedural macros

Two crates are required to define a procedural macro.

### The implementation crate

This crate must contain nothing but procedural macros. Private helper
functions and private modules are fine but nothing can be public.

[&raquo; example of an implementation crate][demo-hack-impl]

Just like you would use a #\[proc_macro\] attribute to define a natively
supported procedural macro, use proc-macro-hack's #\[proc_macro_hack\]
attribute to define a procedural macro that works in expression position.
The function signature is the same as for ordinary function-like procedural
macros.

```rust
use proc_macro::TokenStream;
use proc_macro_hack::proc_macro_hack;
use quote::quote;
use syn::{parse_macro_input, Expr};

#[proc_macro_hack]
pub fn add_one(input: TokenStream) -> TokenStream {
    let expr = parse_macro_input!(input as Expr);
    TokenStream::from(quote! {
        1 + (#expr)
    })
}
```

### The declaration crate

This crate is allowed to contain other public things if you need, for
example traits or functions or ordinary macros.

[&raquo; example of a declaration crate][demo-hack]

Within the declaration crate there needs to be a re-export of your
procedural macro from the implementation crate. The re-export also carries a
\#\[proc_macro_hack\] attribute.

```rust
use proc_macro_hack::proc_macro_hack;

/// Add one to an expression.
///
/// (Documentation goes here on the re-export, not in the other crate.)
#[proc_macro_hack]
pub use demo_hack_impl::add_one;
```

Both crates depend on `proc-macro-hack`:

```toml
[dependencies]
proc-macro-hack = "0.5"
```

Additionally, your implementation crate (but not your declaration crate) is
a proc macro crate:

```toml
[lib]
proc-macro = true
```

## Using procedural macros

Users of your crate depend on your declaration crate (not your
implementation crate), then use your procedural macros as usual.

[&raquo; example of a downstream crate][example]

```rust
use demo_hack::add_one;

fn main() {
    let two = 2;
    let nine = add_one!(two) + add_one!(2 + 3);
    println!("nine = {}", nine);
}
```

[demo-hack-impl]: https://github.com/dtolnay/proc-macro-hack/tree/master/demo-hack-impl
[demo-hack]: https://github.com/dtolnay/proc-macro-hack/tree/master/demo-hack
[example]: https://github.com/dtolnay/proc-macro-hack/tree/master/example

## Limitations

- Only proc macros in expression position are supported. Proc macros in pattern
  position ([#20]) are not supported.

- By default, nested invocations are not supported i.e. the code emitted by a
  proc-macro-hack macro invocation cannot contain recursive calls to the same
  proc-macro-hack macro nor calls to any other proc-macro-hack macros. Use
  [`proc-macro-nested`] if you require support for nested invocations.

- By default, hygiene is structured such that the expanded code can't refer to
  local variables other than those passed by name somewhere in the macro input.
  If your macro must refer to *local* variables that don't get named in the
  macro input, use `#[proc_macro_hack(fake_call_site)]` on the re-export in your
  declaration crate. *Most macros won't need this.*

- On compilers that are new enough to natively support proc macros in expression
  position, proc-macro-hack does not automatically use that support, since the
  hygiene can be subtly different between the two implementations. To opt in to
  compiling your macro to native `#[proc_macro]` on sufficiently new compilers,
  use `#[proc_macro_hack(only_hack_old_rustc)]` on the re-export in your
  declaration crate.

[#10]: https://github.com/dtolnay/proc-macro-hack/issues/10
[#20]: https://github.com/dtolnay/proc-macro-hack/issues/20
[`proc-macro-nested`]: https://docs.rs/proc-macro-nested

<br>

#### License

<sup>
Licensed under either of <a href="LICENSE-APACHE">Apache License, Version
2.0</a> or <a href="LICENSE-MIT">MIT license</a> at your option.
</sup>

<br>

<sub>
Unless you explicitly state otherwise, any contribution intentionally submitted
for inclusion in this hack by you, as defined in the Apache-2.0 license, shall
be dual licensed as above, without any additional terms or conditions.
</sub>
