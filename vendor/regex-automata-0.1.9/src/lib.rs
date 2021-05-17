/*!
A low level regular expression library that uses deterministic finite automata.
It supports a rich syntax with Unicode support, has extensive options for
configuring the best space vs time trade off for your use case and provides
support for cheap deserialization of automata for use in `no_std` environments.

# Overview

This section gives a brief overview of the primary types in this crate:

* A [`Regex`](struct.Regex.html) provides a way to search for matches of a
  regular expression. This includes iterating over matches with both the start
  and end positions of each match.
* A [`RegexBuilder`](struct.RegexBuilder.html) provides a way configure many
  compilation options for a regex.
* A [`DenseDFA`](enum.DenseDFA.html) provides low level access to a DFA that
  uses a dense representation (uses lots of space, but fast searching).
* A [`SparseDFA`](enum.SparseDFA.html) provides the same API as a `DenseDFA`,
  but uses a sparse representation (uses less space, but slower matching).
* A [`DFA`](trait.DFA.html) trait that defines an interface that all DFAs must
  implement.
* Both dense DFAs and sparse DFAs support
  [serialization to raw bytes](enum.DenseDFA.html#method.to_bytes_little_endian)
  and
  [cheap deserialization](enum.DenseDFA.html#method.from_bytes).

# Example: basic regex searching

This example shows how to compile a regex using the default configuration
and then use it to find matches in a byte string:

```
use regex_automata::Regex;

let re = Regex::new(r"[0-9]{4}-[0-9]{2}-[0-9]{2}").unwrap();
let text = b"2018-12-24 2016-10-08";
let matches: Vec<(usize, usize)> = re.find_iter(text).collect();
assert_eq!(matches, vec![(0, 10), (11, 21)]);
```

# Example: use sparse DFAs

By default, compiling a regex will use dense DFAs internally. This uses more
memory, but executes searches more quickly. If you can abide slower searches
(somewhere around 3-5x), then sparse DFAs might make more sense since they can
use significantly less space.

Using sparse DFAs is as easy as using `Regex::new_sparse` instead of
`Regex::new`:

```
use regex_automata::Regex;

# fn example() -> Result<(), regex_automata::Error> {
let re = Regex::new_sparse(r"[0-9]{4}-[0-9]{2}-[0-9]{2}").unwrap();
let text = b"2018-12-24 2016-10-08";
let matches: Vec<(usize, usize)> = re.find_iter(text).collect();
assert_eq!(matches, vec![(0, 10), (11, 21)]);
# Ok(()) }; example().unwrap()
```

If you already have dense DFAs for some reason, they can be converted to sparse
DFAs and used to build a new `Regex`. For example:

```
use regex_automata::Regex;

# fn example() -> Result<(), regex_automata::Error> {
let dense_re = Regex::new(r"[0-9]{4}-[0-9]{2}-[0-9]{2}").unwrap();
let sparse_re = Regex::from_dfas(
    dense_re.forward().to_sparse()?,
    dense_re.reverse().to_sparse()?,
);
let text = b"2018-12-24 2016-10-08";
let matches: Vec<(usize, usize)> = sparse_re.find_iter(text).collect();
assert_eq!(matches, vec![(0, 10), (11, 21)]);
# Ok(()) }; example().unwrap()
```

# Example: deserialize a DFA

This shows how to first serialize a DFA into raw bytes, and then deserialize
those raw bytes back into a DFA. While this particular example is a bit
contrived, this same technique can be used in your program to deserialize a
DFA at start up time or by memory mapping a file. In particular,
deserialization is guaranteed to be cheap because it will always be a constant
time operation.

```
use regex_automata::{DenseDFA, Regex};

# fn example() -> Result<(), regex_automata::Error> {
let re1 = Regex::new(r"[0-9]{4}-[0-9]{2}-[0-9]{2}").unwrap();
// serialize both the forward and reverse DFAs, see note below
let fwd_bytes = re1.forward().to_u16()?.to_bytes_native_endian()?;
let rev_bytes = re1.reverse().to_u16()?.to_bytes_native_endian()?;
// now deserialize both---we need to specify the correct type!
let fwd: DenseDFA<&[u16], u16> = unsafe { DenseDFA::from_bytes(&fwd_bytes) };
let rev: DenseDFA<&[u16], u16> = unsafe { DenseDFA::from_bytes(&rev_bytes) };
// finally, reconstruct our regex
let re2 = Regex::from_dfas(fwd, rev);

// we can use it like normal
let text = b"2018-12-24 2016-10-08";
let matches: Vec<(usize, usize)> = re2.find_iter(text).collect();
assert_eq!(matches, vec![(0, 10), (11, 21)]);
# Ok(()) }; example().unwrap()
```

There are a few points worth noting here:

* We need to extract the raw DFAs used by the regex and serialize those. You
  can build the DFAs manually yourself using
  [`dense::Builder`](dense/struct.Builder.html), but using the DFAs from a
  `Regex` guarantees that the DFAs are built correctly.
* We specifically convert the dense DFA to a representation that uses `u16`
  for its state identifiers using
  [`DenseDFA::to_u16`](enum.DenseDFA.html#method.to_u16). While this isn't
  strictly necessary, if we skipped this step, then the serialized bytes would
  use `usize` for state identifiers, which does not have a fixed size. Using
  `u16` ensures that we can deserialize this DFA even on platforms with a
  smaller pointer size. If our DFA is too big for `u16` state identifiers, then
  one can use `u32` or `u64`.
* To convert the DFA to raw bytes, we use the `to_bytes_native_endian`
  method. In practice, you'll want to use either
  [`DenseDFA::to_bytes_little_endian`](enum.DenseDFA.html#method.to_bytes_little_endian)
  or
  [`DenseDFA::to_bytes_big_endian`](enum.DenseDFA.html#method.to_bytes_big_endian),
  depending on which platform you're deserializing your DFA from. If you intend
  to deserialize on either platform, then you'll need to serialize both and
  deserialize the right one depending on your target's endianness.
* Deserializing a DFA requires the use of `unsafe` because the raw bytes must
  be *trusted*. In particular, while some degree of sanity checks are
  performed, nothing guarantees the integrity of the DFA's transition table
  since deserialization is a constant time operation. Since searching with a
  DFA must be able to follow transitions blindly for performance reasons,
  giving incorrect bytes to the deserialization API can result in memory
  unsafety.

The same process can be achieved with sparse DFAs as well:

```
use regex_automata::{SparseDFA, Regex};

# fn example() -> Result<(), regex_automata::Error> {
let re1 = Regex::new(r"[0-9]{4}-[0-9]{2}-[0-9]{2}").unwrap();
// serialize both
let fwd_bytes = re1.forward().to_u16()?.to_sparse()?.to_bytes_native_endian()?;
let rev_bytes = re1.reverse().to_u16()?.to_sparse()?.to_bytes_native_endian()?;
// now deserialize both---we need to specify the correct type!
let fwd: SparseDFA<&[u8], u16> = unsafe { SparseDFA::from_bytes(&fwd_bytes) };
let rev: SparseDFA<&[u8], u16> = unsafe { SparseDFA::from_bytes(&rev_bytes) };
// finally, reconstruct our regex
let re2 = Regex::from_dfas(fwd, rev);

// we can use it like normal
let text = b"2018-12-24 2016-10-08";
let matches: Vec<(usize, usize)> = re2.find_iter(text).collect();
assert_eq!(matches, vec![(0, 10), (11, 21)]);
# Ok(()) }; example().unwrap()
```

Note that unlike dense DFAs, sparse DFAs have no alignment requirements.
Conversely, dense DFAs must be be aligned to the same alignment as their
state identifier representation.

# Support for `no_std`

This crate comes with a `std` feature that is enabled by default. When the
`std` feature is enabled, the API of this crate will include the facilities
necessary for compiling, serializing, deserializing and searching with regular
expressions. When the `std` feature is disabled, the API of this crate will
shrink such that it only includes the facilities necessary for deserializing
and searching with regular expressions.

The intended workflow for `no_std` environments is thus as follows:

* Write a program with the `std` feature that compiles and serializes a
  regular expression. Serialization should only happen after first converting
  the DFAs to use a fixed size state identifier instead of the default `usize`.
  You may also need to serialize both little and big endian versions of each
  DFA. (So that's 4 DFAs in total for each regex.)
* In your `no_std` environment, follow the examples above for deserializing
  your previously serialized DFAs into regexes. You can then search with them
  as you would any regex.

Deserialization can happen anywhere. For example, with bytes embedded into a
binary or with a file memory mapped at runtime.

Note that the
[`ucd-generate`](https://github.com/BurntSushi/ucd-generate)
tool will do the first step for you with its `dfa` or `regex` sub-commands.

# Syntax

This crate supports the same syntax as the `regex` crate, since they share the
same parser. You can find an exhaustive list of supported syntax in the
[documentation for the `regex` crate](https://docs.rs/regex/1.1/regex/#syntax).

Currently, there are a couple limitations. In general, this crate does not
support zero-width assertions, although they may be added in the future. This
includes:

* Anchors such as `^`, `$`, `\A` and `\z`.
* Word boundary assertions such as `\b` and `\B`.

It is possible to run a search that is anchored at the beginning of the input.
To do that, set the
[`RegexBuilder::anchored`](struct.RegexBuilder.html#method.anchored)
option when building a regex. By default, all searches are unanchored.

# Differences with the regex crate

The main goal of the [`regex`](https://docs.rs/regex) crate is to serve as a
general purpose regular expression engine. It aims to automatically balance low
compile times, fast search times and low memory usage, while also providing
a convenient API for users. In contrast, this crate provides a lower level
regular expression interface that is a bit less convenient while providing more
explicit control over memory usage and search times.

Here are some specific negative differences:

* **Compilation can take an exponential amount of time and space** in the size
  of the regex pattern. While most patterns do not exhibit worst case
  exponential time, such patterns do exist. For example, `[01]*1[01]{N}` will
  build a DFA with `2^(N+1)` states. For this reason, untrusted patterns should
  not be compiled with this library. (In the future, the API may expose an
  option to return an error if the DFA gets too big.)
* This crate does not support sub-match extraction, which can be achieved with
  the regex crate's "captures" API. This may be added in the future, but is
  unlikely.
* While the regex crate doesn't necessarily sport fast compilation times, the
  regexes in this crate are almost universally slow to compile, especially when
  they contain large Unicode character classes. For example, on my system,
  compiling `\w{3}` with byte classes enabled takes just over 1 second and
  almost 5MB of memory! (Compiling a sparse regex takes about the same time
  but only uses about 500KB of memory.) Conversly, compiling the same regex
  without Unicode support, e.g., `(?-u)\w{3}`, takes under 1 millisecond and
  less than 5KB of memory. For this reason, you should only use Unicode
  character classes if you absolutely need them!
* This crate does not support regex sets.
* This crate does not support zero-width assertions such as `^`, `$`, `\b` or
  `\B`.
* As a lower level crate, this library does not do literal optimizations. In
  exchange, you get predictable performance regardless of input. The
  philosophy here is that literal optimizations should be applied at a higher
  level, although there is no easy support for this in the ecosystem yet.
* There is no `&str` API like in the regex crate. In this crate, all APIs
  operate on `&[u8]`. By default, match indices are guaranteed to fall on
  UTF-8 boundaries, unless
  [`RegexBuilder::allow_invalid_utf8`](struct.RegexBuilder.html#method.allow_invalid_utf8)
  is enabled.

With some of the downsides out of the way, here are some positive differences:

* Both dense and sparse DFAs can be serialized to raw bytes, and then cheaply
  deserialized. Deserialization always takes constant time since searching can
  be performed directly on the raw serialized bytes of a DFA.
* This crate was specifically designed so that the searching phase of a DFA has
  minimal runtime requirements, and can therefore be used in `no_std`
  environments. While `no_std` environments cannot compile regexes, they can
  deserialize pre-compiled regexes.
* Since this crate builds DFAs ahead of time, it will generally out-perform
  the `regex` crate on equivalent tasks. The performance difference is likely
  not large. However, because of a complex set of optimizations in the regex
  crate (like literal optimizations), an accurate performance comparison may be
  difficult to do.
* Sparse DFAs provide a way to build a DFA ahead of time that sacrifices search
  performance a small amount, but uses much less storage space. Potentially
  even less than what the regex crate uses.
* This crate exposes DFAs directly, such as
  [`DenseDFA`](enum.DenseDFA.html) and [`SparseDFA`](enum.SparseDFA.html),
  which enables one to do less work in some cases. For example, if you only
  need the end of a match and not the start of a match, then you can use a DFA
  directly without building a `Regex`, which always requires a second DFA to
  find the start of a match.
* Aside from choosing between dense and sparse DFAs, there are several options
  for configuring the space usage vs search time trade off. These include
  things like choosing a smaller state identifier representation, to
  premultiplying state identifiers and splitting a DFA's alphabet into
  equivalence classes. Finally, DFA minimization is also provided, but can
  increase compilation times dramatically.
*/

