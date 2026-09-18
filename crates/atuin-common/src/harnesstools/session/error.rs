use std::path::PathBuf;

use crate::fs::tree_watcher::TreeWatcherError;
use crate::json::jsonl::JsonlError;

#[derive(Debug, thiserror::Error)]
pub enum RuntimeError {
    #[error("harness runtime not found at {0}")]
    NotFound(PathBuf),
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

#[derive(Debug, thiserror::Error)]
pub enum WatchError {
    #[error(transparent)]
    Tree(#[from] TreeWatcherError),
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

#[derive(Debug, thiserror::Error)]
pub enum MessageError {
    #[error(transparent)]
    Jsonl(#[from] JsonlError),
    #[error("unrecognized record on line {line}")]
    Unrecognized {
        line: u64,
    },
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use rstest::rstest;

    use super::*;

    #[rstest]
    fn runtime_not_found_names_the_path() {
        let err = RuntimeError::NotFound(PathBuf::from("/no/such/root"));
        assert!(err.to_string().contains("/no/such/root"));
    }

    #[rstest]
    fn message_error_is_from_jsonl() {
        let io = std::io::Error::new(std::io::ErrorKind::Other, "boom");
        let err: MessageError = crate::json::jsonl::JsonlError::Io(io).into();
        assert!(matches!(err, MessageError::Jsonl(_)));
    }
}
