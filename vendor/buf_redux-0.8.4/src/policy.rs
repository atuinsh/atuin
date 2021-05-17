// Copyright 2016-2018 Austin Bonander <austin.bonander@gmail.com>
//
// Licensed under the Apache License, Version 2.0 <LICENSE-APACHE or
// http://www.apache.org/licenses/LICENSE-2.0> or the MIT license
// <LICENSE-MIT or http://opensource.org/licenses/MIT>, at your
// option. This file may not be copied, modified, or distributed
// except according to those terms.
//! Types which can be used to tune the behavior of `BufReader` and `BufWriter`.
//!
//! Some simple policies are provided for your convenience. You may prefer to create your own
//! types and implement the traits for them instead.

use super::Buffer;

/// Flag for `ReaderPolicy` methods to signal whether or not `BufReader` should read into
/// the buffer.
///
/// See `do_read!()` for a shorthand.
#[derive(Copy, Clone, Debug)]
pub struct DoRead(pub bool);

/// Shorthand for `return DoRead(bool)` or `return DoRead(true)` (empty invocation)
#[macro_export]
macro_rules! do_read (
    ($val:expr) => ( return $crate::policy::DoRead($val); );
    () => ( do_read!(true); )
);

/// Default policy for both `BufReader` and `BufWriter` that reproduces the behaviors of their
/// `std::io` counterparts:
///
/// * `BufReader`: only reads when the buffer is empty, does not resize or move data.
/// * `BufWriter`: only flushes the buffer when there is not enough room for an incoming write.
#[derive(Debug, Default)]
pub struct StdPolicy;

/// Trait that governs `BufReader`'s behavior.
pub trait ReaderPolicy {
    /// Consulted before attempting to read into the buffer.
    ///
    /// Return `DoRead(true)` to issue a read into the buffer before reading data out of it,
    /// or `DoRead(false)` to read from the buffer as it is, even if it's empty.
    /// `do_read!()` is provided as a shorthand.
    ///
    /// If there is no room in the buffer after this method is called,
    /// the buffer will not be read into (so if the buffer is full but you want more data
    /// you should call `.make_room()` or reserve more space). If there *is* room, `BufReader` will
    /// attempt to read into the buffer. If successful (`Ok(x)` where `x > 0` is returned), this
    /// method will be consulted again for another read attempt.
    ///
    /// By default, this implements `std::io::BufReader`'s behavior: only read into the buffer if
    /// it is empty.
    ///
    /// ### Note
    /// If the read will ignore the buffer entirely (if the buffer is empty and the amount to be
    /// read matches or exceeds its capacity) or if `BufReader::read_into_buf()` was called to force
    /// a read into the buffer manually, this method will not be called.
    fn before_read(&mut self, buffer: &mut Buffer) -> DoRead { DoRead(buffer.len() == 0) }

    /// Called after bytes are consumed from the buffer.
    ///
    /// Supplies the true amount consumed if the amount passed to `BufReader::consume`
    /// was in excess.
    ///
    /// This is a no-op by default.
    fn after_consume(&mut self, _buffer: &mut Buffer, _amt: usize) {}
}

/// Behavior of `std::io::BufReader`: the buffer will only be read into if it is empty.
impl ReaderPolicy for StdPolicy {}

