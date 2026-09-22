//! Bounded, offset-resumed line reads of a growing file.
//!
//! Each [`read_new_lines`] call opens the file, reads one chunk of what lies past the
//! [`LineCursor`], and closes it again, so following a file costs no open handle between reads
//! and no more memory than a chunk however large the file. At most 32 blocking reads run at once
//! process-wide (a caller that gives up waiting still counts until its read ends), which bounds
//! the open handles a storm of callers can add. Only complete
//! (newline-terminated) lines are returned: a trailing fragment stays in the file until a later
//! call sees its newline, which is what makes the cursor safe to checkpoint after every call.
//!
//! Writers are assumed to append: an in-place rewrite that keeps the file's identity and does not
//! shrink it below the cursor is not detected. A replaced or truncated file is, and re-yields
//! every line from the start, so a caller that must not see a line twice dedups on its side.

use std::io::{self, Read, Seek, SeekFrom};
use std::path::Path;

use bytes::Bytes;
use tokio::sync::Semaphore;

use crate::os::fs::FdIdentity;
#[cfg(windows)]
use crate::os::fs::FdIdentityExt;

/// Bytes read per call: the memory a follower needs regardless of file size.
const READ_CHUNK: u64 = 64 * 1024;

/// Reads (and so open handles) in flight at once across every caller.
const MAX_IN_FLIGHT: usize = 32;
static IN_FLIGHT: Semaphore = Semaphore::const_new(MAX_IN_FLIGHT);

/// Where reading stopped: the byte offset just past the last complete line, and which file that
/// offset belongs to.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct LineCursor {
    offset: u64,
    line: u64,
    identity: Option<FdIdentity>,
    more: bool,
}

impl LineCursor {
    /// Byte offset of the first byte not yet returned as part of a complete line.
    #[must_use]
    pub fn offset(&self) -> u64 {
        self.offset
    }

    /// Number of complete lines returned so far from the current file contents.
    #[must_use]
    pub fn line(&self) -> u64 {
        self.line
    }

    /// Whether the last call stopped at its chunk limit with bytes still unread, so calling
    /// again returns more right away.
    #[must_use]
    pub fn has_more(&self) -> bool {
        self.more
    }
}

/// Return the complete lines in the next chunk past `cursor`, advancing it past them.
///
/// At most one chunk is read per call (more only to complete a single line longer than a
/// chunk); [`LineCursor::has_more`] says whether another call would return more right away.
/// The cursor restarts from the beginning, re-yielding every line, when the file at `path` is not
/// the one it was read from (replaced) or has shrunk below the offset (truncated); a rewrite that
/// does neither goes unnoticed. An unterminated trailing fragment is withheld and re-read in full
/// once its newline lands. The file is not held open between calls.
///
/// # Errors
///
/// Any I/O failure of the open, stat, seek or read. The cursor never advances on failure, though a
/// replacement or truncation noticed before the failing call has already reset it, which is
/// harmless: the retry re-reads from the start.
pub async fn read_new_lines(path: &Path, cursor: &mut LineCursor) -> io::Result<Vec<Bytes>> {
    let permit = IN_FLIGHT.acquire().await.expect("semaphore is never closed");
    // One blocking hop for the whole open/stat/seek/read/close sequence, rather than one per
    // `tokio::fs` call. The permit travels with it: a caller that stops waiting does not stop
    // the read, and the handle it holds is what the bound is for.
    let path = path.to_path_buf();
    let mut scratch = *cursor;
    let (scratch, lines) = tokio::task::spawn_blocking(move || {
        let _permit = permit;
        let lines = read_blocking(&path, &mut scratch);
        (scratch, lines)
    })
    .await
    .map_err(io::Error::other)?;
    *cursor = scratch;
    lines
}

fn read_blocking(path: &Path, cursor: &mut LineCursor) -> io::Result<Vec<Bytes>> {
    let mut file = std::fs::File::open(path)?;
    let meta = file.metadata()?;
    #[cfg(unix)]
    let identity = Some(FdIdentity::from_metadata(&meta));
    #[cfg(windows)]
    let identity = Some(file.identity()?);
    #[cfg(not(any(unix, windows)))]
    let identity = None;
    if identity != cursor.identity || meta.len() < cursor.offset {
        cursor.offset = 0;
        cursor.line = 0;
    }
    cursor.identity = identity;

    if cursor.offset > 0 {
        file.seek(SeekFrom::Start(cursor.offset))?;
    }
    let remaining = meta.len().saturating_sub(cursor.offset);
    let mut buf = Vec::with_capacity(usize::try_from(remaining.min(READ_CHUNK)).unwrap_or(0));
    // One chunk, extended only as far as it takes to complete a line longer than the chunk.
    let cut = loop {
        let before = buf.len();
        let read = file.by_ref().take(READ_CHUNK).read_to_end(&mut buf)?;
        if let Some(newline) = memchr::memrchr(b'\n', &buf[before..]) {
            break before + newline + 1;
        }
        if u64::try_from(read).expect("read size fits u64") < READ_CHUNK {
            break 0;
        }
    };
    drop(file);

    let buf = Bytes::from(buf);
    let mut lines = Vec::new();
    let mut start = 0;
    for end in memchr::memchr_iter(b'\n', &buf[..cut]) {
        lines.push(buf.slice(start..end));
        start = end + 1;
    }
    let read_end = cursor.offset + u64::try_from(buf.len()).expect("byte offset fits u64");
    cursor.offset += u64::try_from(cut).expect("byte offset fits u64");
    cursor.line += u64::try_from(lines.len()).expect("line count fits u64");
    cursor.more = read_end < meta.len();
    Ok(lines)
}

