// Original implementation Copyright 2013 The Rust Project Developers <https://github.com/rust-lang>
//
// Original source file: https://github.com/rust-lang/rust/blob/master/src/libstd/io/buffered.P
//
// Additions copyright 2016-2018 Austin Bonander <austin.bonander@gmail.com>
//
// Licensed under the Apache License, Version 2.0 <LICENSE-APACHE or
// http://www.apache.org/licenses/LICENSE-2.0> or the MIT license
// <LICENSE-MIT or http://opensource.org/licenses/MIT>, at your
// option. This file may not be copied, modified, or distributed
// except according to those terms.
//! Drop-in replacements for buffered I/O types in `std::io`.
//!
//! These replacements retain the method names/signatures and implemented traits of their stdlib
//! counterparts, making replacement as simple as swapping the import of the type:
//!
//! #### `BufReader`:
//! ```notest
//! - use std::io::BufReader;
//! + use buf_redux::BufReader;
//! ```
//! #### `BufWriter`:
//! ```notest
//! - use std::io::BufWriter;
//! + use buf_redux::BufWriter;
//! ```
//! #### `LineWriter`:
//! ```notest
//! - use std::io::LineWriter;
//! + use buf_redux::LineWriter;
//! ```
//!
//! ### More Direct Control
//! All replacement types provide methods to:
//!
//! * Increase the capacity of the buffer
//! * Get the number of available bytes as well as the total capacity of the buffer
//! * Consume the wrapper without losing data
//!
//! `BufReader` provides methods to:
//!
//! * Access the buffer through an `&`-reference without performing I/O
//! * Force unconditional reads into the buffer
//! * Get a `Read` adapter which empties the buffer and then pulls from the inner reader directly
//! * Shuffle bytes down to the beginning of the buffer to make room for more reading
//! * Get inner reader and trimmed buffer with the remaining data
//!
//! `BufWriter` and `LineWriter` provides methods to:
//!
//! * Flush the buffer and unwrap the inner writer unconditionally.
//! * Get the inner writer and trimmed buffer with the unflushed data.
//!
//! ### More Sensible and Customizable Buffering Behavior
//! Tune the behavior of the buffer to your specific use-case using the types in the
//! [`policy` module]:
//!
//! * Refine `BufReader`'s behavior by implementing the [`ReaderPolicy` trait] or use
//! an existing implementation like [`MinBuffered`] to ensure the buffer always contains
//! a minimum number of bytes (until the underlying reader is empty).
//!
//! * Refine `BufWriter`'s behavior by implementing the [`WriterPolicy` trait]
//! or use an existing implementation like [`FlushOn`] to flush when a particular byte
//! appears in the buffer (used to implement [`LineWriter`]).
//!
//! [`policy` module]: policy
//! [`ReaderPolicy` trait]: policy::ReaderPolicy
//! [`MinBuffered`]: policy::MinBuffered
//! [`WriterPolicy`]: policy::WriterPolicy
//! [`FlushOn`]: policy::FlushOn
//! [`LineWriter`]: LineWriter
//!
//! ### Making Room
//! The buffered types of this crate and their `std::io` counterparts, by default, use `Box<[u8]>`
//! as their buffer types ([`Buffer`](Buffer) is included as well since it is used internally
//! by the other types in this crate).
//!
//! When one of these types inserts bytes into its buffer, via `BufRead::fill_buf()` (implicitly
//! called by `Read::read()`) in `BufReader`'s case or `Write::write()` in `BufWriter`'s case,
//! the entire buffer is provided to be read/written into and the number of bytes written is saved.
//! The read/written data then resides in the `[0 .. bytes_inserted]` slice of the buffer.
//!
//! When bytes are consumed from the buffer, via `BufRead::consume()` or `Write::flush()`,
//! the number of bytes consumed is added to the start of the slice such that the remaining
//! data resides in the `[bytes_consumed .. bytes_inserted]` slice of the buffer.
//!
//! The `std::io` buffered types, and their counterparts in this crate with their default policies,
//! don't have to deal with partially filled buffers as `BufReader` only reads when empty and
//! `BufWriter` only flushes when full.
//!
//! However, because the replacements in this crate are capable of reading on-demand and flushing
//! less than a full buffer, they can run out of room in their buffers to read/write data into even
//! though there is technically free space, because this free space is at the head of the buffer
//! where reading into it would cause the data in the buffer to become non-contiguous.
//!
//! This isn't technically a problem as the buffer could operate like `VecDeque` in `std` and return
//! both slices at once, but this would not fit all use-cases: the `Read::fill_buf()` interface only
//! allows one slice to be returned at a time so the older data would need to be completely consumed
//! before the newer data can be returned; `BufWriter` could support it as the `Write` interface
//! doesn't make an opinion on how the buffer works, but because the data would be non-contiguous
//! it would require two flushes to get it all, which could degrade performance.
//!
//! The obvious solution, then, is to move the existing data down to the beginning of the buffer
//! when there is no more room at the end so that more reads/writes into the buffer can be issued.
//! This works, and may suit some use-cases where the amount of data left is small and thus copying
//! it would be inexpensive, but it is non-optimal. However, this option is provided
//! as the `.make_room()` methods, and is utilized by [`policy::MinBuffered`](policy::MinBuffered)
//! and [`policy::FlushExact`](policy::FlushExact).
//!
//! ### Ringbuffers / `slice-deque` Feature
//! Instead of moving data, however, it is also possible to use virtual-memory tricks to
//! allocate a ringbuffer that loops around on itself in memory and thus is always contiguous,
//! as described in [the Wikipedia article on Ringbuffers][ringbuf-wikipedia].
//!
//! This is the exact trick used by [the `slice-deque` crate](https://crates.io/crates/slice-deque),
//! which is now provided as an optional feature `slice-deque` exposed via the
//! `new_ringbuf()` and `with_capacity_ringbuf()` constructors added to the buffered types here.
//! When a buffered type is constructed using one of these functions, `.make_room()` is turned into
//! a no-op as consuming bytes from the head of the buffer simultaneously makes room at the tail.
//! However, this has some caveats:
//!
//! * It is only available on target platforms with virtual memory support, namely fully fledged
//! OSes such as Windows and Unix-derivative platforms like Linux, OS X, BSD variants, etc.
//!
//! * The default capacity varies based on platform, and custom capacities are rounded up to a
//! multiple of their minimum size, typically the page size of the platform.
//! Windows' minimum size is comparably quite large (**64 KiB**) due to some legacy reasons,
//! so this may be less optimal than the default capacity for a normal buffer (8 KiB) for some
//! use-cases.
//!
//! * Due to the nature of the virtual-memory trick, the virtual address space the buffer
//! allocates will be double its capacity. This means that your program will *appear* to use more
//! memory than it would if it was using a normal buffer of the same capacity. The physical memory
//! usage will be the same in both cases, but if address space is at a premium in your application
//! (32-bit targets) then this may be a concern.
//!
//! [ringbuf-wikipedia]: https://en.wikipedia.org/wiki/Circular_buffer#Optimization
#![warn(missing_docs)]
#![cfg_attr(feature = "nightly", feature(alloc, read_initializer, specialization))]
#![cfg_attr(all(test, feature = "nightly"), feature(io, test))]

