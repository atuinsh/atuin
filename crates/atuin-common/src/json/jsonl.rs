//! Newline-delimited JSON (JSONL): one value per line, blank lines skipped.
//!
//! The lines come from [`crate::io`]: a [`FollowLines`](crate::io::FollowLines) stream decoded
//! with [`JsonlExt::json`], or a single line looked up by [`value_at`].

use std::fs::File;
use std::io;
use std::path::Path;

use futures::{Stream, StreamExt};
use serde::de::DeserializeOwned;

use crate::io::Line;
use crate::sync::BlockingPool;

/// An error encountered while reading a JSONL file.
#[derive(Debug, thiserror::Error)]
pub enum JsonlError {
    #[error(transparent)]
    Io(#[from] io::Error),
    #[error("failed to parse the JSON line ending at byte {end}: {source}")]
    Parse {
        source: serde_json::Error,
        /// The [`Line::end`] of the line that failed to parse.
        end: u64,
    },
}

/// Deserializing a stream of lines as JSONL.
pub trait JsonlExt: Stream<Item = io::Result<Line>> + Sized {
    /// Deserialize each non-blank line as a `T`, yielded with the [`Line::end`] of its line.
    fn json<T: DeserializeOwned>(self) -> impl Stream<Item = Result<(u64, T), JsonlError>> {
        self.filter_map(|line| {
            let item = match line {
                Ok(line) if line.bytes.trim_ascii().is_empty() => None,
                Ok(Line { end, bytes }) => Some(
                    serde_json::from_slice(&bytes)
                        .map(|value| (end, value))
                        .map_err(|source| JsonlError::Parse { source, end }),
                ),
                Err(err) => Some(Err(JsonlError::Io(err))),
            };
            std::future::ready(item)
        })
    }
}

impl<S: Stream<Item = io::Result<Line>>> JsonlExt for S {}

/// The value on the line of the file at `path` that ends at byte `at`, if a line ends there.
///
/// The counterpart of the offsets [`JsonlExt::json`] yields: a reader hands one back to ask what
/// it named, and decides from that whether it may resume there. Reads run in `pool`.
pub async fn value_at<T: DeserializeOwned>(
    path: &Path,
    at: u64,
    pool: &BlockingPool,
) -> Result<Option<T>, JsonlError> {
    let path = path.to_path_buf();
    let line = pool
        .run(move || File::open(path).and_then(|file| Line::ending_at(&file, at)))
        .await
        .map_err(io::Error::other)??;
    line.map(|Line { end, bytes }| {
        serde_json::from_slice(&bytes).map_err(|source| JsonlError::Parse { source, end })
    })
    .transpose()
}

#[cfg(test)]
mod tests {
    use std::io::Write;
    use std::num::NonZeroUsize;

    use bytes::Bytes;
    use futures::{TryStreamExt, stream};
    use proptest::prelude::*;
    use rstest::rstest;
    use serde::{Deserialize, Serialize};
    use tempfile::NamedTempFile;

    use super::*;
    use crate::io::{FollowLines, PathLineReader, PooledReadLines};

    #[derive(Debug, PartialEq, Eq)]
    enum Item {
        Value(u64, i64),
        Malformed(u64),
        Unreadable,
    }

    #[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
    struct Rec {
        n: i64,
        s: String,
    }

    fn file(contents: &[u8]) -> NamedTempFile {
        let mut file = NamedTempFile::new().unwrap();
        file.write_all(contents).unwrap();
        file
    }

    fn pool() -> BlockingPool {
        BlockingPool::new(NonZeroUsize::MIN)
    }

    fn line(end: u64, text: &'static str) -> Line {
        Line {
            end,
            bytes: Bytes::from_static(text.as_bytes()),
        }
    }

    #[rstest]
    #[case::values(
        vec![Ok(line(2, "1")), Ok(line(5, "22"))],
        vec![Item::Value(2, 1), Item::Value(5, 22)],
    )]
    #[case::blank_lines_are_skipped(
        vec![Ok(line(1, "")), Ok(line(4, " \t")), Ok(line(6, "1"))],
        vec![Item::Value(6, 1)],
    )]
    #[case::a_malformed_line_is_not_the_end(
        vec![Ok(line(2, "x")), Ok(line(4, "1"))],
        vec![Item::Malformed(2), Item::Value(4, 1)],
    )]
    #[case::a_read_error_is_not_the_end(
        vec![Err(io::Error::other("broken")), Ok(line(2, "1"))],
        vec![Item::Unreadable, Item::Value(2, 1)],
    )]
    #[tokio::test]
    async fn deserializes_each_non_blank_line(
        #[case] lines: Vec<io::Result<Line>>,
        #[case] expected: Vec<Item>,
    ) {
        let got: Vec<Item> = stream::iter(lines)
            .json::<i64>()
            .map(|item| match item {
                Ok((end, value)) => Item::Value(end, value),
                Err(JsonlError::Parse { end, .. }) => Item::Malformed(end),
                Err(JsonlError::Io(_)) => Item::Unreadable,
            })
            .collect()
            .await;
        assert_eq!(got, expected);
    }

    /// "1\n22\nx\n" has lines ending at 2, 5 and 7.
    #[rstest]
    #[case::a_value(5, Some(22))]
    #[case::no_line_ends_there(4, None)]
    #[tokio::test]
    async fn reads_the_value_on_the_line_ending_at_an_offset(
        #[case] at: u64,
        #[case] expected: Option<i64>,
    ) {
        let file = file(b"1\n22\nx\n");
        assert_eq!(value_at::<i64>(file.path(), at, &pool()).await.unwrap(), expected);
    }

    #[rstest]
    #[tokio::test]
    async fn a_malformed_line_at_an_offset_is_a_parse_error() {
        let file = file(b"1\n22\nx\n");
        assert!(matches!(
            value_at::<i64>(file.path(), 7, &pool()).await,
            Err(JsonlError::Parse { end: 7, .. })
        ));
    }

    proptest! {
        // Records written a line each, blank lines between some, come back in order, each with
        // the offset just past its own line.
        #[test]
        fn records_come_back_with_their_ends(
            recs in prop::collection::vec((any::<i64>(), "[a-z ]{0,12}", any::<bool>()), 0..20),
        ) {
            let mut body = Vec::new();
            let mut expected = Vec::new();
            for (n, s, blank_before) in recs {
                if blank_before {
                    body.extend_from_slice(b" \n");
                }
                let rec = Rec { n, s };
                serde_json::to_writer(&mut body, &rec).unwrap();
                body.push(b'\n');
                expected.push((body.len() as u64, rec));
            }
            let file = file(&body);

            let runtime =
                tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
            let got: Vec<(u64, Rec)> = runtime
                .block_on(
                    FollowLines::new(PooledReadLines::new(PathLineReader::new(file.path()), pool()))
                        .read_to_end()
                        .json()
                        .try_collect(),
                )
                .unwrap();
            prop_assert_eq!(got, expected);
        }
    }
}
