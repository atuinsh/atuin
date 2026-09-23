//! Newline-delimited JSON (JSONL) files, read in full or followed by offset-resumed re-reads.
//!
//! Each read opens the file, reads one bounded chunk past the cursor, and closes it again, so
//! following a file costs no open handle between reads and, however large the file, no more
//! memory than a chunk or its longest line, whichever is bigger. Only complete
//! (newline-terminated) lines are yielded: a trailing fragment stays pending until its newline
//! lands. A replaced or truncated file re-yields every line from the start, so a caller that must
//! not see a line twice dedups on its side.

use std::fs::File;
use std::io;
use std::path::{Path, PathBuf};
use std::time::Duration;

use bytes::Bytes;
use futures::Stream;
use serde::de::DeserializeOwned;
use tokio::sync::watch;

use crate::fs::append::{AppendFile, Fill};
use crate::sync::BlockingPool;

/// Delay before re-reading after a failed read, doubled per failure up to [`RETRY_MAX`].
const RETRY_INITIAL: Duration = Duration::from_millis(100);
const RETRY_MAX: Duration = Duration::from_secs(5);

/// Bytes read per call: the memory a follower needs regardless of file size, unless a single
/// line is longer.
const READ_CHUNK_BYTES: u64 = 64 * 1024;

/// Where a follower stopped: the file position, and the physical lines consumed from the file's
/// current contents.
#[derive(Debug, Default)]
struct Cursor {
    file: AppendFile,
    line: u64,
}

/// The complete lines of one bounded read.
#[derive(Debug, Default)]
struct Lines {
    lines: Vec<Bytes>,
    /// Whether the read stopped at its chunk limit, so reading again returns more right away.
    more: bool,
}

