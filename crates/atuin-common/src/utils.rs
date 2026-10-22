use std::env;
use std::path::{Path, PathBuf};

use base64::prelude::{BASE64_URL_SAFE_NO_PAD, Engine};
use getrandom::fill;
use secrecy::{ExposeSecret, ExposeSecretMut, SecretBox, SecretString};
use uuid::Uuid;

use crate::path::PathBufExt as _;

/// Generate N random bytes, using a cryptographically secure source
#[must_use]
pub fn crypto_random_bytes<const N: usize>() -> SecretBox<[u8; N]> {
    // rand say they are in principle safe for crypto purposes, but that it is perhaps a better
    // idea to use getrandom for things such as passwords.
    let mut ret = SecretBox::new(Box::new([0u8; N]));

    fill(ret.expose_secret_mut()).expect("Failed to generate random bytes!");

    ret
}

/// Generate N random bytes using a cryptographically secure source, return encoded as a string
#[must_use]
pub fn crypto_random_string<const N: usize>() -> SecretString {
    let bytes = crypto_random_bytes::<N>();

    // We only use this to create a random string, and won't be reversing it to find the original
    // data - no padding is OK there. It may be in URLs.
    BASE64_URL_SAFE_NO_PAD.encode(bytes.expose_secret()).into()
}

#[must_use]
pub fn uuid_v7() -> Uuid {
    Uuid::now_v7()
}

#[must_use]
pub fn has_git_dir(path: &Path) -> bool {
    path.join(".git").exists()
}

// in a git worktree, .git is a file containing "gitdir: <path>" pointing
// to the main repo's .git/worktrees/<name> directory. follow the pointer
// back to the main repo root so all worktrees share a workspace.
fn resolve_git_worktree(path: &Path) -> Option<PathBuf> {
    let git_path = path.join(".git");

    if !git_path.is_file() {
        return None;
    }

    let contents = std::fs::read_to_string(&git_path).ok()?;
    let gitdir_str = contents.strip_prefix("gitdir: ")?.trim();

    let gitdir = PathBuf::from(gitdir_str);
    let gitdir = if gitdir.is_absolute() {
        gitdir
    } else {
        path.join(gitdir_str)
    };

    // walk up from e.g. /repo/.git/worktrees/feature to find /repo
    let mut candidate = gitdir.as_path();
    while let Some(parent) = candidate.parent() {
        if parent.join(".git").is_dir() {
            return Some(parent.to_path_buf());
        }
        candidate = parent;
    }

    None
}

// I really don't want to bring in libgit for something simple like this
// If we start to do anything more advanced, then perhaps
/// The nearest repository above `path`, whichever VCS owns it.
/// A git worktree resolves to the main repo; a jj workspace keeps its own root.
#[must_use]
pub fn in_vcs_repo(path: &Path) -> Option<PathBuf> {
    for dir in path.ancestors().take_while(|dir| !dir.as_os_str().is_empty()) {
        if dir.join(".jj").joined("repo").exists() {
            return Some(dir.to_path_buf());
        }

        if dir.join(".git").exists() {
            return Some(resolve_git_worktree(dir).unwrap_or_else(|| dir.to_path_buf()));
        }
    }

    None
}

/// Find the main Git repository for `path`, including linked worktrees.
#[must_use]
pub fn in_git_repo(path: &Path) -> Option<PathBuf> {
    let root = git_checkout_root(path)?;
    Some(resolve_git_worktree(&root).unwrap_or(root))
}

/// The root of the checkout `path` is in: the nearest directory, `path` or above, with a `.git`
/// entry. Unlike [`in_git_repo`], a linked worktree's root is the worktree's own, not its main
/// repository's: the directory its files are checked out in, and whose `HEAD` is its branch.
#[must_use]
pub fn git_checkout_root(path: &Path) -> Option<PathBuf> {
    path.ancestors()
        .take_while(|dir| !dir.as_os_str().is_empty())
        .find(|dir| has_git_dir(dir))
        .map(Path::to_path_buf)
}

#[must_use]
pub fn get_current_dir() -> PathBuf {
    // Prefer PWD environment variable over cwd if available to better support symbolic links
    match crate::env::var("PWD") {
        Ok(v) => v.into(),
        Err(_) => env::current_dir().unwrap_or_default(),
    }
}

