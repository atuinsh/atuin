//! Newline-delimited JSON (JSONL) streaming.

pub mod tail;

use std::io;

/// An error encountered while tailing a JSONL stream.
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