extern crate memchr;

extern crate safemem;

use std::any::Any;
use std::cell::RefCell;
use std::io::prelude::*;
use std::io::SeekFrom;
use std::mem::ManuallyDrop;
use std::{cmp, error, fmt, io, ptr};

#[cfg(all(feature = "nightly", test))]
mod benches;

// std::io's tests require exact allocation which slice_deque cannot provide
#[cfg(test)]
mod std_tests;

#[cfg(all(test, feature = "slice-deque"))]
mod ringbuf_tests;

#[cfg(feature = "nightly")]
mod nightly;

#[cfg(feature = "nightly")]
use nightly::init_buffer;

mod buffer;

use buffer::BufImpl;

pub mod policy;

use self::policy::{ReaderPolicy, WriterPolicy, StdPolicy, FlushOnNewline};

const DEFAULT_BUF_SIZE: usize = 8 * 1024;

/// A drop-in replacement for `std::io::BufReader` with more functionality.
///
/// Original method names/signatures and implemented traits are left untouched,
/// making replacement as simple as swapping the import of the type.
///
/// By default this type implements the behavior of its `std` counterpart: it only reads into
/// the buffer when it is empty.
///
/// To change this type's behavior, change the policy with [`.set_policy()`] using a type
/// from the [`policy` module] or your own implementation of [`ReaderPolicy`].
///
/// Policies that perform alternating reads and consumes without completely emptying the buffer
/// may benefit from using a ringbuffer via the [`new_ringbuf()`] and [`with_capacity_ringbuf()`]
/// constructors. Ringbuffers are only available on supported platforms with the
/// `slice-deque` feature and have some other caveats; see [the crate root docs][ringbufs-root]
/// for more details.
///
/// [`.set_policy()`]: BufReader::set_policy
/// [`policy` module]: policy
/// [`ReaderPolicy`]: policy::ReaderPolicy
/// [`new_ringbuf()`]: BufReader::new_ringbuf
/// [`with_capacity_ringbuf()`]: BufReader::with_capacity_ringbuf
/// [ringbufs-root]: index.html#ringbuffers--slice-deque-feature
pub struct BufReader<R, P = StdPolicy>{
    // First field for null pointer optimization.
    buf: Buffer,
    inner: R,
    policy: P,
}

impl<R> BufReader<R, StdPolicy> {
    /// Create a new `BufReader` wrapping `inner`, utilizing a buffer of
    /// default capacity and the default [`ReaderPolicy`](policy::ReaderPolicy).
    pub fn new(inner: R) -> Self {
        Self::with_capacity(DEFAULT_BUF_SIZE, inner)
    }

    /// Create a new `BufReader` wrapping `inner`, utilizing a buffer with a capacity
    /// of *at least* `cap` bytes and the default [`ReaderPolicy`](policy::ReaderPolicy).
    ///
    /// The actual capacity of the buffer may vary based on implementation details of the global
    /// allocator.
    pub fn with_capacity(cap: usize, inner: R) -> Self {
        Self::with_buffer(Buffer::with_capacity(cap), inner)
    }

    /// Create a new `BufReader` wrapping `inner`, utilizing a ringbuffer with the default capacity
    /// and `ReaderPolicy`.
    ///
    /// A ringbuffer never has to move data to make room; consuming bytes from the head
    /// simultaneously makes room at the tail. This is useful in conjunction with a policy like
    /// [`MinBuffered`](policy::MinBuffered) to ensure there is always room to read more data
    /// if necessary, without expensive copying operations.
    ///
    /// Only available on platforms with virtual memory support and with the `slice-deque` feature
    /// enabled. The default capacity will differ between Windows and Unix-derivative targets.
    /// See [`Buffer::new_ringbuf()`](struct.Buffer.html#method.new_ringbuf)
    /// or [the crate root docs](index.html#ringbuffers--slice-deque-feature) for more info.
    #[cfg(feature = "slice-deque")]
    pub fn new_ringbuf(inner: R) -> Self {
        Self::with_capacity_ringbuf(DEFAULT_BUF_SIZE, inner)
    }

    /// Create a new `BufReader` wrapping `inner`, utilizing a ringbuffer with *at least* the given
    /// capacity and the default `ReaderPolicy`.
    ///
    /// A ringbuffer never has to move data to make room; consuming bytes from the head
    /// simultaneously makes room at the tail. This is useful in conjunction with a policy like
    /// [`MinBuffered`](policy::MinBuffered) to ensure there is always room to read more data
    /// if necessary, without expensive copying operations.
    ///
    /// Only available on platforms with virtual memory support and with the `slice-deque` feature
    /// enabled. The capacity will be rounded up to the minimum size for the target platform.
    /// See [`Buffer::with_capacity_ringbuf()`](struct.Buffer.html#method.with_capacity_ringbuf)
    /// or [the crate root docs](index.html#ringbuffers--slice-deque-feature) for more info.
    #[cfg(feature = "slice-deque")]
    pub fn with_capacity_ringbuf(cap: usize, inner: R) -> Self {
        Self::with_buffer(Buffer::with_capacity_ringbuf(cap), inner)
    }

