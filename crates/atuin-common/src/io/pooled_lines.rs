//! Line reads moved off the async runtime.
//!
//! Reads run in a [`BlockingPool`] one bounded batch at a time, so a large file never holds a
//! worker for long and a reader needs no more memory than a batch or its longest line.

use std::io;
use std::sync::Arc;

use futures::Stream;
use parking_lot::Mutex;

use crate::io::{Line, ReadLines, ReadLinesError};
use crate::sync::BlockingPool;

/// Bytes of lines pulled per trip to the pool.
const BATCH_BYTES: u64 = 64 * 1024;

/// Reads the complete lines appended to a file without blocking the async runtime.
pub trait AsyncReadLines {
    /// The complete lines past the last one handed out, read as the stream is pulled.
    ///
    /// The stream ends at the end of the file or after its first error; a truncated or replaced
    /// file is read on from its start. Lines read but not yet yielded are dropped with the stream
    /// and not read again.
    fn lines(&mut self) -> impl Stream<Item = io::Result<Line>> + Send + '_;
}

/// Reads a [`ReadLines`] in a [`BlockingPool`].
#[derive(Debug)]
pub struct PooledLines<R> {
    /// Shared with the trip in flight, which runs on after a dropped stream stops waiting for it.
    reader: Arc<Mutex<R>>,
    pool: BlockingPool,
}

impl<R> PooledLines<R> {
    #[must_use]
    pub fn new(reader: R, pool: BlockingPool) -> Self {
        Self {
            reader: Arc::new(Mutex::new(reader)),
            pool,
        }
    }
}

impl<R: ReadLines + Send + 'static> AsyncReadLines for PooledLines<R> {
    fn lines(&mut self) -> impl Stream<Item = io::Result<Line>> + Send + '_ {
        async_stream::stream! {
            loop {
                let reader = Arc::clone(&self.reader);
                let batch = match self.pool.run(move || Batch::read(&mut *reader.lock())).await {
                    Ok(batch) => batch,
                    Err(cancelled) => {
                        yield Err(io::Error::other(cancelled));
                        break;
                    }
                };
                for line in batch.lines {
                    yield Ok(line);
                }
                match batch.stop {
                    Stop::Drained => break,
                    Stop::Full => {}
                    Stop::Failed(err) => {
                        yield Err(err);
                        break;
                    }
                }
            }
        }
    }
}

/// The lines of one trip to the pool, and why it stopped there.
#[derive(Debug)]
struct Batch {
    lines: Vec<Line>,
    stop: Stop,
}

#[derive(Debug)]
enum Stop {
    /// The reader has no complete line left.
    Drained,
    /// The lines reached [`BATCH_BYTES`]; the reader may have more right away.
    Full,
    /// A read failed after the lines in the batch.
    Failed(io::Error),
}

