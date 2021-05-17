// Original implementation Copyright 2013 The Rust Project Developers <https://github.com/rust-lang>
//
// Original source file: https://github.com/rust-lang/rust/blob/master/src/libstd/io/buffered.rs
//
// Modifications copyright 2018 Austin Bonander <austin.bonander@gmail.com>
//
// Licensed under the Apache License, Version 2.0 <LICENSE-APACHE or
// http://www.apache.org/licenses/LICENSE-2.0> or the MIT license
// <LICENSE-MIT or http://opensource.org/licenses/MIT>, at your
// option. This file may not be copied, modified, or distributed
// except according to those terms.

//! Tests checking `Buffer::new_ringbuf()` and friends.
//!
//! Some may be adapted from rust/src/libstd/io/buffered.rs
//!
//! Since `SliceDeque` rounds allocations up to the page size or larger, these cannot assume
//! a small capacity like `std_test` does.

// TODO: add tests centered around the mirrored buf boundary

use std::io::prelude::*;
use std::io::{self, SeekFrom};

use {Buffer, BufReader, DEFAULT_BUF_SIZE};

use std_tests::ShortReader;

macro_rules! assert_capacity {
    ($buf:expr, $cap:expr) => {
        let cap = $buf.capacity();
            if cfg!(windows) {
            // Windows' minimum allocation size is 64K
            assert_eq!(cap, ::std::cmp::max(64 * 1024, cap));
        } else {
            assert_eq!(cap, $cap);
        }
    }
}

#[test]
fn test_buffer_new() {
    let buf = Buffer::new_ringbuf();
    assert_capacity!(buf, DEFAULT_BUF_SIZE);
    assert_eq!(buf.capacity(), buf.usable_space());
}

#[test]
fn test_buffer_with_cap() {
    let buf = Buffer::with_capacity_ringbuf(4 * 1024);
    assert_capacity!(buf, 4 * 1024);

    // test rounding up to page size
    let buf = Buffer::with_capacity_ringbuf(64);
    assert_capacity!(buf, 4 * 1024);
    assert_eq!(buf.capacity(), buf.usable_space());
}

#[test]
fn test_buffered_reader() {
    let inner: &[u8] = &[5, 6, 7, 0, 1, 2, 3, 4];
    let mut reader = BufReader::new_ringbuf(inner);

    let mut buf = [0, 0, 0];
    let nread = reader.read(&mut buf);
    assert_eq!(nread.unwrap(), 3);
    let b: &[_] = &[5, 6, 7];
    assert_eq!(buf, b);

    let mut buf = [0, 0];
    let nread = reader.read(&mut buf);
    assert_eq!(nread.unwrap(), 2);
    let b: &[_] = &[0, 1];
    assert_eq!(buf, b);

    let mut buf = [0];
    let nread = reader.read(&mut buf);
    assert_eq!(nread.unwrap(), 1);
    let b: &[_] = &[2];
    assert_eq!(buf, b);

    let mut buf = [0, 0, 0];
    let nread = reader.read(&mut buf);
    assert_eq!(nread.unwrap(), 2);
    let b: &[_] = &[3, 4, 0];
    assert_eq!(buf, b);

    assert_eq!(reader.read(&mut buf).unwrap(), 0);
}

#[test]
fn test_buffered_reader_seek() {
    let inner: &[u8] = &[5, 6, 7, 0, 1, 2, 3, 4];
    let mut reader = BufReader::new_ringbuf(io::Cursor::new(inner));

    assert_eq!(reader.seek(SeekFrom::Start(3)).ok(), Some(3));
    assert_eq!(reader.fill_buf().ok(), Some(&[0, 1, 2, 3, 4][..]));
    assert_eq!(reader.seek(SeekFrom::Current(0)).ok(), Some(3));
    assert_eq!(reader.fill_buf().ok(), Some(&[0, 1, 2, 3, 4][..]));
    assert_eq!(reader.seek(SeekFrom::Current(1)).ok(), Some(4));
    assert_eq!(reader.fill_buf().ok(), Some(&[1, 2, 3, 4][..]));
    reader.consume(1);
    assert_eq!(reader.seek(SeekFrom::Current(-2)).ok(), Some(3));
    assert_eq!(reader.fill_buf().ok(), Some(&[0, 1, 2, 3, 4][..]));
}

#[test]
fn test_buffered_reader_seek_underflow() {
    // gimmick reader that yields its position modulo 256 for each byte
    struct PositionReader {
        pos: u64
    }
    impl Read for PositionReader {
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            let len = buf.len();
            for x in buf {
                *x = self.pos as u8;
                self.pos = self.pos.wrapping_add(1);
            }
            Ok(len)
        }
    }
    impl Seek for PositionReader {
        fn seek(&mut self, pos: SeekFrom) -> io::Result<u64> {
            match pos {
                SeekFrom::Start(n) => {
                    self.pos = n;
                }
                SeekFrom::Current(n) => {
                    self.pos = self.pos.wrapping_add(n as u64);
                }
                SeekFrom::End(n) => {
                    self.pos = u64::max_value().wrapping_add(n as u64);
                }
            }
            Ok(self.pos)
        }
    }

    let mut reader = BufReader::with_capacity(5, PositionReader { pos: 0 });
    assert_eq!(reader.fill_buf().ok(), Some(&[0, 1, 2, 3, 4][..]));
    assert_eq!(reader.seek(SeekFrom::End(-5)).ok(), Some(u64::max_value()-5));
    assert_eq!(reader.fill_buf().ok().map(|s| s.len()), Some(5));
    // the following seek will require two underlying seeks
    let expected = 9223372036854775802;
    assert_eq!(reader.seek(SeekFrom::Current(i64::min_value())).ok(), Some(expected));
    assert_eq!(reader.fill_buf().ok().map(|s| s.len()), Some(5));
    // seeking to 0 should empty the buffer.
    assert_eq!(reader.seek(SeekFrom::Current(0)).ok(), Some(expected));
    assert_eq!(reader.get_ref().pos, expected);
}

