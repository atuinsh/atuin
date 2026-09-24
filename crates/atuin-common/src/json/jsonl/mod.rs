//! Newline-delimited JSON (JSONL) files, read in full or followed by offset-resumed re-reads.
//!
//! Each read opens the file, pulls one bounded batch of lines through a [`PathLineReader`], and
//! closes it again, so following a file costs no open handle between reads and, however large the
//! file, no more memory than a batch or its longest line, whichever is bigger. Only complete
//! (newline-terminated) lines are yielded: a trailing fragment stays pending until its newline
//! lands. A replaced or truncated file re-yields every line from the start, so a caller that must
//! not see a line twice dedups on its side.

use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::time::Duration;

use bytes::Bytes;
use futures::{Stream, TryStreamExt};
use serde::de::DeserializeOwned;
use tokio::sync::watch;

use crate::io::{Line, PathLineReader, ReadLinesError};
use crate::sync::BlockingPool;

/// Delay before re-reading after a failed read, doubled per failure up to [`RETRY_MAX`].
const RETRY_INITIAL: Duration = Duration::from_millis(100);
const RETRY_MAX: Duration = Duration::from_secs(5);

/// Bytes of lines pulled per read: the memory a follower needs regardless of file size, unless a
/// single line is longer.
const READ_CHUNK_BYTES: u64 = 64 * 1024;

