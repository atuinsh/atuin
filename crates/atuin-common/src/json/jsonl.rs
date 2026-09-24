//! Newline-delimited JSON (JSONL): one value per line, blank lines skipped.
//!
//! The lines come from [`crate::io`], such as a [`FollowLines`](crate::io::FollowLines) stream,
//! decoded with [`JsonlExt::json`].

use std::io;

use futures::{Stream, StreamExt};
use serde::de::DeserializeOwned;

use crate::io::Line;

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
    /// Deserialize each non-blank line as a `T`, yielded with the line it came from.
    fn json<T: DeserializeOwned>(self) -> impl Stream<Item = Result<(Line, T), JsonlError>> {
        self.json_with(|bytes| serde_json::from_slice(bytes))
    }

    /// [`Self::json`] for files a JavaScript program wrote, read as `JSON.parse` reads them
    /// (see [`crate::json::js`]).
    fn js_json<T: DeserializeOwned>(self) -> impl Stream<Item = Result<(Line, T), JsonlError>> {
        self.json_with(|bytes| crate::json::js::from_slice(bytes))
    }

    /// Each non-blank line decoded by `parse`, yielded with the line it came from.
    fn json_with<T>(
        self,
        parse: impl Fn(&[u8]) -> serde_json::Result<T>,
    ) -> impl Stream<Item = Result<(Line, T), JsonlError>> {
        self.filter_map(move |line| {
            let item = match line {
                Ok(line) if line.bytes.trim_ascii().is_empty() => None,
                Ok(line) => Some(match parse(&line.bytes) {
                    Ok(value) => Ok((line, value)),
                    Err(source) => Err(JsonlError::Parse {
                        source,
                        end: line.end,
                    }),
                }),
                Err(err) => Some(Err(JsonlError::Io(err))),
            };
            std::future::ready(item)
        })
    }
}

impl<S: Stream<Item = io::Result<Line>>> JsonlExt for S {}

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
    use crate::sync::BlockingPool;

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
                Ok((line, value)) => Item::Value(line.end, value),
                Err(JsonlError::Parse { end, .. }) => Item::Malformed(end),
                Err(JsonlError::Io(_)) => Item::Unreadable,
            })
            .collect()
            .await;
        assert_eq!(got, expected);
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
                        .map_ok(|(line, rec)| (line.end, rec))
                        .try_collect(),
                )
                .unwrap();
            prop_assert_eq!(got, expected);
        }
    }
}
