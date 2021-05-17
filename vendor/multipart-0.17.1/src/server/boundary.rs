// Copyright 2016 `multipart` Crate Developers
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

//! Boundary parsing for `multipart` requests.

use ::safemem;

use super::buf_redux::BufReader;
use super::buf_redux::policy::MinBuffered;
use super::twoway;

use std::cmp;
use std::borrow::Borrow;

use std::io;
use std::io::prelude::*;

use self::State::*;

pub const MIN_BUF_SIZE: usize = 1024;

#[derive(Debug, PartialEq, Eq)]
enum State {
    Searching,
    BoundaryRead,
    AtEnd
}

/// A struct implementing `Read` and `BufRead` that will yield bytes until it sees a given sequence.
#[derive(Debug)]
pub struct BoundaryReader<R> {
    source: BufReader<R, MinBuffered>,
    boundary: Vec<u8>,
    search_idx: usize,
    state: State,
}

impl<R> BoundaryReader<R> where R: Read {
    /// Internal API
    pub fn from_reader<B: Into<Vec<u8>>>(reader: R, boundary: B) -> BoundaryReader<R> {
        let mut boundary = boundary.into();
        safemem::prepend(b"--", &mut boundary);
        let source = BufReader::new(reader).set_policy(MinBuffered(MIN_BUF_SIZE));

        BoundaryReader {
            source,
            boundary,
            search_idx: 0,
            state: Searching,
        }
    }

    fn read_to_boundary(&mut self) -> io::Result<&[u8]> {
        let buf = self.source.fill_buf()?;

        trace!("Buf: {:?}", String::from_utf8_lossy(buf));

        debug!("Before search Buf len: {} Search idx: {} State: {:?}",
               buf.len(), self.search_idx, self.state);

        if self.state == BoundaryRead || self.state == AtEnd {
            return Ok(&buf[..self.search_idx])
        }

        if self.state == Searching && self.search_idx < buf.len() {
            let lookahead = &buf[self.search_idx..];

            // Look for the boundary, or if it isn't found, stop near the end.
            match find_boundary(lookahead, &self.boundary) {
                Ok(found_idx) => {
                    self.search_idx += found_idx;
                    self.state = BoundaryRead;
                },
                Err(yield_len) => {
                    self.search_idx += yield_len;
                }
            }
        }        
        
        debug!("After search Buf len: {} Search idx: {} State: {:?}",
               buf.len(), self.search_idx, self.state);

        // back up the cursor to before the boundary's preceding CRLF if we haven't already
        if self.search_idx >= 2 && !buf[self.search_idx..].starts_with(b"\r\n") {
            let two_bytes_before = &buf[self.search_idx - 2 .. self.search_idx];

            trace!("Two bytes before: {:?} ({:?}) (\"\\r\\n\": {:?})",
                   String::from_utf8_lossy(two_bytes_before), two_bytes_before, b"\r\n");

            if two_bytes_before == *b"\r\n" {
                debug!("Subtract two!");
                self.search_idx -= 2;
            }
        }

        let ret_buf = &buf[..self.search_idx];

        trace!("Returning buf: {:?}", String::from_utf8_lossy(ret_buf));

        Ok(ret_buf)
    }

    pub fn set_min_buf_size(&mut self, min_buf_size: usize) {
        // ensure the minimum buf size is at least enough to find a boundary with some extra
        let min_buf_size = cmp::max(self.boundary.len() * 2, min_buf_size);

        self.source.policy_mut().0 = min_buf_size;
    }

    pub fn consume_boundary(&mut self) -> io::Result<bool> {
        if self.state == AtEnd {
            return Ok(false);
        }

        while self.state == Searching {
            debug!("Boundary not found yet");

            let buf_len = self.read_to_boundary()?.len();

            if buf_len == 0 && self.state == Searching {
                return Err(io::Error::new(io::ErrorKind::UnexpectedEof,
                                          "unexpected end of request body"));
            }

            debug!("Discarding {} bytes", buf_len);

            self.consume(buf_len);
        }

        let consume_amt = {
            let buf = self.source.fill_buf()?;

            // if the boundary is found we should have at least this much in-buffer
            let mut consume_amt = self.search_idx + self.boundary.len();

            // we don't care about data before the cursor
            let bnd_segment = &buf[self.search_idx..];

            if bnd_segment.starts_with(b"\r\n") {
                // preceding CRLF needs to be consumed as well
                consume_amt += 2;

                // assert that we've found the boundary after the CRLF
                debug_assert_eq!(*self.boundary, bnd_segment[2 .. self.boundary.len() + 2]);
            } else {
                // assert that we've found the boundary
                debug_assert_eq!(*self.boundary, bnd_segment[..self.boundary.len()]);
            }

            // include the trailing CRLF or --
            consume_amt += 2;

            if buf.len() < consume_amt {
                return Err(io::Error::new(io::ErrorKind::UnexpectedEof,
                                          "not enough bytes to verify boundary"));
            }

            // we have enough bytes to verify
            self.state = Searching;

            let last_two = &buf[consume_amt - 2 .. consume_amt];

            match last_two {
                b"\r\n" => self.state = Searching,
                b"--" => self.state = AtEnd,
                _ => return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("unexpected bytes following multipart boundary: {:X} {:X}",
                            last_two[0], last_two[1])
                )),
            }

