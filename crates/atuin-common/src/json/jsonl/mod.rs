//! Newline-delimited JSON (JSONL) files, read in full or followed by offset-resumed re-reads.

use std::io;
use std::path::{Path, PathBuf};
use std::time::Duration;

use futures::{Stream, TryStreamExt};
use serde::de::DeserializeOwned;
use tokio::sync::watch;

use crate::fs::lines::{LineCursor, read_new_lines};

/// Delay before re-reading after a failed read, doubled per failure up to [`RETRY_MAX`].
const RETRY_INITIAL: Duration = Duration::from_millis(100);
const RETRY_MAX: Duration = Duration::from_secs(5);

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
/// deserialize each as it is pulled from the returned iterator.
///
/// Blank lines are skipped but still counted, so `line` in a [`JsonlError::Parse`] is the 1-based
/// physical line in the file as it stands now; it restarts at 1 with the cursor when the file is
/// truncated or replaced. Like [`read_new_lines`], one call reads a bounded chunk: keep calling
/// while [`LineCursor::has_more`] to drain a large file.
///
/// # Errors
///
/// Any I/O failure of the underlying read; see [`read_new_lines`].
pub async fn read_new<T>(
    path: &Path,
    cursor: &mut LineCursor,
) -> io::Result<impl Iterator<Item = Result<T, JsonlError>> + Send + use<T>>
where
    T: DeserializeOwned,
{
    let lines = read_new_lines(path, cursor).await?;
    let count = u64::try_from(lines.len()).expect("line count fits u64");
    let first = cursor.line() - count + 1;
    Ok(lines
        .into_iter()
        .zip(first..)
        .filter(|(bytes, _)| !bytes.iter().all(u8::is_ascii_whitespace))
        .map(|(bytes, line)| {
            serde_json::from_slice(&bytes).map_err(|source| JsonlError::Parse { source, line })
        }))
}