    /// Wrap `inner` with an existing `Buffer` instance and the default `ReaderPolicy`.
    ///
    /// ### Note
    /// Does **not** clear the buffer first! If there is data already in the buffer
    /// then it will be returned in `read()` and `fill_buf()` ahead of any data from `inner`.
    pub fn with_buffer(buf: Buffer, inner: R) -> Self {
        BufReader {
            buf, inner, policy: StdPolicy
        }
    }
}

impl<R, P> BufReader<R, P> {
    /// Apply a new `ReaderPolicy` to this `BufReader`, returning the transformed type.
    pub fn set_policy<P_: ReaderPolicy>(self, policy: P_) -> BufReader<R, P_> {
        BufReader {
            inner: self.inner,
            buf: self.buf,
            policy
        }
    }

    /// Mutate the current [`ReaderPolicy`](policy::ReaderPolicy) in-place.
    ///
    /// If you want to change the type, use `.set_policy()`.
    pub fn policy_mut(&mut self) -> &mut P { &mut self.policy }

    /// Inspect the current `ReaderPolicy`.
    pub fn policy(&self) -> &P {
        &self.policy
    }

    /// Move data to the start of the buffer, making room at the end for more 
    /// reading.
    ///
    /// This is a no-op with the `*_ringbuf()` constructors (requires `slice-deque` feature).
    pub fn make_room(&mut self) {
        self.buf.make_room();
    }

    /// Ensure room in the buffer for *at least* `additional` bytes. May not be
    /// quite exact due to implementation details of the buffer's allocator.
    pub fn reserve(&mut self, additional: usize) {
        self.buf.reserve(additional);
    }

    // RFC: pub fn shrink(&mut self, new_len: usize) ?

    /// Get the section of the buffer containing valid data; may be empty.
    ///
    /// Call `.consume()` to remove bytes from the beginning of this section.
    pub fn buffer(&self) -> &[u8] {
        self.buf.buf()
    }

    /// Get the current number of bytes available in the buffer.
    pub fn buf_len(&self) -> usize {
        self.buf.len()
    }

    /// Get the total buffer capacity.
    pub fn capacity(&self) -> usize {
        self.buf.capacity()
    }

    /// Get an immutable reference to the underlying reader.
    pub fn get_ref(&self) -> &R { &self.inner }

    /// Get a mutable reference to the underlying reader.
    ///
    /// ## Note
    /// Reading directly from the underlying reader is not recommended, as some
    /// data has likely already been moved into the buffer.
    pub fn get_mut(&mut self) -> &mut R { &mut self.inner }

    /// Consume `self` and return the inner reader only.
    pub fn into_inner(self) -> R {
        self.inner
    }

    /// Consume `self` and return both the underlying reader and the buffer.
    ///
    /// See also: `BufReader::unbuffer()`
    pub fn into_inner_with_buffer(self) -> (R, Buffer) {
        (self.inner, self.buf)
    }

    /// Consume `self` and return an adapter which implements `Read` and will
    /// empty the buffer before reading directly from the underlying reader.
    pub fn unbuffer(self) -> Unbuffer<R> {
        Unbuffer {
            inner: self.inner,
            buf: Some(self.buf),
        }
    }
}

impl<R, P: ReaderPolicy> BufReader<R, P> {
    #[inline]
    fn should_read(&mut self) -> bool {
        self.policy.before_read(&mut self.buf).0
    }
}

impl<R: Read, P> BufReader<R, P> {
    /// Unconditionally perform a read into the buffer.
    ///
    /// Does not invoke `ReaderPolicy` methods.
    /// 
    /// If the read was successful, returns the number of bytes read.
    pub fn read_into_buf(&mut self) -> io::Result<usize> {
        self.buf.read_from(&mut self.inner)
    }

    /// Box the inner reader without losing data.
    pub fn boxed<'a>(self) -> BufReader<Box<Read + 'a>, P> where R: 'a {
        let inner: Box<Read + 'a> = Box::new(self.inner);
        
        BufReader {
            inner,
            buf: self.buf,
            policy: self.policy,
        }
    }
}

impl<R: Read, P: ReaderPolicy> Read for BufReader<R, P> {
    fn read(&mut self, out: &mut [u8]) -> io::Result<usize> {
        // If we don't have any buffered data and we're doing a read matching
        // or exceeding the internal buffer's capacity, bypass the buffer.
        if self.buf.is_empty() && out.len() >= self.buf.capacity() {
            return self.inner.read(out);
        }

        let nread = self.fill_buf()?.read(out)?;
        self.consume(nread);
        Ok(nread)
    }
}

impl<R: Read, P: ReaderPolicy> BufRead for BufReader<R, P> {
    fn fill_buf(&mut self) -> io::Result<&[u8]> {
        // If we've reached the end of our internal buffer then we need to fetch
        // some more data from the underlying reader.
        // This execution order is important; the policy may want to resize the buffer or move data
        // before reading into it.
        while self.should_read() && self.buf.usable_space() > 0 {
            if self.read_into_buf()? == 0 { break; };
        }

        Ok(self.buffer())
    }

    fn consume(&mut self, mut amt: usize) {
        amt = cmp::min(amt, self.buf_len());
        self.buf.consume(amt);
        self.policy.after_consume(&mut self.buf, amt);
    }
}

impl<R: fmt::Debug, P: fmt::Debug> fmt::Debug for BufReader<R, P> {
    fn fmt(&self, fmt: &mut fmt::Formatter) -> fmt::Result {
        fmt.debug_struct("buf_redux::BufReader")
            .field("reader", &self.inner)
            .field("buf_len", &self.buf_len())
            .field("capacity", &self.capacity())
            .field("policy", &self.policy)
            .finish()
    }
}

