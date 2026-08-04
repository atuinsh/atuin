//! Git integration: running the `git` CLI to answer repository-path questions
//! that filesystem inspection can't resolve reliably (linked worktrees,
//! submodules, bare repos, `GIT_DIR`/`GIT_WORK_TREE`, `core.worktree`).
//!
//! Adapted from harmont-cli (MIT-licensed, © Marko Vejnovic):
//! <https://github.com/harmont-dev/harmont-cli/blob/main/crates/hm-common/src/git.rs>
//! Trimmed to the repo-path queries atuin needs, made generic over the git
//! binary's backing path, and reworked to spawn via [`std::process::Command`]
//! directly instead of harmont's `process` helpers.

use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus};

use bstr::{BString, ByteSlice};

/// An error running or interpreting a `git` invocation.
#[derive(Debug, thiserror::Error)]
pub enum GitError {
    /// `git` could not be spawned (e.g. not installed, or not executable).
    #[error("failed to spawn `git {command}`")]
    Spawn {
        command: String,
        #[source]
        source: std::io::Error,
    },

    /// `git` ran but exited non-zero. Carries git's own stderr.
    #[error("`git {command}` failed ({status}): {stderr}")]
    Failed {
        command: String,
        status: ExitStatus,
        stderr: BString,
    },

    /// `git` emitted a path that isn't representable on this platform. Only
    /// reachable off Unix, where paths must round-trip through UTF-8.
    #[error("`git {command}` emitted a non-UTF-8 path: {path}")]
    NonUtf8Path { command: String, path: BString },
}

/// A single git work tree, as reported by `git worktree list`.
///
/// Note that `git worktree list` has additional data -- HEAD, branch and more, but those are
/// omitted here as they are unnecessary at this moment.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct WorktreeMeta {
    /// Absolute path to the work tree's root. For a `bare` entry this is the
    /// git directory, not a work tree.
    pub path: PathBuf,
    /// Whether this is the bare repository (it has no working tree, so `path`
    /// is not a directory commands are meaningfully "run in").
    pub bare: bool,
}

/// A resolved `git` executable.
#[derive(Debug, Clone, Copy)]
pub struct Git<B> {
    bin: B,
}

impl<B: AsRef<Path>> Git<B> {
    /// Wrap a `git` executable.
    #[must_use]
    pub fn new(bin: B) -> Self {
        Self { bin }
    }

    /// Bind to the git work tree at `repo`, validating it with
    /// `git rev-parse --git-dir`.
    ///
    /// # Errors
    /// A [`GitError`] if `git` cannot be run, or if `repo` is not inside a work
    /// tree (surfaced as [`GitError::Failed`] carrying git's message).
    pub fn repo<'g>(&'g self, repo: &'g Path) -> Result<GitRepo<'g, B>, GitError> {
        let bound = GitRepo { git: self, repo };
        bound.output(&["rev-parse", "--git-dir"])?;
        Ok(bound)
    }
}

/// A git work tree, bound to a [`Git`] and a repository path.
#[derive(Debug, Clone, Copy)]
pub struct GitRepo<'g, B> {
    git: &'g Git<B>,
    repo: &'g Path,
}