#![deny(missing_docs)]
#![cfg_attr(not(feature = "std"), no_std)]

#[cfg(feature = "std")]
extern crate core;

#[cfg(all(test, feature = "transducer"))]
extern crate bstr;
extern crate byteorder;
#[cfg(feature = "transducer")]
extern crate fst;
#[cfg(feature = "std")]
extern crate regex_syntax;

pub use dense::DenseDFA;
pub use dfa::DFA;
#[cfg(feature = "std")]
pub use error::{Error, ErrorKind};
pub use regex::Regex;
#[cfg(feature = "std")]
pub use regex::RegexBuilder;
pub use sparse::SparseDFA;
pub use state_id::StateID;

mod classes;
#[path = "dense.rs"]
mod dense_imp;
#[cfg(feature = "std")]
mod determinize;
mod dfa;
#[cfg(feature = "std")]
mod error;
#[cfg(feature = "std")]
mod minimize;
#[cfg(feature = "std")]
#[doc(hidden)]
pub mod nfa;
mod regex;
#[path = "sparse.rs"]
mod sparse_imp;
#[cfg(feature = "std")]
mod sparse_set;
mod state_id;
#[cfg(feature = "transducer")]
mod transducer;

/// Types and routines specific to dense DFAs.
///
/// This module is the home of [`DenseDFA`](enum.DenseDFA.html) and each of its
/// corresponding variant DFA types, such as [`Standard`](struct.Standard.html)
/// and [`ByteClass`](struct.ByteClass.html).
///
/// This module also contains a [builder](struct.Builder.html) for
/// configuring the construction of a dense DFA.
pub mod dense {
    pub use dense_imp::*;
}

/// Types and routines specific to sparse DFAs.
///
/// This module is the home of [`SparseDFA`](enum.SparseDFA.html) and each of
/// its corresponding variant DFA types, such as
/// [`Standard`](struct.Standard.html) and
/// [`ByteClass`](struct.ByteClass.html).
///
/// Unlike the [`dense`](../dense/index.html) module, this module does not
/// contain a builder specific for sparse DFAs. Instead, the intended way to
/// build a sparse DFA is either by using a default configuration with its
/// [constructor](enum.SparseDFA.html#method.new),
/// or by first
/// [configuring the construction of a dense DFA](../dense/struct.Builder.html)
/// and then calling
/// [`DenseDFA::to_sparse`](../enum.DenseDFA.html#method.to_sparse).
pub mod sparse {
    pub use sparse_imp::*;
}
