//! Reading files that writers only append to.
//!
//! [`AppendFile`] remembers how far a caller has consumed a file across calls that each hand it a
//! handle, so a follower may reopen the file per read and hold nothing open in between. Bytes read
//! but not yet consumed stay in memory rather than being re-read, which lets a caller consume only
//! whole records and leave a partial one pending until the rest of it is appended.
//!
//! Writers are assumed to append: an in-place rewrite that keeps the file's identity and does not
//! shrink it below the read position is not detected. A replaced or truncated file is, and is
//! reported as [`Fill::Reset`] before its contents are read again from the start.

use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom};

use bytes::{BufMut, Bytes, BytesMut};

use crate::os::fs::FdIdentity;
#[cfg(windows)]
use crate::os::fs::FdIdentityExt;

/// A read position in an append-only file, and the bytes read past it but not yet consumed.
#[derive(Debug, Default)]
pub struct AppendFile {
    /// Bytes handed out by [`Self::consume`].
    consumed: u64,
    /// The file the position belongs to; `None` before the first fill, and always on platforms
    /// without file identities, where only truncation is detected.
    identity: Option<FdIdentity>,
    /// Read past `consumed`, not yet consumed.
    pending: BytesMut,
}

/// What [`AppendFile::fill`] did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Fill {
    /// Appended bytes, if any, were added to [`AppendFile::pending`]; `more` if the read stopped
    /// at its limit with bytes still unread.
    Read {
        more: bool,
    },
    /// The file was replaced or truncated: the position is back at `0` and nothing is pending.
    Reset,
}

impl AppendFile {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// A reader resuming at `offset`, which an earlier reader of the same file reported from
    /// [`Self::offset`]. The file's identity is not known until the first fill, so only a file
    /// shorter than `offset` resets.
    #[must_use]
    pub fn at(offset: u64) -> Self {
        Self {
            consumed: offset,
            ..Self::default()
        }
    }

    /// Read up to `max` bytes appended to `file` since the last fill.
    ///
    /// `file` is any handle to the file being followed; it is seeked, not held. A reset reads
    /// nothing, so the caller can drop what it derived from the old contents before filling again.
    ///
    /// # Errors
    ///
    /// Any I/O failure of the stat, seek or read. Bytes read before a failure stay pending.
    pub fn fill(&mut self, file: &File, max: u64) -> io::Result<Fill> {
        let meta = file.metadata()?;
        #[cfg(unix)]
        let identity = Some(FdIdentity::from_metadata(&meta));
        #[cfg(windows)]
        let identity = Some(file.identity()?);
        #[cfg(not(any(unix, windows)))]
        let identity = None;

        let replaced = self.identity.is_some() && self.identity != identity;
        if replaced || meta.len() < self.position() {
            *self = Self {
                identity,
                ..Self::default()
            };
            return Ok(Fill::Reset);
        }
        self.identity = identity;

        let mut file = file;
        file.seek(SeekFrom::Start(self.position()))?;
        io::copy(&mut file.take(max), &mut (&mut self.pending).writer())?;
        Ok(Fill::Read {
            more: self.position() < meta.len(),
        })
    }

    /// Bytes read but not yet consumed.
    #[must_use]
    pub fn pending(&self) -> &[u8] {
        &self.pending
    }

    /// Consume the first `n` pending bytes, advancing the position past them.
    ///
    /// # Panics
    ///
    /// If `n` exceeds [`Self::pending`]'s length.
    pub fn consume(&mut self, n: usize) -> Bytes {
        let taken = self.pending.split_to(n).freeze();
        self.consumed += u64::try_from(n).expect("a byte count fits u64");
        taken
    }

    /// Byte offset of the first byte not yet consumed.
    #[must_use]
    pub fn offset(&self) -> u64 {
        self.consumed
    }

    fn position(&self) -> u64 {
        self.consumed + u64::try_from(self.pending.len()).expect("a byte count fits u64")
    }
}

#[cfg(test)]
mod tests {
    use std::fs::OpenOptions;
    use std::io::Write;
    use std::path::{Path, PathBuf};

    use proptest::prelude::*;
    use rstest::{fixture, rstest};

    use super::*;

    struct TempFile {
        _dir: tempfile::TempDir,
        path: PathBuf,
    }

    #[fixture]
    fn file(#[default(b"")] contents: &[u8]) -> TempFile {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("f");
        std::fs::write(&path, contents).unwrap();
        TempFile { _dir: dir, path }
    }

    fn append(path: &Path, bytes: &[u8]) {
        OpenOptions::new().append(true).open(path).unwrap().write_all(bytes).unwrap();
    }

    fn fill(reader: &mut AppendFile, path: &Path, max: u64) -> Fill {
        reader.fill(&File::open(path).unwrap(), max).unwrap()
    }