/// An error encountered while reading a JSONL file.
#[derive(Debug, thiserror::Error)]
pub enum JsonlError {
    #[error(transparent)]
    Io(#[from] io::Error),
    #[error("failed to parse JSON on line {line}: {source}")]
    Parse {
        source: serde_json::Error,
        /// 1-based physical line, blank ones included, counted from where the stream started; it
        /// restarts at 1 when the file is truncated or replaced.
        line: u64,
    },
}

/// The lines of one bounded read, and what ended it early.
#[derive(Debug, Default)]
struct Batch {
    lines: Vec<Line>,
    /// The read stopped at [`READ_CHUNK_BYTES`], so reading again may return more right away.
    more: bool,
    /// The error that ended the read, after the lines in `lines`.
    error: Option<ReadLinesError>,
}

impl Batch {
    /// Pull lines from `reader` until they reach [`READ_CHUNK_BYTES`], the file ends, or a read
    /// fails.
    fn read(reader: &mut PathLineReader) -> Self {
        let mut batch = Self::default();
        let lines = match reader.lines() {
            Ok(lines) => lines,
            Err(error) => {
                batch.error = Some(error);
                return batch;
            }
        };
        // A loop rather than `collect::<Result<_, _>>()`: lines pulled before an error are
        // already consumed and would be lost with it.
        let mut bytes = 0;
        for line in lines {
            let line = match line {
                Ok(line) => line,
                Err(error) => {
                    batch.error = Some(error.into());
                    break;
                }
            };
            bytes += u64::try_from(line.bytes.len()).expect("a line length fits u64") + 1;
            batch.lines.push(line);
            if bytes >= READ_CHUNK_BYTES {
                batch.more = true;
                break;
            }
        }
        batch
    }
}

/// Deserialize each non-blank line of the file at `path`, re-reading on every change signal.
///
/// With `changes`, the stream reads whatever is new each time the receiver reports a change (its
/// value is ignored) and ends once its sender is dropped; a change that lands while a read is in
/// progress triggers one more read, never a missed one. A read that fails is retried with a
/// growing delay until it succeeds or the sender is dropped, and only the first failure of such a
/// streak is yielded. Without `changes`, the stream ends after a single pass, like [`read_all`].
/// Reads run in `pool`.
pub fn follow<T>(
    path: PathBuf,
    changes: Option<watch::Receiver<impl Send + Sync + 'static>>,
    pool: BlockingPool,
) -> impl Stream<Item = Result<T, JsonlError>> + Send + 'static
where
    T: DeserializeOwned + Send + 'static,
{
    follow_from(path, 0, changes, pool).map_ok(|(_, value)| value)
}

/// [`follow`], resumed at byte `start` and yielding with each value the byte offset just past
/// its line, so a caller can checkpoint that offset and resume from it later.
///
/// `start` must be an offset this stream reported, so it sits on a line boundary. One past the
/// file's current end means the file was replaced or truncated: the read restarts from zero.
/// Line numbers in a [`JsonlError::Parse`] count from `start`, not from the file's first line.
pub fn follow_from<T>(
    path: PathBuf,
    start: u64,
    changes: Option<watch::Receiver<impl Send + Sync + 'static>>,
    pool: BlockingPool,
) -> impl Stream<Item = Result<(u64, T), JsonlError>> + Send + 'static
where
    T: DeserializeOwned + Send + 'static,
{
    async_stream::stream! {
        let mut changes = changes;
        let mut reader = PathLineReader::at(&path, start);
        let mut line: u64 = 0;
        let mut retry: Option<Duration> = None;
        loop {
            // Mark the signal seen before reading, so a change that lands during the read is
            // still pending when we wait: an extra read at worst, never a missed one.
            if let Some(rx) = &mut changes {
                rx.borrow_and_update();
            }
            // The reader travels with the read; losing it to a runtime shutting down ends the
            // stream, which that shutdown drops anyway.
            let Ok((returned, batch)) = pool
                .run(move || {
                    let batch = Batch::read(&mut reader);
                    (reader, batch)
                })
                .await
            else {
                break;
            };
            reader = returned;

            for next in batch.lines {
                line += 1;
                if next.bytes.iter().all(u8::is_ascii_whitespace) {
                    continue;
                }
                yield serde_json::from_slice(&next.bytes)
                    .map(|value| (next.end, value))
                    .map_err(|source| JsonlError::Parse { source, line });
            }
            match batch.error {
                None => {
                    retry = None;
                    if batch.more {
                        continue;
                    }
                }
                Some(error @ (ReadLinesError::Truncated | ReadLinesError::Replaced)) => {
                    tracing::debug!(path = %path.display(), %error, "reading from the start");
                    line = 0;
                    retry = None;
                    continue;
                }
                Some(ReadLinesError::Io(e)) => {
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

/// The value the line ending at byte `at` carries, or `None` when the file has no line there or
/// the line does not parse as a `T`. Reads run in `pool`.
///
/// The counterpart of the offsets [`follow_from`] reports: a reader hands one back to ask what it
/// named, and decides from that whether it may resume there.
pub async fn value_at<T: DeserializeOwned>(path: &Path, at: u64, pool: &BlockingPool) -> Option<T> {
    let path = path.to_path_buf();
    let line = pool
        .run(move || File::open(path).and_then(|file| line_ending_at(&file, at)))
        .await
        .ok()?
        .ok()
        .flatten()?;
    serde_json::from_slice(&line).ok()
}

/// The complete line that ends exactly at byte `offset` of `file`, without its newline, or `None`
/// when no line does: `offset` is zero, past the end, or the byte before it is not a newline.
fn line_ending_at(mut file: &File, offset: u64) -> io::Result<Option<Bytes>> {
    if offset == 0 || file.metadata()?.len() < offset {
        return Ok(None);
    }
    // Read backwards in growing windows until the previous newline (or the file start).
    let mut window = READ_CHUNK_BYTES;
    loop {
        let start = offset.saturating_sub(window);
        file.seek(SeekFrom::Start(start))?;
        let mut buf = vec![0; usize::try_from(offset - start).expect("window fits usize")];
        file.read_exact(&mut buf)?;
        if buf.last() != Some(&b'\n') {
            return Ok(None);
        }
        let body = &buf[..buf.len() - 1];
        if let Some(newline) = memchr::memrchr(b'\n', body) {
            return Ok(Some(Bytes::copy_from_slice(&body[newline + 1..])));
        }
        if start == 0 {
            return Ok(Some(Bytes::copy_from_slice(body)));
        }
        window *= 2;
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
    follow(path, None::<watch::Receiver<()>>, pool)
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

    #[rstest]
    #[tokio::test]
    async fn line_numbers_continue_across_reads() {
        let (_dir, path) = write_jsonl(&["1", "2", "3", ""]);
        let (tx, rx) = watch::channel(());
        let mut stream = std::pin::pin!(follow::<i64>(path.clone(), Some(rx), pool()));
        for expected in 1..=3 {
            assert_eq!(stream.next().await.unwrap().unwrap(), expected);
        }

        append(&path, b"4\n\nnot-json\n");
        tx.send_replace(());
        assert_eq!(stream.next().await.unwrap().unwrap(), 4);
        // Physical line 6: the blank line 5 is skipped but counted.
        assert!(matches!(stream.next().await, Some(Err(JsonlError::Parse { line: 6, .. }))));
    }

    #[rstest]
    #[tokio::test]
    async fn line_numbers_restart_when_the_file_is_truncated() {
        let (_dir, path) = write_jsonl(&["1111", "2222", "3333", ""]);
        let (tx, rx) = watch::channel(());
        let mut stream = std::pin::pin!(follow::<i64>(path.clone(), Some(rx), pool()));
        for expected in [1111, 2222, 3333] {
            assert_eq!(stream.next().await.unwrap().unwrap(), expected);
        }

        std::fs::write(&path, "1\nbad\n").unwrap();
        tx.send_replace(());
        assert_eq!(stream.next().await.unwrap().unwrap(), 1);
        assert!(matches!(stream.next().await, Some(Err(JsonlError::Parse { line: 2, .. }))));
    }

    #[rstest]
    #[tokio::test]
    async fn follow_from_reports_the_offset_past_each_line() {
        // "1\n" ends at 2, the blank line at 3, "bad\n" at 7, "22\n" at 10.
        let (_dir, path) = write_jsonl(&["1", "", "bad", "22", ""]);
        let results: Vec<Result<(u64, i64), JsonlError>> =
            follow_from::<i64>(path, 0, None::<watch::Receiver<()>>, pool()).collect().await;
        assert_eq!(results.len(), 3);
        assert_eq!(results[0].as_ref().unwrap(), &(2, 1));
        assert!(matches!(results[1], Err(JsonlError::Parse { line: 3, .. })));
        assert_eq!(results[2].as_ref().unwrap(), &(10, 22));
    }

    /// "1\n22\n333\n" is 9 bytes: a boundary resumes after it, the end yields nothing, and past
    /// the end restarts from the first line.
    #[rstest]
    #[case(2, vec![(5, 22), (9, 333)])]
    #[case(9, vec![])]
    #[case(100, vec![(2, 1), (5, 22), (9, 333)])]
    #[tokio::test]
    async fn follow_from_resumes_at_a_boundary_and_restarts_past_the_end(
        #[case] start: u64,
        #[case] expected: Vec<(u64, i64)>,
    ) {
        let (_dir, path) = write_jsonl(&["1", "22", "333", ""]);
        let got: Vec<(u64, i64)> =
            follow_from::<i64>(path, start, None::<watch::Receiver<()>>, pool())
                .try_collect()
                .await
                .unwrap();
        assert_eq!(got, expected);
    }

    /// "1\n22\n333\n" is 9 bytes, with lines ending at 2, 5 and 9.
    #[rstest]
    #[case::first_line(2, Some(1))]
    #[case::last_line(9, Some(333))]
    #[case::start_of_file(0, None)]
    #[case::mid_line(4, None)]
    #[case::past_the_end(10, None)]
    #[tokio::test]
    async fn value_at_reads_the_line_ending_at_an_offset(
        #[case] at: u64,
        #[case] expected: Option<i64>,
    ) {
        let (_dir, path) = write_jsonl(&["1", "22", "333", ""]);
        assert_eq!(value_at::<i64>(&path, at, &pool()).await, expected);
    }

    #[rstest]
    #[tokio::test]
    async fn value_at_finds_a_line_longer_than_a_read_window() {
        let long = "9".repeat(3 * usize::try_from(READ_CHUNK_BYTES).unwrap());
        let (_dir, path) = write_jsonl(&["1", &format!("\"{long}\""), ""]);
        let at = 2 + u64::try_from(long.len()).unwrap() + 3;
        assert_eq!(value_at::<String>(&path, at, &pool()).await, Some(long));
    }

    #[rstest]
    fn a_batch_stops_once_it_holds_a_read_chunk() {
        // Three 40 KiB lines: the second takes the batch past one chunk.
        let line = "x".repeat(40 * 1024);
        let (_dir, path) = write_jsonl(&[&line, &line, &line, ""]);
        let mut reader = PathLineReader::new(&path);
        let first = Batch::read(&mut reader);
        assert_eq!((first.lines.len(), first.more), (2, true));

        let rest = Batch::read(&mut reader);
        assert_eq!((rest.lines.len(), rest.more), (1, false));
    }

    #[rstest]
    #[tokio::test]
    async fn without_a_change_signal_the_stream_ends_after_one_pass() {
        let (_dir, path) = write_jsonl(&["1", "2", "{\"partial\":"]);
        let got: Vec<i64> =
            follow::<i64>(path, None::<watch::Receiver<()>>, pool()).try_collect().await.unwrap();
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
        assert_eq!(stream.next().await.unwrap().unwrap(), 1);

        // The file is briefly unreadable at its path when the signal lands.
        let away = dir.path().join("away");
        std::fs::rename(&path, &away).unwrap();
        append(&away, b"2\n");
        tx.send_replace(());
        assert!(matches!(stream.next().await, Some(Err(JsonlError::Io(_)))));

        std::fs::rename(&away, &path).unwrap();
        assert_eq!(stream.next().await.unwrap().unwrap(), 2);

        drop(tx);
        assert!(stream.next().await.is_none());
    }

    #[rstest]
    #[tokio::test]
    async fn removal_after_the_sender_is_gone_ends_without_an_error() {
        let (_dir, path) = write_jsonl(&["1", ""]);
        let (tx, rx) = watch::channel(());
        let mut stream = std::pin::pin!(follow::<i64>(path.clone(), Some(rx), pool()));
        assert_eq!(stream.next().await.unwrap().unwrap(), 1);

        // The vanished path's last event bumps the version just before its handler drops.
        std::fs::remove_file(&path).unwrap();
        tx.send_replace(());
        drop(tx);
        assert!(stream.next().await.is_none());
    }

    #[rstest]
    #[tokio::test]
    async fn a_write_racing_a_read_is_still_delivered() {
        let (_dir, path) = write_jsonl(&["1", ""]);
        let (tx, rx) = watch::channel(());
        let mut stream = std::pin::pin!(follow::<i64>(path.clone(), Some(rx), pool()));
        assert_eq!(stream.next().await.unwrap().unwrap(), 1);

        // The stream is parked between its read and its wait: the change must not be lost.
        append(&path, b"2\n");
        tx.send_replace(());
        assert_eq!(stream.next().await.unwrap().unwrap(), 2);

        drop(tx);
        assert!(stream.next().await.is_none());
    }

    #[rstest]
    #[tokio::test]
    async fn a_change_completing_a_withheld_line_yields_it_once() {
        let (_dir, path) = write_jsonl(&["1", "{\"n\":"]);
        let (tx, rx) = watch::channel(());
        let mut stream =
            std::pin::pin!(follow::<serde_json::Value>(path.clone(), Some(rx), pool()));
        assert_eq!(stream.next().await.unwrap().unwrap(), 1);

        append(&path, b"2}\n");
        tx.send_replace(());
        assert_eq!(stream.next().await.unwrap().unwrap(), serde_json::json!({ "n": 2 }));

        drop(tx);
        assert!(stream.next().await.is_none());
    }

    fn rec_strategy() -> impl Strategy<Value = Rec> {
        (any::<i64>(), "[a-z ]{0,12}").prop_map(|(n, s)| Rec { n, s })
    }

    proptest! {
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
