use std::cmp;
use std::collections::VecDeque;
use std::io;
use std::io::Read;

/// This is a byte buffer that is built from a vector
/// of byte vectors.  This avoids extra copies when
/// appending a new byte vector, at the expense of
/// more complexity when reading out.
pub struct ChunkVecBuffer {
    chunks: VecDeque<Vec<u8>>,
    limit: usize,
}

impl ChunkVecBuffer {
    pub fn new() -> ChunkVecBuffer {
        ChunkVecBuffer {
            chunks: VecDeque::new(),
            limit: 0,
        }
    }

    /// Sets the upper limit on how many bytes this
    /// object can store.
    ///
    /// Setting a lower limit than the currently stored
    /// data is not an error.
    ///
    /// A zero limit is interpreted as no limit.
    pub fn set_limit(&mut self, new_limit: usize) {
        self.limit = new_limit;
    }

    /// If we're empty
    pub fn is_empty(&self) -> bool {
        self.chunks.is_empty()
    }

    /// How many bytes we're storing
    pub fn len(&self) -> usize {
        let mut len = 0;
        for ch in &self.chunks {
            len += ch.len();
        }
        len
    }

    /// For a proposed append of `len` bytes, how many
    /// bytes should we actually append to adhere to the
    /// currently set `limit`?
    pub fn apply_limit(&self, len: usize) -> usize {
        if self.limit == 0 {
            len
        } else {
            let space = self.limit.saturating_sub(self.len());
            cmp::min(len, space)
        }
    }

    /// Append a copy of `bytes`, perhaps a prefix if
    /// we're near the limit.
    pub fn append_limited_copy(&mut self, bytes: &[u8]) -> usize {
        let take = self.apply_limit(bytes.len());
        self.append(bytes[..take].to_vec());
        take
    }

    /// Take and append the given `bytes`.
    pub fn append(&mut self, bytes: Vec<u8>) -> usize {
        let len = bytes.len();

        if !bytes.is_empty() {
            self.chunks.push_back(bytes);
        }

        len
    }

    /// Take one of the chunks from this object.  This
    /// function panics if the object `is_empty`.
    pub fn take_one(&mut self) -> Vec<u8> {
        self.chunks.pop_front().unwrap()
    }

    /// Read data out of this object, writing it into `buf`
    /// and returning how many bytes were written there.
    pub fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let mut offs = 0;

        while offs < buf.len() && !self.is_empty() {
            let used = self.chunks[0]
                .as_slice()
                .read(&mut buf[offs..])?;

            self.consume(used);
            offs += used;
        }

        Ok(offs)
    }

    fn consume(&mut self, mut used: usize) {
        while used > 0 && !self.is_empty() {
            if used >= self.chunks[0].len() {
                used -= self.chunks[0].len();
                self.take_one();
            } else {
                self.chunks[0] = self.chunks[0].split_off(used);
                used = 0;
            }
        }
    }

    /// Read data out of this object, passing it `wr`
    pub fn write_to(&mut self, wr: &mut dyn io::Write) -> io::Result<usize> {
        if self.is_empty() {
            return Ok(0);
        }

        let used = wr.write_vectored(
            &self
                .chunks
                .iter()
                .map(|ch| io::IoSlice::new(ch))
                .collect::<Vec<io::IoSlice>>(),
        )?;
        self.consume(used);
        Ok(used)
    }
}

#[cfg(test)]
mod test {
    use super::ChunkVecBuffer;

    #[test]
    fn short_append_copy_with_limit() {
        let mut cvb = ChunkVecBuffer::new();
        cvb.set_limit(12);
        assert_eq!(cvb.append_limited_copy(b"hello"), 5);
        assert_eq!(cvb.append_limited_copy(b"world"), 5);
        assert_eq!(cvb.append_limited_copy(b"hello"), 2);
        assert_eq!(cvb.append_limited_copy(b"world"), 0);

        let mut buf = [0u8; 12];
        assert_eq!(cvb.read(&mut buf).unwrap(), 12);
        assert_eq!(buf.to_vec(), b"helloworldhe".to_vec());
    }
}