/// An error encountered while reading a JSONL file.
#[derive(Debug, thiserror::Error)]
pub enum JsonlError {
    #[error(transparent)]
    Io(#[from] io::Error),
    #[error("failed to parse JSON on line {line}: {source}")]
    Parse {
        source: serde_json::Error,
        line: u64,
    },
}

/// Read the complete lines in the next chunk past `cursor`, advancing it past them, and
/// deserialize each as it is pulled from the returned iterator; the flag is [`Lines::more`].
///
/// Blank lines are skipped but still counted, so `line` in a [`JsonlError::Parse`] is the 1-based
/// physical line in the file as it stands now; it restarts at 1 when the file is truncated or
/// replaced.
async fn read_new<T>(
    path: &Path,
    cursor: &mut Cursor,
    pool: &BlockingPool,
) -> io::Result<(impl Iterator<Item = Result<T, JsonlError>> + Send + use<T>, bool)>
where
    T: DeserializeOwned,
{
    let Lines { lines, more } = read_blocking(path, cursor, pool).await?;
    let count = u64::try_from(lines.len()).expect("line count fits u64");
    let first = cursor.line - count + 1;
    let items = lines
        .into_iter()
        .zip(first..)
        .filter(|(bytes, _)| !bytes.iter().all(u8::is_ascii_whitespace))
        .map(|(bytes, line)| {
            serde_json::from_slice(&bytes).map_err(|source| JsonlError::Parse { source, line })
        });
    Ok((items, more))
}

/// Open `path` and run [`read_lines`] on it in `pool`.
///
/// The cursor travels with the read and is lost if the caller stops waiting or the runtime shuts
/// down; both drop the follower that owns it anyway.
async fn read_blocking(path: &Path, cursor: &mut Cursor, pool: &BlockingPool) -> io::Result<Lines> {
    let path = path.to_path_buf();
    let mut moved = std::mem::take(cursor);
    let (moved, lines) = pool
        .run(move || {
            let lines = File::open(&path).and_then(|file| read_lines(&file, &mut moved));
            (moved, lines)
        })
        .await
        .map_err(io::Error::other)?;
    *cursor = moved;
    lines
}

/// Consume the complete lines in the next chunk of `file` past `cursor`.
///
/// At most one chunk is read, more only to complete a single line longer than a chunk.
fn read_lines(file: &File, cursor: &mut Cursor) -> io::Result<Lines> {
    loop {
        // What is already pending holds no newline: it is a fragment left by an earlier read.
        let scanned = cursor.file.pending().len();
        let more = match cursor.file.fill(file, READ_CHUNK_BYTES)? {
            Fill::Reset => {
                cursor.line = 0;
                continue;
            }
            Fill::Read { more } => more,
        };
        let Some(newline) = memchr::memrchr(b'\n', &cursor.file.pending()[scanned..]) else {
            if more {
                continue;
            }
            return Ok(Lines::default());
        };
        let lines = split_lines(&cursor.file.consume(scanned + newline + 1));
        cursor.line += u64::try_from(lines.len()).expect("line count fits u64");
        return Ok(Lines { lines, more });
    }
}

/// Split newline-terminated `bytes` into its lines, without their newlines.
fn split_lines(bytes: &Bytes) -> Vec<Bytes> {
    let mut start = 0;
    memchr::memchr_iter(b'\n', bytes)
        .map(|end| {
            let line = bytes.slice(start..end);
            start = end + 1;
            line
        })
        .collect()
}

/// Deserialize each non-blank line of the file at `path`, re-reading on every change signal.
///
/// With `changes`, the stream reads whatever is new each time the receiver reports a change and
/// ends once its sender is dropped; a change that lands while a read is in progress triggers one
/// more read, never a missed one. A read that fails is retried with a growing delay until it
/// succeeds or the sender is dropped, and only the first failure of such a streak is yielded.
/// Without `changes`, the stream ends after a single pass, like [`read_all`]. Reads run in `pool`.
pub fn follow<T>(
    path: PathBuf,
    changes: Option<watch::Receiver<()>>,
    pool: BlockingPool,
) -> impl Stream<Item = Result<T, JsonlError>> + Send + 'static
where
    T: DeserializeOwned + Send + 'static,
{
    async_stream::stream! {
        let mut changes = changes;
        let mut cursor = Cursor::default();
        let mut retry: Option<Duration> = None;
        loop {
            // Mark the signal seen before reading, so a change that lands during the read is
            // still pending when we wait: an extra read at worst, never a missed one.
            if let Some(rx) = &mut changes {
                rx.borrow_and_update();
            }
            match read_new::<T>(&path, &mut cursor, &pool).await {
                Ok((items, more)) => {
                    retry = None;
                    for item in items {
                        yield item;
                    }
                    if more {
                        continue;
                    }
                }
                Err(e) => {
                    // Once the handler is gone so is the file (or the watcher): a failed last
                    // read is expected, not news.
                    if changes.as_ref().is_some_and(|rx| rx.has_changed().is_err()) {
                        break;
                    }
                    if retry.is_none() {
                        yield Err(JsonlError::Io(e));
                    }
                    retry = Some(retry.map_or(RETRY_INITIAL, |d| (d * 2).min(RETRY_MAX)));
                }
            }
            let Some(rx) = &mut changes else { break };
            tokio::select! {
                changed = rx.changed() => if changed.is_err() { break },
                () = tokio::time::sleep(retry.unwrap_or_default()), if retry.is_some() => {}
            }
        }
    }
}

/// Deserialize each non-blank complete line of the file at `path` as it stands now.
///
/// A final line with no newline is taken to be still being written and is left out rather than
/// parsed half-formed; the harness writers this serves terminate every record. Reads run in
/// `pool`.
pub fn read_all<T>(
    path: PathBuf,
    pool: BlockingPool,
) -> impl Stream<Item = Result<T, JsonlError>> + Send + 'static
where
    T: DeserializeOwned + Send + 'static,
{
    follow(path, None, pool)
}

#[cfg(test)]
mod tests {
    use std::fs::OpenOptions;
    use std::io::Write;
    use std::num::NonZeroUsize;

    use futures::{StreamExt, TryStreamExt};
    use proptest::prelude::*;
    use rstest::rstest;
    use serde::{Deserialize, Serialize};

    use super::*;
    use crate::futures::stream::timed_next;

    #[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
    struct Rec {
        n: i64,
        s: String,
    }

    fn pool() -> BlockingPool {
        BlockingPool::new(NonZeroUsize::MIN)
    }

    fn write_jsonl(lines: &[&str]) -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("f.jsonl");
        std::fs::write(&path, lines.join("\n")).unwrap();
        (dir, path)
    }

    fn append(path: &Path, bytes: &[u8]) {
        OpenOptions::new().append(true).open(path).unwrap().write_all(bytes).unwrap();
    }

