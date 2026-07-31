# axoasset

[![Github Actions Rust](https://github.com/axodotdev/axoasset/actions/workflows/rust.yml/badge.svg)](https://github.com/axodotdev/axoasset/actions)
[![crates.io](https://img.shields.io/crates/v/axoasset.svg)](https://crates.io/crates/axoasset)
[![License: MPL 2.0](https://img.shields.io/badge/License-MPL_2.0-brightgreen.svg)](https://opensource.org/licenses/MPL-2.0)

This library offers `read`, `write`, and `copy` functions, for local and remote
assets given a string that contains a relative or absolute local path or a
remote address using http or https.


## Example

```rust
use axoasset;

let assets = vec!("https://my.co/logo.png", "./profile.jpg", "README.md");
let dest = "public";

for asset in assets {
    axoasset::copy(asset, dest)?;
}
```

## License

Licensed under either of

* Apache License, Version 2.0, ([LICENSE-APACHE](LICENSE-APACHE) or [apache.org/licenses/LICENSE-2.0](https://www.apache.org/licenses/LICENSE-2.0))
* MIT license ([LICENSE-MIT](LICENSE-MIT) or [opensource.org/licenses/MIT](https://opensource.org/licenses/MIT))

at your option.

## Contributions

Unless you explicitly state otherwise, any contribution intentionally
submitted for inclusion in the work by you, as defined in the Apache-2.0
license, shall be dual licensed as above, without any additional terms or
conditions.

If you are interested in contributing, please read our [CONTRIBUTING notes] and our [Code of Conduct].

**Copyright 2022 Axo Developer Co.**

[CONTRIBUTING notes]: CONTRIBUTING.md
[Code of Conduct]: CODE_OF_CONDUCT.md
