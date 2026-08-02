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
    axoasset::copy(asset, "site assets", dest)?;
}
```

## License

This software is subject to the terms of the Mozilla Public License, v. 2.0. If a copy of the MPL was not distributed with this file, You can obtain one at http://mozilla.org/MPL/2.0/.

Copyright 2022 Axo Developer Co.