#[derive(Debug, thiserror::Error)]
pub enum UnquoteError {
    #[error("not enough chars")]
    NotEnoughChars,
    #[error("unexpected eof, quotes do not match")]
    MismatchedQuotes,
}

pub fn unquote(s: &str) -> Result<String, UnquoteError> {
    if s.chars().count() < 2 {
        return Err(UnquoteError::NotEnoughChars);
    }

    let quote = s.chars().next().unwrap();

    // not quoted, do nothing
    if quote != '"' && quote != '\'' && quote != '`' {
        return Ok(s.to_string());
    }

    if s.chars().last().unwrap() != quote {
        return Err(UnquoteError::MismatchedQuotes);
    }

    // removes quote characters
    // the sanity checks performed above ensure that the quotes will be ASCII and this will not
    // panic
    let s = &s[1..s.len() - 1];

    Ok(s.to_string())
}

/// Normalize an optional string by trimming whitespace and filtering out empty strings.
///
/// This function always returns either [`None`], or a nonempty string with no leading or trailing
/// whitespace.
pub fn normalize_optional_string<T>(string: T) -> Option<String>
where
    T: Into<Option<String>>,
{
    let mut string = string.into()?;
    // Remove whitespace at end
    string.truncate(string.trim_end().len());
    // Remove whitespace at start
    string.drain(0..(string.len() - string.trim_start().len()));
    if string.is_empty() {
        None
    } else {
        Some(string)
    }
}

#[allow(unsafe_code)]
#[cfg(test)]
mod tests {
    use pretty_assertions::assert_ne;
    use rstest::{fixture, rstest};
    use tempfile::TempDir;

    use super::*;
    use crate::path::AbsPathBuf;

    #[fixture]
    fn tmp() -> TempDir {
        tempfile::tempdir().expect("could not create a temp dir")
    }

    /// `<root>/.git` as a directory: an ordinary clone.
    fn make_git_repo(root: &Path) {
        std::fs::create_dir_all(root.join(".git")).unwrap();
    }

    /// `<root>/.git` as a file pointing into `<main>/.git/worktrees/<name>`.
    fn make_git_worktree(root: &Path, main: &Path, name: &str) {
        let git_dir = main.join(".git").joined("worktrees").joined(name);
        std::fs::create_dir_all(&git_dir).unwrap();
        std::fs::create_dir_all(root).unwrap();
        std::fs::write(root.join(".git"), format!("gitdir: {}", git_dir.display())).unwrap();
    }

    /// `<root>/.jj/repo` as a directory: a main jj workspace.
    fn make_jj_workspace(root: &Path) {
        std::fs::create_dir_all(root.join(".jj").joined("repo")).unwrap();
    }

    /// `<root>/.jj/repo` as a file that holds `pointer`: a secondary jj workspace.
    fn make_jj_pointer(root: &Path, pointer: &str) {
        let jj_dir = root.join(".jj");
        std::fs::create_dir_all(&jj_dir).unwrap();
        std::fs::write(jj_dir.join("repo"), pointer).unwrap();
    }

    /// A directory well below `root`.
    fn subdir(root: &Path) -> PathBuf {
        let deep = root.join("src").joined("deep");
        std::fs::create_dir_all(&deep).unwrap();
        deep
    }

    #[cfg(not(windows))]
    #[rstest]
    fn git_repo_resolves_to_its_root(tmp: TempDir) {
        let root = tmp.path();
        make_git_repo(root);
        let deep = subdir(root);

        assert_eq!(in_git_repo(&deep), Some(root.to_path_buf()));
        assert_eq!(in_vcs_repo(&deep), Some(root.to_path_buf()));
    }

    /// Following the pointer keeps every worktree of a repo in one workspace. A jj
    /// workspace does not do this, see [`jj_secondary_workspace_resolves_to_its_own_root`].
    #[cfg(not(windows))]
    #[rstest]
    fn git_worktree_resolves_to_the_main_repo(tmp: TempDir) {
        let main = tmp.path().join("main");
        make_git_repo(&main);
        let worktree = tmp.path().join("worktree");
        make_git_worktree(&worktree, &main, "feature");
        let deep = subdir(&worktree);

        assert_eq!(in_git_repo(&deep), Some(main.clone()));
        assert_eq!(in_vcs_repo(&deep), Some(main));
    }

