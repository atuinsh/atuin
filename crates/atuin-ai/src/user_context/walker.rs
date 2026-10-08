//! Filesystem traversal for `TERMINAL.md` context files.
//!
//! Walks from the starting directory up to the filesystem root, checking for
//! `.atuin/TERMINAL.md` and `TERMINAL.md` at each level. Then checks the global
//! config directory. Returns files ordered from shallowest (global/root) to
//! deepest (most project-specific), so that context layers naturally from
//! general to specific.

use std::path::{Path, PathBuf};

use atuin_common::path::PathExt as _;
use eyre::Result;
use tokio::task::JoinSet;

const CONTEXT_FILENAME: &str = "TERMINAL.md";

/// A context file found on disk, before interpolation.
#[derive(Debug)]
pub struct RawContextFile {
    pub path: PathBuf,
    pub content: String,
}

struct FoundFile {
    depth: usize,
    file: RawContextFile,
}

/// Walk from `start` up to the filesystem root collecting `TERMINAL.md`
/// context files, then check the global path. Returns files shallowest-first.
///
/// At each ancestor directory, checks two locations:
/// - `.atuin/TERMINAL.md` (dotdir-scoped)
/// - `TERMINAL.md` (project root)
pub async fn walk(start: &Path, global_path: Option<&Path>) -> Result<Vec<RawContextFile>> {
    let dirs: Vec<PathBuf> = start.ancestors().map(PathBuf::from).collect();
    let dir_count = dirs.len();

    let mut set: JoinSet<Result<Option<FoundFile>>> = JoinSet::new();

    for (index, dir) in dirs.into_iter().enumerate() {
        let dotdir_file =
            PathBuf::from_iter([dir.as_path(), ".atuin".as_ref(), CONTEXT_FILENAME.as_ref()]);
        // Avoid loading ~/.atuin/TERMINAL.md twice.
        if !global_path.is_some_and(|global| dotdir_file.is_same_path(global)) {
            set.spawn(async move { load_context_file(&dotdir_file, index).await });
        }
        set.spawn(async move { load_context_file(&dir.join(CONTEXT_FILENAME), index).await });
    }

    if let Some(global) = global_path {
        let global = global.to_path_buf();
        let depth = dir_count;
        set.spawn(async move { load_context_file(&global, depth).await });
    }

    let mut found = Vec::new();
    while let Some(result) = set.join_next().await {
        match result? {
            Ok(Some(f)) => found.push(f),
            Ok(None) => {}
            Err(e) => {
                tracing::warn!("Error reading context file, skipping: {e}");
            }
        }
    }

    // Sort shallowest-first (highest depth index = shallowest ancestor).
    // The global file has the highest depth index so it sorts last... but we
    // actually want global first, then root → cwd. Reverse the depth ordering.
    found.sort_by_key(|b| std::cmp::Reverse(b.depth));

    Ok(found.into_iter().map(|f| f.file).collect())
}

/// The default global context file path (`~/.atuin/TERMINAL.md`).
pub fn global_context_path() -> PathBuf {
    atuin_common::dirs::config_path(CONTEXT_FILENAME)
}

async fn load_context_file(path: &Path, depth: usize) -> Result<Option<FoundFile>> {
    match tokio::fs::read_to_string(path).await {
        Ok(content) => Ok(Some(FoundFile {
            depth,
            file: RawContextFile {
                path: path.to_path_buf(),
                content,
            },
        })),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e.into()),
    }
}

#[cfg(test)]
mod tests {
    use rstest::rstest;

    use super::*;

    /// A `.atuin/TERMINAL.md` above the working directory that is also the global file, as
    /// `~/.atuin/TERMINAL.md` is by default, is loaded once. Any other is a project's, and is still
    /// loaded alongside the global one.
    #[rstest]
    #[case::global(true, 1)]
    #[case::project(false, 2)]
    #[tokio::test]
    async fn the_global_file_is_not_also_a_project_file(
        #[case] is_global: bool,
        #[case] expected: usize,
    ) {
        let home = tempfile::tempdir().unwrap();
        let dotfile = home.path().join(".atuin").join(CONTEXT_FILENAME);
        let elsewhere = home.path().join("elsewhere").join(CONTEXT_FILENAME);
        for file in [&dotfile, &elsewhere] {
            std::fs::create_dir_all(file.parent().unwrap()).unwrap();
            std::fs::write(file, "context").unwrap();
        }

        let global = if is_global {
            dotfile
        } else {
            elsewhere
        };
        let files = walk(home.path(), Some(&global)).await.unwrap();
        assert_eq!(files.len(), expected);
    }
}
