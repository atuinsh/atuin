//! Line reads of files that writers only append to.

use std::borrow::Borrow;
use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom};
use std::iter;
use std::path::PathBuf;

use bytes::{BufMut, Bytes, BytesMut};
use futures::Stream;

use crate::os::fs::FdIdentity;
#[cfg(windows)]
use crate::os::fs::FdIdentityExt;

/// Bytes read at a time: with the longest line, all a reader holds in memory, however large the
/// file.
const READ_CHUNK_BYTES: u64 = 64 * 1024;

/// A complete line, without its newline.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Line {
    /// Byte offset just past the line's newline, which a reader made with `at` resumes from.
    pub end: u64,
    pub bytes: Bytes,
}

impl Line {
    /// The line of `file` that ends at byte `end`, if the byte before `end` is a newline.
    pub fn ending_at(mut file: &File, end: u64) -> io::Result<Option<Self>> {
        if end == 0 || file.metadata()?.len() < end {
            return Ok(None);
        }
        let mut window = READ_CHUNK_BYTES;
        loop {
            let start = end.saturating_sub(window);
            let mut buf = vec![0; usize::try_from(end - start).map_err(io::Error::other)?];
            file.seek(SeekFrom::Start(start))?;
            file.read_exact(&mut buf)?;
            let Some((&b'\n', body)) = buf.split_last() else {
                return Ok(None);
            };
            let from = match memchr::memrchr(b'\n', body) {
                Some(newline) => newline + 1,
                None if start == 0 => 0,
                None => {
                    window = window.saturating_mul(2);
                    continue;
                }
            };
            let to = body.len();
            return Ok(Some(Self {
                end,
                bytes: Bytes::from(buf).slice(from..to),
            }));
        }
    }
}

/// Errors returned by [`ReadLines::lines`].
///
/// `Truncated` and `Replaced` have already restarted the reader: the next call reads the file from
/// its first byte.
#[derive(Debug, thiserror::Error)]
pub enum ReadLinesError {
    #[error(transparent)]
    Io(#[from] io::Error),
    #[error("the file shrank below the read position; reading restarts from its start")]
    Truncated,
    #[error("the file was replaced; reading restarts from its start")]
    Replaced,
}

/// Reads the complete lines appended to a file, each once.
///
/// Which file is the implementor's to say: [`LineReader`] reads the one it holds open,
/// [`PathLineReader`] whatever is at its path.
pub trait ReadLines {
    /// The complete lines past the last one handed out, read as the iterator is pulled.
    fn lines(&mut self) -> Result<impl Iterator<Item = io::Result<Line>>, ReadLinesError>;
}

/// Reads the complete lines appended to a file, each once, as a stream.
pub trait AsyncReadLines {
    /// The complete lines past the last one handed out, read as the stream is pulled.
    ///
    /// The stream ends at the end of the file or after its first error; a truncated or replaced
    /// file is read on from its start. Lines read but not yet yielded are dropped with the stream
    /// and not read again.
    fn lines(&mut self) -> impl Stream<Item = io::Result<Line>> + Send + '_;
}

/// What a reader keeps between reads.
#[derive(Debug, Default)]
struct Cursor {
    /// Just past the last line handed out.
    offset: u64,
    /// Read past `offset` but not yet handed out.
    pending: BytesMut,
    /// How much of `pending` is known to hold no newline.
    scanned: usize,
    /// The file `offset` belongs to; `None` before the first read, and always on platforms
    /// without file identities.
    identity: Option<FdIdentity>,
}

impl Cursor {
    fn at(offset: u64) -> Self {
        Self {
            offset,
            ..Self::default()
        }
    }

    fn position(&self) -> u64 {
        self.offset + u64::try_from(self.pending.len()).expect("a byte count fits u64")
    }

    /// The complete lines of `file` past the position, read as the iterator is pulled.
    fn lines<F: Borrow<File>>(
        &mut self,
        file: F,
    ) -> Result<impl Iterator<Item = io::Result<Line>>, ReadLinesError> {
        self.restart_unless_appended(file.borrow())?;
        // Ending on the first error keeps `filter_map(Result::ok)` from spinning on one that
        // persists; the next call retries.
        let mut failed = false;
        Ok(iter::from_fn(move || {
            if failed {
                return None;
            }
            let next = self.next_line(file.borrow()).transpose();
            failed = matches!(next, Some(Err(_)));
            next
        }))
    }

