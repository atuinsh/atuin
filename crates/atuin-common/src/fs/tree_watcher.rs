use std::path::{Path, PathBuf};

#[derive(Debug, thiserror::Error)]
pub enum TreeWatcherError {
    #[error("watch root is not a directory: {0}")]
    NotADirectory(PathBuf),
    #[error(transparent)]
    Notify(#[from] notify::Error),
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileKind {
    File,
    Dir,
    Symlink,
    Other,
}

impl From<std::fs::FileType> for FileKind {
    fn from(value: std::fs::FileType) -> Self {
        if value.is_file() {
            Self::File
        } else if value.is_dir() {
            Self::Dir
        } else if value.is_symlink() {
            Self::Symlink
        } else {
            Self::Other
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Origin {
    Notify,
    Scan,
}

#[derive(Debug, Clone)]
pub struct NodeContext {
    path: PathBuf,
    kind: FileKind,
    origin: Origin,
}

impl NodeContext {
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    #[must_use]
    pub fn kind(&self) -> FileKind {
        self.kind
    }

    #[must_use]
    pub fn origin(&self) -> Origin {
        self.origin
    }

    #[must_use]
    pub fn is_file(&self) -> bool {
        matches!(self.kind, FileKind::File)
    }

    #[must_use]
    pub fn is_dir(&self) -> bool {
        matches!(self.kind, FileKind::Dir)
    }

    #[must_use]
    pub fn is_symlink(&self) -> bool {
        matches!(self.kind, FileKind::Symlink)
    }

    #[must_use]
    pub fn into_path(self) -> PathBuf {
        self.path
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rstest::rstest;

    #[test]
    fn not_a_directory_displays_path() {
        let err = TreeWatcherError::NotADirectory(PathBuf::from("/nope"));
        assert_eq!(err.to_string(), "watch root is not a directory: /nope");
    }

    #[test]
    fn file_kind_from_file_type() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("f");
        std::fs::write(&file, b"x").unwrap();
        let kind = FileKind::from(std::fs::symlink_metadata(&file).unwrap().file_type());
        assert_eq!(kind, FileKind::File);

        let kind = FileKind::from(std::fs::symlink_metadata(dir.path()).unwrap().file_type());
        assert_eq!(kind, FileKind::Dir);
    }

    #[rstest]
    #[case(FileKind::File, true, false, false)]
    #[case(FileKind::Dir, false, true, false)]
    #[case(FileKind::Symlink, false, false, true)]
    #[case(FileKind::Other, false, false, false)]
    fn node_context_kind_predicates(
        #[case] kind: FileKind,
        #[case] is_file: bool,
        #[case] is_dir: bool,
        #[case] is_symlink: bool,
    ) {
        let ctx = NodeContext {
            path: PathBuf::from("/root/x"),
            kind,
            origin: Origin::Scan,
        };
        assert_eq!(ctx.is_file(), is_file);
        assert_eq!(ctx.is_dir(), is_dir);
        assert_eq!(ctx.is_symlink(), is_symlink);
        assert_eq!(ctx.path(), Path::new("/root/x"));
        assert_eq!(ctx.origin(), Origin::Scan);
    }
}