impl Batch {
    /// Pull lines from `reader` until they reach [`BATCH_BYTES`], the file ends, or a read fails;
    /// a truncated or replaced file is read on from its start.
    fn read(reader: &mut impl ReadLines) -> Self {
        // A reader reports each truncation or replacement once, so this goes round again only
        // for a file replaced again between two opens.
        let lines = loop {
            match reader.lines() {
                Ok(lines) => break lines,
                Err(error @ (ReadLinesError::Truncated | ReadLinesError::Replaced)) => {
                    tracing::debug!(%error, "reading from the start");
                }
                Err(ReadLinesError::Io(err)) => {
                    return Self {
                        lines: Vec::new(),
                        stop: Stop::Failed(err),
                    };
                }
            }
        };
        // A loop rather than `collect::<Result<_, _>>()`: lines pulled before an error are
        // already consumed and would be lost with it.
        let mut batch = Vec::new();
        let mut bytes = 0;
        for line in lines {
            let line = match line {
                Ok(line) => line,
                Err(err) => {
                    return Self {
                        lines: batch,
                        stop: Stop::Failed(err),
                    };
                }
            };
            bytes += u64::try_from(line.bytes.len()).expect("a line length fits u64") + 1;
            batch.push(line);
            if bytes >= BATCH_BYTES {
                return Self {
                    lines: batch,
                    stop: Stop::Full,
                };
            }
        }
        Self {
            lines: batch,
            stop: Stop::Drained,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;
    use std::io::Write;
    use std::num::NonZeroUsize;

    use bytes::Bytes;
    use futures::{StreamExt, TryStreamExt};
    use rstest::rstest;
    use tempfile::NamedTempFile;

    use super::*;
    use crate::io::PathLineReader;

    /// A reader handing out one scripted read per call, then nothing.
    #[derive(Debug)]
    struct Scripted(VecDeque<Result<Vec<io::Result<Line>>, ReadLinesError>>);

    impl ReadLines for Scripted {
        fn lines(&mut self) -> Result<impl Iterator<Item = io::Result<Line>>, ReadLinesError> {
            self.0.pop_front().unwrap_or_else(|| Ok(Vec::new())).map(Vec::into_iter)
        }
    }

    fn file(contents: &[u8]) -> NamedTempFile {
        let mut file = NamedTempFile::new().unwrap();
        file.write_all(contents).unwrap();
        file
    }

    fn pool() -> BlockingPool {
        BlockingPool::new(NonZeroUsize::MIN)
    }

    /// A line as [`Scripted`] hands it out; nothing here reads its end.
    fn line(text: &'static str) -> Line {
        Line {
            end: 0,
            bytes: Bytes::from_static(text.as_bytes()),
        }
    }

    #[rstest]
    fn a_batch_stops_once_it_is_full() {
        // Three 40 KiB lines: the second takes the batch past its budget.
        let long = "x".repeat(40 * 1024);
        let file = file(format!("{long}\n{long}\n{long}\n").as_bytes());
        let mut reader = PathLineReader::new(file.path());
        let first = Batch::read(&mut reader);
        assert!(matches!(first, Batch { ref lines, stop: Stop::Full } if lines.len() == 2));
        let rest = Batch::read(&mut reader);
        assert!(matches!(rest, Batch { ref lines, stop: Stop::Drained } if lines.len() == 1));
    }

    #[rstest]
    fn a_restart_is_read_through_in_the_same_batch(
        #[values(ReadLinesError::Truncated, ReadLinesError::Replaced)] restart: ReadLinesError,
    ) {
        let mut reader = Scripted([Err(restart), Ok(vec![Ok(line("a"))])].into());
        let batch = Batch::read(&mut reader);
        assert!(matches!(batch, Batch { ref lines, stop: Stop::Drained } if *lines == [line("a")]));
    }

    #[rstest]
    #[tokio::test]
    async fn a_file_larger_than_a_batch_is_read_to_its_end() {
        let body: String = (0..20_000).map(|n| format!("{n}\n")).collect();
        let file = file(format!("{body}partial").as_bytes());
        let mut reader = PooledLines::new(PathLineReader::new(file.path()), pool());
        let lines: Vec<Line> = reader.lines().try_collect().await.unwrap();
        let texts: Vec<Bytes> = lines.into_iter().map(|line| line.bytes).collect();
        let expected: Vec<Bytes> = (0..20_000).map(|n| Bytes::from(n.to_string())).collect();
        assert_eq!(texts, expected);
    }

    #[rstest]
    #[tokio::test]
    async fn the_lines_before_a_failure_are_yielded_then_the_stream_ends() {
        let reader = Scripted([Ok(vec![Ok(line("a")), Err(io::Error::other("broken"))])].into());
        let mut reader = PooledLines::new(reader, pool());
        let got: Vec<io::Result<Line>> = reader.lines().collect().await;
        assert!(matches!(&got[..], [Ok(a), Err(_)] if *a == line("a")));
    }
}
