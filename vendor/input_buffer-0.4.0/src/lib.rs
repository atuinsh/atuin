//! A buffer for reading data from the network.
//!
//! The `InputBuffer` is a buffer of bytes similar to a first-in, first-out queue.
//! It is filled by reading from a stream supporting `Read` and is then
//! accessible as a cursor for reading bytes.
#![deny(missing_debug_implementations)]
extern crate bytes;

use std::error;
use std::fmt;
use std::io::{Cursor, Read, Result as IoResult};

use bytes::{Buf, BufMut};

/// A FIFO buffer for reading packets from network.
#[derive(Debug)]
pub struct InputBuffer(Cursor<Vec<u8>>);

/// The recommended minimum read size.
pub const MIN_READ: usize = 4096;

impl InputBuffer {
    /// Create a new empty input buffer.
    pub fn new() -> Self {
        Self::with_capacity(MIN_READ)
    }

    /// Create a new empty input buffer.
    pub fn with_capacity(capacity: usize) -> Self {
        Self::from_partially_read(Vec::with_capacity(capacity))
    }

    /// Create a input buffer filled with previously read data.
    pub fn from_partially_read(part: Vec<u8>) -> Self {
        InputBuffer(Cursor::new(part))
    }

    /// Get the data as a cursor.
    pub fn as_cursor(&self) -> &Cursor<Vec<u8>> {
        &self.0
    }

    /// Get the data as a mutable cursor.
    pub fn as_cursor_mut(&mut self) -> &mut Cursor<Vec<u8>> {
        &mut self.0
    }

    /// Remove the already consumed portion of the data.
    pub fn remove_garbage(&mut self) {
        let pos = self.0.position() as usize;
        self.0.get_mut().drain(0..pos).count();
        self.0.set_position(0);
    }

    /// Get the rest of the buffer and destroy the buffer.
    pub fn into_vec(mut self) -> Vec<u8> {
        self.remove_garbage();
        self.0.into_inner()
    }

    /// Read next portion of data from the given input stream.
    pub fn read_from<S: Read>(&mut self, stream: &mut S) -> IoResult<usize> {
        self.prepare().read_from(stream)
    }

    /// Prepare reading.
    pub fn prepare<'t>(&'t mut self) -> DoRead<'t> {
        self.prepare_reserve(MIN_READ)
    }

    /// Prepare reading with the given reserve.
    pub fn prepare_reserve<'t>(&'t mut self, reserve: usize) -> DoRead<'t> {
        // Space that we have right now.
        let free_space = self.total_len() - self.filled_len();
        // Space that we could have after garbage collect.
        let total_space = free_space + self.consumed_len();
        // If garbage collect would help, schedule it.
        let remove_garbage = free_space < reserve && total_space >= reserve;

        DoRead {
            buf: self,
            remove_garbage,
            reserve,
        }
    }
}

impl InputBuffer {
    /// Get the total buffer length.
    fn total_len(&self) -> usize {
        self.0.get_ref().capacity()
    }

    /// Get the filled buffer length.
    fn filled_len(&self) -> usize {
        self.0.get_ref().len()
    }

    /// Get the consumed data length.
    fn consumed_len(&self) -> usize {
        self.0.position() as usize
    }
}

impl Buf for InputBuffer {
    fn remaining(&self) -> usize {
        Buf::remaining(self.as_cursor())
    }
    fn chunk(&self) -> &[u8] {
        Buf::chunk(self.as_cursor())
    }
    fn advance(&mut self, size: usize) {
        Buf::advance(self.as_cursor_mut(), size)
    }
}

/// The reference to the buffer used for reading.
#[derive(Debug)]
pub struct DoRead<'t> {
    buf: &'t mut InputBuffer,
    remove_garbage: bool,
    reserve: usize,
}

impl<'t> DoRead<'t> {
    /// Enforce the size limit.
    pub fn with_limit(mut self, limit: usize) -> Result<Self, SizeLimit> {
        // Total size we shall have after reserve.
        let total_len = self.buf.filled_len() + self.reserve;
        // Size we could free if we collect garbage.
        let consumed_len = self.buf.consumed_len();
        // Shall we fit if we remove data already consumed?
        if total_len - consumed_len <= limit {
            // Shall we not fit if we don't remove data already consumed?
            if total_len > limit {
                self.remove_garbage = true;
            }
            Ok(self)
        } else {
            Err(SizeLimit)
        }
    }