    #[rstest]
    fn unconsumed_bytes_stay_pending_and_are_not_read_again(#[with(b"ab")] file: TempFile) {
        let mut reader = AppendFile::new();
        assert_eq!(fill(&mut reader, &file.path, 64), Fill::Read { more: false });
        assert_eq!(&reader.consume(1)[..], b"a");

        append(&file.path, b"c");
        assert_eq!(fill(&mut reader, &file.path, 64), Fill::Read { more: false });
        assert_eq!(reader.pending(), b"bc");
        assert_eq!(reader.offset(), 1);
    }

    #[rstest]
    fn a_fill_reads_at_most_max_bytes(#[with(b"0123456789")] file: TempFile) {
        let mut reader = AppendFile::new();
        assert_eq!(fill(&mut reader, &file.path, 4), Fill::Read { more: true });
        assert_eq!(reader.pending(), b"0123");
        assert_eq!(fill(&mut reader, &file.path, 4), Fill::Read { more: true });
        assert_eq!(fill(&mut reader, &file.path, 4), Fill::Read { more: false });
        assert_eq!(reader.pending(), b"0123456789");
    }

    #[rstest]
    fn truncation_below_the_read_position_resets(#[with(b"aaaa")] file: TempFile) {
        let mut reader = AppendFile::new();
        fill(&mut reader, &file.path, 64);
        reader.consume(2);

        // Longer than what was consumed, shorter than what was read.
        std::fs::write(&file.path, b"xyz").unwrap();
        assert_eq!(fill(&mut reader, &file.path, 64), Fill::Reset);
        assert_eq!((reader.offset(), reader.pending()), (0, &b""[..]));
        fill(&mut reader, &file.path, 64);
        assert_eq!(reader.pending(), b"xyz");
    }

    #[rstest]
    #[case::at_a_boundary(2, Fill::Read { more: false }, b"b\n")]
    #[case::at_the_end(4, Fill::Read { more: false }, b"")]
    #[case::past_the_end(5, Fill::Reset, b"")]
    fn a_resumed_reader_continues_at_its_offset(
        #[with(b"a\nb\n")] file: TempFile,
        #[case] offset: u64,
        #[case] first: Fill,
        #[case] pending: &[u8],
    ) {
        let mut reader = AppendFile::at(offset);
        assert_eq!(fill(&mut reader, &file.path, 64), first);
        assert_eq!(reader.pending(), pending);
    }

    #[cfg(unix)]
    #[rstest]
    fn a_replaced_file_resets_even_when_larger(#[with(b"1\n")] file: TempFile) {
        let mut reader = AppendFile::new();
        fill(&mut reader, &file.path, 64);
        reader.consume(2);

        // Larger, so only the identity change can reveal the swap.
        let tmp = file.path.with_file_name("tmp");
        std::fs::write(&tmp, b"x\ny\n").unwrap();
        std::fs::rename(&tmp, &file.path).unwrap();
        assert_eq!(fill(&mut reader, &file.path, 64), Fill::Reset);
        fill(&mut reader, &file.path, 64);
        assert_eq!(reader.pending(), b"x\ny\n");
    }

    #[derive(Debug, Clone)]
    enum Op {
        Append(Vec<u8>),
        Fill(u64),
        /// Consume this fraction, in 256ths, of what is pending.
        Consume(u8),
    }

    fn op() -> impl Strategy<Value = Op> {
        prop_oneof![
            prop::collection::vec(any::<u8>(), 0..16).prop_map(Op::Append),
            (1u64..24).prop_map(Op::Fill),
            any::<u8>().prop_map(Op::Consume),
        ]
    }

    proptest! {
        // Against the bytes written so far as the model: whatever interleaving of appends, fills
        // and consumes, the consumed bytes followed by the pending ones are exactly a prefix of
        // the file, so nothing is skipped, duplicated or reordered.
        #[test]
        fn consumed_then_pending_is_always_a_prefix_of_the_file(
            ops in prop::collection::vec(op(), 0..64),
        ) {
            let file = file(b"");
            let mut reader = AppendFile::new();
            let mut written = Vec::new();
            let mut consumed = Vec::new();
            for op in ops {
                match op {
                    Op::Append(bytes) => {
                        append(&file.path, &bytes);
                        written.extend_from_slice(&bytes);
                    }
                    Op::Fill(max) => {
                        let before = reader.pending().len();
                        prop_assert_ne!(fill(&mut reader, &file.path, max), Fill::Reset);
                        prop_assert!(reader.pending().len() - before <= usize::try_from(max).unwrap());
                    }
                    Op::Consume(share) => {
                        let n = reader.pending().len() * usize::from(share) / 256;
                        consumed.extend_from_slice(&reader.consume(n));
                    }
                }
                let mut seen = consumed.clone();
                seen.extend_from_slice(reader.pending());
                prop_assert_eq!(&seen[..], &written[..seen.len()]);
                prop_assert_eq!(reader.offset(), consumed.len() as u64);
            }
        }
    }
}
