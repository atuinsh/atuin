//! Following the lines appended to a file as it changes.

use std::io;
use std::time::Duration;

use futures::Stream;
use tokio::sync::watch;

use crate::io::{AsyncReadLines, Line};

/// Delay before re-reading after a failed read, doubled per failure up to [`RETRY_MAX`].
const RETRY_INITIAL: Duration = Duration::from_millis(100);
const RETRY_MAX: Duration = Duration::from_secs(5);

/// Streams the lines an [`AsyncReadLines`] hands out, once or each time a file changes.
///
/// # Example
///
/// ```
/// use std::fs::OpenOptions;
/// use std::io::Write;
/// use std::num::NonZeroUsize;
/// use std::pin::pin;
///
/// use atuin_common::io::{FollowLines, Line, PathLineReader, PooledLines};
/// use atuin_common::sync::BlockingPool;
/// use futures::{StreamExt, TryStreamExt};
/// use tokio::sync::watch;
///
/// # #[tokio::main(flavor = "current_thread")]
/// # async fn main() {
/// # let dir = tempfile::tempdir().unwrap();
/// # let path = dir.path().join("log");
/// std::fs::write(&path, b"first\n").unwrap();
/// let pool = BlockingPool::new(NonZeroUsize::MIN);
///
/// // One pass over the lines the file holds now.
/// let source = PooledLines::new(PathLineReader::new(&path), pool.clone());
/// let lines: Vec<Line> = FollowLines::new(source).read_to_end().try_collect().await.unwrap();
/// assert_eq!(lines.len(), 1);
/// assert_eq!(lines[0].bytes, "first");
///
/// // The same lines, then those appended after each change, until the sender drops.
/// let (changed, changes) = watch::channel(());
/// let source = PooledLines::new(PathLineReader::new(&path), pool);
/// let mut lines = pin!(FollowLines::new(source).follow(changes));
/// assert_eq!(lines.next().await.unwrap().unwrap().bytes, "first");
///
/// OpenOptions::new().append(true).open(&path).unwrap().write_all(b"second\n").unwrap();
/// changed.send_replace(());
/// assert_eq!(lines.next().await.unwrap().unwrap().bytes, "second");
///
/// drop(changed);
/// assert!(lines.next().await.is_none());
/// # }
/// ```
#[derive(Debug)]
pub struct FollowLines<S> {
    source: S,
}

impl<S> FollowLines<S> {
    #[must_use]
    pub const fn new(source: S) -> Self {
        Self { source }
    }
}

impl<S: AsyncReadLines + Send + 'static> FollowLines<S> {
    /// The lines the source has now; the stream ends at the file's end or after a failed read.
    pub fn read_to_end(self) -> impl Stream<Item = io::Result<Line>> + Send + 'static {
        async_stream::stream! {
            let mut source = self.source;
            for await line in source.lines() {
                yield line;
            }
        }
    }

    /// The lines the source has now, then those it has each time `changes` reports a change (its
    /// value is ignored), until the sender drops.
    pub fn follow(
        self,
        mut changes: watch::Receiver<impl Send + Sync + 'static>,
    ) -> impl Stream<Item = io::Result<Line>> + Send + 'static {
        async_stream::stream! {
            let mut source = self.source;
            let mut retry: Option<Duration> = None;
            loop {
                changes.borrow_and_update();

                let mut failure = None;
                for await line in source.lines() {
                    match line {
                        Ok(line) => yield Ok(line),
                        Err(err) => failure = Some(err),
                    }
                }

                match failure {
                    None => retry = None,
                    Some(err) => {
                        // Once the sender is gone so is the file (or its watcher): a failed last
                        // read is expected, not news.
                        if changes.has_changed().is_err() {
                            break;
                        }
                        if retry.is_none() {
                            yield Err(err);
                        }
                        retry =
                            Some(retry.map_or(RETRY_INITIAL, |delay| (delay * 2).min(RETRY_MAX)));
                    }
                }
                tokio::select! {
                    changed = changes.changed() => if changed.is_err() { break },
                    () = tokio::time::sleep(retry.unwrap_or_default()), if retry.is_some() => {}
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;
    use std::pin::pin;

    use bytes::Bytes;
    use futures::{StreamExt, TryStreamExt, stream};
    use rstest::rstest;

    use super::*;

    /// A source handing out one scripted pass per call, then nothing.
    #[derive(Debug)]
    struct Scripted(VecDeque<Vec<io::Result<Line>>>);

    impl AsyncReadLines for Scripted {
        fn lines(&mut self) -> impl Stream<Item = io::Result<Line>> + Send + '_ {
            stream::iter(self.0.pop_front().unwrap_or_default())
        }
    }

    /// A line as [`Scripted`] hands it out; nothing here reads its end.
    fn line(text: &'static str) -> Line {
        Line {
            end: 0,
            bytes: Bytes::from_static(text.as_bytes()),
        }
    }

    fn broken() -> io::Result<Line> {
        Err(io::Error::other("broken"))
    }

    #[rstest]
    #[tokio::test]
    async fn read_to_end_reads_a_single_pass() {
        let source = Scripted([vec![Ok(line("a"))], vec![Ok(line("b"))]].into());
        let got: Vec<Line> = FollowLines::new(source).read_to_end().try_collect().await.unwrap();
        assert_eq!(got, [line("a")]);
    }

    #[rstest]
    #[tokio::test]
    async fn a_change_while_the_stream_is_suspended_is_not_lost() {
        let source = Scripted([vec![Ok(line("a"))], vec![Ok(line("b"))]].into());
        let (tx, rx) = watch::channel(());
        let mut lines = pin!(FollowLines::new(source).follow(rx));
        assert_eq!(lines.next().await.unwrap().unwrap(), line("a"));

        // The stream is suspended at its yield, after its read and before its wait.
        tx.send_replace(());
        assert_eq!(lines.next().await.unwrap().unwrap(), line("b"));
    }

    #[rstest]
    #[tokio::test(start_paused = true)]
    async fn a_failing_read_is_retried_and_only_its_first_failure_yielded() {
        let source = Scripted([vec![broken()], vec![broken()], vec![Ok(line("a"))]].into());
        let (tx, rx) = watch::channel(());
        let mut lines = pin!(FollowLines::new(source).follow(rx));
        assert!(matches!(lines.next().await, Some(Err(_))));
        assert_eq!(lines.next().await.unwrap().unwrap(), line("a"));

        drop(tx);
        assert!(lines.next().await.is_none());
    }

    #[rstest]
    #[tokio::test]
    async fn a_failed_read_after_the_sender_drops_ends_the_stream_quietly() {
        let source = Scripted([vec![Ok(line("a"))], vec![broken()]].into());
        let (tx, rx) = watch::channel(());
        let mut lines = pin!(FollowLines::new(source).follow(rx));
        assert_eq!(lines.next().await.unwrap().unwrap(), line("a"));

        // A removed file's last event bumps the version just before its watcher drops the sender.
        tx.send_replace(());
        drop(tx);
        assert!(lines.next().await.is_none());
    }
}
