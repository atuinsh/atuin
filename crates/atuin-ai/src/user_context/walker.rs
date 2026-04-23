//! Filesystem traversal for `TERMINAL.md` context files.
//!
//! Walks from the starting directory up to the filesystem root, checking for
//! `.atuin/TERMINAL.md` and `TERMINAL.md` at each level. Then checks the global
//! config directory. Returns files ordered from shallowest (global/root) to
//! deepest (most project-specific), so that context layers naturally from
//! general to specific.

use std::path::{Path, PathBuf};

use eyre::Result;
use tokio::task::JoinSet;

const CONTEXT_FILENAME: &str = "TERMINAL.md";

/// A context file found on disk, before interpolation.
#[derive(Debug)]
pub(crate) struct RawContextFile {
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
pub(crate) async fn walk(start: &Path, global_path: Option<&Path>) -> Result<Vec<RawContextFile>> {
    let dirs: Vec<PathBuf> = start.ancestors().map(PathBuf::from).collect();
    let dir_count = dirs.len();

    let mut set: JoinSet<Result<Option<FoundFile>>> = JoinSet::new();

    for (index, dir) in dirs.into_iter().enumerate() {
        let dir2 = dir.clone();
        set.spawn(async move {
            load_context_file(&dir.join(".atuin").join(CONTEXT_FILENAME), index).await
        });
        set.spawn(async move { load_context_file(&dir2.join(CONTEXT_FILENAME), index).await });
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

/// The default global context file path (`~/.config/atuin/TERMINAL.md`).
pub(crate) fn global_context_path() -> PathBuf {
    atuin_common::utils::config_dir().join(CONTEXT_FILENAME)
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
