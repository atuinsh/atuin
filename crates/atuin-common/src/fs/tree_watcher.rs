use std::path::PathBuf;

#[derive(Debug, thiserror::Error)]
pub enum TreeWatcherError {
    #[error("watch root is not a directory: {0}")]
    NotADirectory(PathBuf),
    #[error(transparent)]
    Notify(#[from] notify::Error),
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn not_a_directory_displays_path() {
        let err = TreeWatcherError::NotADirectory(PathBuf::from("/nope"));
        assert_eq!(err.to_string(), "watch root is not a directory: /nope");
    }
}