impl<R: Seek, P: ReaderPolicy> Seek for BufReader<R, P> {
    /// Seek to an ofPet, in bytes, in the underlying reader.
    ///
    /// The position used for seeking with `SeekFrom::Current(_)` is the
    /// position the underlying reader would be at if the `BufReader` had no
    /// internal buffer.
    ///
    /// Seeking always discards the internal buffer, even if the seek position
    /// would otherwise fall within it. This guarantees that calling
    /// `.unwrap()` immediately after a seek yields the underlying reader at
    /// the same position.
    ///
    /// See `std::io::Seek` for more details.
    ///
    /// Note: In the edge case where you're seeking with `SeekFrom::Current(n)`
    /// where `n` minus the internal buffer length underflows an `i64`, two
    /// seeks will be performed instead of one. If the second seek returns
    /// `Err`, the underlying reader will be left at the same position it would
    /// have if you seeked to `SeekFrom::Current(0)`.
    fn seek(&mut self, pos: SeekFrom) -> io::Result<u64> {
        let result: u64;
        if let SeekFrom::Current(n) = pos {
            let remainder = self.buf_len() as i64;
            // it should be safe to assume that remainder fits within an i64 as the alternative
            // means we managed to allocate 8 ebibytes and that's absurd.
            // But it's not out of the realm of possibility for some weird underlying reader to
            // support seeking by i64::min_value() so we need to handle underflow when subtracting
            // remainder.
            if let Some(offset) = n.checked_sub(remainder) {
                result = self.inner.seek(SeekFrom::Current(offset))?;
            } else {
                // seek backwards by our remainder, and then by the offset
                self.inner.seek(SeekFrom::Current(-remainder))?;
                self.buf.clear(); // empty the buffer
                result = self.inner.seek(SeekFrom::Current(n))?;
            }
        } else {
            // Seeking with Start/End doesn't care about our buffer length.
            result = self.inner.seek(pos)?;
        }
        self.buf.clear();
        Ok(result)
    }
}

/// A drop-in replacement for `std::io::BufWriter` with more functionality.
///
/// Original method names/signatures and implemented traits are left untouched,
/// making replacement as simple as swapping the import of the type.
///
/// By default this type implements the behavior of its `std` counterpart: it only flushes
/// the buffer if an incoming write is larger than the remaining space.
///
/// To change this type's behavior, change the policy with [`.set_policy()`] using a type
/// from the [`policy` module] or your own implentation of [`WriterPolicy`].
///
/// Policies that perform alternating writes and flushes without completely emptying the buffer
/// may benefit from using a ringbuffer via the [`new_ringbuf()`] and [`with_capacity_ringbuf()`]
/// constructors. Ringbuffers are only available on supported platforms with the
/// `slice-deque` feature and have some caveats; see [the docs at the crate root][ringbufs-root]
/// for more details.
///
/// [`.set_policy()`]: BufWriter::set_policy
/// [`policy` module]: policy
/// [`WriterPolicy`]: policy::WriterPolicy
/// [`new_ringbuf()`]: BufWriter::new_ringbuf
/// [`with_capacity_ringbuf()`]: BufWriter::with_capacity_ringbuf
/// [ringbufs-root]: index.html#ringbuffers--slice-deque-feature
pub struct BufWriter<W: Write, P = StdPolicy> {
    buf: Buffer,
    inner: W,
    policy: P,
    panicked: bool,
}

impl<W: Write> BufWriter<W> {
    /// Create a new `BufWriter` wrapping `inner` with the default buffer capacity and
    /// [`WriterPolicy`](policy::WriterPolicy).
    pub fn new(inner: W) -> Self {
        Self::with_buffer(Buffer::new(), inner)
    }

    /// Create a new `BufWriter` wrapping `inner`, utilizing a buffer with a capacity
    /// of *at least* `cap` bytes and the default [`WriterPolicy`](policy::WriterPolicy).
    ///
    /// The actual capacity of the buffer may vary based on implementation details of the global
    /// allocator.
    pub fn with_capacity(cap: usize, inner: W) -> Self {
        Self::with_buffer(Buffer::with_capacity(cap), inner)
    }

    /// Create a new `BufWriter` wrapping `inner`, utilizing a ringbuffer with the default
    /// capacity and [`WriterPolicy`](policy::WriterPolicy).
    ///
    /// A ringbuffer never has to move data to make room; consuming bytes from the head
    /// simultaneously makes room at the tail. This is useful in conjunction with a policy like
    ///  [`FlushExact`](policy::FlushExact) to ensure there is always room to write more data if
    /// necessary, without expensive copying operations.
    ///
    /// Only available on platforms with virtual memory support and with the `slice-deque` feature
    /// enabled. The default capacity will differ between Windows and Unix-derivative targets.
    /// See [`Buffer::new_ringbuf()`](Buffer::new_ringbuf)
    /// or [the crate root docs](index.html#ringbuffers--slice-deque-feature) for more info.
    #[cfg(feature = "slice-deque")]
    pub fn new_ringbuf(inner: W) -> Self {
        Self::with_buffer(Buffer::new_ringbuf(), inner)
    }

    /// Create a new `BufWriter` wrapping `inner`, utilizing a ringbuffer with *at least* `cap`
    /// capacity and the default [`WriterPolicy`](policy::WriterPolicy).
    ///
    /// A ringbuffer never has to move data to make room; consuming bytes from the head
    /// simultaneously makes room at the tail. This is useful in conjunction with a policy like
    /// [`FlushExact`](policy::FlushExact) to ensure there is always room to write more data if
    /// necessary, without expensive copying operations.
    ///
    /// Only available on platforms with virtual memory support and with the `slice-deque` feature
    /// enabled. The capacity will be rounded up to the minimum size for the target platform.
    /// See [`Buffer::with_capacity_ringbuf()`](Buffer::with_capacity_ringbuf)
    /// or [the crate root docs](index.html#ringbuffers--slice-deque-feature) for more info.
    #[cfg(feature = "slice-deque")]
    pub fn with_capacity_ringbuf(cap: usize, inner: W) -> Self {
        Self::with_buffer(Buffer::with_capacity_ringbuf(cap), inner)
    }

    /// Create a new `BufWriter` wrapping `inner`, utilizing the existing [`Buffer`](Buffer)
    /// instance and the default [`WriterPolicy`](policy::WriterPolicy).
    ///
    /// ### Note
    /// Does **not** clear the buffer first! If there is data already in the buffer
    /// it will be written out on the next flush!
    pub fn with_buffer(buf: Buffer, inner: W) -> BufWriter<W> {
        BufWriter {
            buf, inner, policy: StdPolicy, panicked: false,
        }
    }
}