#[cfg(test)]
mod tests {
    use std::fs::OpenOptions;
    use std::io::Write;
    use std::path::PathBuf;
    use std::time::Duration;

    use rstest::rstest;

    use super::*;

    fn temp_file(contents: &[u8]) -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("f");
        std::fs::write(&path, contents).unwrap();
        (dir, path)
    }

    fn append(path: &Path, bytes: &[u8]) {
        OpenOptions::new().append(true).open(path).unwrap().write_all(bytes).unwrap();
    }

    fn strs(lines: &[Bytes]) -> Vec<&str> {
        lines.iter().map(|l| std::str::from_utf8(l).unwrap()).collect()
    }

    #[rstest]
    #[case::empty(b"", &[], 0)]
    #[case::one(b"a\n", &["a"], 2)]
    #[case::blank_lines_are_lines(b"\n\n", &["", ""], 2)]
    #[case::unterminated_tail_withheld(b"a\nb", &["a"], 2)]
    #[case::only_a_fragment(b"abc", &[], 0)]
    #[tokio::test]
    async fn returns_complete_lines_and_stops_at_the_last_newline(
        #[case] contents: &[u8],
        #[case] expected: &[&str],
        #[case] offset: u64,
    ) {
        let (_dir, path) = temp_file(contents);
        let mut cursor = LineCursor::default();
        let lines = read_new_lines(&path, &mut cursor).await.unwrap();
        assert_eq!(strs(&lines), expected);
        assert_eq!(cursor.offset(), offset);
        assert_eq!(cursor.line(), expected.len() as u64);
    }

    #[rstest]
    #[tokio::test]
    async fn a_withheld_fragment_is_yielded_once_when_completed() {
        let (_dir, path) = temp_file(b"a\nb");
        let mut cursor = LineCursor::default();
        assert_eq!(strs(&read_new_lines(&path, &mut cursor).await.unwrap()), ["a"]);

        // Still incomplete: nothing new, cursor still parked before the fragment.
        assert!(read_new_lines(&path, &mut cursor).await.unwrap().is_empty());
        assert_eq!(cursor.offset(), 2);

        append(&path, b"c\n");
        assert_eq!(strs(&read_new_lines(&path, &mut cursor).await.unwrap()), ["bc"]);
        assert_eq!(cursor.offset(), 5);
        assert_eq!(cursor.line(), 2);
        assert!(read_new_lines(&path, &mut cursor).await.unwrap().is_empty());
    }

    #[rstest]
    #[tokio::test]
    async fn a_second_call_reads_only_the_appended_delta() {
        let (_dir, path) = temp_file(b"first line\nsecond line\n");
        let mut cursor = LineCursor::default();
        let lines = read_new_lines(&path, &mut cursor).await.unwrap();
        assert_eq!(lines.len(), 2);
        assert_eq!(cursor.offset(), 23);

        append(&path, b"x\n");
        let lines = read_new_lines(&path, &mut cursor).await.unwrap();
        assert_eq!(strs(&lines), ["x"]);
        assert_eq!(cursor.offset(), 25);
        assert_eq!(cursor.line(), 3);
    }

    #[rstest]
    #[tokio::test]
    async fn a_large_file_is_drained_in_bounded_chunks() {
        // Three 40 KiB lines: no two fit in one chunk, so each call returns exactly one.
        let line = vec![b'x'; 40 * 1024];
        let mut body = Vec::new();
        for _ in 0..3 {
            body.extend_from_slice(&line);
            body.push(b'\n');
        }
        let (_dir, path) = temp_file(&body);
        let mut cursor = LineCursor::default();
        for n in 1..=3 {
            let lines = read_new_lines(&path, &mut cursor).await.unwrap();
            assert_eq!(lines.len(), 1, "call {n}");
            assert_eq!(cursor.line(), n);
            assert_eq!(cursor.has_more(), n < 3, "call {n}");
        }
        assert!(read_new_lines(&path, &mut cursor).await.unwrap().is_empty());
        assert!(!cursor.has_more());
    }

    #[rstest]
    #[tokio::test]
    async fn a_line_longer_than_a_chunk_is_returned_whole() {
        let mut body = vec![b'y'; 200 * 1024];
        let (_dir, path) = temp_file(&body);
        let mut cursor = LineCursor::default();
        // Still a fragment however long it is.
        assert!(read_new_lines(&path, &mut cursor).await.unwrap().is_empty());
        assert_eq!(cursor.offset(), 0);
        assert!(!cursor.has_more());

        body.push(b'\n');
        std::fs::write(&path, &body).unwrap();
        let lines = read_new_lines(&path, &mut cursor).await.unwrap();
        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0].len(), 200 * 1024);
        assert_eq!(cursor.offset(), 200 * 1024 + 1);
        assert!(!cursor.has_more());
    }

    #[rstest]
    #[tokio::test]
    async fn a_long_line_between_short_ones_is_delivered_whole_and_in_order() {
        let long = vec![b'm'; 3 * usize::try_from(READ_CHUNK).unwrap()];
        let mut body = b"first\n".to_vec();
        body.extend_from_slice(&long);
        body.extend_from_slice(b"\nlast\n");
        let (_dir, path) = temp_file(&body);
        let mut cursor = LineCursor::default();
        let mut got = Vec::new();
        loop {
            got.extend(read_new_lines(&path, &mut cursor).await.unwrap());
            // The cursor only ever rests just past a newline.
            assert!(
                cursor.offset() == 0
                    || body[usize::try_from(cursor.offset()).unwrap() - 1] == b'\n'
            );
            if !cursor.has_more() {
                break;
            }
        }
        assert_eq!(got.len(), 3);
        assert_eq!(&got[0][..], b"first");
        assert_eq!(&got[1][..], &long[..]);
        assert_eq!(&got[2][..], b"last");
        assert_eq!(cursor.offset(), u64::try_from(body.len()).unwrap());
    }

    #[rstest]
    #[tokio::test]
    async fn truncation_restarts_from_the_beginning() {
        let (_dir, path) = temp_file(b"aaaa\nbbbb\n");
        let mut cursor = LineCursor::default();
        read_new_lines(&path, &mut cursor).await.unwrap();
        assert_eq!(cursor.offset(), 10);

        std::fs::write(&path, b"c\n").unwrap();
        let lines = read_new_lines(&path, &mut cursor).await.unwrap();
        assert_eq!(strs(&lines), ["c"]);
        assert_eq!(cursor.offset(), 2);
        assert_eq!(cursor.line(), 1);
    }

    #[cfg(unix)]
    #[rstest]
    #[tokio::test]
    async fn a_replaced_file_restarts_from_the_beginning_even_when_larger() {
        let (dir, path) = temp_file(b"1\n2\n");
        let mut cursor = LineCursor::default();
        read_new_lines(&path, &mut cursor).await.unwrap();
        assert_eq!(cursor.offset(), 4);

        // Same size or larger, so only the identity change can reveal the swap.
        let tmp = dir.path().join("tmp");
        std::fs::write(&tmp, b"x\ny\nz\n").unwrap();
        std::fs::rename(&tmp, &path).unwrap();
        let lines = read_new_lines(&path, &mut cursor).await.unwrap();
        assert_eq!(strs(&lines), ["x", "y", "z"]);
        assert_eq!(cursor.offset(), 6);
        assert_eq!(cursor.line(), 3);
    }

    // The permit must follow the blocking read, not the awaiting caller: a cancelled caller
    // leaves its open handle in the blocking pool, and that is what the bound is for.
    #[cfg(unix)]
    #[rstest]
    #[tokio::test]
    async fn a_cancelled_read_holds_its_permit_until_the_blocking_read_ends() {
        let dir = tempfile::tempdir().unwrap();
        let fifo = dir.path().join("fifo");
        assert!(std::process::Command::new("mkfifo").arg(&fifo).status().unwrap().success());

        // Opening a FIFO with no writer blocks, so the caller times out mid-open.
        let mut cursor = LineCursor::default();
        let read = read_new_lines(&fifo, &mut cursor);
        assert!(tokio::time::timeout(Duration::from_millis(100), read).await.is_err());
        let held = IN_FLIGHT.available_permits() < MAX_IN_FLIGHT;

        // Connect and close a writer before asserting, or a failure leaves the open blocked and
        // the runtime never shuts down: the open completes, the read sees EOF, the permit returns.
        drop(OpenOptions::new().write(true).open(&fifo).unwrap());
        assert!(held, "permit released while the open still blocks");
        tokio::time::timeout(Duration::from_secs(5), async {
            while IN_FLIGHT.available_permits() < MAX_IN_FLIGHT {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("permit returns once the blocking read ends");
    }

    #[rstest]
    #[tokio::test]
    async fn a_missing_file_errors_and_leaves_the_cursor_alone() {
        let (_dir, path) = temp_file(b"1\n");
        let mut cursor = LineCursor::default();
        read_new_lines(&path, &mut cursor).await.unwrap();
        let before = cursor;

        std::fs::remove_file(&path).unwrap();
        let err = read_new_lines(&path, &mut cursor).await.unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::NotFound);
        assert_eq!(cursor, before);
    }
}