/// A policy for [`BufReader`](::BufReader) which ensures there is at least the given number of
/// bytes in  the buffer, failing this only if the reader is at EOF.
///
/// If the minimum buffer length is greater than the buffer capacity, it will be resized.
///
/// ### Example
/// ```rust
/// use buf_redux::BufReader;
/// use buf_redux::policy::MinBuffered;
/// use std::io::{BufRead, Cursor};
/// 
/// let data = (1 .. 16).collect::<Vec<u8>>();
///
/// // normally you should use `BufReader::new()` or give a capacity of several KiB or more
/// let mut reader = BufReader::with_capacity(8, Cursor::new(data))
///     // always at least 4 bytes in the buffer (or until the source is empty)
///     .set_policy(MinBuffered(4)); // always at least 4 bytes in the buffer
///
/// // first buffer fill, same as `std::io::BufReader`
/// assert_eq!(reader.fill_buf().unwrap(), &[1, 2, 3, 4, 5, 6, 7, 8]);
/// reader.consume(3);
///
/// // enough data in the buffer, another read isn't done yet
/// assert_eq!(reader.fill_buf().unwrap(), &[4, 5, 6, 7, 8]);
/// reader.consume(4);
///
/// // `std::io::BufReader` would return `&[8]`
/// assert_eq!(reader.fill_buf().unwrap(), &[8, 9, 10, 11, 12, 13, 14, 15]);
/// reader.consume(5);
///
/// // no data left in the reader
/// assert_eq!(reader.fill_buf().unwrap(), &[13, 14, 15]);
/// ```
#[derive(Debug)]
pub struct MinBuffered(pub usize);

impl MinBuffered {
    /// Set the number of bytes to ensure are in the buffer.
    pub fn set_min(&mut self, min: usize) {
        self.0 = min;
    }
}

impl ReaderPolicy for MinBuffered {
    fn before_read(&mut self, buffer: &mut Buffer) -> DoRead {
        // do nothing if we have enough data
        if buffer.len() >= self.0 { do_read!(false) }

        let cap = buffer.capacity();

        // if there's enough room but some of it's stuck after the head
        if buffer.usable_space() < self.0 && buffer.free_space() >= self.0 {
            buffer.make_room();
        } else if cap < self.0 {
            buffer.reserve(self.0 - cap);
        }

        DoRead(true)
    }
}

/// Flag for `WriterPolicy` methods to tell `BufWriter` how many bytes to flush to the
/// underlying reader.
///
/// See `flush_amt!()` for a shorthand.
#[derive(Copy, Clone, Debug)]
pub struct FlushAmt(pub usize);

/// Shorthand for `return FlushAmt(n)` or `return FlushAmt(0)` (empty invocation)
#[macro_export]
macro_rules! flush_amt (
    ($n:expr) => ( return $crate::policy::FlushAmt($n); );
    () => ( flush_amt!(0); )
);

/// A trait which tells `BufWriter` when to flush.
pub trait WriterPolicy {
    /// Return `FlushAmt(n > 0)` if the buffer should be flushed before reading into it.
    /// If the returned amount is 0 or greater than the amount of buffered data, no flush is
    /// performed.
    ///
    /// The buffer is provided, as well as `incoming` which is
    /// the size of the buffer that will be written to the `BufWriter`.
    ///
    /// By default, flushes the buffer if the usable space is smaller than the incoming write.
    fn before_write(&mut self, buf: &mut Buffer, incoming: usize) -> FlushAmt {
        FlushAmt(if incoming > buf.usable_space() { buf.len() } else { 0 })
    }

    /// Return `true` if the buffer should be flushed after reading into it.
    ///
    /// `buf` references the updated buffer after the read.
    ///
    /// Default impl is a no-op.
    fn after_write(&mut self, _buf: &Buffer) -> FlushAmt {
        FlushAmt(0)
    }
}

/// Default behavior of `std::io::BufWriter`: flush before a read into the buffer
/// only if the incoming data is larger than the buffer's writable space.
impl WriterPolicy for StdPolicy {}

/// Flush the buffer if it contains at least the given number of bytes.
#[derive(Debug, Default)]
pub struct FlushAtLeast(pub usize);

impl WriterPolicy for FlushAtLeast {
    fn before_write(&mut self, buf: &mut Buffer, incoming: usize) -> FlushAmt {
        ensure_capacity(buf, self.0);
        FlushAmt(if incoming > buf.usable_space() { buf.len() } else { 0 })
    }

    fn after_write(&mut self, buf: &Buffer) -> FlushAmt {
        FlushAmt(::std::cmp::max(buf.len(), self.0))
    }
}

/// Only ever flush exactly the given number of bytes, until the writer is empty.
#[derive(Debug, Default)]
pub struct FlushExact(pub usize);

