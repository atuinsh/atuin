use core::slice;

pub struct Bytes<'a> {
    slice: &'a [u8],
    pos: usize
}

impl<'a> Bytes<'a> {
    #[inline]
    pub fn new(slice: &'a [u8]) -> Bytes<'a> {
        Bytes {
            slice: slice,
            pos: 0
        }
    }

    #[inline]
    pub fn pos(&self) -> usize {
        self.pos
    }

    #[inline]
    pub fn peek(&self) -> Option<u8> {
        self.slice.get(self.pos).cloned()
    }

    #[inline]
    pub unsafe fn bump(&mut self) {
        debug_assert!(self.pos + 1 <= self.slice.len(), "overflow");
        self.pos += 1;
    }

    #[allow(unused)]
    #[inline]
    pub unsafe fn advance(&mut self, n: usize) {
        debug_assert!(self.pos + n <= self.slice.len(), "overflow");
        self.pos += n;
    }

    #[inline]
    pub fn len(&self) -> usize {
        self.slice.len()
    }

    #[inline]
    pub fn slice(&mut self) -> &'a [u8] {
        // not moving position at all, so it's safe
        unsafe {
            self.slice_skip(0)
        }
    }

    #[inline]
    pub unsafe fn slice_skip(&mut self, skip: usize) -> &'a [u8] {
        debug_assert!(self.pos >= skip);
        let head_pos = self.pos - skip;
        let ptr = self.slice.as_ptr();
        let head = slice::from_raw_parts(ptr, head_pos);
        let tail = slice::from_raw_parts(ptr.offset(self.pos as isize), self.slice.len() - self.pos);
        self.pos = 0;
        self.slice = tail;
        head
    }

    #[inline]
    pub fn next_8<'b>(&'b mut self) -> Option<Bytes8<'b, 'a>> {
        if self.slice.len() >= self.pos + 8 {
            Some(Bytes8::new(self))
        } else {
            None
        }
    }
}

impl<'a> AsRef<[u8]> for Bytes<'a> {
    #[inline]
    fn as_ref(&self) -> &[u8] {
        &self.slice[self.pos..]
    }
}

impl<'a> Iterator for Bytes<'a> {
    type Item = u8;

    #[inline]
    fn next(&mut self) -> Option<u8> {
        if self.slice.len() > self.pos {
            let b = unsafe { *self.slice.get_unchecked(self.pos) };
            self.pos += 1;
            Some(b)
        } else {
            None
        }
    }
}

pub struct Bytes8<'a, 'b: 'a> {
    bytes: &'a mut Bytes<'b>,
    #[cfg(debug_assertions)]
    pos: usize
}

macro_rules! bytes8_methods {
    ($f:ident, $pos:expr) => {
        #[inline]
        pub fn $f(&mut self) -> u8 {
            self.assert_pos($pos);
            let b = unsafe { *self.bytes.slice.get_unchecked(self.bytes.pos) };
            self.bytes.pos += 1;
            b
        }
    };
    () => {
        bytes8_methods!(_0, 0);
        bytes8_methods!(_1, 1);
        bytes8_methods!(_2, 2);
        bytes8_methods!(_3, 3);
        bytes8_methods!(_4, 4);
        bytes8_methods!(_5, 5);
        bytes8_methods!(_6, 6);
        bytes8_methods!(_7, 7);
    }
}

impl<'a, 'b: 'a> Bytes8<'a, 'b> {
    bytes8_methods! {}

    #[cfg(not(debug_assertions))]
    #[inline]
    fn new(bytes: &'a mut Bytes<'b>) -> Bytes8<'a, 'b> {
        Bytes8 {
            bytes: bytes,
        }
    }

    #[cfg(debug_assertions)]
    #[inline]
    fn new(bytes: &'a mut Bytes<'b>) -> Bytes8<'a, 'b> {
        Bytes8 {
            bytes: bytes,
            pos: 0,
        }
    }

    #[cfg(not(debug_assertions))]
    #[inline]
    fn assert_pos(&mut self, _pos: usize) {
    }

    #[cfg(debug_assertions)]
    #[inline]
    fn assert_pos(&mut self, pos: usize) {
        assert!(self.pos == pos);
        self.pos += 1;
    }
}

#[cfg(test)]
mod tests {
    use super::Bytes;

    #[test]
    fn test_next_8_too_short() {
        // Start with 10 bytes.
        let slice = [0u8, 1u8, 2u8, 3u8, 4u8, 5u8, 6u8, 7u8, 8u8, 9u8];
        let mut bytes = Bytes::new(&slice);
        // Skip 3 of them.
        unsafe { bytes.advance(3); }
        // There should be 7 left, not enough to call next_8.
        assert!(bytes.next_8().is_none());
    }

    #[test]
    fn test_next_8_just_right() {
        // Start with 10 bytes.
        let slice = [0u8, 1u8, 2u8, 3u8, 4u8, 5u8, 6u8, 7u8, 8u8, 9u8];
        let mut bytes = Bytes::new(&slice);
        // Skip 2 of them.
        unsafe { bytes.advance(2); }
        // There should be 8 left, just enough to call next_8.
        let ret = bytes.next_8();
        assert!(ret.is_some());
        let mut ret = ret.unwrap();
        // They should be the bytes starting with 2.
        assert_eq!(ret._0(), 2u8);
        assert_eq!(ret._1(), 3u8);
        assert_eq!(ret._2(), 4u8);
        assert_eq!(ret._3(), 5u8);
        assert_eq!(ret._4(), 6u8);
        assert_eq!(ret._5(), 7u8);
        assert_eq!(ret._6(), 8u8);
        assert_eq!(ret._7(), 9u8);
    }

    #[test]
    fn test_next_8_extra() {
        // Start with 10 bytes.
        let slice = [0u8, 1u8, 2u8, 3u8, 4u8, 5u8, 6u8, 7u8, 8u8, 9u8];
        let mut bytes = Bytes::new(&slice);
        // Skip 1 of them.
        unsafe { bytes.advance(1); }
        // There should be 9 left, more than enough to call next_8.
        let ret = bytes.next_8();
        assert!(ret.is_some());
        let mut ret = ret.unwrap();
        // They should be the bytes starting with 1.
        assert_eq!(ret._0(), 1u8);
        assert_eq!(ret._1(), 2u8);
        assert_eq!(ret._2(), 3u8);
        assert_eq!(ret._3(), 4u8);
        assert_eq!(ret._4(), 5u8);
        assert_eq!(ret._5(), 6u8);
        assert_eq!(ret._6(), 7u8);
        assert_eq!(ret._7(), 8u8);
    }
}
