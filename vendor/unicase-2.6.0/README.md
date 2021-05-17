# unicase

[![Build Status](https://travis-ci.org/seanmonstar/unicase.svg?branch=master)](https://travis-ci.org/seanmonstar/unicase)

[Documentation](https://docs.rs/unicase)

Compare strings when case is not important (using Unicode Case-folding).

```rust
// ignore ASCII case
let a = UniCase::new("foobar");
let b = UniCase::new("FOOBAR");

assert_eq!(a, b);

// using unicode case-folding
let c = UniCase::new("Maße")
let d = UniCase::new("MASSE");
assert_eq!(c, d);
```

## License

Licensed under either of

- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE) or http://apache.org/licenses/LICENSE-2.0)
- MIT license ([LICENSE-MIT](LICENSE-MIT) or http://opensource.org/licenses/MIT)

### Contribution

Unless you explicitly state otherwise, any contribution intentionally submitted for inclusion in the work by you, as defined in the Apache-2.0 license, shall be dual licensed as above, without any additional terms or conditions.