impl<W: Write, P> BufWriter<W, P> {
    /// Set a new [`WriterPolicy`](policy::WriterPolicy), returning the transformed type.
    pub fn set_policy<P_: WriterPolicy>(self, policy: P_) -> BufWriter<W, P_> {
        let panicked = self.panicked;
        let (inner, buf) = self.into_inner_();

        BufWriter {
            inner, buf, policy, panicked
        }
    }

    /// Mutate the current [`WriterPolicy`](policy::WriterPolicy).
    pub fn policy_mut(&mut self) -> &mut P {
        &mut self.policy
    }

    /// Inspect the current `WriterPolicy`.
    pub fn policy(&self) -> &P {
        &self.policy
    }

    /// Get a reference to the inner writer.
    pub fn get_ref(&self) -> &W {
        &self.inner
    }

    /// Get a mutable reference to the inner writer.
    ///
    /// ### Note
    /// If the buffer has not been flushed, writing directly to the inner type will cause
    /// data inconsistency.
    pub fn get_mut(&mut self) -> &mut W {
        &mut self.inner
    }

    /// Get the capacty of the inner buffer.
    pub fn capacity(&self) -> usize {
        self.buf.capacity()
    }

    /// Get the number of bytes currently in the buffer.
    pub fn buf_len(&self) -> usize {
        self.buf.len()
    }

    /// Reserve space in the buffer for at least `additional` bytes. May not be
    /// quite exact due to implementation details of the buffer's allocator.
    pub fn reserve(&mut self, additional: usize) {
        self.buf.reserve(additional);
    }

    /// Move data to the start of the buffer, making room at the end for more
    /// writing.
    ///
    /// This is a no-op with the `*_ringbuf()` constructors (requires `slice-deque` feature).
    pub fn make_room(&mut self) {
        self.buf.make_room();
    }

    /// Consume `self` and return both the underlying writer and the buffer
    pub fn into_inner_with_buffer(self) -> (W, Buffer) {
        self.into_inner_()
    }

    // copy the fields out and forget `self` to avoid dropping twice
    fn into_inner_(self) -> (W, Buffer) {
        let s = ManuallyDrop::new(self);
        unsafe {
            // safe because we immediately forget `self`
            let inner = ptr::read(&s.inner);
            let buf = ptr::read(&s.buf);
            (inner, buf)
        }
    }

    fn flush_buf(&mut self, amt: usize) -> io::Result<()> {
        if amt == 0 || amt > self.buf.len() { return Ok(()) }

        self.panicked = true;
        let ret = self.buf.write_max(amt, &mut self.inner);
        self.panicked = false;
        ret
    }
}

impl<W: Write, P: WriterPolicy> BufWriter<W, P> {
    /// Flush the buffer and unwrap, returning the inner writer on success,
    /// or a type wrapping `self` plus the error otherwise.
    pub fn into_inner(mut self) -> Result<W, IntoInnerError<Self>> {
        match self.flush() {
            Err(e) => Err(IntoInnerError(self, e)),
            Ok(()) => Ok(self.into_inner_().0),
        }
    }

    /// Flush the buffer and unwrap, returning the inner writer and
    /// any error encountered during flushing.
    pub fn into_inner_with_err(mut self) -> (W, Option<io::Error>) {
        let err = self.flush().err();
        (self.into_inner_().0, err)
    }
}

impl<W: Write, P: WriterPolicy> Write for BufWriter<W, P> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let flush_amt = self.policy.before_write(&mut self.buf, buf.len()).0;
        self.flush_buf(flush_amt)?;

        let written = if self.buf.is_empty() && buf.len() >= self.buf.capacity() {
            self.panicked = true;
            let result = self.inner.write(buf);
            self.panicked = false;
            result?
        } else {
            self.buf.copy_from_slice(buf)
        };

        let flush_amt = self.policy.after_write(&self.buf).0;

        let _ = self.flush_buf(flush_amt);

        Ok(written)
    }

    fn flush(&mut self) -> io::Result<()> {
        let flush_amt = self.buf.len();
        self.flush_buf(flush_amt)?;
        self.inner.flush()
    }
}

impl<W: Write + Seek, P: WriterPolicy> Seek for BufWriter<W, P> {
    /// Seek to the ofPet, in bytes, in the underlying writer.
    ///
    /// Seeking always writes out the internal buffer before seeking.
    fn seek(&mut self, pos: SeekFrom) -> io::Result<u64> {
        self.flush().and_then(|_| self.get_mut().seek(pos))
    }
}

impl<W: Write + fmt::Debug, P: fmt::Debug> fmt::Debug for BufWriter<W, P> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.debug_struct("buf_redux::BufWriter")
            .field("writer", &self.inner)
            .field("capacity", &self.capacity())
            .field("policy", &self.policy)
            .finish()
    }
}


/// Attempt to flush the buffer to the underlying writer.
///
/// If an error occurs, the thread-local handler is invoked, if one was previously
/// set by [`set_drop_err_handler`](set_drop_err_handler) for this thread.
impl<W: Write, P> Drop for BufWriter<W, P> {
    fn drop(&mut self) {
        if !self.panicked {
            // instead of ignoring a failed flush, call the handler
            let buf_len = self.buf.len();
            if let Err(err) = self.flush_buf(buf_len) {
                DROP_ERR_HANDLER.with(|deh| {
                    (*deh.borrow())(&mut self.inner, &mut self.buf, err)
                });
            }
        }
    }
}

/// A drop-in replacement for `std::io::LineWriter` with more functionality.
///
/// This is, in fact, only a thin wrapper around
/// [`BufWriter`](BufWriter)`<W, `[`policy::FlushOnNewline`](policy::FlushOnNewline)`>`, which
/// demonstrates the power of custom [`WriterPolicy`](policy::WriterPolicy) implementations.
pub struct LineWriter<W: Write>(BufWriter<W, FlushOnNewline>);