#[test]
fn test_read_until() {
    let inner: &[u8] = &[0, 1, 2, 1, 0];
    let mut reader = BufReader::with_capacity(2, inner);
    let mut v = Vec::new();
    reader.read_until(0, &mut v).unwrap();
    assert_eq!(v, [0]);
    v.truncate(0);
    reader.read_until(2, &mut v).unwrap();
    assert_eq!(v, [1, 2]);
    v.truncate(0);
    reader.read_until(1, &mut v).unwrap();
    assert_eq!(v, [1]);
    v.truncate(0);
    reader.read_until(8, &mut v).unwrap();
    assert_eq!(v, [0]);
    v.truncate(0);
    reader.read_until(9, &mut v).unwrap();
    assert_eq!(v, []);
}

#[test]
fn test_read_line() {
    let in_buf: &[u8] = b"a\nb\nc";
    let mut reader = BufReader::with_capacity(2, in_buf);
    let mut s = String::new();
    reader.read_line(&mut s).unwrap();
    assert_eq!(s, "a\n");
    s.truncate(0);
    reader.read_line(&mut s).unwrap();
    assert_eq!(s, "b\n");
    s.truncate(0);
    reader.read_line(&mut s).unwrap();
    assert_eq!(s, "c");
    s.truncate(0);
    reader.read_line(&mut s).unwrap();
    assert_eq!(s, "");
}

#[test]
fn test_lines() {
    let in_buf: &[u8] = b"a\nb\nc";
    let reader = BufReader::with_capacity(2, in_buf);
    let mut it = reader.lines();
    assert_eq!(it.next().unwrap().unwrap(), "a".to_string());
    assert_eq!(it.next().unwrap().unwrap(), "b".to_string());
    assert_eq!(it.next().unwrap().unwrap(), "c".to_string());
    assert!(it.next().is_none());
}

#[test]
fn test_short_reads() {
    let inner = ShortReader{lengths: vec![0, 1, 2, 0, 1, 0]};
    let mut reader = BufReader::new(inner);
    let mut buf = [0, 0];
    assert_eq!(reader.read(&mut buf).unwrap(), 0);
    assert_eq!(reader.read(&mut buf).unwrap(), 1);
    assert_eq!(reader.read(&mut buf).unwrap(), 2);
    assert_eq!(reader.read(&mut buf).unwrap(), 0);
    assert_eq!(reader.read(&mut buf).unwrap(), 1);
    assert_eq!(reader.read(&mut buf).unwrap(), 0);
    assert_eq!(reader.read(&mut buf).unwrap(), 0);
}

#[cfg(feature = "nightly")]
#[test]
fn read_char_buffered() {
    let buf = [195, 159];
    let reader = BufReader::with_capacity(1, &buf[..]);
    assert_eq!(reader.chars().next().unwrap().unwrap(), 'ß');
}

#[cfg(feature = "nightly")]
#[test]
fn test_chars() {
    let buf = [195, 159, b'a'];
    let reader = BufReader::with_capacity(1, &buf[..]);
    let mut it = reader.chars();
    assert_eq!(it.next().unwrap().unwrap(), 'ß');
    assert_eq!(it.next().unwrap().unwrap(), 'a');
    assert!(it.next().is_none());
}

/// Test that the ringbuffer wraps as intended
#[test]
fn test_mirror_boundary() {
    // pretends the given bytes have been read
    struct FakeReader(usize);

    impl Read for FakeReader {
        fn read(&mut self, _buf: &mut [u8]) -> io::Result<usize> {
            Ok(self.0)
        }
    }

    let mut buffer = Buffer::new_ringbuf();
    let cap = buffer.capacity();

    // declaring these as variables for sanity
    let read_amt = cap; // fill the buffer
    let test_slice = &[1, 2, 3, 4, 5];
    let consume_amt = read_amt - 5; // leave several bytes on the head side of the mirror

    assert_eq!(buffer.read_from(&mut FakeReader(read_amt)).unwrap(), read_amt);
    assert_eq!(buffer.usable_space(), cap - read_amt); // should be 0
    assert_eq!(buffer.read_from(&mut FakeReader(read_amt)).unwrap(), 0); // buffer is full
    buffer.consume(consume_amt);
    assert_eq!(buffer.usable_space(), consume_amt);
    assert_eq!(buffer.copy_from_slice(test_slice), test_slice.len());

    // zeroes are the bytes we didn't consume
    assert_eq!(buffer.buf(), &[0, 0, 0, 0, 0, 1, 2, 3, 4, 5]);
    buffer.clear();
    assert_eq!(buffer.usable_space(), cap);
}

#[test]
fn issue_8(){
    let source = vec![0u8; 4096*4];

    let mut rdr = BufReader::with_capacity_ringbuf(4096, source.as_slice());

    loop {
        let n = rdr.read_into_buf().unwrap();
        if n == 0 {
            break;
        }
        rdr.consume(4000);
        // rdr.make_room(); // (only necessary with 'standard' reader)

        println!("{}", n);
    }
}

// `BufWriter` doesn't utilize a ringbuffer
