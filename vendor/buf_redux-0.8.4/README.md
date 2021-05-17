# buf\_re(a)dux 
[![Travis](https://img.shields.io/travis/abonander/buf_redux.svg)](https://travis-ci.org/abonander/buf_redux)
[![Crates.io](https://img.shields.io/crates/v/buf_redux.svg)](https://crates.io/crates/buf_redux)
[![Crates.io](https://img.shields.io/crates/d/buf_redux.svg)](https://crates.io/crates/buf_redux)
[![Crates.io](https://img.shields.io/crates/l/buf_redux.svg)](https://crates.io/crates/buf_redux)

Drop-in replacements for buffered I/O types in `std::io`.

These replacements retain the method names/signatures and implemented traits of their stdlib
counterparts, making replacement as simple as swapping the import of the type.

### More Direct Control

All replacement types provide methods to:
* Increase the capacity of the buffer
* Get the number of available bytes as well as the total capacity of the buffer
* Consume the wrapper without losing data

`BufReader` provides methods to:
* Access the buffer through an `&`-reference without performing I/O
* Force unconditional reads into the buffer
* Get a `Read` adapter which empties the buffer and then pulls from the inner reader directly
* Shuffle bytes down to the beginning of the buffer to make room for more reading
* Get inner reader and trimmed buffer with the remaining data

`BufWriter` and `LineWriter` provide methods to:
* Flush the buffer and unwrap the inner writer unconditionally.

### More Sensible and Customizable Buffering Behavior
Tune the behavior of the buffer to your specific use-case using the types in the
`policy` module:

* Refine `BufReader`'s behavior by implementing the `ReaderPolicy` trait or use
an existing implementation like `MinBuffered` to ensure the buffer always contains
a minimum number of bytes (until the underlying reader is empty).

* Refine `BufWriter`'s behavior by implementing the `WriterPolicy` trait
or use an existing implementation like `FlushOn` to flush when a particular byte
appears in the buffer (used to implement `LineWriter`).


## Usage

#### [Documentation](http://docs.rs/buf_redux/)

`Cargo.toml`:
```toml
[dependencies]
buf_redux = "0.2"
```

`lib.rs` or `main.rs`:
```rust
extern crate buf_redux;
```

And then simply swap the import of the types you want to replace:

#### `BufReader`:
```
- use std::io::BufReader;
+ use buf_redux::BufReader;
```
#### `BufWriter`:
```
- use std::io::BufWriter;
+ use buf_redux::BufWriter;
```

#### `LineWriter`:
```
- use std::io::LineWriter;
+ use buf_redux::LineWriter;
```

### Using `MinBuffered`
The new `policy::MinBuffered` reader-policy can be used to ensure that `BufReader` always has at least a
certain number of bytes in its buffer. This can be useful for parsing applications that require a 
certain amount of lookahead.

```rust
use buf_redux::BufReader;
use buf_redux::policy::MinBuffered;
use std::io::{BufRead, Cursor};

let data = (1 .. 16).collect::<Vec<u8>>();

// normally you should use `BufReader::new()` or give a capacity of several KiB or more
let mut reader = BufReader::with_capacity(8, Cursor::new(data))
    // always at least 4 bytes in the buffer (or until the source is empty)
    .set_policy(MinBuffered(4)); // always at least 4 bytes in the buffer

// first buffer fill, same as `std::io::BufReader`
assert_eq!(reader.fill_buf().unwrap(), &[1, 2, 3, 4, 5, 6, 7, 8]);
reader.consume(3);

// enough data in the buffer, another read isn't done yet
assert_eq!(reader.fill_buf().unwrap(), &[4, 5, 6, 7, 8]);
reader.consume(4);

// `std::io::BufReader` would return `&[8]`
assert_eq!(reader.fill_buf().unwrap(), &[8, 9, 10, 11, 12, 13, 14, 15]);
reader.consume(5);

// no data left in the reader
assert_eq!(reader.fill_buf().unwrap(), &[13, 14, 15]);
```

### Note: Making Room / Ringbuffers / `slice-deque` Feature
With policies like `MinBuffered`, that will read into the buffer and consume bytes from it without completely 
emptying it, normal buffer handling can run out of room to read/write into as all the free space is at the
head of the buffer. If the amount of data in the buffer is small, you can call `.make_room()` on the buffered
type to make more room for reading. `MinBuffered` will do this automatically.

Instead of this, with the `slice-deque` feature, you can instead have your buffered type allocate a *ringbuffer*,
simply by using the `::new_ringbuf()` or `::with_capacity_ringbuf()` constructors instead of 
`::new()` or `with_capacity()`, respectively. With a ringbuffer, consuming/flushing bytes 
from a buffer instantly makes room for more reading/writing at the end.
However, this has some caveats:

* It is only available on target platforms with virtual memory support, namely fully fledged
OSes such as Windows and Unix-derivative platforms like Linux, OS X, BSD variants, etc.

* The default capacity varies based on platform, and custom capacities are rounded up to a
multiple of their minimum size, typically the page size of the platform.
Windows' minimum size is comparably quite large (**64 KiB**) due to some legacy reasons,
so this may be less optimal than the default capacity for a normal buffer (8 KiB) for some
use-cases.

* Due to the nature of the virtual-memory trick, the virtual address space the buffer
allocates will be double its capacity. This means that your program will *appear* to use more
memory than it would if it was using a normal buffer of the same capacity. The physical memory
usage will be the same in both cases, but if address space is at a premium in your application
(32-bit targets) then this may be a concern.

It is up to you to decide if the benefits outweigh the costs. With a policy like `MinBuffered`,
it could significantly improve performance.

## License

Licensed under either of

 * Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE) or http://www.apache.org/licenses/LICENSE-2.0)
 * MIT license ([LICENSE-MIT](LICENSE-MIT) or http://opensource.org/licenses/MIT)

at your option.

### Contribution

Unless you explicitly state otherwise, any contribution intentionally submitted
for inclusion in the work by you, as defined in the Apache-2.0 license, shall be dual licensed as above, without any
additional terms or conditions.
