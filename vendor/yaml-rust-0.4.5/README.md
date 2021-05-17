# yaml-rust

The missing YAML 1.2 implementation for Rust.

[![Travis](https://travis-ci.org/chyh1990/yaml-rust.svg?branch=master)](https://travis-ci.org/chyh1990/yaml-rust)
[![AppVeyor](https://ci.appveyor.com/api/projects/status/scf47535ckp4ylg4?svg=true)](https://ci.appveyor.com/project/chyh1990/yaml-rust)
[![crates.io](https://img.shields.io/crates/v/yaml-rust.svg)](https://crates.io/crates/yaml-rust)
[![docs.rs](https://img.shields.io/badge/api-rustdoc-blue.svg)](https://docs.rs/yaml-rust)

`yaml-rust` is a pure Rust YAML 1.2 implementation,
which enjoys the memory safety
property and other benefits from the Rust language.
The parser is heavily influenced by `libyaml` and `yaml-cpp`.

## Quick Start

Add the following to the Cargo.toml of your project:

```toml
[dependencies]
yaml-rust = "0.4"
```

and import:

```rust
extern crate yaml_rust;
```

Use `yaml::YamlLoader` to load the YAML documents and access it
as Vec/HashMap:

```rust
extern crate yaml_rust;
use yaml_rust::{YamlLoader, YamlEmitter};

fn main() {
    let s =
"
foo:
    - list1
    - list2
bar:
    - 1
    - 2.0
";
    let docs = YamlLoader::load_from_str(s).unwrap();

    // Multi document support, doc is a yaml::Yaml
    let doc = &docs[0];

    // Debug support
    println!("{:?}", doc);

    // Index access for map & array
    assert_eq!(doc["foo"][0].as_str().unwrap(), "list1");
    assert_eq!(doc["bar"][1].as_f64().unwrap(), 2.0);

    // Chained key/array access is checked and won't panic,
    // return BadValue if they are not exist.
    assert!(doc["INVALID_KEY"][100].is_badvalue());

    // Dump the YAML object
    let mut out_str = String::new();
    {
        let mut emitter = YamlEmitter::new(&mut out_str);
        emitter.dump(doc).unwrap(); // dump the YAML object to a String
    }
    println!("{}", out_str);
}
```

Note that `yaml_rust::Yaml` implements `Index<&'a str>` & `Index<usize>`:

* `Index<usize>` assumes the container is an Array
* `Index<&'a str>` assumes the container is a string to value Map
* otherwise, `Yaml::BadValue` is returned

If your document does not conform to this convention (e.g. map with
complex type key), you can use the `Yaml::as_XXX` family API to access your
documents.

## Features

* Pure Rust
* Ruby-like Array/Hash access API
* Low-level YAML events emission

## Specification Compliance

This implementation aims to provide YAML parser fully compatible with
the YAML 1.2 specification. The parser can correctly parse almost all
examples in the specification, except for the following known bugs:

* Empty plain scalar in certain contexts

However, the widely used library `libyaml` also fails to parse these examples,
so it may not be a huge problem for most users.

## Goals

* Encoder
* Tag directive
* Alias while deserialization

## Minimum Rust version policy

This crate's minimum supported `rustc` version is 1.31 (released with Rust 2018, after v0.4.3), as this is the currently known minimum version for [`regex`](https://crates.io/crates/regex#minimum-rust-version-policy) as well.

## License

Licensed under either of

 * Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE) or http://www.apache.org/licenses/LICENSE-2.0)
 * MIT license ([LICENSE-MIT](LICENSE-MIT) or http://opensource.org/licenses/MIT)

at your option.

## Contribution

Fork & PR on Github.

Unless you explicitly state otherwise, any contribution intentionally submitted
for inclusion in the work by you, as defined in the Apache-2.0 license, shall be dual licensed as above, without any
additional terms or conditions.