    /// Restart at byte 0 if `file` is not the file being read, or is shorter than what was read.
    fn restart_unless_appended(&mut self, file: &File) -> Result<(), ReadLinesError> {
        let meta = file.metadata()?;
        #[cfg(unix)]
        let identity = Some(FdIdentity::from_metadata(&meta));
        #[cfg(windows)]
        let identity = Some(file.identity()?);
        #[cfg(not(any(unix, windows)))]
        let identity = None;

        if self.identity.is_some() && self.identity != identity {
            *self = Self {
                identity,
                ..Self::default()
            };
            return Err(ReadLinesError::Replaced);
        }
        if meta.len() < self.position() {
            *self = Self {
                identity,
                ..Self::default()
            };
            return Err(ReadLinesError::Truncated);
        }
        self.identity = identity;
        Ok(())
    }

    /// The next complete line, reading on from `file` as needed; `None` once it has none.
    fn next_line(&mut self, file: &File) -> io::Result<Option<Line>> {
        loop {
            if let Some(line) = self.split_line() {
                return Ok(Some(line));
            }
            if self.fill(file)? == 0 {
                return Ok(None);
            }
        }
    }

    /// Hand out the first complete line in `pending`, if any, advancing `offset` past it.
    fn split_line(&mut self) -> Option<Line> {
        let Some(newline) = memchr::memchr(b'\n', &self.pending[self.scanned..]) else {
            self.scanned = self.pending.len();
            return None;
        };

        let len = self.scanned + newline + 1;
        let mut bytes = self.pending.split_to(len).freeze();
        bytes.truncate(len - 1);
        self.scanned = 0;
        self.offset += u64::try_from(len).expect("a byte count fits u64");
        Some(Line {
            end: self.offset,
            bytes,
        })
    }

    /// Read up to [`READ_CHUNK_BYTES`] past the position into `pending`, returning how many.
    fn fill(&mut self, mut file: &File) -> io::Result<u64> {
        file.seek(SeekFrom::Start(self.position()))?;
        io::copy(&mut file.take(READ_CHUNK_BYTES), &mut (&mut self.pending).writer())
    }
}

/// Reads the complete lines appended to an open file.
///
/// **Be warned**: it reads the file it holds, not whatever is at its path, so a file replaced at
/// the path is never read and never reported as [`ReadLinesError::Replaced`].
///
/// # Example
///
/// ```
/// use std::fs::OpenOptions;
/// use std::io::Write;
///
/// use atuin_common::io::{Line, LineReader, ReadLines};
///
/// # let dir = tempfile::tempdir().unwrap();
/// # let path = dir.path().join("log");
/// # let mut log = OpenOptions::new().read(true).append(true).create(true).open(&path).unwrap();
/// let mut reader = LineReader::new(log.try_clone().unwrap());
///
/// log.write_all(b"first\nsec").unwrap();
/// let lines: Vec<Line> = reader.lines().unwrap().map(Result::unwrap).collect();
/// assert_eq!(lines.len(), 1);
/// assert_eq!(lines[0].bytes, "first");
///
/// // The unterminated tail is withheld until its newline lands.
/// log.write_all(b"ond\n").unwrap();
/// let lines: Vec<Line> = reader.lines().unwrap().map(Result::unwrap).collect();
/// assert_eq!(lines[0].bytes, "second");
///
/// // A reader resumed at a line's end picks up just past it.
/// let mut resumed = LineReader::at(log.try_clone().unwrap(), lines[0].end);
/// log.write_all(b"third\n").unwrap();
/// let lines: Vec<Line> = resumed.lines().unwrap().map(Result::unwrap).collect();
/// assert_eq!(lines[0].bytes, "third");
/// ```
#[derive(Debug)]
pub struct LineReader {
    file: File,
    cursor: Cursor,
}

impl LineReader {
    #[must_use]
    pub fn new(file: File) -> Self {
        Self::at(file, 0)
    }