impl<W: Write> LineWriter<W> {
    /// Wrap `inner` with the default buffer capacity.
    pub fn new(inner: W) -> Self {
        Self::with_buffer(Buffer::new(), inner)
    }

    /// Wrap `inner` with the given buffer capacity.
    pub fn with_capacity(cap: usize, inner: W) -> Self {
        Self::with_buffer(Buffer::with_capacity(cap), inner)
    }

    /// Wrap `inner` with the default buffer capacity using a ringbuffer.
    #[cfg(feature = "slice-deque")]
    pub fn new_ringbuf(inner: W) -> Self {
        Self::with_buffer(Buffer::new_ringbuf(), inner)
    }

    /// Wrap `inner` with the given buffer capacity using a ringbuffer.
    #[cfg(feature = "slice-deque")]
    pub fn with_capacity_ringbuf(cap: usize, inner: W) -> Self {
        Self::with_buffer(Buffer::with_capacity_ringbuf(cap), inner)
    }

    /// Wrap `inner` with an existing `Buffer` instance.
    ///
    /// ### Note
    /// Does **not** clear the buffer first! If there is data already in the buffer
    /// it will be written out on the next flush!
    pub fn with_buffer(buf: Buffer, inner: W) -> LineWriter<W> {
        LineWriter(BufWriter::with_buffer(buf, inner).set_policy(FlushOnNewline))
    }

    /// Get a reference to the inner writer.
    pub fn get_ref(&self) -> &W {
        self.0.get_ref()
    }

    /// Get a mutable reference to the inner writer.
    ///
    /// ### Note
    /// If the buffer has not been flushed, writing directly to the inner type will cause
    /// data inconsistency.
    pub fn get_mut(&mut self) -> &mut W {
        self.0.get_mut()
    }

    /// Get the capacity of the inner buffer.
    pub fn capacity(&self) -> usize {
        self.0.capacity()
    }

    /// Get the number of bytes currently in the buffer.
    pub fn buf_len(&self) -> usize {
        self.0.buf_len()
    }

    /// Ensure enough space in the buffer for *at least* `additional` bytes. May not be
    /// quite exact due to implementation details of the buffer's allocator.
    pub fn reserve(&mut self, additional: usize) {
        self.0.reserve(additional);
    }

    /// Flush the buffer and unwrap, returning the inner writer on success,
    /// or a type wrapping `self` plus the error otherwise.
    pub fn into_inner(self) -> Result<W, IntoInnerError<Self>> {
        self.0.into_inner()
            .map_err(|IntoInnerError(inner, e)| IntoInnerError(LineWriter(inner), e))
    }

    /// Flush the buffer and unwrap, returning the inner writer and
    /// any error encountered during flushing.
    pub fn into_inner_with_err(self) -> (W, Option<io::Error>) {
        self.0.into_inner_with_err()
    }

    /// Consume `self` and return both the underlying writer and the buffer.
    pub fn into_inner_with_buf(self) -> (W, Buffer){
        self.0.into_inner_with_buffer()
    }
}

impl<W: Write> Write for LineWriter<W> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.0.write(buf)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.0.flush()
    }
}

impl<W: Write + fmt::Debug> fmt::Debug for LineWriter<W> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.debug_struct("buf_redux::LineWriter")
            .field("writer", self.get_ref())
            .field("capacity", &self.capacity())
            .finish()
    }
}

/// The error type for `BufWriter::into_inner()`,
/// contains the `BufWriter` as well as the error that occurred.
#[derive(Debug)]
pub struct IntoInnerError<W>(pub W, pub io::Error);

impl<W> IntoInnerError<W> {
    /// Get the error
    pub fn error(&self) -> &io::Error {
        &self.1
    }

    /// Take the writer.
    pub fn into_inner(self) -> W {
        self.0
    }
}

impl<W> Into<io::Error> for IntoInnerError<W> {
    fn into(self) -> io::Error {
        self.1
    }
}

impl<W: Any + Send + fmt::Debug> error::Error for IntoInnerError<W> {
    fn description(&self) -> &str {
        error::Error::description(self.error())
    }

    fn cause(&self) -> Option<&error::Error> {
        Some(&self.1)
    }
}

impl<W> fmt::Display for IntoInnerError<W> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        self.error().fmt(f)
    }
}

/// A deque-like datastructure for managing bytes.
///
/// Supports interacting via I/O traits like `Read` and `Write`, and direct access.
pub struct Buffer {
    buf: BufImpl,
    zeroed: usize,
}

impl Buffer {
    /// Create a new buffer with a default capacity.
    pub fn new() -> Self {
        Self::with_capacity(DEFAULT_BUF_SIZE)
    }

    /// Create a new buffer with *at least* the given capacity.
    ///
    /// If the global allocator returns extra capacity, `Buffer` will use all of it.
    pub fn with_capacity(cap: usize) -> Self {
        Buffer {
            buf: BufImpl::with_capacity(cap),
            zeroed: 0,
        }
    }

    /// Allocate a buffer with a default capacity that never needs to move data to make room
    /// (consuming from the head simultaneously makes more room at the tail).
    ///
    /// The default capacity varies based on the target platform:
    ///
    /// * Unix-derivative platforms; Linux, OS X, BSDs, etc: **8KiB** (the default buffer size for
    /// `std::io` buffered types)
    /// * Windows: **64KiB** because of legacy reasons, of course (see below)
    ///
    /// Only available on platforms with virtual memory support and with the `slice-deque` feature
    /// enabled. The current platforms that are supported/tested are listed
    /// [in the README for the `slice-deque` crate][slice-deque].
    ///
    /// [slice-deque]: https://github.com/gnzlbg/slice_deque#platform-support
    #[cfg(feature = "slice-deque")]
    pub fn new_ringbuf() -> Self {
        Self::with_capacity_ringbuf(DEFAULT_BUF_SIZE)
    }

