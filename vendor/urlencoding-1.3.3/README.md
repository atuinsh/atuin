# urlencoding

[![Latest Version](https://img.shields.io/crates/v/urlencoding.svg)](https://lib.rs/crates/urlencoding)

A tiny Rust library for doing URL percentage encoding and decoding. It percent-encodes everything except alphanumerics and `-`, `_`, `.`, `~`.

When decoding `+` is not treated as a space. Error recovery from incomplete percent-escapes follows the [WHATWG URL standard](https://url.spec.whatwg.org/).

## Usage

To encode a string, do the following:

```rust
use urlencoding::encode;

fn main() {
  let encoded = encode("This string will be URL encoded.");
  println!("{}", encoded);
  // This%20string%20will%20be%20URL%20encoded.
}
```

To decode a string, it's only slightly different:

```rust
use urlencoding::decode;

fn main() {
  let decoded = decode("%F0%9F%91%BE%20Exterminate%21");
  println!("{}", decoded.unwrap());
  // 👾 Exterminate!
}
```

## License

This project is licensed under the MIT license, Copyright (c) 2017 Bertram Truong. For more information see the `LICENSE` file.