            consume_amt
        };

        trace!("Consuming {} bytes, remaining buf: {:?}",
               consume_amt,
               String::from_utf8_lossy(self.source.buffer()));

        self.source.consume(consume_amt);

        if cfg!(debug_assertions) {

        }

        self.search_idx = 0;

        trace!("Consumed boundary (state: {:?}), remaining buf: {:?}", self.state,
               String::from_utf8_lossy(self.source.buffer()));

        Ok(self.state != AtEnd)
    }
}

/// Find the boundary occurrence or the highest length to safely yield
fn find_boundary(buf: &[u8], boundary: &[u8]) -> Result<usize, usize> {
    if let Some(idx) = twoway::find_bytes(buf, boundary) {
        return Ok(idx);
    }

    let search_start = buf.len().saturating_sub(boundary.len());

    // search for just the boundary fragment
    for i in search_start .. buf.len() {
        if boundary.starts_with(&buf[i..]) {
            return Err(i);
        }
    }

    Err(buf.len())
}

#[cfg(feature = "bench")]
impl<'a> BoundaryReader<io::Cursor<&'a [u8]>> {
    fn new_with_bytes(bytes: &'a [u8], boundary: &str) -> Self {
        Self::from_reader(io::Cursor::new(bytes), boundary)
    }

    fn reset(&mut self) {
        // Dump buffer and reset cursor
        self.source.seek(io::SeekFrom::Start(0));
        self.state = Searching;
        self.search_idx = 0;
    }
}

impl<R> Borrow<R> for BoundaryReader<R> {
    fn borrow(&self) -> &R {
        self.source.get_ref()
    }
}

impl<R> Read for BoundaryReader<R> where R: Read {
    fn read(&mut self, out: &mut [u8]) -> io::Result<usize> {
        let read = {
            let mut buf = self.read_to_boundary()?;
            // This shouldn't ever be an error so unwrapping is fine.
            buf.read(out).unwrap()
        };

        self.consume(read);
        Ok(read)
    }
}

impl<R> BufRead for BoundaryReader<R> where R: Read {
    fn fill_buf(&mut self) -> io::Result<&[u8]> {
        self.read_to_boundary()
    }

    fn consume(&mut self, amt: usize) {
        let true_amt = cmp::min(amt, self.search_idx);

        debug!("Consume! amt: {} true amt: {}", amt, true_amt);

        self.source.consume(true_amt);
        self.search_idx -= true_amt;
    }
}

#[cfg(test)]
mod test {
    use super::BoundaryReader;

    use std::io;
    use std::io::prelude::*;

    const BOUNDARY: &'static str = "boundary";
    const TEST_VAL: &'static str = "--boundary\r\n\
                                    dashed-value-1\r\n\
                                    --boundary\r\n\
                                    dashed-value-2\r\n\
                                    --boundary--";
        
    #[test]
    fn test_boundary() {
        ::init_log();

        debug!("Testing boundary (no split)");

        let src = &mut TEST_VAL.as_bytes();
        let mut reader = BoundaryReader::from_reader(src, BOUNDARY);

        let mut buf = String::new();
        
        test_boundary_reader(&mut reader, &mut buf);
    }