    /// Read next portion of data from the given input stream.
    pub fn read_from<S: Read>(self, stream: &mut S) -> IoResult<usize> {
        if self.remove_garbage {
            self.buf.remove_garbage();
        }

        let v: &mut Vec<u8> = self.buf.0.get_mut();

        v.reserve(self.reserve);

        assert!(v.capacity() > v.len());
        let size = unsafe {
            // TODO: This can be replaced by std::mem::MaybeUninit::first_ptr_mut() once
            // it is stabilized.
            let data = &mut v.chunk_mut()[..self.reserve];
            // We first have to initialize the data or otherwise casting to a byte slice
            // below is UB. See also code of std::io::copy(), tokio::AsyncRead::poll_read_buf()
            // and others.
            //
            // Read::read() might read uninitialized data otherwise, and generally creating
            // references to uninitialized data is UB.
            for i in 0..data.len() {
                data.write_byte(i, 0);
            }
            // Now it's safe to cast it to a byte slice
            let data = std::slice::from_raw_parts_mut(data.as_mut_ptr() as *mut u8, data.len());
            let size = stream.read(data)?;
            v.advance_mut(size);
            size
        };
        Ok(size)
    }
}

/// Size limit error.
#[derive(Debug, Clone, Copy)]
pub struct SizeLimit;

impl fmt::Display for SizeLimit {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "SizeLimit")
    }
}

impl error::Error for SizeLimit {
    fn description(&self) -> &'static str {
        "Size limit exceeded"
    }
}

#[cfg(test)]
mod tests {

    use super::InputBuffer;
    use bytes::Buf;
    use std::io::Cursor;

    #[test]
    fn simple_reading() {
        let mut inp = Cursor::new(b"Hello World!".to_vec());
        let mut buf = InputBuffer::new();
        let size = buf.read_from(&mut inp).unwrap();
        assert_eq!(size, 12);
        assert_eq!(buf.chunk(), b"Hello World!");
    }

    #[test]
    fn partial_reading() {
        let mut inp = Cursor::new(b"Hello World!".to_vec());
        let mut buf = InputBuffer::with_capacity(4);
        let size = buf.prepare_reserve(4).read_from(&mut inp).unwrap();
        assert_eq!(size, 4);
        assert_eq!(buf.chunk(), b"Hell");
        buf.advance(2);
        assert_eq!(buf.chunk(), b"ll");
        let size = buf.prepare_reserve(1).read_from(&mut inp).unwrap();
        assert_eq!(size, 1);
        assert_eq!(buf.chunk(), b"llo");
        let size = buf.prepare_reserve(4).read_from(&mut inp).unwrap();
        assert_eq!(size, 4);
        assert_eq!(buf.chunk(), b"llo Wor");
        let size = buf.prepare_reserve(16).read_from(&mut inp).unwrap();
        assert_eq!(size, 3);
        assert_eq!(buf.chunk(), b"llo World!");
    }

    #[test]
    fn limiting() {
        let mut inp = Cursor::new(b"Hello World!".to_vec());
        let mut buf = InputBuffer::with_capacity(4);
        let size = buf
            .prepare_reserve(4)
            .with_limit(5)
            .unwrap()
            .read_from(&mut inp)
            .unwrap();
        assert_eq!(size, 4);
        assert_eq!(buf.chunk(), b"Hell");
        buf.advance(2);
        assert_eq!(buf.chunk(), b"ll");
        {
            let e = buf.prepare_reserve(4).with_limit(5);
            assert!(e.is_err());
        }
        buf.advance(1);
        let size = buf
            .prepare_reserve(4)
            .with_limit(5)
            .unwrap()
            .read_from(&mut inp)
            .unwrap();
        assert_eq!(size, 4);
        assert_eq!(buf.chunk(), b"lo Wo");
    }
}
