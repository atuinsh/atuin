// Licensed under the Apache License, Version 2.0 <LICENSE-APACHE or
// http://www.apache.org/licenses/LICENSE-2.0> or the MIT license
// <LICENSE-MIT or http://opensource.org/licenses/MIT>, at your
// option. This file may not be copied, modified, or distributed
// except according to those terms.

use std::cmp;

use cryptoutil;

#[derive(Clone,Copy)]
pub enum BufferResult {
    BufferUnderflow,
    BufferOverflow
}

pub trait ReadBuffer {
    fn is_empty(&self) -> bool;
    fn is_full(&self) -> bool;
    fn remaining(&self) -> usize;
    fn capacity(&self) -> usize;
    fn position(&self) -> usize { self.capacity() - self.remaining() }

    fn rewind(&mut self, distance: usize);
    fn truncate(&mut self, amount: usize);
    fn reset(&mut self);

    fn peek_next(&self, count: usize) -> &[u8];
    fn peek_remaining(&self) -> &[u8] {
        self.peek_next(self.remaining())
    }

    fn take_next(&mut self, count: usize) -> &[u8];
    fn take_remaining(&mut self) -> &[u8] {
        let rem = self.remaining();
        self.take_next(rem)
    }

    fn push_to<W: WriteBuffer>(&mut self, output: &mut W) {
        let count = cmp::min(output.remaining(), self.remaining());
        cryptoutil::copy_memory(self.take_next(count), output.take_next(count));
    }
}

pub trait WriteBuffer {
    fn is_empty(&self) -> bool;
    fn is_full(&self) -> bool;
    fn remaining(&self) -> usize;
    fn capacity(&self) -> usize;
    fn position(&self) -> usize { self.capacity() - self.remaining() }

    fn rewind(&mut self, distance: usize);
    fn reset(&mut self);

    // FIXME - Shouldn't need mut self
    fn peek_read_buffer(&mut self) -> RefReadBuffer;

    fn take_next(&mut self, count: usize) -> &mut [u8];
    fn take_remaining(&mut self) -> &mut [u8] {
        let rem = self.remaining();
        self.take_next(rem)
    }
    fn take_read_buffer(&mut self) -> RefReadBuffer;
}

pub struct RefReadBuffer<'a> {
    buff: &'a [u8],
    pos: usize
}

impl <'a> RefReadBuffer<'a> {
    pub fn new(buff: &[u8]) -> RefReadBuffer {
        RefReadBuffer {
            buff: buff,
            pos: 0
        }
    }
}

impl <'a> ReadBuffer for RefReadBuffer<'a> {
    fn is_empty(&self) -> bool { self.pos == self.buff.len() }
    fn is_full(&self) -> bool { self.pos == 0 }
    fn remaining(&self) -> usize { self.buff.len() - self.pos }
    fn capacity(&self) -> usize { self.buff.len() }

    fn rewind(&mut self, distance: usize) { self.pos -= distance; }
    fn truncate(&mut self, amount: usize) {
        self.buff = &self.buff[..self.buff.len() - amount];
    }
    fn reset(&mut self) { self.pos = 0; }

    fn peek_next(&self, count: usize) -> &[u8] { &self.buff[self.pos..count] }

    fn take_next(&mut self, count: usize) -> &[u8] {
        let r = &self.buff[self.pos..self.pos + count];
        self.pos += count;
        r
    }
}

pub struct OwnedReadBuffer {
    buff: Vec<u8>,
    len: usize,
    pos: usize
}

impl OwnedReadBuffer {
    pub fn new(buff: Vec<u8>) -> OwnedReadBuffer {
        let len = buff.len();
        OwnedReadBuffer {
            buff: buff,
            len: len,
            pos: 0
        }
    }
    pub fn new_with_len<'a>(buff: Vec<u8>, len: usize) -> OwnedReadBuffer {
        OwnedReadBuffer {
            buff: buff,
            len: len,
            pos: 0
        }
    }
    pub fn into_write_buffer(self) -> OwnedWriteBuffer {
        OwnedWriteBuffer::new(self.buff)
    }
    pub fn borrow_write_buffer(&mut self) -> BorrowedWriteBuffer {
        self.pos = 0;
        self.len = 0;
        BorrowedWriteBuffer::new(self)
    }
}

impl ReadBuffer for OwnedReadBuffer {
    fn is_empty(&self) -> bool { self.pos == self.len }
    fn is_full(&self) -> bool { self.pos == 0 }
    fn remaining(&self) -> usize { self.len - self.pos }
    fn capacity(&self) -> usize { self.len }

    fn rewind(&mut self, distance: usize) { self.pos -= distance; }
    fn truncate(&mut self, amount: usize) { self.len -= amount; }
    fn reset(&mut self) { self.pos = 0; }