impl WriterPolicy for FlushExact {
    /// Flushes the buffer if there is not enough room to fit `incoming` bytes,
    /// but only when the buffer contains at least `self.0` bytes.
    ///
    /// Otherwise, calls [`Buffer::make_room()`](::Buffer::make_room)
    fn before_write(&mut self, buf: &mut Buffer, incoming: usize) -> FlushAmt {
        ensure_capacity(buf, self.0);

        // don't have enough room to fit the additional bytes but we can't flush,
        // then make room for (at least some of) the incoming bytes.
        if incoming > buf.usable_space() && buf.len() < self.0 {
            buf.make_room();
        }

        FlushAmt(self.0)
    }

    /// Flushes the given amount if possible, nothing otherwise.
    fn after_write(&mut self, _buf: &Buffer) -> FlushAmt {
        FlushAmt(self.0)
    }
}

/// Flush the buffer if it contains the given byte.
///
/// Only scans the buffer after reading. Searches from the end first.
#[derive(Debug, Default)]
pub struct FlushOn(pub u8);

impl WriterPolicy for FlushOn {
    fn after_write(&mut self, buf: &Buffer) -> FlushAmt {
        // include the delimiter in the flush
        FlushAmt(::memchr::memrchr(self.0, buf.buf()).map_or(0, |n| n + 1))
    }
}

/// Flush the buffer if it contains a newline (`\n`).
///
/// Equivalent to `FlushOn(b'\n')`.
#[derive(Debug, Default)]
pub struct FlushOnNewline;

impl WriterPolicy for FlushOnNewline {
    fn after_write(&mut self, buf: &Buffer) -> FlushAmt {
        FlushAmt(::memchr::memrchr(b'\n', buf.buf()).map_or(0, |n| n + 1))
    }
}

fn ensure_capacity(buf: &mut Buffer, min_cap: usize) {
    let cap = buf.capacity();

    if cap < min_cap {
        buf.reserve(min_cap - cap);
    }
}

#[cfg(test)]
mod test {
    use {BufReader, BufWriter};
    use policy::*;
    use std::io::{BufRead, Cursor, Write};

    #[test]
    fn test_min_buffered() {
        let min_buffered = 4;
        let data = (0 .. 20).collect::<Vec<u8>>();
        // create a reader with 0 capacity
        let mut reader = BufReader::with_capacity(0, Cursor::new(data))
            .set_policy(MinBuffered(min_buffered));

        // policy reserves the required space in the buffer
        assert_eq!(reader.fill_buf().unwrap(), &[0, 1, 2, 3][..]);
        assert_eq!(reader.capacity(), min_buffered);

        // double the size now that the buffer's full
        reader.reserve(min_buffered);
        assert_eq!(reader.capacity(), min_buffered * 2);

        // we haven't consumed anything, the reader should have the same data
        assert_eq!(reader.fill_buf().unwrap(), &[0, 1, 2, 3]);
        reader.consume(2);
        // policy read more data, `std::io::BufReader` doesn't do that
        assert_eq!(reader.fill_buf().unwrap(), &[2, 3, 4, 5, 6, 7]);
        reader.consume(4);
        // policy made room and read more
        assert_eq!(reader.fill_buf().unwrap(), &[6, 7, 8, 9, 10, 11, 12, 13]);
        reader.consume(4);
        assert_eq!(reader.fill_buf().unwrap(), &[10, 11, 12, 13]);
        reader.consume(2);
        assert_eq!(reader.fill_buf().unwrap(), &[12, 13, 14, 15, 16, 17, 18, 19]);
        reader.consume(8);
        assert_eq!(reader.fill_buf().unwrap(), &[])
    }