    #[rstest]
    #[tokio::test]
    async fn parses_each_line_into_a_value() {
        let (_dir, path) = write_jsonl(&["1", "2", "3", ""]);
        let got: Vec<i64> = read_all::<i64>(path, pool()).try_collect().await.unwrap();
        assert_eq!(got, vec![1, 2, 3]);
    }

    #[rstest]
    #[tokio::test]
    async fn skips_blank_and_whitespace_only_lines() {
        let (_dir, path) = write_jsonl(&["1", "", "   ", "2", ""]);
        let got: Vec<i64> = read_all::<i64>(path, pool()).try_collect().await.unwrap();
        assert_eq!(got, vec![1, 2]);
    }

    #[rstest]
    #[tokio::test]
    async fn a_malformed_line_errors_then_recovery_continues() {
        let (_dir, path) = write_jsonl(&["1", "not-json", "2", ""]);
        let results: Vec<Result<i64, JsonlError>> = read_all::<i64>(path, pool()).collect().await;
        assert_eq!(results.len(), 3);
        assert_eq!(results[0].as_ref().unwrap(), &1);
        assert!(matches!(results[1], Err(JsonlError::Parse { line: 2, .. })));
        assert_eq!(results[2].as_ref().unwrap(), &2);
    }

    async fn read_values(path: &Path, cursor: &mut Cursor) -> Vec<Result<i64, JsonlError>> {
        read_new::<i64>(path, cursor, &pool()).await.unwrap().0.collect()
    }