    /// A reader resuming at `offset`, a [`Line::end`] an earlier reader of the same file reported.
    ///
    /// The file's identity is unknown until the first read, so only a file shorter than `offset`
    /// is caught, as [`ReadLinesError::Truncated`].
    #[must_use]
    pub fn at(file: File, offset: u64) -> Self {
        Self {
            file,
            cursor: Cursor::at(offset),
        }
    }
}

impl ReadLines for LineReader {
    fn lines(&mut self) -> Result<impl Iterator<Item = io::Result<Line>>, ReadLinesError> {
        self.cursor.lines(&self.file)
    }
}

/// Reads the complete lines appended to the file at a path, opening it for each read.
///
/// This is identical to [`LineReader`] except that it re-opens files for the duration of the
/// [`ReadLines::lines`] call. Using this over [`LineReader`] is recommended to avoid FD
/// exhaustion. Note that this is significantly slower than [`LineReader`] because of it.
///
/// # Example
///
/// ```
/// use atuin_common::io::{Line, PathLineReader, ReadLines, ReadLinesError};
///
/// # let dir = tempfile::tempdir().unwrap();
/// # let path = dir.path().join("log");
/// std::fs::write(&path, b"first\n").unwrap();
/// let mut reader = PathLineReader::new(&path);
/// let lines: Vec<Line> = reader.lines().unwrap().map(Result::unwrap).collect();
/// assert_eq!(lines[0].bytes, "first");
///
/// // A file replaced at the path is reported once, then read from its start.
/// let tmp = path.with_file_name("tmp");
/// std::fs::write(&tmp, b"second\n").unwrap();
/// std::fs::rename(&tmp, &path).unwrap();
/// assert!(matches!(reader.lines(), Err(ReadLinesError::Replaced)));
/// let lines: Vec<Line> = reader.lines().unwrap().map(Result::unwrap).collect();
/// assert_eq!(lines[0].bytes, "second");
/// ```
#[derive(Debug)]
pub struct PathLineReader {
    path: PathBuf,
    cursor: Cursor,
}

impl PathLineReader {
    #[must_use]
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self::at(path, 0)
    }

    /// Equivalent to [`LineReader::at`], for the file at `path`.
    #[must_use]
    pub fn at(path: impl Into<PathBuf>, offset: u64) -> Self {
        Self {
            path: path.into(),
            cursor: Cursor::at(offset),
        }
    }
}

impl ReadLines for PathLineReader {
    /// Open the file for the read; it closes when the iterator drops.
    fn lines(&mut self) -> Result<impl Iterator<Item = io::Result<Line>>, ReadLinesError> {
        self.cursor.lines(File::open(&self.path)?)
    }
}

#[cfg(test)]
mod tests {
    use std::fs::OpenOptions;
    use std::io::Write;
    use std::path::Path;

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

    fn replace(path: &Path, contents: &[u8]) {
        let tmp = path.with_file_name("tmp");
        std::fs::write(&tmp, contents).unwrap();
        std::fs::rename(&tmp, path).unwrap();
    }

    fn line(end: u64, text: &'static str) -> Line {
        Line {
            end,
            bytes: Bytes::from_static(text.as_bytes()),
        }
    }

    fn drain(lines: impl Iterator<Item = io::Result<Line>>) -> Vec<Line> {
        lines.collect::<io::Result<_>>().unwrap()
    }

    #[rstest]
    #[case::empty(b"", vec![])]
    #[case::one(b"a\n", vec![line(2, "a")])]
    #[case::blank_lines_are_lines(b"\n\n", vec![line(1, ""), line(2, "")])]
    #[case::unterminated_tail_withheld(b"a\nb", vec![line(2, "a")])]
    #[case::only_a_fragment(b"abc", vec![])]
    fn returns_complete_lines_only(#[case] contents: &[u8], #[case] expected: Vec<Line>) {
        let file = file(contents);
        assert_eq!(drain(PathLineReader::new(&file.path).lines().unwrap()), expected);
    }

