//! Streaming values from a JSONL file, following appends.

use std::path::PathBuf;

use futures::{Stream, StreamExt};
use serde::de::DeserializeOwned;

use super::JsonlError;
use crate::fs::tail::{Positioned, Tail};

fn blank(bytes: &[u8]) -> bool {
    bytes.iter().all(u8::is_ascii_whitespace)
}

/// Follow a JSONL file, deserializing each non-blank line into `T`.
pub fn from_tail<T>(tail: Tail) -> impl Stream<Item = Result<T, JsonlError>> + Send
where
    T: DeserializeOwned + Send + 'static,
{
    let lines = tail.lines();
    async_stream::stream! {
        futures::pin_mut!(lines);
        let mut line_no: u64 = 0;
        while let Some(line) = lines.next().await {
            line_no += 1;
            match line {
                Ok(bytes) if blank(&bytes) => {}
                Ok(bytes) => yield serde_json::from_slice::<T>(&bytes)
                    .map_err(|source| JsonlError::Parse { source, line: line_no }),
                Err(e) => yield Err(JsonlError::Io(e)),
            }
        }
    }
}

/// [`from_tail`] with default options for the file at `path` (follows by default, so it does not
/// end on a static file; build a [`Tail`] with `Read::Once` for a one-shot read).
pub fn from_path<T>(path: impl Into<PathBuf>) -> impl Stream<Item = Result<T, JsonlError>> + Send
where
    T: DeserializeOwned + Send + 'static,
{
    from_tail(Tail::builder().path(path).build())
}

/// Like [`from_tail`], pairing each value with the byte offset just past its line.
pub fn from_tail_positioned<T>(
    tail: Tail,
) -> impl Stream<Item = Positioned<Result<T, JsonlError>>> + Send
where
    T: DeserializeOwned + Send + 'static,
{
    let lines = tail.lines_positioned();
    async_stream::stream! {
        futures::pin_mut!(lines);
        let mut line_no: u64 = 0;
        while let Some(Positioned { offset, value }) = lines.next().await {
            line_no += 1;
            match value {
                Ok(bytes) if blank(&bytes) => {}
                Ok(bytes) => yield Positioned {
                    offset,
                    value: serde_json::from_slice::<T>(&bytes)
                        .map_err(|source| JsonlError::Parse { source, line: line_no }),
                },
                Err(e) => yield Positioned { offset, value: Err(JsonlError::Io(e)) },
            }
        }
    }
}

/// [`from_tail_positioned`] with default options for the file at `path`.
pub fn from_path_positioned<T>(
    path: impl Into<PathBuf>,
) -> impl Stream<Item = Positioned<Result<T, JsonlError>>> + Send
where
    T: DeserializeOwned + Send + 'static,
{
    from_tail_positioned(Tail::builder().path(path).build())
}

#[cfg(test)]
mod tests {
    use futures::TryStreamExt;
    use proptest::prelude::*;
    use rstest::rstest;
    use serde::{Deserialize, Serialize};

    use super::*;
    use crate::fs::tail::{Anchor, Read};

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

    fn bounded<T>(path: &std::path::Path) -> impl Stream<Item = Result<T, JsonlError>>
    where
        T: DeserializeOwned + Send + 'static,
    {
        from_tail(Tail::builder().path(path).read(Read::Once(Anchor::Beginning)).build())
    }

    #[rstest]
    #[tokio::test]
    async fn parses_each_line_into_a_value() {
        let (_dir, path) = write_jsonl(&["1", "2", "3", ""]);
        let got: Vec<i64> = bounded::<i64>(&path).try_collect().await.unwrap();
        assert_eq!(got, vec![1, 2, 3]);
    }

    #[rstest]
    #[tokio::test]
    async fn skips_blank_and_whitespace_only_lines() {
        let (_dir, path) = write_jsonl(&["1", "", "   ", "2", ""]);
        let got: Vec<i64> = bounded::<i64>(&path).try_collect().await.unwrap();
        assert_eq!(got, vec![1, 2]);
    }

    #[rstest]
    #[tokio::test]
    async fn a_malformed_line_errors_then_recovery_continues() {
        let (_dir, path) = write_jsonl(&["1", "not-json", "2", ""]);
        let results: Vec<Result<i64, JsonlError>> = bounded::<i64>(&path).collect().await;
        assert_eq!(results.len(), 3);
        assert_eq!(results[0].as_ref().unwrap(), &1);
        assert!(matches!(results[1], Err(JsonlError::Parse { line: 2, .. })));
        assert_eq!(results[2].as_ref().unwrap(), &2);
    }

    #[rstest]
    #[tokio::test]
    async fn positioned_offsets_allow_resuming_from_a_checkpoint() {
        let (_dir, path) = write_jsonl(&["1", "2", "3", ""]);
        let tail = Tail::builder().path(&path).read(Read::Once(Anchor::Beginning)).build();
        let positioned: Vec<(u64, i64)> =
            from_tail_positioned::<i64>(tail).map(|p| (p.offset, p.value.unwrap())).collect().await;
        assert_eq!(positioned, vec![(2, 1), (4, 2), (6, 3)]);

        let checkpoint = positioned[0].0;
        let resumed: Vec<i64> = from_tail::<i64>(
            Tail::builder().path(&path).read(Read::Once(Anchor::Offset(checkpoint))).build(),
        )
        .map(Result::unwrap)
        .collect()
        .await;
        assert_eq!(resumed, vec![2, 3]);
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
                runtime.block_on(async { bounded::<Rec>(&path).try_collect().await.unwrap() });
            prop_assert_eq!(got, recs);
        }
    }
}