    struct SplitReader<'a> {
        left: &'a [u8],
        right: &'a [u8],
    }

    impl<'a> SplitReader<'a> {
        fn split(data: &'a [u8], at: usize) -> SplitReader<'a> {
            let (left, right) = data.split_at(at);

            SplitReader { 
                left: left,
                right: right,
            }
        }
    }

    impl<'a> Read for SplitReader<'a> {
        fn read(&mut self, dst: &mut [u8]) -> io::Result<usize> {
            fn copy_bytes_partial(src: &mut &[u8], dst: &mut [u8]) -> usize {
                src.read(dst).unwrap()
            }

            let mut copy_amt = copy_bytes_partial(&mut self.left, dst);

            if copy_amt == 0 {
                copy_amt = copy_bytes_partial(&mut self.right, dst)
            };

            Ok(copy_amt)
        }
    }

    #[test]
    fn test_split_boundary() {
        ::init_log();

        debug!("Testing boundary (split)");

        let mut buf = String::new();
        
        // Substitute for `.step_by()` being unstable.
        for split_at in 0 .. TEST_VAL.len(){
            debug!("Testing split at: {}", split_at);

            let src = SplitReader::split(TEST_VAL.as_bytes(), split_at);
            let mut reader = BoundaryReader::from_reader(src, BOUNDARY);
            test_boundary_reader(&mut reader, &mut buf);
        }
    }

    fn test_boundary_reader<R: Read>(reader: &mut BoundaryReader<R>, buf: &mut String) {
        buf.clear();

        debug!("Read 1");
        let _ = reader.read_to_string(buf).unwrap();
        assert!(buf.is_empty(), "Buffer not empty: {:?}", buf);
        buf.clear();

        debug!("Consume 1");
        reader.consume_boundary().unwrap();

        debug!("Read 2");
        let _ = reader.read_to_string(buf).unwrap();
        assert_eq!(buf, "dashed-value-1");
        buf.clear();

        debug!("Consume 2");
        reader.consume_boundary().unwrap();

        debug!("Read 3");
        let _ = reader.read_to_string(buf).unwrap();
        assert_eq!(buf, "dashed-value-2");
        buf.clear();

        debug!("Consume 3");
        reader.consume_boundary().unwrap();

        debug!("Read 4");
        let _ = reader.read_to_string(buf).unwrap();
        assert_eq!(buf, "");
    }

    #[test]
    fn test_empty_body() {
        ::init_log();

        // empty body contains closing boundary only
        let mut body: &[u8] = b"--boundary--";

        let ref mut buf = String::new();
        let mut reader = BoundaryReader::from_reader(&mut body, BOUNDARY);

        debug!("Consume 1");
        assert_eq!(reader.consume_boundary().unwrap(), false);

        debug!("Read 1");
        let _ = reader.read_to_string(buf).unwrap();
        assert_eq!(buf, "");
        buf.clear();

        debug!("Consume 2");
        assert_eq!(reader.consume_boundary().unwrap(), false);
    }

    #[test]
    fn test_leading_crlf() {
        ::init_log();

        let mut body: &[u8] = b"\r\n\r\n--boundary\r\n\
                         asdf1234\
                         \r\n\r\n--boundary--";

        let ref mut buf = String::new();
        let mut reader = BoundaryReader::from_reader(&mut body, BOUNDARY);


        debug!("Consume 1");
        assert_eq!(reader.consume_boundary().unwrap(), true);

        debug!("Read 1");
        let _ = reader.read_to_string(buf).unwrap();
        assert_eq!(buf, "asdf1234\r\n");
        buf.clear();

        debug!("Consume 2");
        assert_eq!(reader.consume_boundary().unwrap(), false);

        debug!("Read 2 (empty)");
        let _ = reader.read_to_string(buf).unwrap();
        assert_eq!(buf, "");
    }

    #[test]
    fn test_trailing_crlf() {
        ::init_log();

        let mut body: &[u8] = b"--boundary\r\n\
                         asdf1234\
                         \r\n\r\n--boundary\r\n\
                         hjkl5678\r\n--boundary--";

        let ref mut buf = String::new();
        let mut reader = BoundaryReader::from_reader(&mut body, BOUNDARY);

        debug!("Consume 1");
        assert_eq!(reader.consume_boundary().unwrap(), true);

        debug!("Read 1");

        // Repro for https://github.com/abonander/multipart/issues/93
        // These two reads should produce the same buffer
        let buf1 = reader.read_to_boundary().unwrap().to_owned();
        let buf2 = reader.read_to_boundary().unwrap().to_owned();
        assert_eq!(buf1, buf2);

        let _ = reader.read_to_string(buf).unwrap();
        assert_eq!(buf, "asdf1234\r\n");
        buf.clear();

        debug!("Consume 2");
        assert_eq!(reader.consume_boundary().unwrap(), true);

        debug!("Read 2");
        let _ = reader.read_to_string(buf).unwrap();
        assert_eq!(buf, "hjkl5678");
        buf.clear();

        debug!("Consume 3");
        assert_eq!(reader.consume_boundary().unwrap(), false);

        debug!("Read 3 (empty)");
        let _ = reader.read_to_string(buf).unwrap();
        assert_eq!(buf, "");
    }

    // https://github.com/abonander/multipart/issues/93#issuecomment-343610587
    #[test]
    fn test_trailing_lflf() {
        ::init_log();

        let mut body: &[u8] = b"--boundary\r\n\
                         asdf1234\
                         \n\n\r\n--boundary\r\n\
                         hjkl5678\r\n--boundary--";

        let ref mut buf = String::new();
        let mut reader = BoundaryReader::from_reader(&mut body, BOUNDARY);

        debug!("Consume 1");
        assert_eq!(reader.consume_boundary().unwrap(), true);

        debug!("Read 1");

        // same as above
        let buf1 = reader.read_to_boundary().unwrap().to_owned();
        let buf2 = reader.read_to_boundary().unwrap().to_owned();
        assert_eq!(buf1, buf2);

        let _ = reader.read_to_string(buf).unwrap();
        assert_eq!(buf, "asdf1234\n\n");
        buf.clear();

        debug!("Consume 2");
        assert_eq!(reader.consume_boundary().unwrap(), true);

        debug!("Read 2");
        let _ = reader.read_to_string(buf).unwrap();
        assert_eq!(buf, "hjkl5678");
        buf.clear();

        debug!("Consume 3");
        assert_eq!(reader.consume_boundary().unwrap(), false);

        debug!("Read 3 (empty)");
        let _ = reader.read_to_string(buf).unwrap();
        assert_eq!(buf, "");
    }

    // https://github.com/abonander/multipart/issues/104
    #[test]
    fn test_unterminated_body() {
        ::init_log();

        let mut body: &[u8] = b"--boundary\r\n\
                         asdf1234\
                         \n\n\r\n--boundary\r\n\
                         hjkl5678  ";

        let ref mut buf = String::new();
        let mut reader = BoundaryReader::from_reader(&mut body, BOUNDARY);

        debug!("Consume 1");
        assert_eq!(reader.consume_boundary().unwrap(), true);

        debug!("Read 1");

        // same as above
        let buf1 = reader.read_to_boundary().unwrap().to_owned();
        let buf2 = reader.read_to_boundary().unwrap().to_owned();
        assert_eq!(buf1, buf2);

        let _ = reader.read_to_string(buf).unwrap();
        assert_eq!(buf, "asdf1234\n\n");
        buf.clear();

        debug!("Consume 2");
        assert_eq!(reader.consume_boundary().unwrap(), true);

        debug!("Read 2");
        let _ = reader.read_to_string(buf).unwrap();
        assert_eq!(buf, "hjkl5678  ");
        buf.clear();

        debug!("Consume 3 - expecting error");
        reader.consume_boundary().unwrap_err();
    }

    #[test]
    fn test_lone_boundary() {
        let mut body: &[u8] = b"--boundary";
        let mut reader = BoundaryReader::from_reader(&mut body, "boundary");
        reader.consume_boundary().unwrap_err();
    }

    #[test]
    fn test_invalid_boundary() {
        let mut body: &[u8] = b"--boundary\x00\x00";
        let mut reader = BoundaryReader::from_reader(&mut body, "boundary");
        reader.consume_boundary().unwrap_err();
    }

    #[test]
    fn test_skip_field() {
        let mut body: &[u8] = b"--boundary\r\nfield1\r\n--boundary\r\nfield2\r\n--boundary--";
        let mut reader = BoundaryReader::from_reader(&mut body, "boundary");

        assert_eq!(reader.consume_boundary().unwrap(), true);
        // skip `field1`
        assert_eq!(reader.consume_boundary().unwrap(), true);

        let mut buf = String::new();
        reader.read_to_string(&mut buf).unwrap();
        assert_eq!(buf, "field2");

        assert_eq!(reader.consume_boundary().unwrap(), false);
    }

    #[cfg(feature = "bench")]
    mod bench {
        extern crate test;
        use self::test::Bencher;

        use super::*;

        #[bench]
        fn bench_boundary_reader(b: &mut Bencher) {
            let mut reader = BoundaryReader::new_with_bytes(TEST_VAL.as_bytes(), BOUNDARY);
            let mut buf = String::with_capacity(256);

            b.iter(|| {
                reader.reset();
                test_boundary_reader(&mut reader, &mut buf);
            });
        }
    }
}
