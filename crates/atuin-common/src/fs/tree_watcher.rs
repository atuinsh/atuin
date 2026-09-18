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

fn scan_fs(root: &Path, recursive: bool) -> std::io::Result<Vec<(PathBuf, FileKind)>> {
    let mut out = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(&dir)? {
            let entry = entry?;
            let file_type = entry.file_type()?;
            let path = entry.path();
            if recursive && file_type.is_dir() {
                stack.push(path.clone());
            }
            out.push((path, FileKind::from(file_type)));
        }
    }
    Ok(out)
}

async fn scan(root: &Path, recursive: bool) -> Option<Vec<(PathBuf, FileKind)>> {
    let root = root.to_path_buf();
    match tokio::task::spawn_blocking(move || scan_fs(&root, recursive)).await {
        Ok(Ok(entries)) => Some(entries),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use rstest::rstest;

    use super::*;

    fn kinds(
        entries: &[(PathBuf, FileKind)],
        root: &Path,
    ) -> std::collections::BTreeMap<String, FileKind> {
        entries
            .iter()
            .map(|(p, k)| (p.strip_prefix(root).unwrap().to_string_lossy().into_owned(), *k))
            .collect()
    }

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

    #[test]
    fn scan_fs_lists_direct_children_only_when_not_recursive() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a"), b"x").unwrap();
        std::fs::create_dir(dir.path().join("sub")).unwrap();
        std::fs::write(dir.path().join("sub/b"), b"x").unwrap();

        let found = scan_fs(dir.path(), false).unwrap();
        let map = kinds(&found, dir.path());
        assert_eq!(map.get("a"), Some(&FileKind::File));
        assert_eq!(map.get("sub"), Some(&FileKind::Dir));
        assert!(!map.contains_key("sub/b"));
    }

    #[test]
    fn scan_fs_descends_when_recursive() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("sub")).unwrap();
        std::fs::write(dir.path().join("sub/b"), b"x").unwrap();

        let found = scan_fs(dir.path(), true).unwrap();
        let map = kinds(&found, dir.path());
        assert_eq!(map.get("sub/b"), Some(&FileKind::File));
    }

    #[cfg(unix)]
    #[test]
    fn scan_fs_does_not_follow_symlinked_dirs() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("real")).unwrap();
        std::fs::write(dir.path().join("real/inner"), b"x").unwrap();
        std::os::unix::fs::symlink(dir.path().join("real"), dir.path().join("link")).unwrap();

        let found = scan_fs(dir.path(), true).unwrap();
        let map = kinds(&found, dir.path());
        assert_eq!(map.get("link"), Some(&FileKind::Symlink));
        assert!(!map.contains_key("link/inner"));
        assert_eq!(map.get("real/inner"), Some(&FileKind::File));
    }

    #[tokio::test]
    async fn scan_returns_none_for_missing_root() {
        let missing = PathBuf::from("/this/does/not/exist/anywhere");
        assert!(scan(&missing, true).await.is_none());
    }
}
