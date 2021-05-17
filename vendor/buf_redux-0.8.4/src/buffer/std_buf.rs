// Copyright 2016-2018 Austin Bonander <austin.bonander@gmail.com>
//
// Licensed under the Apache License, Version 2.0 <LICENSE-APACHE or
// http://www.apache.org/licenses/LICENSE-2.0> or the MIT license
// <LICENSE-MIT or http://opensource.org/licenses/MIT>, at your
// option. This file may not be copied, modified, or distributed
// except according to those terms.

use safemem;

use std::cmp;

use self::impl_::RawBuf;

pub struct StdBuf {
    buf: RawBuf,
    pos: usize,
    end: usize,
}

impl StdBuf {
    pub fn with_capacity(cap: usize) -> Self {
        StdBuf {
            buf: RawBuf::with_capacity(cap),
            pos: 0,
            end: 0,
        }
    }

    pub fn capacity(&self) -> usize {
        self.buf.capacity()
    }

    pub fn len(&self) -> usize {
        self.end - self.pos
    }

    pub fn usable_space(&self) -> usize {
        self.capacity() - self.end
    }

    pub fn reserve(&mut self, additional: usize) -> bool {
        self.check_cursors();
        let usable_space = self.usable_space();

        // there's already enough space
        if usable_space >= additional { return false }

        // attempt to reserve additional capacity in-place
        if self.buf.reserve_in_place(additional - usable_space) {
            return false;
        }

        // don't copy the contents of the buffer as they're irrelevant now
        if self.pos == self.end {
            let capacity = self.buf.capacity();
            // free the existing memory
            self.buf = RawBuf::with_capacity(0);
            self.buf = RawBuf::with_capacity(capacity + additional);
            return true;
        }

        self.buf.reserve(additional - usable_space)
    }

    pub fn make_room(&mut self) {
        self.check_cursors();

        // no room at the head of the buffer
        if self.pos == 0 { return; }

        // simply move the bytes down to the beginning
        let len = self.len();

        safemem::copy_over(unsafe { self.buf.as_mut_slice() },
                           self.pos, 0, len);

        self.pos = 0;
        self.end = len;
    }

    pub fn buf(&self) -> &[u8] {
        unsafe {
            &self.buf.as_slice()[self.pos .. self.end]
        }
    }

    pub fn buf_mut(&mut self) -> &mut [u8] {
        unsafe {
            &mut self.buf.as_mut_slice()[self.pos .. self.end]
        }
    }

    pub unsafe fn write_buf(&mut self) -> &mut [u8] {
        &mut self.buf.as_mut_slice()[self.end ..]
    }

    pub unsafe fn bytes_written(&mut self, amt: usize) {
        self.end = cmp::min(self.end + amt, self.capacity());
    }

    pub fn consume(&mut self, amt: usize) {
        self.pos = cmp::min(self.pos + amt, self.end);
        self.check_cursors();
    }

    pub fn check_cursors(&mut self) -> bool {
        if self.pos == self.end {
            self.pos = 0;
            self.end = 0;
            true
        } else {
            false
        }
    }
}

#[cfg(not(feature = "nightly"))]
mod impl_ {
    use std::mem;

    pub struct RawBuf {
        buf: Box<[u8]>,
    }

    impl RawBuf {
        pub fn with_capacity(capacity: usize) -> Self {
            let mut buf = Vec::with_capacity(capacity);
            let true_cap = buf.capacity();

            unsafe {
                buf.set_len(true_cap);
            }

            RawBuf {
                buf: buf.into_boxed_slice(),
            }
        }

        pub fn capacity(&self) -> usize {
            self.buf.len()
        }

        pub fn reserve(&mut self, additional: usize) -> bool {
            let mut buf = mem::replace(&mut self.buf, Box::new([])).into_vec();

            let old_ptr = self.buf.as_ptr();

            buf.reserve_exact(additional);

            unsafe {
                let new_cap = buf.capacity();
                buf.set_len(new_cap);
            }

            self.buf = buf.into_boxed_slice();

            old_ptr == self.buf.as_ptr()
        }

        pub fn reserve_in_place(&mut self, _additional: usize) -> bool {
            // `Vec` does not support this
            return false;
        }

        pub unsafe fn as_slice(&self) -> &[u8] {
            &self.buf
        }

        pub unsafe fn as_mut_slice(&mut self) -> &mut [u8] {
            &mut self.buf
        }
    }
}

#[cfg(feature = "nightly")]
mod impl_ {
    extern crate alloc;

    use self::alloc::raw_vec::RawVec;

    use std::slice;

    pub struct RawBuf {
        buf: RawVec<u8>,
    }

    impl RawBuf {
        pub fn with_capacity(capacity: usize) -> Self {
            RawBuf {
                buf: RawVec::with_capacity(capacity)
            }
        }

        pub fn capacity(&self) -> usize {
            self.buf.cap()
        }

        pub fn reserve(&mut self, additional: usize) -> bool {
            let cap = self.capacity();
            let old_ptr = self.buf.ptr();
            self.buf.reserve_exact(cap, additional);
            old_ptr != self.buf.ptr()
        }

        pub fn reserve_in_place(&mut self, additional: usize) -> bool {
            let cap = self.capacity();
            self.buf.reserve_in_place(cap, additional)
        }

        pub unsafe fn as_slice(&self) -> &[u8] {
            slice::from_raw_parts(self.buf.ptr(), self.buf.cap())
        }

        pub unsafe fn as_mut_slice(&mut self) -> &mut [u8] {
            slice::from_raw_parts_mut(self.buf.ptr(), self.buf.cap())
        }

    }
}

#[test]
fn read_into_full() {
    use Buffer;

    let mut buffer = Buffer::with_capacity(1);

    assert_eq!(buffer.capacity(), 1);

    let mut bytes = &[1u8, 2][..];

    // Result<usize, io::Error> does not impl PartialEq
    assert_eq!(buffer.read_from(&mut bytes).unwrap(), 1);
    assert_eq!(buffer.read_from(&mut bytes).unwrap(), 0);
}
