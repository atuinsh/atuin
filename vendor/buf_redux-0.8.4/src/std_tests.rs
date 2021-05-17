// Original implementation Copyright 2013 The Rust Project Developers <https://github.com/rust-lang>
//
// Original source file: https://github.com/rust-lang/rust/blob/master/src/libstd/io/buffered.rs
//
// Modifications copyright 2016-2018 Austin Bonander <austin.bonander@gmail.com>
//
// Licensed under the Apache License, Version 2.0 <LICENSE-APACHE or
// http://www.apache.org/licenses/LICENSE-2.0> or the MIT license
// <LICENSE-MIT or http://opensource.org/licenses/MIT>, at your
// option. This file may not be copied, modified, or distributed
// except according to those terms.

//! These tests are copied from rust/src/libstd/io/buffered.rs
//! They assume exact capacity allocation

use std::io::prelude::*;
use std::io::{self, SeekFrom};
use {BufReader, BufWriter, LineWriter};

/// A dummy reader intended at testing short-reads propagation.
pub struct ShortReader {
    pub lengths: Vec<usize>,
}

impl Read for ShortReader {
    fn read(&mut self, _: &mut [u8]) -> io::Result<usize> {
        if self.lengths.is_empty() {
            Ok(0)
        } else {
            Ok(self.lengths.remove(0))
        }
    }
}

#[test]
fn test_buffered_reader() {
    let inner: &[u8] = &[5, 6, 7, 0, 1, 2, 3, 4];
    let mut reader = BufReader::with_capacity(2, inner);

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
    assert_eq!(nread.unwrap(), 1);
    let b: &[_] = &[3, 0, 0];
    assert_eq!(buf, b);

    let nread = reader.read(&mut buf);
    assert_eq!(nread.unwrap(), 1);
    let b: &[_] = &[4, 0, 0];
    assert_eq!(buf, b);

    assert_eq!(reader.read(&mut buf).unwrap(), 0);
}

#[test]
fn test_buffered_reader_seek() {
    let inner: &[u8] = &[5, 6, 7, 0, 1, 2, 3, 4];
    let mut reader = BufReader::with_capacity(2, io::Cursor::new(inner));

    assert_eq!(reader.seek(SeekFrom::Start(3)).ok(), Some(3));
    assert_eq!(reader.fill_buf().ok(), Some(&[0, 1][..]));
    assert_eq!(reader.seek(SeekFrom::Current(0)).ok(), Some(3));
    assert_eq!(reader.fill_buf().ok(), Some(&[0, 1][..]));
    assert_eq!(reader.seek(SeekFrom::Current(1)).ok(), Some(4));
    assert_eq!(reader.fill_buf().ok(), Some(&[1, 2][..]));
    reader.consume(1);
    assert_eq!(reader.seek(SeekFrom::Current(-2)).ok(), Some(3));
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

// BufWriter tests
#[test]
fn test_buffered_writer() {
    let inner = Vec::new();
    let mut writer = BufWriter::with_capacity(2, inner);

    assert_eq!(writer.capacity(), 2);

    writer.write(&[0, 1]).unwrap();
    assert_eq!(*writer.get_ref(), [0, 1]);

    writer.write(&[2]).unwrap();
    assert_eq!(*writer.get_ref(), [0, 1]);

    writer.write(&[3]).unwrap();
    assert_eq!(*writer.get_ref(), [0, 1]);

    writer.flush().unwrap();
    assert_eq!(*writer.get_ref(), [0, 1, 2, 3]);

    writer.write(&[4]).unwrap();
    writer.write(&[5]).unwrap();
    assert_eq!(*writer.get_ref(), [0, 1, 2, 3]);

    writer.write(&[6]).unwrap();
    assert_eq!(*writer.get_ref(), [0, 1, 2, 3, 4, 5]);

    writer.write(&[7, 8]).unwrap();
    assert_eq!(*writer.get_ref(), [0, 1, 2, 3, 4, 5, 6, 7, 8]);

    writer.write(&[9, 10, 11]).unwrap();
    assert_eq!(*writer.get_ref(), [0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11]);

    writer.flush().unwrap();
    assert_eq!(*writer.get_ref(), [0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11]);
}

#[test]
fn test_buffered_writer_inner_flushes() {
    let mut w = BufWriter::with_capacity(3, Vec::new());
    w.write(&[0, 1]).unwrap();
    assert_eq!(*w.get_ref(), []);
    let w = w.into_inner().unwrap();
    assert_eq!(w, [0, 1]);
}

#[test]
fn test_buffered_writer_seek() {
    let mut w = BufWriter::with_capacity(3, io::Cursor::new(Vec::new()));
    w.write_all(&[0, 1, 2, 3, 4, 5]).unwrap();
    w.write_all(&[6, 7]).unwrap();
    assert_eq!(w.seek(SeekFrom::Current(0)).ok(), Some(8));
    assert_eq!(&w.get_ref().get_ref()[..], &[0, 1, 2, 3, 4, 5, 6, 7][..]);
    assert_eq!(w.seek(SeekFrom::Start(2)).ok(), Some(2));
    w.write_all(&[8, 9]).unwrap();
    assert_eq!(&w.into_inner().unwrap().into_inner()[..], &[0, 1, 8, 9, 4, 5, 6, 7]);
}

#[test]
fn test_line_buffer() {
    let mut writer = LineWriter::new(Vec::new());
    writer.write(&[0]).unwrap();
    assert_eq!(*writer.get_ref(), []);
    writer.write(&[1]).unwrap();
    assert_eq!(*writer.get_ref(), []);
    writer.flush().unwrap();
    assert_eq!(*writer.get_ref(), [0, 1]);
    writer.write(&[0, b'\n', 1, b'\n', 2]).unwrap();
    assert_eq!(*writer.get_ref(), [0, 1, 0, b'\n', 1, b'\n']);
    writer.flush().unwrap();
    assert_eq!(*writer.get_ref(), [0, 1, 0, b'\n', 1, b'\n', 2]);
    writer.write(&[3, b'\n']).unwrap();
    assert_eq!(*writer.get_ref(), [0, 1, 0, b'\n', 1, b'\n', 2, 3, b'\n']);
}

#[test]
fn test_buf_writer_drops_once() {
    struct CountDrops(usize);

    impl Write for CountDrops {
        fn write(&mut self, _buf: &[u8]) -> io::Result<usize> {
            unimplemented!()
        }

        fn flush(&mut self) -> io::Result<()> {
            unimplemented!()
        }
    }

    impl Drop for CountDrops {
        fn drop(&mut self) {
            assert_eq!(self.0, 0);
            self.0 += 1;
        }
    }

    let writer = BufWriter::new(CountDrops(0));
    let (_, _) = writer.into_inner_with_buffer();
}