    #[test]
    fn test_flush_at_least() {
        let flush_min = 4;

        let mut writer = BufWriter::with_capacity(0, vec![]).set_policy(FlushAtLeast(flush_min));
        assert_eq!(writer.capacity(), 0);
        assert_eq!(writer.write(&[1]).unwrap(), 1);
        // policy reserved space for writing
        assert_eq!(writer.capacity(), flush_min);
        // one byte in buffer, we want to double our capacity
        writer.reserve(flush_min * 2 - 1);
        assert_eq!(writer.capacity(), flush_min * 2);

        assert_eq!(writer.write(&[2, 3]).unwrap(), 2);
        // no flush yet, only 3 bytes in buffer
        assert_eq!(*writer.get_ref(), &[]);

        assert_eq!(writer.write(&[4, 5, 6]).unwrap(), 3);
        // flushed all
        assert_eq!(*writer.get_ref(), &[1, 2, 3, 4, 5, 6]);

        assert_eq!(writer.write(&[7, 8, 9]).unwrap(), 3);
        // `.into_inner()` should flush always
        assert_eq!(writer.into_inner().unwrap(), &[1, 2, 3, 4, 5, 6, 7, 8, 9]);
    }

    #[test]
    fn test_flush_exact() {
        let flush_exact = 4;

        let mut writer = BufWriter::with_capacity(0, vec![]).set_policy(FlushExact(flush_exact));
        assert_eq!(writer.capacity(), 0);
        assert_eq!(writer.write(&[1]).unwrap(), 1);
        // policy reserved space for writing
        assert_eq!(writer.capacity(), flush_exact);
        // one byte in buffer, we want to double our capacity
        writer.reserve(flush_exact * 2 - 1);
        assert_eq!(writer.capacity(), flush_exact * 2);

        assert_eq!(writer.write(&[2, 3]).unwrap(), 2);
        // no flush yet, only 3 bytes in buffer
        assert_eq!(*writer.get_ref(), &[]);

        assert_eq!(writer.write(&[4, 5, 6]).unwrap(), 3);
        // flushed exactly 4 bytes
        assert_eq!(*writer.get_ref(), &[1, 2, 3, 4]);

        assert_eq!(writer.write(&[7, 8, 9, 10]).unwrap(), 4);
        // flushed another 4 bytes
        assert_eq!(*writer.get_ref(), &[1, 2, 3, 4, 5, 6, 7, 8]);
        // `.into_inner()` should flush always
        assert_eq!(writer.into_inner().unwrap(), &[1, 2, 3, 4, 5, 6, 7, 8, 9, 10]);
    }

    #[test]
    fn test_flush_on() {
        let mut writer = BufWriter::with_capacity(8, vec![]).set_policy(FlushOn(0));

        assert_eq!(writer.write(&[1, 2, 3]).unwrap(), 3);
        assert_eq!(*writer.get_ref(), &[]);

        assert_eq!(writer.write(&[0, 4, 5]).unwrap(), 3);
        assert_eq!(*writer.get_ref(), &[1, 2, 3, 0]);

        assert_eq!(writer.write(&[6, 7, 8, 9, 10, 11, 12]).unwrap(), 7);
        assert_eq!(*writer.get_ref(), &[1, 2, 3, 0, 4, 5]);

        assert_eq!(writer.write(&[0]).unwrap(), 1);
        assert_eq!(*writer.get_ref(), &[1, 2, 3, 0, 4, 5, 6, 7, 8, 9, 10, 11, 12, 0]);
    }

    #[test]
    fn test_flush_on_newline() {
        let mut writer = BufWriter::with_capacity(8, vec![]).set_policy(FlushOnNewline);

        assert_eq!(writer.write(&[1, 2, 3]).unwrap(), 3);
        assert_eq!(*writer.get_ref(), &[]);

        assert_eq!(writer.write(&[b'\n', 4, 5]).unwrap(), 3);
        assert_eq!(*writer.get_ref(), &[1, 2, 3, b'\n']);

        assert_eq!(writer.write(&[6, 7, 8, 9, b'\n', 11, 12]).unwrap(), 7);
        assert_eq!(*writer.get_ref(), &[1, 2, 3, b'\n', 4, 5, 6, 7, 8, 9, b'\n']);

        assert_eq!(writer.write(&[b'\n']).unwrap(), 1);
        assert_eq!(*writer.get_ref(), &[1, 2, 3, b'\n', 4, 5, 6, 7, 8, 9, b'\n', 11, 12, b'\n']);
    }
}