/// Deserialize each non-blank line of the file at `path`, re-reading on every change signal.
///
/// With `changes`, the stream reads whatever is new each time the receiver reports a change and
/// ends once its sender is dropped; a change that lands while a read is in progress triggers one
/// more read, never a missed one. A read that fails is retried with a growing delay until it
/// succeeds or the sender is dropped, and only the first failure of such a streak is yielded.
/// Without `changes`, the stream ends after a single pass, like [`read_all`].
pub fn follow<T>(
    path: PathBuf,
    changes: Option<watch::Receiver<()>>,
) -> impl Stream<Item = Result<T, JsonlError>> + Send + 'static
where
    T: DeserializeOwned + Send + 'static,
{
    follow_from(path, 0, changes).map_ok(|(_, value)| value)
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
    changes: Option<watch::Receiver<()>>,
) -> impl Stream<Item = Result<(u64, T), JsonlError>> + Send + 'static
where
    T: DeserializeOwned + Send + 'static,
{
    async_stream::stream! {
        let mut changes = changes;
        let mut cursor = LineCursor::at(start);
        let mut retry: Option<Duration> = None;
        let mut first = true;
        loop {
            // Mark the signal seen before reading, so a change that lands during the read is
            // still pending when we wait: an extra read at worst, never a missed one.
            if let Some(rx) = &mut changes {
                rx.borrow_and_update();
            }
            match read_new_lines(&path, &mut cursor).await {
                Ok(lines) => {
                    retry = None;
                    let count = u64::try_from(lines.len()).expect("line count fits u64");
                    let bytes: u64 = lines
                        .iter()
                        .map(|line| u64::try_from(line.len()).expect("line length fits u64") + 1)
                        .sum();
                    // Taken after the read: a replaced or truncated file resets the cursor.
                    let mut at = cursor.offset() - bytes;
                    if first && start > 0 && at == 0 {
                        tracing::debug!(
                            path = %path.display(),
                            start,
                            "resume offset is past the end of the file; reading from the start"
                        );
                    }
                    first = false;
                    let first_line = cursor.line() - count + 1;
                    for (i, line) in lines.into_iter().enumerate() {
                        at += u64::try_from(line.len()).expect("line length fits u64") + 1;
                        if line.iter().all(u8::is_ascii_whitespace) {
                            continue;
                        }
                        let number = first_line + u64::try_from(i).expect("line index fits u64");
                        yield serde_json::from_slice(&line)
                            .map(|value| (at, value))
                            .map_err(|source| JsonlError::Parse { source, line: number });
                    }
                    if cursor.has_more() {
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
/// parsed half-formed; the harness writers this serves terminate every record.
pub fn read_all<T>(path: PathBuf) -> impl Stream<Item = Result<T, JsonlError>> + Send + 'static
where
    T: DeserializeOwned + Send + 'static,
{
    follow(path, None)
}

#[cfg(test)]
mod tests {
    use std::fs::OpenOptions;
    use std::io::Write;

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
        let got: Vec<i64> = read_all::<i64>(path).try_collect().await.unwrap();
        assert_eq!(got, vec![1, 2, 3]);
    }

    #[rstest]
    #[tokio::test]
    async fn skips_blank_and_whitespace_only_lines() {
        let (_dir, path) = write_jsonl(&["1", "", "   ", "2", ""]);
        let got: Vec<i64> = read_all::<i64>(path).try_collect().await.unwrap();
        assert_eq!(got, vec![1, 2]);
    }

    #[rstest]
    #[tokio::test]
    async fn a_malformed_line_errors_then_recovery_continues() {
        let (_dir, path) = write_jsonl(&["1", "not-json", "2", ""]);
        let results: Vec<Result<i64, JsonlError>> = read_all::<i64>(path).collect().await;
        assert_eq!(results.len(), 3);
        assert_eq!(results[0].as_ref().unwrap(), &1);
        assert!(matches!(results[1], Err(JsonlError::Parse { line: 2, .. })));
        assert_eq!(results[2].as_ref().unwrap(), &2);
    }

    #[rstest]
    #[tokio::test]
    async fn resumes_from_the_cursor_with_monotonic_line_numbers() {
        let (_dir, path) = write_jsonl(&["1", "2", "3", ""]);
        let mut cursor = LineCursor::default();
        let got: Vec<i64> =
            read_new::<i64>(&path, &mut cursor).await.unwrap().map(Result::unwrap).collect();
        assert_eq!(got, vec![1, 2, 3]);

        append(&path, b"4\n\nnot-json\n");
        let results: Vec<_> = read_new::<i64>(&path, &mut cursor).await.unwrap().collect();
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].as_ref().unwrap(), &4);
        // Physical line 6: the blank line 5 is skipped but counted.
        assert!(matches!(results[1], Err(JsonlError::Parse { line: 6, .. })));
        assert_eq!(read_new::<i64>(&path, &mut cursor).await.unwrap().count(), 0);
    }

    #[rstest]
    #[tokio::test]
    async fn line_numbers_restart_with_the_cursor() {
        let (_dir, path) = write_jsonl(&["1111", "2222", "3333", ""]);
        let mut cursor = LineCursor::default();
        assert_eq!(read_new::<i64>(&path, &mut cursor).await.unwrap().count(), 3);

        std::fs::write(&path, "1\nbad\n").unwrap();
        let results: Vec<_> = read_new::<i64>(&path, &mut cursor).await.unwrap().collect();
        assert_eq!(results.len(), 2);
        assert!(matches!(results[1], Err(JsonlError::Parse { line: 2, .. })));
    }

    #[rstest]
    #[tokio::test]
    async fn follow_from_reports_the_offset_past_each_line() {
        // "1\n" ends at 2, the blank line at 3, "bad\n" at 7, "22\n" at 10.
        let (_dir, path) = write_jsonl(&["1", "", "bad", "22", ""]);
        let results: Vec<Result<(u64, i64), JsonlError>> =
            follow_from::<i64>(path, 0, None).collect().await;
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
            follow_from::<i64>(path, start, None).try_collect().await.unwrap();
        assert_eq!(got, expected);
    }

    #[rstest]
    #[tokio::test]
    async fn without_a_change_signal_the_stream_ends_after_one_pass() {
        let (_dir, path) = write_jsonl(&["1", "2", "{\"partial\":"]);
        let got: Vec<i64> = follow::<i64>(path, None).try_collect().await.unwrap();
        assert_eq!(got, vec![1, 2]);
    }

    #[rstest]
    #[tokio::test]
    async fn a_file_larger_than_one_read_is_drained() {
        // Well past one read chunk (64 KiB).
        let lines: Vec<String> = (0..20_000).map(|n| n.to_string()).collect();
        let refs: Vec<&str> = lines.iter().map(String::as_str).chain(std::iter::once("")).collect();
        let (_dir, path) = write_jsonl(&refs);
        let got: Vec<i64> = read_all::<i64>(path).try_collect().await.unwrap();
        assert_eq!(got, (0..20_000).collect::<Vec<i64>>());
    }

    #[rstest]
    #[tokio::test]
    async fn a_read_that_fails_transiently_is_retried() {
        let (dir, path) = write_jsonl(&["1", ""]);
        let (tx, rx) = watch::channel(());
        let mut stream = std::pin::pin!(follow::<i64>(path.clone(), Some(rx)));
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
        let mut stream = std::pin::pin!(follow::<i64>(path.clone(), Some(rx)));
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
        let mut stream = std::pin::pin!(follow::<i64>(path.clone(), Some(rx)));
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
        let mut stream = std::pin::pin!(follow::<serde_json::Value>(path.clone(), Some(rx)));
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
                runtime.block_on(async { read_all::<Rec>(path).try_collect().await.unwrap() });
            prop_assert_eq!(got, recs);
        }
    }
}