    #[rstest]
    fn a_line_longer_than_a_read_is_returned_whole() {
        let long = "m".repeat(3 * usize::try_from(READ_CHUNK_BYTES).unwrap());
        let file = file(format!("a\n{long}\nb\n").as_bytes());
        let lines: Vec<Bytes> = drain(PathLineReader::new(&file.path).lines().unwrap())
            .into_iter()
            .map(|line| line.bytes)
            .collect();
        assert_eq!(lines, [&b"a"[..], long.as_bytes(), &b"b"[..]]);
    }

    /// "a\nb\n" is 4 bytes, with lines ending at 2 and 4.
    #[rstest]
    #[case::at_a_boundary(2, vec![line(4, "b")])]
    #[case::at_the_end(4, vec![])]
    fn a_resumed_reader_continues_at_its_offset(
        #[with(b"a\nb\n")] file: TempFile,
        #[case] offset: u64,
        #[case] expected: Vec<Line>,
    ) {
        let lines = drain(PathLineReader::at(&file.path, offset).lines().unwrap());
        assert_eq!(lines, expected);
    }

    #[rstest]
    fn a_reader_resumed_past_the_end_restarts(#[with(b"a\n")] file: TempFile) {
        let mut reader = PathLineReader::at(&file.path, 5);
        assert!(matches!(reader.lines(), Err(ReadLinesError::Truncated)));
        assert_eq!(drain(reader.lines().unwrap()), [line(2, "a")]);
    }

    #[rstest]
    fn truncation_below_the_read_position_restarts(#[with(b"aa\naaaa")] file: TempFile) {
        let mut reader = PathLineReader::new(&file.path);
        drain(reader.lines().unwrap());

        // Past the 3 bytes handed out, short of the 7 read.
        std::fs::write(&file.path, b"xyzab\n").unwrap();
        assert!(matches!(reader.lines(), Err(ReadLinesError::Truncated)));
        assert_eq!(drain(reader.lines().unwrap()), [line(6, "xyzab")]);
    }

    #[cfg(unix)]
    #[rstest]
    fn a_file_replaced_at_the_path_restarts(#[with(b"1\n")] file: TempFile) {
        let mut reader = PathLineReader::new(&file.path);
        drain(reader.lines().unwrap());

        // Larger, so only the identity change can reveal the swap.
        replace(&file.path, b"x\ny\n");
        assert!(matches!(reader.lines(), Err(ReadLinesError::Replaced)));
        assert_eq!(drain(reader.lines().unwrap()), [line(2, "x"), line(4, "y")]);
    }

    #[cfg(unix)]
    #[rstest]
    fn a_held_file_is_followed_past_a_replacement_at_its_path(#[with(b"1\n")] file: TempFile) {
        let mut writer = OpenOptions::new().append(true).open(&file.path).unwrap();
        let mut reader = LineReader::new(File::open(&file.path).unwrap());
        drain(reader.lines().unwrap());

        replace(&file.path, b"x\ny\n");
        writer.write_all(b"2\n").unwrap();
        assert_eq!(drain(reader.lines().unwrap()), [line(4, "2")]);
    }

    #[rstest]
    fn a_failed_open_keeps_the_position(#[with(b"1\n")] file: TempFile) {
        let mut reader = PathLineReader::new(&file.path);
        drain(reader.lines().unwrap());

        let away = file.path.with_file_name("away");
        std::fs::rename(&file.path, &away).unwrap();
        append(&away, b"2\n");
        assert!(matches!(
            reader.lines(),
            Err(ReadLinesError::Io(err)) if err.kind() == io::ErrorKind::NotFound
        ));

        std::fs::rename(&away, &file.path).unwrap();
        assert_eq!(drain(reader.lines().unwrap()), [line(4, "2")]);
    }

    #[cfg(unix)]
    #[rstest]
    fn the_iterator_ends_after_its_first_error() {
        // A directory opens on unix, but reading it fails.
        let dir = tempfile::tempdir().unwrap();
        let mut reader = PathLineReader::new(dir.path());
        let mut lines = reader.lines().unwrap();
        assert!(matches!(lines.next(), Some(Err(_))));
        assert!(lines.next().is_none());
    }