    fn peek_next(&self, count: usize) -> &[u8] { &self.buff[self.pos..count] }

    fn take_next(&mut self, count: usize) -> &[u8] {
        let r = &self.buff[self.pos..self.pos + count];
        self.pos += count;
        r
    }
}

pub struct RefWriteBuffer<'a> {
    buff: &'a mut [u8],
    len: usize,
    pos: usize
}

impl <'a> RefWriteBuffer<'a> {
    pub fn new(buff: &mut [u8]) -> RefWriteBuffer {
        let len = buff.len();
        RefWriteBuffer {
            buff: buff,
            len: len,
            pos: 0
        }
    }
}

impl <'a> WriteBuffer for RefWriteBuffer<'a> {
    fn is_empty(&self) -> bool { self.pos == 0 }
    fn is_full(&self) -> bool { self.pos == self.len }
    fn remaining(&self) -> usize { self.len - self.pos }
    fn capacity(&self) -> usize { self.len }

    fn rewind(&mut self, distance: usize) { self.pos -= distance; }
    fn reset(&mut self) { self.pos = 0; }

    fn peek_read_buffer(&mut self) -> RefReadBuffer {
        RefReadBuffer::new(&mut self.buff[..self.pos])
    }

    fn take_next(&mut self, count: usize) -> &mut [u8] {
        let r = &mut self.buff[self.pos..self.pos + count];
        self.pos += count;
        r
    }
    fn take_read_buffer(&mut self) -> RefReadBuffer {
        let r = RefReadBuffer::new(&mut self.buff[..self.pos]);
        self.pos = 0;
        r
    }
}

pub struct BorrowedWriteBuffer<'a> {
    parent: &'a mut OwnedReadBuffer,
    pos: usize,
    len: usize
}

impl <'a> BorrowedWriteBuffer<'a> {
    fn new(parent: &mut OwnedReadBuffer) -> BorrowedWriteBuffer {
        let buff_len = parent.buff.len();
        BorrowedWriteBuffer {
            parent: parent,
            pos: 0,
            len: buff_len
        }
    }
}

impl <'a> WriteBuffer for BorrowedWriteBuffer<'a> {
    fn is_empty(&self) -> bool { self.pos == 0 }
    fn is_full(&self) -> bool { self.pos == self.len }
    fn remaining(&self) -> usize { self.len - self.pos }
    fn capacity(&self) -> usize { self.len }

    fn rewind(&mut self, distance: usize) {
        self.pos -= distance;
        self.parent.len -= distance;
    }
    fn reset(&mut self) {
        self.pos = 0;
        self.parent.len = 0;
    }

    fn peek_read_buffer(&mut self) -> RefReadBuffer {
        RefReadBuffer::new(&self.parent.buff[..self.pos])
    }

    fn take_next<>(&mut self, count: usize) -> &mut [u8] {
        let r = &mut self.parent.buff[self.pos..self.pos + count];
        self.pos += count;
        self.parent.len += count;
        r
    }
    fn take_read_buffer(&mut self) -> RefReadBuffer {
        let r = RefReadBuffer::new(&self.parent.buff[..self.pos]);
        self.pos = 0;
        self.parent.len = 0;
        r
    }
}

pub struct OwnedWriteBuffer {
    buff: Vec<u8>,
    len: usize,
    pos: usize
}

impl OwnedWriteBuffer {
    pub fn new(buff: Vec<u8>) -> OwnedWriteBuffer {
        let len = buff.len();
        OwnedWriteBuffer {
            buff: buff,
            len: len,
            pos: 0
        }
    }
    pub fn into_read_buffer(self) -> OwnedReadBuffer {
        let pos = self.pos;
        OwnedReadBuffer::new_with_len(self.buff, pos)
    }
}

impl WriteBuffer for OwnedWriteBuffer {
    fn is_empty(&self) -> bool { self.pos == 0 }
    fn is_full(&self) -> bool { self.pos == self.len }
    fn remaining(&self) -> usize { self.len - self.pos }
    fn capacity(&self) -> usize { self.len }

    fn rewind(&mut self, distance: usize) { self.pos -= distance; }
    fn reset(&mut self) { self.pos = 0; }

    fn peek_read_buffer<'a>(&'a mut self) -> RefReadBuffer<'a> {
        RefReadBuffer::new(&self.buff[..self.pos])
    }

    fn take_next<'a>(&'a mut self, count: usize) -> &'a mut [u8] {
        let r = &mut self.buff[self.pos..self.pos + count];
        self.pos += count;
        r
    }
    fn take_read_buffer<'a>(&'a mut self) -> RefReadBuffer<'a> {
        let r = RefReadBuffer::new(&self.buff[..self.pos]);
        self.pos = 0;
        r
    }
}
