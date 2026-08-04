//! Git-authoritative, cached resolution of the work-tree root that scopes
//! [`FilterMode::Workspace`](crate::settings::FilterMode::Workspace).
//!
//! Asking `git` (rather than scanning for a `.git` inode) is what lets this see
//! a linked worktree's *own* root, bare repositories, and
//! `GIT_DIR`/`GIT_WORK_TREE`/`core.worktree` setups -- none of which leave a
//! `.git` entry a filesystem walk could find.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use atuin_common::git::{Git, GitError};

/// Resolves and caches the git work-tree root for a cwd.
///
/// Held by [`AppCtx`](super::AppCtx); reach it with `ctx::app().workspace()`.
/// The cache lives in this struct rather than a module static, so the state is
/// owned in one place. Results are memoized by cwd (stable within a process).
pub struct WorkspaceCtx {
    cache: Mutex<HashMap<String, Option<PathBuf>>>,
}

impl WorkspaceCtx {
    pub(crate) fn new() -> Self {
        Self {
            cache: Mutex::new(HashMap::new()),
        }
    }

    /// The work-tree root a Workspace filter should scope `cwd` to. `None` when
    /// `cwd` is not in a repository (callers fall back to `cwd`). Runs the
    /// `git` invocation off the async executor.
    pub async fn git_root(&self, cwd: &str) -> Option<PathBuf> {
        if let Some(hit) = self.cached(cwd) {
            return hit;
        }

        let owned = cwd.to_owned();
        let root = tokio::task::spawn_blocking(move || resolve(Path::new(&owned)))
            .await
            .unwrap_or(None);
        self.store(cwd, root)
    }

    /// Blocking [`Self::git_root`], for synchronous callers such as
    /// [`Context::from_history`](crate::database::Context::from_history).
    /// Shares the cache, so it is a no-op once `cwd` has been resolved.
    pub fn git_root_blocking(&self, cwd: &str) -> Option<PathBuf> {
        if let Some(hit) = self.cached(cwd) {
            return hit;
        }
        let root = resolve(Path::new(cwd));
        self.store(cwd, root)
    }

    fn cached(&self, cwd: &str) -> Option<Option<PathBuf>> {
        self.cache.lock().unwrap().get(cwd).cloned()
    }

    fn store(&self, cwd: &str, root: Option<PathBuf>) -> Option<PathBuf> {
        self.cache
            .lock()
            .unwrap()
            .insert(cwd.to_owned(), root.clone());
        root
    }
}

/// Ask `git` for the work-tree root of `cwd` (blocking): the current work
/// tree's `--show-toplevel`, or the git dir itself for a bare repo (which has
/// no work tree). `None` when `cwd` is not a repository, or `git` cannot run.
fn resolve(cwd: &Path) -> Option<PathBuf> {
    let git = Git::new(Path::new("git"));
    let repo = git.repo(cwd).ok()?;
    match repo.toplevel() {
        Ok(root) => Some(root),
        // A bare repo has no work tree; scope to the git dir instead. Fall
        // through only on that specific failure, never on a spawn error.
        Err(GitError::Failed { .. }) => repo.git_dir().ok(),
        Err(_) => None,
    }
}

#[cfg(all(test, not(windows)))]
mod tests {
    use super::*;
    use rstest::{fixture, rstest};
    use std::process::Command;
    use tempfile::TempDir;

    fn git(cwd: &Path, args: &[&str]) {
        let ok = Command::new("git")
            .arg("-C")
            .arg(cwd)
            .args(args)
            .output()
            .unwrap()
            .status
            .success();
        assert!(ok, "git {args:?} failed");
    }

    #[fixture]
    fn repo() -> TempDir {
        let dir = tempfile::tempdir().unwrap();
        git(dir.path(), &["init", "-q"]);
        dir
    }

    #[rstest]
    fn resolves_the_work_tree_toplevel_from_a_subdir(repo: TempDir) {
        let subdir = repo.path().join("src").join("deep");
        std::fs::create_dir_all(&subdir).unwrap();

        // canonicalize: git resolves the /var -> /private/var symlink on macOS.
        assert_eq!(
            std::fs::canonicalize(resolve(&subdir).unwrap()).unwrap(),
            std::fs::canonicalize(repo.path()).unwrap()
        );
    }

    #[rstest]
    fn bare_repo_resolves_to_the_git_dir(repo: TempDir) {
        let bare = repo.path().join("bare.git");
        std::fs::create_dir_all(&bare).unwrap();
        git(&bare, &["init", "-q", "--bare"]);

        assert_eq!(
            std::fs::canonicalize(resolve(&bare).unwrap()).unwrap(),
            std::fs::canonicalize(&bare).unwrap()
        );
    }

    #[rstest]
    fn outside_a_repository_resolves_to_none() {
        let dir = tempfile::tempdir().unwrap();
        assert!(resolve(dir.path()).is_none());
    }
}
