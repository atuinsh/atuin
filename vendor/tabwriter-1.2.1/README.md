tabwriter is a crate that implements
[elastic tabstops](http://nickgravgaard.com/elastictabstops/index.html). It
provides both a library for wrapping Rust `Writer`s and a small program that
exposes the same functionality at the command line.

[![Build status](https://github.com/BurntSushi/tabwriter/workflows/ci/badge.svg)](https://github.com/BurntSushi/tabwriter/actions)
[![](http://meritbadge.herokuapp.com/tabwriter)](https://crates.io/crates/tabwriter)

Dual-licensed under MIT or the [UNLICENSE](http://unlicense.org).


### Simple example of library

```rust
use std::io::Write;

use tabwriter::TabWriter;

let mut tw = TabWriter::new(vec![]);
tw.write_all(b"
Bruce Springsteen\tBorn to Run
Bob Seger\tNight Moves
Metallica\tBlack
The Boss\tDarkness on the Edge of Town
").unwrap();
tw.flush().unwrap();

let written = String::from_utf8(tw.into_inner().unwrap()).unwrap();

assert_eq!(&written, "
Bruce Springsteen  Born to Run
Bob Seger          Night Moves
Metallica          Black
The Boss           Darkness on the Edge of Town
");
```

You can see an example of *real* use in my
[CSV toolkit](https://github.com/BurntSushi/xsv/blob/master/src/cmd/table.rs#L57-L60).


### Simple example of command line utility

```bash
[andrew@Liger tabwriter] cat sample | sed 's/   /\\t/g'
a\tb\tc
abc\tmnopqrstuv\txyz
abcmnoxyz\tmore text

a\tb\tc
[andrew@Liger tabwriter] ./target/tabwriter < sample
a          b           c
abc        mnopqrstuv  xyz
abcmnoxyz  more text

a   b   c
```

Notice that once a column block is broken, alignment starts over again.


### Documentation

The API is fully documented with some examples:
[http://burntsushi.net/rustdoc/tabwriter/](http://burntsushi.net/rustdoc/tabwriter/).


### Installation

This crate works with Cargo. Assuming you have Rust and
[Cargo](http://crates.io/) installed, simply check out the source and run
tests:

```bash
git clone git://github.com/BurntSushi/tabwriter
cd tabwriter
cargo test
```

You can also add `tabwriter` as a dependency to your project's `Cargo.toml`:

```toml
[dependencies]
tabwriter = "1"
```


### Dealing with ANSI escape codes

If you want `tabwriter` to be aware of ANSI escape codes, then compile it with
the `ansi_formatting` feature enabled.


### Minimum Rust version policy

This crate's minimum supported `rustc` version is `1.34.0`.

The current policy is that the minimum Rust version required to use this crate
can be increased in minor version updates. For example, if `crate 1.0` requires
Rust 1.20.0, then `crate 1.0.z` for all values of `z` will also require Rust
1.20.0 or newer. However, `crate 1.y` for `y > 0` may require a newer minimum
version of Rust.

In general, this crate will be conservative with respect to the minimum
supported version of Rust.