impl<B: AsRef<Path>> GitRepo<'_, B> {
    /// Run `git -C <repo> <args>`, returning raw stdout bytes on a zero exit.
    ///
    /// # Errors
    /// [`GitError::Spawn`] if `git` cannot be launched, or [`GitError::Failed`]
    /// (carrying stderr) on a non-zero exit.
    fn output(&self, args: &[&str]) -> Result<BString, GitError> {
        let output = Command::new(self.git.bin.as_ref())
            .arg("-C")
            .arg(self.repo)
            .args(args)
            .output()
            .map_err(|source| GitError::Spawn {
                command: args.join(" "),
                source,
            })?;

        if !output.status.success() {
            return Err(GitError::Failed {
                command: args.join(" "),
                status: output.status,
                stderr: output.stderr.into(),
            });
        }

        Ok(output.stdout.into())
    }

    /// The absolute path to the root of this work tree (`git rev-parse --show-toplevel`). For a
    /// linked worktree this is the worktree's own root, not the main repository's.
    ///
    /// # Errors
    /// A [`GitError`] if git fails (e.g. [`GitError::Failed`] in a bare repo,
    /// which has no work tree), or [`GitError::NonUtf8Path`] if the path is not
    /// representable on this platform.
    pub fn toplevel(&self) -> Result<PathBuf, GitError> {
        const CMD: &str = "rev-parse --show-toplevel";
        let stdout = self.output(&["rev-parse", "--show-toplevel"])?;
        bytes_to_path(stdout.trim(), CMD)
    }

    /// The absolute path to this repository's git directory
    /// (`git rev-parse --absolute-git-dir`). For a bare repo this is the repo
    /// itself -- a usable workspace root when [`Self::toplevel`] has none.
    ///
    /// # Errors
    /// A [`GitError`] if git fails, or [`GitError::NonUtf8Path`] if the path is
    /// not representable on this platform.
    pub fn git_dir(&self) -> Result<PathBuf, GitError> {
        const CMD: &str = "rev-parse --absolute-git-dir";
        let stdout = self.output(&["rev-parse", "--absolute-git-dir"])?;
        bytes_to_path(stdout.trim(), CMD)
    }

    /// Every work tree of this repository -- the main checkout and every linked
    /// worktree -- from `git worktree list --porcelain`.
    ///
    /// Linked worktrees commonly live in sibling directories with no shared
    /// path prefix, so this is the only reliable way to enumerate them. A bare
    /// repository yields a single [`WorktreeMeta`] with `bare` set.
    ///
    /// # Errors
    /// A [`GitError`] if git fails, or [`GitError::NonUtf8Path`] if any worktree
    /// path is not representable on this platform.
    pub fn worktrees(&self) -> Result<impl Iterator<Item = WorktreeMeta>, GitError> {
        const CMD: &str = "worktree list --porcelain";
        let stdout = self.output(&["worktree", "list", "--porcelain"])?;

        // Porcelain records are blank-line separated; each opens with a
        // `worktree <path>` line, and a bare repo's record carries a `bare`
        // line. `lines()` borrows `stdout` (a local), so materialize once into
        // an owning iterator.
        let mut worktrees = Vec::new();
        let mut path: Option<PathBuf> = None;
        let mut bare = false;
        for line in stdout.lines() {
            if let Some(p) = line.strip_prefix(b"worktree ".as_slice()) {
                path = Some(bytes_to_path(p, CMD)?);
                bare = false;
            } else if line == b"bare".as_slice() {
                bare = true;
            } else if line.is_empty()
                && let Some(path) = path.take()
            {
                worktrees.push(WorktreeMeta { path, bare });
            }
        }
        if let Some(path) = path {
            worktrees.push(WorktreeMeta { path, bare });
        }

        Ok(worktrees.into_iter())
    }
}

/// Convert git's raw path bytes into a [`PathBuf`], erroring only where the
/// platform can't represent non-UTF-8 paths (i.e. not on Unix).
fn bytes_to_path(bytes: &[u8], command: &str) -> Result<PathBuf, GitError> {
    bytes
        .to_path()
        .map(Path::to_path_buf)
        .map_err(|_| GitError::NonUtf8Path {
            command: command.to_owned(),
            path: BString::from(bytes.to_vec()),
        })
}

/// Shared scaffolding for the crate's git-backed tests. These require a real
/// `git` on PATH (guaranteed in CI); `not(windows)` mirrors the pre-existing
/// convention for path/worktree tests in this crate.
#[cfg(all(test, not(windows)))]
pub(crate) mod testutil {
    use std::path::{Path, PathBuf};
    use std::process::Command;
    use std::sync::atomic::{AtomicU32, Ordering};

    /// A throwaway directory, removed on drop even if a test panics.
    pub(crate) struct ScratchDir {
        path: PathBuf,
    }

    impl ScratchDir {
        pub(crate) fn path(&self) -> &Path {
            &self.path
        }
    }