    /// Allocate a buffer with *at least* the given capacity that never needs to move data to
    /// make room (consuming from the head simultaneously makes more room at the tail).
    ///
    /// The capacity will be rounded up to the minimum size for the current target:
    ///
    /// * Unix-derivative platforms; Linux, OS X, BSDs, etc: the next multiple of the page size
    /// (typically 4KiB but can vary based on system configuration)
    /// * Windows: the next muliple of **64KiB**; see [this Microsoft dev blog post][Win-why-64k]
    /// for why it's 64KiB and not the page size (TL;DR: Alpha AXP needs it and it's applied on
    /// all targets for consistency/portability)
    ///
    /// [Win-why-64k]: https://blogs.msdn.microsoft.com/oldnewthing/20031008-00/?p=42223
    ///
    /// Only available on platforms with virtual memory support and with the `slice-deque` feature
    /// enabled. The current platforms that are supported/tested are listed
    /// [in the README for the `slice-deque` crate][slice-deque].
    ///
    /// [slice-deque]: https://github.com/gnzlbg/slice_deque#platform-support
    #[cfg(feature = "slice-deque")]
    pub fn with_capacity_ringbuf(cap: usize) -> Self {
        Buffer {
            buf: BufImpl::with_capacity_ringbuf(cap),
            zeroed: 0,
        }
    }

    /// Return `true` if this is a ringbuffer.
    pub fn is_ringbuf(&self) -> bool {
        self.buf.is_ringbuf()
    }

    /// Return the number of bytes currently in this buffer.
    ///
    /// Equivalent to `self.buf().len()`.
    pub fn len(&self) -> usize {
        self.buf.len()
    }

    /// Return the number of bytes that can be read into this buffer before it needs
    /// to grow or the data in the buffer needs to be moved.
    ///
    /// This may not constitute all free space in the buffer if bytes have been consumed
    /// from the head. Use `free_space()` to determine the total free space in the buffer.
    pub fn usable_space(&self) -> usize {
        self.buf.usable_space()
    }

    /// Returns the total amount of free space in the buffer, including bytes
    /// already consumed from the head.
    ///
    /// This will be greater than or equal to `usable_space()`. On supported platforms
    /// with the `slice-deque` feature enabled, it should be equal.
    pub fn free_space(&self) -> usize {
        self.capacity() - self.len()
    }

    /// Return the total capacity of this buffer.
    pub fn capacity(&self) -> usize {
        self.buf.capacity()
    }

    /// Returns `true` if there are no bytes in the buffer, false otherwise.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Move bytes down in the buffer to maximize usable space.
    ///
    /// This is a no-op on supported platforms with the `slice-deque` feature enabled.
    pub fn make_room(&mut self) {
        self.buf.make_room();
    }

    /// Ensure space for at least `additional` more bytes in the buffer.
    ///
    /// This is a no-op if `usable_space() >= additional`. Note that this will reallocate
    /// even if there is enough free space at the head of the buffer for `additional` bytes,
    /// because that free space is not at the tail where it can be read into.
    /// If you prefer copying data down in the buffer before attempting to reallocate you may wish
    /// to call `.make_room()` first.
    ///
    /// ### Panics
    /// If `self.capacity() + additional` overflows.
    pub fn reserve(&mut self, additional: usize) {
        // Returns `true` if we reallocated out-of-place and thus need to re-zero.
        if self.buf.reserve(additional) {
            self.zeroed = 0;
        }
    }

    /// Get an immutable slice of the available bytes in this buffer.
    ///
    /// Call `.consume()` to remove bytes from the beginning of this slice.
    pub fn buf(&self) -> &[u8] { self.buf.buf() }

    /// Get a mutable slice representing the available bytes in this buffer.
    ///
    /// Call `.consume()` to remove bytes from the beginning of this slice.
    pub fn buf_mut(&mut self) -> &mut [u8] { self.buf.buf_mut() }

    /// Read from `rdr`, returning the number of bytes read or any errors.
    ///
    /// If there is no more room at the head of the buffer, this will return `Ok(0)`.
    ///
    /// Uses `Read::initializer()` to initialize the buffer if the `nightly`
    /// feature is enabled, otherwise the buffer is zeroed if it has never been written.
    ///
    /// ### Panics
    /// If the returned count from `rdr.read()` overflows the tail cursor of this buffer.
    pub fn read_from<R: Read + ?Sized>(&mut self, rdr: &mut R) -> io::Result<usize> {
        if self.usable_space() == 0 {
            return Ok(0);
        }

        let cap = self.capacity();
        if self.zeroed < cap {
            unsafe {
                let buf = self.buf.write_buf();
                init_buffer(&rdr, buf);
            }

            self.zeroed = cap;
        }

        let read = {
            let mut buf = unsafe { self.buf.write_buf() };
            rdr.read(buf)?
        };

        unsafe {
            self.buf.bytes_written(read);
        }

        Ok(read)
    }

    /// Copy from `src` to the tail of this buffer. Returns the number of bytes copied.
    ///
    /// This will **not** grow the buffer if `src` is larger than `self.usable_space()`; instead,
    /// it will fill the usable space and return the number of bytes copied. If there is no usable
    /// space, this returns 0.
    pub fn copy_from_slice(&mut self, src: &[u8]) -> usize {
        let len = unsafe {
            let mut buf = self.buf.write_buf();
            let len = cmp::min(buf.len(), src.len());
            buf[..len].copy_from_slice(&src[..len]);
            len
        };

        unsafe {
            self.buf.bytes_written(len);
        }

        len
    }

    /// Write bytes from this buffer to `wrt`. Returns the number of bytes written or any errors.
    ///
    /// If the buffer is empty, returns `Ok(0)`.
    ///
    /// ### Panics
    /// If the count returned by `wrt.write()` would cause the head cursor to overflow or pass
    /// the tail cursor if added to it.
    pub fn write_to<W: Write + ?Sized>(&mut self, wrt: &mut W) -> io::Result<usize> {
        if self.len() == 0 {
            return Ok(0);
        }

        let written = wrt.write(self.buf())?;
        self.consume(written);
        Ok(written)
    }

