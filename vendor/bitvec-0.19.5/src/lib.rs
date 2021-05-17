/*! # Addressable Bits

`bitvec` is a foundation library for memory compaction techniques that rely on
viewing memory as bit-addressed rather than byte-addressed.

The `bitvec` project is designed to provide a comprehensive set of tools for
users who need memory compaction, with as low a cost as possible.

# Usage

`bitvec` provides data structures that specialize the major sequence types in
the standard library:

- `[bool]` becomes [`BitSlice`]
- `[bool; N]` becomes [`BitArray`]
- `Box<[bool]>` becomes [`BitBox`]
- `Vec<bool>` becomes [`BitVec`]

You can start using the crate in an existing codebase by replacing types and
chasing compiler errors from there.

As an example,

```rust
# #[cfg(feature = "alloc")] {
let mut io_buf: Vec<u8> = Vec::new();
io_buf.extend(&[0x47, 0xA5]);

let mut stats: Vec<bool> = Vec::new();
stats.extend(&[true, false, true, true, false, false, true, false]);
# }
```

would become

```rust
# #[cfg(feature = "alloc")] {
use bitvec::prelude::*;

let mut io_buf = bitvec![Msb0, u8; 0; 16];
io_buf[.. 4].store(4u8);
io_buf[4 .. 8].store(7u8);
io_buf[8 .. 16].store(0xA5u8);

let mut stats: BitVec = BitVec::new();
stats.extend(&[true, false, true, true, false, false, true, false]);
# }
```

# Capabilities

`bitvec` stands out from other bit-vector libraries, both in Rust and in other
languages, in a few significant ways.

Unlike other Rust libraries, `bitvec` stores its information in pointers to
memory regions, rather than in the region directly. By using its own pointer
encoding scheme, it can use references `&BitSlice` and `&mut BitSlice` to manage
memory and fit seamlessly into the Rust language rules and API signatures.

Unlike *any* other bit-sequence system, `bitvec` enables users to specify the
register element type used to store data, and the ordering of bits within those
elements. This sidesteps the problems found in C [bitfields], C++
[`std::bitset`], Python [`bitstring`], Erlang [`bitstream`], and Rust libraries
such as [`bit-vec`].

By permitting the in-memory layout to be specified by the user, rather than
within the library, users are able to have the behavior characteristics they
want without effort or workarounds.

This works by suppling two type parameters: `O: BitOrder` specifies the ordering
of bits within a register element, and `T: BitStore` specifies which register
element is used to store bits. `T` is restricted to be only the unsigned
integers, and `Cell` or `Atomic` variants of them.

`bitvec` correctly handles memory aliasing by leveraging the type system to mark
regions that have become subject to concurrency and either force the use of
atomic memory accesses or forbid simultaneous multiprocessing. You will never
need to insert your own guards to prevent race conditions, and [`BitSlice`]
provides APIs to separate any slice into its aliased and unaliased sub-regions.

# Library Structure

You should generally import the library prelude, with

```rust
use bitvec::prelude::*;
```

The prelude contains all the symbols you will need to make use of the crate.
Almost all begin with the prefix `Bit`; only the orderings `Lsb0` and `Msb0` do
not. This will reduce the likelihood of name collisions. See the prelude module
documentation for more detail on which symbols are imported, and how you can
more precisely control this.

Each major component in the library is divided into its own module. This
includes each data structure and trait, as well as utility objects used for
implementation. The data structures that mirror the language distribution have
submodules for each part of their mirroring: `api` ports inherent methods,
`iter` contains iteration logic, `ops` operator overrides, and `traits` all
other trait implementations.The data structure’s own module only contains its
own definition and its inherent methods that are not ports of the standard
libraries.

# Usage

As a replacement for `bool` data structures, you should be able to replace old
type definition and value construction sites with their corresponding items from
this crate, and the rest of your project should just work with the new types.

To use `bitvec` for bitfields, use [`BitArray`] or [`BitVec`] to manage your data
buffers (compile-time static and run-time dynamic, respectively), and the
[`BitField`] trait to manage transferring values into and out of them.

The [`BitSlice`] type contains most of the methods and trait implementations used
to interact with the *contents* of a memory buffer. [`BitVec`] adds methods for
operating on allocations, and specializes [`BitSlice`] methods that can take
advantage of owned buffers.

The `domain` module, whose types are accessed by the `.{bit_,}domain{,_mut}`
methods on [`BitSlice`], allows users to split their views of memory on aliasing
boundaries, removing synchronization where provably safe.

There are many ways to construct a bit-level view of data. The [`BitArray`],
`BitBox`, and [`BitVec`] types are all owning types that contain a buffer of
memory and dereference to [`BitSlice`] in order to view it. In addition, you can
borrow any piece of ordinary Rust memory as a [`BitSlice`] view using its
borrowing constructor functions, and the [`BitView`] trait methods.

# Examples

See the `examples/` directory of the project repository for detailed examples,
or the type documentation for introductory samples.

[`BitArray`]: array/struct.BitArray.html
[`BitBox`]: boxed/struct.BitBox.html
[`BitField`]: field/trait.BitField.html
[`BitSlice`]: slice/struct.BitSlice.html
[`BitVec`]: vec/struct.BitVec.html
[`BitView`]: view/trait.BitView.html
[`bitstream`]: https://erlang.org/doc/programming_examples/bit_syntax.html
[`bitstring`]: https://pypi.org/project/bitstring/
[`bit-vec`]: https://crates.io/crates/bit-vec
[`std::bitset`]: https://en.cppreference.com/w/cpp/utility/bitset
[bitfields]: https://en.cppreference.com/w/c/language/bit_field
!*/

#![cfg_attr(not(feature = "std"), no_std)]
#![cfg_attr(debug_assertions, warn(missing_docs))]
#![cfg_attr(not(debug_assertions), deny(missing_docs))]
#![deny(unconditional_recursion)]

#[cfg(feature = "alloc")]
extern crate alloc;

#[macro_use]
pub mod macros;

mod access;
pub mod array;
pub mod domain;
pub mod field;
pub mod index;
pub mod mem;
pub mod order;
mod pointer;
pub mod prelude;
pub mod slice;
pub mod store;
pub mod view;

#[cfg(feature = "alloc")]
pub mod boxed;

#[cfg(feature = "alloc")]
pub mod vec;

#[cfg(not(feature = "devel"))]
mod devel;

#[cfg(feature = "devel")]
pub mod devel;

#[cfg(feature = "serde")]
mod serdes;
