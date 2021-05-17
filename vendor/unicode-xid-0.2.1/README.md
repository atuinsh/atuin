# unicode-xid

Determine if a `char` is a valid identifier for a parser and/or lexer according to
[Unicode Standard Annex #31](http://www.unicode.org/reports/tr31/) rules.

[![Build Status](https://travis-ci.org/unicode-rs/unicode-xid.svg)](https://travis-ci.org/unicode-rs/unicode-xid)

[Documentation](https://unicode-rs.github.io/unicode-xid/unicode_xid/index.html)

```rust
extern crate unicode_xid;

use unicode_xid::UnicodeXID;

fn main() {
    let ch = 'a';
    println!("Is {} a valid start of an identifier? {}", ch, UnicodeXID::is_xid_start(ch));
}
```

# features

unicode-xid supports a `no_std` feature. This eliminates dependence
on std, and instead uses equivalent functions from core.


# changelog

## 0.2.0

- Update to Unicode 12.1.0.

## 0.1.0

- Initial release.