    /// Write, at most, the given number of bytes from this buffer to `wrt`, continuing
    /// to write and ignoring interrupts until the number is reached or the buffer is empty.
    ///
    /// ### Panics
    /// If the count returned by `wrt.write()` would cause the head cursor to overflow or pass
    /// the tail cursor if added to it.
    pub fn write_max<W: Write + ?Sized>(&mut self, mut max: usize, wrt: &mut W) -> io::Result<()> {
        while self.len() > 0 && max > 0 {
            let len = cmp::min(self.len(), max);
            let n = match wrt.write(&self.buf()[..len]) {
                Ok(0) => return Err(io::Error::new(io::ErrorKind::WriteZero,
                                                   "Buffer::write_all() got zero-sized write")),
                Ok(n) => n,
                Err(ref e) if e.kind() == io::ErrorKind::Interrupted => continue,
                Err(e) => return Err(e),
            };

            self.consume(n);
            max = max.saturating_sub(n);
        }

        Ok(())
    }

    /// Write all bytes in this buffer to `wrt`, ignoring interrupts. Continues writing until
    /// the buffer is empty or an error is returned.
    ///
    /// ### Panics
    /// If `self.write_to(wrt)` panics.
    pub fn write_all<W: Write + ?Sized>(&mut self, wrt: &mut W) -> io::Result<()> {
        while self.len() > 0 {
            match self.write_to(wrt) {
                Ok(0) => return Err(io::Error::new(io::ErrorKind::WriteZero,
                                                   "Buffer::write_all() got zero-sized write")),
                Ok(_) => (),
                Err(ref e) if e.kind() == io::ErrorKind::Interrupted => (),
                Err(e) => return Err(e),
            }
        }

        Ok(())
    }

    /// Copy bytes to `out` from this buffer, returning the number of bytes written.
    pub fn copy_to_slice(&mut self, out: &mut [u8]) -> usize {
        let len = {
            let buf = self.buf();

            let len = cmp::min(buf.len(), out.len());
            out[..len].copy_from_slice(&buf[..len]);
            len
        };

        self.consume(len);

        len
    }

    /// Push `bytes` to the end of the buffer, growing it if necessary.
    ///
    /// If you prefer moving bytes down in the buffer to reallocating, you may wish to call
    /// `.make_room()` first.
    pub fn push_bytes(&mut self, bytes: &[u8]) {
        let s_len = bytes.len();

        if self.usable_space() < s_len {
            self.reserve(s_len * 2);
        }

        unsafe {
            self.buf.write_buf()[..s_len].copy_from_slice(bytes);
            self.buf.bytes_written(s_len);
        }
    }

    /// Consume `amt` bytes from the head of this buffer.
    pub fn consume(&mut self, amt: usize) {
        self.buf.consume(amt);
    }

    /// Empty this buffer by consuming all bytes.
    pub fn clear(&mut self) {
        let buf_len = self.len();
        self.consume(buf_len);
    }
}

impl fmt::Debug for Buffer {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.debug_struct("buf_redux::Buffer")
            .field("capacity", &self.capacity())
            .field("len", &self.len())
            .finish()
    }
}

/// A `Read` adapter for a consumed `BufReader` which will empty bytes from the buffer before
/// reading from `R` directly. Frees the buffer when it has been emptied.
pub struct Unbuffer<R> {
    inner: R,
    buf: Option<Buffer>,
}

impl<R> Unbuffer<R> {
    /// Returns `true` if the buffer still has some bytes left, `false` otherwise.
    pub fn is_buf_empty(&self) -> bool {
        !self.buf.is_some()
    }

    /// Returns the number of bytes remaining in the buffer.
    pub fn buf_len(&self) -> usize {
        self.buf.as_ref().map(Buffer::len).unwrap_or(0)
    }

    /// Get a slice over the available bytes in the buffer.
    pub fn buf(&self) -> &[u8] {
        self.buf.as_ref().map_or(&[], Buffer::buf)
    }

    /// Return the underlying reader, releasing the buffer.
    pub fn into_inner(self) -> R {
        self.inner
    }
}

impl<R: Read> Read for Unbuffer<R> {
    fn read(&mut self, out: &mut [u8]) -> io::Result<usize> {
        if let Some(ref mut buf) = self.buf.as_mut() {
            let read = buf.copy_to_slice(out);

            if out.len() != 0 && read != 0 {
                return Ok(read);
            }
        }

        self.buf = None;

        self.inner.read(out)
    }
}

impl<R: fmt::Debug> fmt::Debug for Unbuffer<R> {
    fn fmt(&self, fmt: &mut fmt::Formatter) -> fmt::Result {
        fmt.debug_struct("buf_redux::Unbuffer")
            .field("reader", &self.inner)
            .field("buffer", &self.buf)
            .finish()
    }
}

/// Copy data between a `BufRead` and a `Write` without an intermediate buffer.
///
/// Retries on interrupts. Returns the total bytes copied or the first error;
/// even if an error is returned some bytes may still have been copied.
pub fn copy_buf<B: BufRead, W: Write>(b: &mut B, w: &mut W) -> io::Result<u64> {
    let mut total_copied = 0;

    loop {
        let copied = match b.fill_buf().and_then(|buf| w.write(buf)) {
            Err(ref e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(e),
            Ok(buf) => buf,
        };

        if copied == 0 { break; }

        b.consume(copied);

        total_copied += copied as u64;
    }

    Ok(total_copied)
}

thread_local!(
    static DROP_ERR_HANDLER: RefCell<Box<Fn(&mut Write, &mut Buffer, io::Error)>>
        = RefCell::new(Box::new(|_, _, _| ()))
);

/// Set a thread-local handler for errors thrown in `BufWriter`'s `Drop` impl.
///
/// The `Write` impl, buffer (at the time of the erroring write) and IO error are provided.
///
/// Replaces the previous handler. By default this is a no-op.
///
/// ### Panics
/// If called from within a handler previously provided to this function.
pub fn set_drop_err_handler<F: 'static>(handler: F)
where F: Fn(&mut Write, &mut Buffer, io::Error)
{
    DROP_ERR_HANDLER.with(|deh| *deh.borrow_mut() = Box::new(handler))
}

#[cfg(not(feature = "nightly"))]
fn init_buffer<R: Read + ?Sized>(_r: &R, buf: &mut [u8]) {
    // we can't trust a reader without nightly
    safemem::write_bytes(buf, 0);
}