    #[rstest]
    #[tokio::test]
    async fn resumes_from_the_cursor_with_monotonic_line_numbers() {
        let (_dir, path) = write_jsonl(&["1", "2", "3", ""]);
        let mut cursor = Cursor::default();
        let got: Vec<i64> =
            read_values(&path, &mut cursor).await.into_iter().map(Result::unwrap).collect();
        assert_eq!(got, vec![1, 2, 3]);

        append(&path, b"4\n\nnot-json\n");
        let results = read_values(&path, &mut cursor).await;
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].as_ref().unwrap(), &4);
        // Physical line 6: the blank line 5 is skipped but counted.
        assert!(matches!(results[1], Err(JsonlError::Parse { line: 6, .. })));
        assert!(read_values(&path, &mut cursor).await.is_empty());
    }

    #[rstest]
    #[tokio::test]
    async fn line_numbers_restart_when_the_file_is_truncated() {
        let (_dir, path) = write_jsonl(&["1111", "2222", "3333", ""]);
        let mut cursor = Cursor::default();
        assert_eq!(read_values(&path, &mut cursor).await.len(), 3);

        std::fs::write(&path, "1\nbad\n").unwrap();
        let results = read_values(&path, &mut cursor).await;
        assert_eq!(results.len(), 2);
        assert!(matches!(results[1], Err(JsonlError::Parse { line: 2, .. })));
    }

    #[rstest]
    #[tokio::test]
    async fn a_missing_file_errors_and_leaves_the_cursor_alone() {
        let (_dir, path) = write_jsonl(&["1", ""]);
        let mut cursor = Cursor::default();
        read_values(&path, &mut cursor).await;

        std::fs::remove_file(&path).unwrap();
        let err = read_new::<i64>(&path, &mut cursor, &pool()).await.err().unwrap();
        assert_eq!(err.kind(), io::ErrorKind::NotFound);
        assert_eq!((cursor.file.offset(), cursor.line), (2, 1));
    }

    fn lines_of(path: &Path, cursor: &mut Cursor) -> Lines {
        read_lines(&File::open(path).unwrap(), cursor).unwrap()
    }

    fn strs(lines: &Lines) -> Vec<&str> {
        lines.lines.iter().map(|l| std::str::from_utf8(l).unwrap()).collect()
    }

    #[rstest]
    #[case::empty(b"", &[], 0)]
    #[case::one(b"a\n", &["a"], 2)]
    #[case::blank_lines_are_lines(b"\n\n", &["", ""], 2)]
    #[case::unterminated_tail_withheld(b"a\nb", &["a"], 2)]
    #[case::only_a_fragment(b"abc", &[], 0)]
    fn returns_complete_lines_and_stops_at_the_last_newline(
        #[case] contents: &[u8],
        #[case] expected: &[&str],
        #[case] offset: u64,
    ) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("f");
        std::fs::write(&path, contents).unwrap();
        let mut cursor = Cursor::default();
        assert_eq!(strs(&lines_of(&path, &mut cursor)), expected);
        assert_eq!(cursor.file.offset(), offset);
        assert_eq!(cursor.line, expected.len() as u64);
    }

    #[rstest]
    fn a_withheld_fragment_is_yielded_once_when_completed() {
        let (_dir, path) = write_jsonl(&["a", "b"]);
        let mut cursor = Cursor::default();
        assert_eq!(strs(&lines_of(&path, &mut cursor)), ["a"]);
        assert!(lines_of(&path, &mut cursor).lines.is_empty());

        append(&path, b"c\n");
        assert_eq!(strs(&lines_of(&path, &mut cursor)), ["bc"]);
        assert_eq!((cursor.file.offset(), cursor.line), (5, 2));
        assert!(lines_of(&path, &mut cursor).lines.is_empty());
    }

    #[rstest]
    fn a_large_file_is_drained_in_bounded_chunks() {
        // Three 40 KiB lines: no two fit in the first chunk.
        let line = "x".repeat(40 * 1024);
        let (_dir, path) = write_jsonl(&[&line, &line, &line, ""]);
        let mut cursor = Cursor::default();
        let first = lines_of(&path, &mut cursor);
        assert_eq!((first.lines.len(), first.more), (1, true));

        let rest = lines_of(&path, &mut cursor);
        assert_eq!((rest.lines.len(), rest.more), (2, false));
        assert_eq!(cursor.line, 3);
    }

    #[rstest]
    fn a_long_line_between_short_ones_is_delivered_whole_and_in_order() {
        let long = "m".repeat(3 * usize::try_from(READ_CHUNK_BYTES).unwrap());
        let (_dir, path) = write_jsonl(&["first", &long, "last", ""]);
        let mut cursor = Cursor::default();
        let mut got = Vec::new();
        loop {
            let lines = lines_of(&path, &mut cursor);
            got.extend(lines.lines);
            if !lines.more {
                break;
            }
        }
        assert_eq!(got, [&b"first"[..], long.as_bytes(), b"last"]);
    }

    #[rstest]
    #[tokio::test]
    async fn without_a_change_signal_the_stream_ends_after_one_pass() {
        let (_dir, path) = write_jsonl(&["1", "2", "{\"partial\":"]);
        let got: Vec<i64> = follow::<i64>(path, None, pool()).try_collect().await.unwrap();
        assert_eq!(got, vec![1, 2]);
    }

    #[rstest]
    #[tokio::test]
    async fn a_file_larger_than_one_read_is_drained() {
        // Well past one read chunk (64 KiB).
        let lines: Vec<String> = (0..20_000).map(|n| n.to_string()).collect();
        let refs: Vec<&str> = lines.iter().map(String::as_str).chain(std::iter::once("")).collect();
        let (_dir, path) = write_jsonl(&refs);
        let got: Vec<i64> = read_all::<i64>(path, pool()).try_collect().await.unwrap();
        assert_eq!(got, (0..20_000).collect::<Vec<i64>>());
    }

    #[rstest]
    #[tokio::test]
    async fn a_read_that_fails_transiently_is_retried() {
        let (dir, path) = write_jsonl(&["1", ""]);
        let (tx, rx) = watch::channel(());
        let mut stream = std::pin::pin!(follow::<i64>(path.clone(), Some(rx), pool()));
        assert_eq!(timed_next(&mut stream, 5).await.unwrap().unwrap(), 1);

        // The file is briefly unreadable at its path when the signal lands.
        let away = dir.path().join("away");
        std::fs::rename(&path, &away).unwrap();
        append(&away, b"2\n");
        tx.send_replace(());
        assert!(matches!(timed_next(&mut stream, 5).await, Some(Err(JsonlError::Io(_)))));

        std::fs::rename(&away, &path).unwrap();
        assert_eq!(timed_next(&mut stream, 5).await.unwrap().unwrap(), 2);

        drop(tx);
        assert!(timed_next(&mut stream, 5).await.is_none());
    }

    #[rstest]
    #[tokio::test]
    async fn removal_after_the_sender_is_gone_ends_without_an_error() {
        let (_dir, path) = write_jsonl(&["1", ""]);
        let (tx, rx) = watch::channel(());
        let mut stream = std::pin::pin!(follow::<i64>(path.clone(), Some(rx), pool()));
        assert_eq!(timed_next(&mut stream, 5).await.unwrap().unwrap(), 1);

        // The vanished path's last event bumps the version just before its handler drops.
        std::fs::remove_file(&path).unwrap();
        tx.send_replace(());
        drop(tx);
        assert!(timed_next(&mut stream, 5).await.is_none());
    }

    #[rstest]
    #[tokio::test]
    async fn a_write_racing_a_read_is_still_delivered() {
        let (_dir, path) = write_jsonl(&["1", ""]);
        let (tx, rx) = watch::channel(());
        let mut stream = std::pin::pin!(follow::<i64>(path.clone(), Some(rx), pool()));
        assert_eq!(timed_next(&mut stream, 5).await.unwrap().unwrap(), 1);

        // The stream is parked between its read and its wait: the change must not be lost.
        append(&path, b"2\n");
        tx.send_replace(());
        assert_eq!(timed_next(&mut stream, 5).await.unwrap().unwrap(), 2);

        drop(tx);
        assert!(timed_next(&mut stream, 5).await.is_none());
    }

    #[rstest]
    #[tokio::test]
    async fn a_change_completing_a_withheld_line_yields_it_once() {
        let (_dir, path) = write_jsonl(&["1", "{\"n\":"]);
        let (tx, rx) = watch::channel(());
        let mut stream =
            std::pin::pin!(follow::<serde_json::Value>(path.clone(), Some(rx), pool()));
        assert_eq!(timed_next(&mut stream, 5).await.unwrap().unwrap(), 1);

        append(&path, b"2}\n");
        tx.send_replace(());
        assert_eq!(
            timed_next(&mut stream, 5).await.unwrap().unwrap(),
            serde_json::json!({ "n": 2 })
        );

        drop(tx);
        assert!(timed_next(&mut stream, 5).await.is_none());
    }

    fn rec_strategy() -> impl Strategy<Value = Rec> {
        (any::<i64>(), "[a-z ]{0,12}").prop_map(|(n, s)| Rec { n, s })
    }

    proptest! {
        // Drive the cursor across a randomised sequence of chunked appends -- some completing a
        // withheld fragment, some not -- and assert every complete line is yielded exactly once
        // and in order, and no fragment is emitted before its newline lands.
        #[test]
        fn every_complete_line_is_yielded_once_across_chunked_appends(
            lines in prop::collection::vec("[^\n]{0,24}", 0..8),
            final_newline in any::<bool>(),
            splits in prop::collection::vec(1usize..=4, 0..64),
        ) {
            let mut body = lines.join("\n").into_bytes();
            if final_newline && !body.is_empty() {
                body.push(b'\n');
            }
            let expected: Vec<&[u8]> = match memchr::memrchr(b'\n', &body) {
                Some(last) => body[..last].split(|&b| b == b'\n').collect(),
                None => Vec::new(),
            };

            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("f");
            std::fs::write(&path, b"").unwrap();
            let mut cursor = Cursor::default();
            let mut got: Vec<Bytes> = Vec::new();
            let mut pos = 0;
            let mut sizes = splits.into_iter();
            while pos < body.len() {
                let n = sizes.next().unwrap_or(body.len()).min(body.len() - pos);
                append(&path, &body[pos..pos + n]);
                pos += n;
                loop {
                    let lines = lines_of(&path, &mut cursor);
                    got.extend(lines.lines);
                    if !lines.more {
                        break;
                    }
                }
            }
            prop_assert!(lines_of(&path, &mut cursor).lines.is_empty());
            prop_assert_eq!(got.iter().map(|b| &b[..]).collect::<Vec<&[u8]>>(), expected);
        }

        #[test]
        fn round_trips_records(recs in prop::collection::vec(rec_strategy(), 0..20)) {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("f.jsonl");
            let mut body = String::new();
            for rec in &recs {
                body.push_str(&serde_json::to_string(rec).unwrap());
                body.push('\n');
            }
            std::fs::write(&path, body).unwrap();

            let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
            let got: Vec<Rec> =
                runtime.block_on(async { read_all::<Rec>(path, pool()).try_collect().await.unwrap() });
            prop_assert_eq!(got, recs);
        }
    }
}
