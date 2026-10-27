use std::env;
use std::path::{Path, PathBuf};

use base64::prelude::{BASE64_URL_SAFE_NO_PAD, Engine};
use getrandom::fill;
use secrecy::{ExposeSecret, ExposeSecretMut, SecretBox, SecretString};
use uuid::Uuid;

use crate::path::AbsPathBuf;

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

/// in a linked worktree, .git is a file containing `gitdir: <path>` pointing to the main repo's
/// `.git/worktrees/<name>` directory, whose `commondir` file points on to the main repo's .git.
/// follow both back to the main repo root so all worktrees share a workspace. like git, resolve a
/// relative pointer from the real location of its file: the `..` applies after symlinks.
///
/// TODO(markovejnovic): This is slop that should really not live here. It would be much better if
/// we used some correct git library.
///
/// There are also bugs with this implementation---without shelling out to git, there's no correct
/// way to actually resolve the git worktree. PR #3850 was supposed to fix this, but never landed.
fn resolve_git_worktree(path: &Path) -> Option<AbsPathBuf> {
    let git_path = path.join(".git");

    if !git_path.is_file() {
        return None;
    }

    let contents = std::fs::read_to_string(&git_path).ok()?;
    let gitdir = contents.strip_prefix("gitdir: ")?.trim();
    let gitdir = AbsPathBuf::canonicalize(path.join(gitdir)).ok()?;

    // a submodule's gitdir is a repository of its own, with no `commondir`
    let commondir = std::fs::read_to_string(gitdir.join("commondir")).ok()?;
    let commondir = AbsPathBuf::canonicalize(gitdir.join(commondir.trim())).ok()?;

    // a bare repository has no main worktree to share
    if commondir.file_name()? != ".git" {
        return None;
    }
    commondir.parent().map(ToOwned::to_owned)
}

// detect if any parent dir has a git repo in it
// I really don't want to bring in libgit for something simple like this
// If we start to do anything more advanced, then perhaps
//
// A linked worktree resolves to its main repository, so that all of a repository's worktrees
// share a workspace. [`git_checkout_root`] stops at the worktree instead.
#[must_use]
pub fn in_git_repo(path: &Path) -> Option<PathBuf> {
    let root = git_checkout_root(path)?;
    Some(resolve_git_worktree(&root).map_or(root, AbsPathBuf::into_path_buf))
}

/// The root of the checkout `path` is in: the nearest directory, `path` or above, with a `.git`
/// entry. Unlike [`in_git_repo`], a linked worktree's root is the worktree's own, not its main
/// repository's: the directory its files are checked out in, and whose `HEAD` is its branch.
#[must_use]
pub fn git_checkout_root(path: &Path) -> Option<PathBuf> {
    let mut gitdir = PathBuf::from(path);

    while gitdir.parent().is_some() && !has_git_dir(&gitdir) {
        gitdir.pop();
    }

    // No parent? then we hit root, finding no git
    gitdir.parent().is_some().then_some(gitdir)
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

    use super::*;
    use crate::path::PathBufExt as _;

    #[cfg(unix)]
    #[fixture]
    fn tmp() -> tempfile::TempDir {
        tempfile::tempdir().unwrap()
    }

    /// The layouts are the ones git 2.56 writes. A `.git` at the top stands for an unrelated
    /// repository above the checkouts, like dotfiles in `$HOME`, which a pointer resolved the wrong
    /// way lands in.
    #[cfg(unix)]
    #[rstest]
    #[case::repository(&[("repo/.git/HEAD", "")], &[], "repo/src", "repo")]
    #[case::worktree_with_a_relative_gitdir(
        &[
            ("main/.git/worktrees/wt/commondir", "../..\n"),
            ("wt/.git", "gitdir: ../main/.git/worktrees/wt\n"),
        ],
        &[],
        "wt/src",
        "main",
    )]
    // Git applies the `..` after following the symlink, so it leads out of `data/`, not `home/`.
    #[case::worktree_with_a_relative_gitdir_through_a_symlink(
        &[
            (".git/HEAD", ""),
            ("data/main/.git/worktrees/wt/commondir", "../..\n"),
            ("data/wt/.git", "gitdir: ../main/.git/worktrees/wt\n"),
        ],
        &[("home/wt", "../data/wt")],
        "home/wt/src",
        "data/main",
    )]
    #[case::worktree_of_a_bare_repository(
        &[
            (".git/HEAD", ""),
            ("repo.git/worktrees/wt/commondir", "../..\n"),
            ("wt/.git", "gitdir: ../repo.git/worktrees/wt\n"),
        ],
        &[],
        "wt/src",
        "wt",
    )]
    #[case::submodule(
        &[
            ("super/.git/modules/sub/HEAD", ""),
            ("super/sub/.git", "gitdir: ../.git/modules/sub\n"),
        ],
        &[],
        "super/sub/src",
        "super/sub",
    )]
    #[case::dangling_gitdir(
        &[(".git/HEAD", ""), ("wt/.git", "gitdir: ../gone/.git/worktrees/wt\n")],
        &[],
        "wt/src",
        "wt",
    )]
    fn only_a_linked_worktree_resolves_to_its_main_repository(
        tmp: tempfile::TempDir,
        #[case] files: &[(&str, &str)],
        #[case] symlinks: &[(&str, &str)],
        #[case] cwd: &str,
        #[case] workspace: &str,
    ) {
        // Resolved like the pointers are (on macOS, /var is /private/var).
        let root = AbsPathBuf::canonicalize(tmp.path()).unwrap();
        for (path, contents) in files {
            let path = root.join(path);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, contents).unwrap();
        }
        for (link, target) in symlinks {
            let link = root.join(link);
            std::fs::create_dir_all(link.parent().unwrap()).unwrap();
            std::os::unix::fs::symlink(target, link).unwrap();
        }
        let cwd = root.join(cwd);
        std::fs::create_dir_all(&cwd).unwrap();

        assert_eq!(in_git_repo(&cwd), Some(root.join(workspace)));
    }

    /// Runs `git` in `dir`, failing the test if it fails.
    #[cfg(unix)]
    fn git(dir: &Path, args: &[&str]) {
        let status = std::process::Command::new("git")
            .args(["-c", "user.name=atuin", "-c", "user.email=atuin@example.com"])
            .args(args)
            .current_dir(dir)
            .env_remove("GIT_DIR")
            .env_remove("GIT_WORK_TREE")
            .status()
            .unwrap();
        assert!(status.success(), "git {args:?} failed");
    }

    #[cfg(unix)]
    #[rstest]
    fn a_real_worktree_is_its_own_checkout_in_the_main_repository(tmp: tempfile::TempDir) {
        let main = tmp.path().join("main");
        std::fs::create_dir_all(&main).unwrap();
        git(&main, &["init", "-q", "-b", "trunk"]);
        git(&main, &["commit", "-q", "--allow-empty", "-m", "init"]);
        let worktree = tmp.path().join("feature-wt");
        git(&main, &["worktree", "add", "-q", "-b", "feature", worktree.to_str().unwrap()]);
        let deep = worktree.join("src").joined("deep");
        std::fs::create_dir_all(&deep).unwrap();

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