    #[rstest]
    #[case::first_line(b"1\n22\n", 2, Some("1"))]
    #[case::last_line(b"1\n22\n", 5, Some("22"))]
    #[case::blank_line(b"1\n\n", 3, Some(""))]
    #[case::start_of_file(b"1\n", 0, None)]
    #[case::mid_line(b"1\n22\n", 4, None)]
    #[case::past_the_end(b"1\n", 3, None)]
    fn finds_the_line_ending_at_an_offset(
        #[case] contents: &[u8],
        #[case] end: u64,
        #[case] expected: Option<&'static str>,
    ) {
        let file = file(contents);
        let found = Line::ending_at(&File::open(&file.path).unwrap(), end).unwrap();
        assert_eq!(found, expected.map(|text| line(end, text)));
    }

    #[rstest]
    fn finds_a_line_longer_than_a_read(#[values("", "a\n")] before: &str) {
        let long = "m".repeat(3 * usize::try_from(READ_CHUNK_BYTES).unwrap());
        let file = file(format!("{before}{long}\n").as_bytes());
        let end = u64::try_from(before.len() + long.len() + 1).unwrap();
        let found = Line::ending_at(&File::open(&file.path).unwrap(), end).unwrap();
        assert_eq!(found.map(|line| line.bytes), Some(Bytes::from(long)));
    }

    #[derive(Debug, Clone)]
    enum Op {
        Append(Vec<u8>),
        /// Pull up to this many lines, then drop the iterator.
        Pull(usize),
    }

    fn byte() -> impl Strategy<Value = u8> {
        prop_oneof![3 => any::<u8>(), 1 => Just(b'\n')]
    }

    fn op() -> impl Strategy<Value = Op> {
        prop_oneof![
            prop::collection::vec(byte(), 0..16).prop_map(Op::Append),
            (0usize..4).prop_map(Op::Pull),
        ]
    }

    proptest! {
        // Against the reader as the oracle: an offset names a line exactly when the reader handed
        // out a line ending there.
        #[test]
        fn a_line_is_found_by_its_end(contents in prop::collection::vec(byte(), 0..64)) {
            let file = file(&contents);
            let lines = drain(PathLineReader::new(&file.path).lines().unwrap());
            let handle = File::open(&file.path).unwrap();
            for end in 0..=contents.len() as u64 + 1 {
                let expected = lines.iter().find(|line| line.end == end).cloned();
                prop_assert_eq!(Line::ending_at(&handle, end).unwrap(), expected);
            }
        }

        // Against the bytes written so far as the model: whatever interleaving of appends and
        // partly pulled iterators, the lines handed out followed by what is pending are a prefix
        // of the file, so nothing is skipped, duplicated or reordered, and pulling to the end
        // hands out every complete line.
        #[test]
        fn lines_read_are_the_complete_lines_of_the_file(ops in prop::collection::vec(op(), 0..64)) {
            let file = file(b"");
            let mut reader = PathLineReader::new(&file.path);
            let mut written = Vec::new();
            let mut read = Vec::new();
            for op in ops.into_iter().chain([Op::Pull(usize::MAX)]) {
                let lines = match op {
                    Op::Append(bytes) => {
                        append(&file.path, &bytes);
                        written.extend_from_slice(&bytes);
                        Vec::new()
                    }
                    Op::Pull(n) => drain(reader.lines().unwrap().take(n)),
                };
                for line in lines {
                    prop_assert!(!line.bytes.contains(&b'\n'));
                    read.extend_from_slice(&line.bytes);
                    read.push(b'\n');
                    prop_assert_eq!(line.end, read.len() as u64);
                }
                let mut seen = read.clone();
                seen.extend_from_slice(&reader.cursor.pending);
                prop_assert_eq!(&seen[..], &written[..seen.len()]);
            }

            let complete = written.iter().rposition(|&b| b == b'\n').map_or(0, |last| last + 1);
            prop_assert_eq!(&read[..], &written[..complete]);
        }
    }
}