    #[cfg(not(windows))]
    #[rstest]
    fn jj_main_workspace_resolves_to_its_root(tmp: TempDir) {
        let root = tmp.path();
        make_jj_workspace(root);
        let deep = subdir(root);

        assert_eq!(in_vcs_repo(&deep), Some(root.to_path_buf()));
    }

    /// Pointer to the main workspace; scope stays on the tree where commands
    /// run.
    #[cfg(not(windows))]
    #[rstest]
    fn jj_secondary_workspace_resolves_to_its_own_root(
        tmp: TempDir,
        #[values(false, true)] colocated: bool,
    ) {
        let main = tmp.path().join("main");
        make_jj_workspace(&main);
        let workspace = tmp.path().join("workspace");
        make_jj_pointer(&workspace, "../../main/.jj/repo");
        if colocated {
            make_git_repo(&main);
            make_git_worktree(&workspace, &main, "workspace");
        }
        let deep = subdir(&workspace);

        assert_eq!(in_vcs_repo(&deep), Some(workspace));
    }

    #[cfg(not(windows))]
    #[rstest]
    fn colocated_repo_resolves_the_same_through_every_layer(tmp: TempDir) {
        let root = tmp.path();
        make_git_repo(root);
        make_jj_workspace(root);
        let deep = subdir(root);

        assert_eq!(in_git_repo(&deep), Some(root.to_path_buf()));
        assert_eq!(in_vcs_repo(&deep), Some(root.to_path_buf()));
    }

    /// The nearest repository wins.
    #[cfg(not(windows))]
    #[rstest]
    fn a_nested_jj_workspace_wins_over_the_outer_git_repo(tmp: TempDir) {
        let outer = tmp.path();
        make_git_repo(outer);
        let inner = outer.join("vendored");
        make_jj_workspace(&inner);
        let deep = subdir(&inner);

        assert_eq!(in_vcs_repo(&deep), Some(inner));
        assert_eq!(in_git_repo(&deep), Some(outer.to_path_buf()));
    }

    #[cfg(not(windows))]
    #[rstest]
    fn a_nested_git_repo_wins_over_the_outer_jj_repo(tmp: TempDir) {
        let outer = tmp.path();
        make_jj_workspace(outer);
        let inner = outer.join("vendored");
        make_git_repo(&inner);
        let deep = subdir(&inner);

        assert_eq!(in_vcs_repo(&deep), Some(inner));
    }

    /// The workspace root does not depend on the pointer.
    #[cfg(not(windows))]
    #[rstest]
    #[case::dangling("../../missing/.jj/repo")]
    #[case::garbage("this is not a path")]
    #[case::empty("")]
    fn a_jj_workspace_with_a_broken_pointer_keeps_its_root(tmp: TempDir, #[case] pointer: &str) {
        let workspace = tmp.path().join("workspace");
        make_jj_pointer(&workspace, pointer);
        let deep = subdir(&workspace);

        assert_eq!(in_vcs_repo(&deep), Some(workspace));
    }

    #[cfg(not(windows))]
    #[rstest]
    fn a_bare_jj_directory_is_not_a_workspace(tmp: TempDir) {
        let root = tmp.path();
        std::fs::create_dir_all(root.join(".jj")).unwrap();
        let deep = subdir(root);

        assert_eq!(in_vcs_repo(&deep), None);
    }

    #[cfg(not(windows))]
    #[rstest]
    fn a_jj_tree_is_invisible_to_the_git_layer(tmp: TempDir) {
        let root = tmp.path();
        make_jj_workspace(root);
        let deep = subdir(root);

        assert_eq!(in_git_repo(&deep), None);
        assert_eq!(in_vcs_repo(&deep), Some(root.to_path_buf()));
    }

    #[cfg(not(windows))]
    #[rstest]
    fn a_tree_with_no_repository_resolves_to_nothing(tmp: TempDir) {
        let deep = subdir(tmp.path());

        assert_eq!(in_git_repo(&deep), None);
        assert_eq!(in_vcs_repo(&deep), None);
    }

    /// Empty root is a prefix for every recorded cwd.
    #[rstest]
    #[case::empty("")]
    #[case::relative("src/deep")]
    fn the_walk_never_probes_the_empty_path(#[case] path: &str) {
        let path = Path::new(path);
        assert_eq!(in_vcs_repo(path), None);
        assert_eq!(in_git_repo(path), None);
        assert_eq!(git_checkout_root(path), None);
    }

