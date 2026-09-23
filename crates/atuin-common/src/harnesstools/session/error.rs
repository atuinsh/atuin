use std::path::PathBuf;

use crate::fs::tree_watcher::TreeWatcherError;
use crate::harnesstools::session::model::SessionId;
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
    #[error("observing {}: {source}", db.display())]
    Observe {
        db: PathBuf,
        #[source]
        source: crate::db::sqlite::observe::ObserveError,
    },
}

#[derive(Debug, thiserror::Error)]
pub enum MessageError {
    #[error(transparent)]
    Jsonl(#[from] JsonlError),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
    #[error("unrecognized record on line {line}")]
    Unrecognized {
        line: u64,
    },
}

#[derive(Debug, thiserror::Error)]
pub enum CaptureError {
    #[error(transparent)]
    Watch(#[from] WatchError),
    #[error("message error in session {session}: {source}")]
    Message {
        session: SessionId,
        #[source]
        source: MessageError,
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
        let io = std::io::Error::other("boom");
        let err: MessageError = crate::json::jsonl::JsonlError::Io(io).into();
        assert!(matches!(err, MessageError::Jsonl(_)));
    }
}