    impl Drop for ScratchDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.path);
        }
    }

    /// Create a fresh, process-unique scratch directory under the temp dir.
    pub(crate) fn scratch(tag: &str) -> ScratchDir {
        static COUNTER: AtomicU32 = AtomicU32::new(0);
        let path = std::env::temp_dir().join(format!(
            "atuin-{tag}-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).unwrap();
        ScratchDir { path }
    }

    /// Run `git -C <cwd> <args>`, asserting a zero exit.
    pub(crate) fn run_git(cwd: &Path, args: &[&str]) {
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

    /// Initialise a real repository with one empty commit at `dir`.
    pub(crate) fn init_repo(dir: &Path) {
        run_git(dir, &["init", "-q", "-b", "main"]);
        run_git(dir, &["config", "user.email", "t@t.dev"]);
        run_git(dir, &["config", "user.name", "t"]);
        run_git(dir, &["commit", "-q", "--allow-empty", "-m", "init"]);
    }
}

#[cfg(all(test, not(windows)))]
mod tests {
    use super::testutil::{ScratchDir, init_repo, run_git, scratch};
    use super::*;
    use rstest::{fixture, rstest};
    use std::collections::HashSet;

    #[fixture]
    fn repo() -> ScratchDir {
        let dir = scratch("git-repo");
        init_repo(dir.path());
        dir
    }

    /// A main repository plus two linked worktrees in sibling directories.
    struct RepoWithWorktrees {
        _scratch: ScratchDir,
        main: PathBuf,
        linked: Vec<PathBuf>,
    }

    #[fixture]
    fn repo_with_worktrees() -> RepoWithWorktrees {
        let scratch = scratch("git-worktrees");
        let main = scratch.path().join("main");
        std::fs::create_dir_all(&main).unwrap();
        init_repo(&main);

        let linked = ["wt-a", "wt-b"]
            .into_iter()
            .map(|name| {
                let wt = scratch.path().join(name);
                run_git(&main, &["worktree", "add", "-q", wt.to_str().unwrap()]);
                wt
            })
            .collect();

        RepoWithWorktrees {
            _scratch: scratch,
            main,
            linked,
        }
    }

    #[rstest]
    fn repo_rejects_a_non_repo_dir() {
        let dir = scratch("git-nonrepo");
        assert!(Git::new(Path::new("git")).repo(dir.path()).is_err());
    }

    #[rstest]
    fn worktrees_lists_the_main_checkout_and_every_linked_worktree(
        repo_with_worktrees: RepoWithWorktrees,
    ) {
        let git = Git::new(Path::new("git"));
        let repo = git.repo(&repo_with_worktrees.main).unwrap();

        let got: HashSet<PathBuf> = repo
            .worktrees()
            .unwrap()
            .inspect(|w| assert!(!w.bare, "a normal checkout must not be flagged bare"))
            .map(|w| std::fs::canonicalize(w.path).unwrap())
            .collect();

        let expected: HashSet<PathBuf> = std::iter::once(&repo_with_worktrees.main)
            .chain(&repo_with_worktrees.linked)
            .map(|p| std::fs::canonicalize(p).unwrap())
            .collect();

        assert_eq!(got, expected);
    }

    #[rstest]
    fn toplevel_reports_the_work_tree_root(repo: ScratchDir) {
        let git = Git::new(Path::new("git"));
        let top = git.repo(repo.path()).unwrap().toplevel().unwrap();

        // canonicalize both sides: on macOS temp_dir is a /var -> /private/var
        // symlink that git resolves but our input path does not.
        assert_eq!(
            std::fs::canonicalize(top).unwrap(),
            std::fs::canonicalize(repo.path()).unwrap()
        );
    }

    #[rstest]
    fn git_dir_points_at_dot_git_of_a_normal_repo(repo: ScratchDir) {
        let git = Git::new(Path::new("git"));
        let git_dir = git.repo(repo.path()).unwrap().git_dir().unwrap();
        assert_eq!(
            std::fs::canonicalize(git_dir).unwrap(),
            std::fs::canonicalize(repo.path().join(".git")).unwrap()
        );
    }

    #[rstest]
    fn bare_repo_is_flagged_and_git_dir_resolves_to_itself() {
        // A bare repo has no `.git` entry and no work tree: `worktrees()` must
        // flag it `bare`, and `git_dir()` is the usable root (toplevel errors).
        let dir = scratch("git-bare");
        run_git(dir.path(), &["init", "-q", "--bare"]);
        let git = Git::new(Path::new("git"));
        let repo = git.repo(dir.path()).unwrap();

        let worktrees: Vec<_> = repo.worktrees().unwrap().collect();
        assert_eq!(worktrees.len(), 1);
        assert!(worktrees[0].bare);
        assert!(repo.toplevel().is_err(), "bare repo has no work tree");
        assert_eq!(
            std::fs::canonicalize(repo.git_dir().unwrap()).unwrap(),
            std::fs::canonicalize(dir.path()).unwrap()
        );
    }
}