    #[rstest]
    fn repo_walk_returns_the_nearest_marker() {
        use proptest::prelude::*;

        let parts = prop::collection::vec("[a-z]{1,12}", 0..16);
        proptest!(|(parts in parts, index in any::<usize>())| {
            let tmp = tempfile::tempdir().unwrap();
            let root = tmp.path().to_path_buf();
            let mut path = root.clone();
            let mut ancestors = vec![root.clone()];
            for part in parts {
                path.push(part);
                ancestors.push(path.clone());
            }
            let expected = &ancestors[index % ancestors.len()];
            make_jj_workspace(&root);
            make_jj_workspace(expected);
            let found = in_vcs_repo(&path);
            prop_assert_eq!(found.as_ref(), Some(expected));
        });
    }

    /// [`get_current_dir`] falls back to `""` when it cannot read the cwd, and that value
    /// reaches these functions directly.
    #[rstest]
    fn an_empty_path_is_in_no_repository() {
        assert_eq!(in_git_repo(Path::new("")), None);
        assert_eq!(in_vcs_repo(Path::new("")), None);
    }

    #[cfg(not(windows))]
    #[rstest]
    fn repo_markers_resolve_to_their_roots(tmp: TempDir) {
        let git = tmp.path().join("git");
        make_git_repo(&git);
        let jj = tmp.path().join("jj");
        make_jj_workspace(&jj);
        let bare = tmp.path().join("bare");
        std::fs::create_dir_all(&bare).unwrap();

        assert_eq!(in_git_repo(&git), Some(git.clone()));
        assert_eq!(in_vcs_repo(&git), Some(git.clone()));

        assert_eq!(in_git_repo(&jj), None);
        assert_eq!(in_vcs_repo(&jj), Some(jj.clone()));

        assert_eq!(in_git_repo(&bare), None);
        assert_eq!(in_vcs_repo(&bare), None);
    }

    /// Runs `git` in `dir`, failing the test if it fails.
    #[cfg(unix)]
    fn git(dir: &Path, args: &[&str]) {
        let output = std::process::Command::new("git")
            .args(["-c", "user.name=atuin", "-c", "user.email=atuin@example.com"])
            .args(args)
            .current_dir(dir)
            .env_remove("GIT_DIR")
            .env_remove("GIT_WORK_TREE")
            .output()
            .unwrap();
        assert!(output.status.success(), "git {args:?} failed: {output:?}");
    }

    #[cfg(unix)]
    #[rstest]
    fn a_real_worktree_is_its_own_checkout_in_the_main_repository(tmp: TempDir) {
        let main = tmp.path().join("main");
        std::fs::create_dir_all(&main).unwrap();
        git(&main, &["init", "-q", "-b", "trunk"]);
        git(&main, &["commit", "-q", "--allow-empty", "-m", "init"]);
        let worktree = tmp.path().join("feature-wt");
        git(&main, &["worktree", "add", "-q", "-b", "feature", worktree.to_str().unwrap()]);
        let deep = subdir(&worktree);

        assert_eq!(git_checkout_root(&deep), Some(worktree));
        assert_eq!(git_checkout_root(&main), Some(main.clone()));
        // Through the `.git` file, whose path may be resolved (on macOS, /var is /private/var).
        let repo = |path: &Path| in_git_repo(path).map(|p| AbsPathBuf::canonicalize(p).unwrap());
        assert_eq!(repo(&deep), Some(AbsPathBuf::canonicalize(&main).unwrap()));
        assert_eq!(repo(&main), Some(AbsPathBuf::canonicalize(&main).unwrap()));
    }

    #[rstest]
    #[allow(
        clippy::used_underscore_binding,
        reason = "unused _n is used to drive generic N within rstest"
    )]
    fn dumb_random_test<const N: usize>(#[values([(); 8], [(); 16], [(); 32])] _n: [(); N]) {
        // Obviously not a test of randomness, but make sure we haven't made some
        // catastrophic error

        assert_ne!(
            crypto_random_string::<N>().expose_secret(),
            crypto_random_string::<N>().expose_secret()
        );
    }
}
