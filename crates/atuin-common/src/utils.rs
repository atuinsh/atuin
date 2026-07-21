use std::env;
use std::path::{Path, PathBuf};

use eyre::{Result, eyre};

use base64::prelude::{BASE64_URL_SAFE_NO_PAD, Engine};
use getrandom::fill;
use uuid::Uuid;

/// Generate N random bytes, using a cryptographically secure source
pub fn crypto_random_bytes<const N: usize>() -> [u8; N] {
    // rand say they are in principle safe for crypto purposes, but that it is perhaps a better
    // idea to use getrandom for things such as passwords.
    let mut ret = [0u8; N];

    fill(&mut ret).expect("Failed to generate random bytes!");

    ret
}

/// Generate N random bytes using a cryptographically secure source, return encoded as a string
pub fn crypto_random_string<const N: usize>() -> String {
    let bytes = crypto_random_bytes::<N>();

    // We only use this to create a random string, and won't be reversing it to find the original
    // data - no padding is OK there. It may be in URLs.
    BASE64_URL_SAFE_NO_PAD.encode(bytes)
}

pub fn uuid_v7() -> Uuid {
    Uuid::now_v7()
}

pub fn uuid_v4() -> String {
    Uuid::new_v4().as_simple().to_string()
}

pub fn has_git_dir(path: &str) -> bool {
    let mut gitdir = PathBuf::from(path);
    gitdir.push(".git");

    gitdir.exists()
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

// detect if any parent dir has a git repo in it
// I really don't want to bring in libgit for something simple like this
// If we start to do anything more advanced, then perhaps
pub fn in_git_repo(path: &str) -> Option<PathBuf> {
    let mut gitdir = PathBuf::from(path);

    while gitdir.parent().is_some() && !has_git_dir(gitdir.to_str().unwrap()) {
        gitdir.pop();
    }

    // No parent? then we hit root, finding no git
    if gitdir.parent().is_some() {
        // if .git is a file (worktree), resolve to the main repo root
        if let Some(main_repo) = resolve_git_worktree(&gitdir) {
            return Some(main_repo);
        }
        return Some(gitdir);
    }

    None
}

// TODO: more reliable, more tested
// I don't want to use ProjectDirs, it puts config in awkward places on
// mac. Data too. Seems to be more intended for GUI apps.

pub fn home_dir() -> PathBuf {
    directories::BaseDirs::new()
        .map(|d| d.home_dir().to_path_buf())
        .expect("could not determine home directory")
}

pub fn config_dir() -> PathBuf {
    let config_dir =
        std::env::var("XDG_CONFIG_HOME").map_or_else(|_| home_dir().join(".config"), PathBuf::from);
    config_dir.join("atuin")
}

pub fn data_dir() -> PathBuf {
    let data_dir = std::env::var("XDG_DATA_HOME")
        .map_or_else(|_| home_dir().join(".local").join("share"), PathBuf::from);

    data_dir.join("atuin")
}

pub fn runtime_dir() -> PathBuf {
    std::env::var("XDG_RUNTIME_DIR").map_or_else(|_| data_dir(), PathBuf::from)
}

pub fn logs_dir() -> PathBuf {
    home_dir().join(".atuin").join("logs")
}

pub fn dotfiles_cache_dir() -> PathBuf {
    // In most cases, this will be  ~/.local/share/atuin/dotfiles/cache
    let data_dir = std::env::var("XDG_DATA_HOME")
        .map_or_else(|_| home_dir().join(".local").join("share"), PathBuf::from);

    data_dir.join("atuin").join("dotfiles").join("cache")
}

pub fn get_current_dir() -> String {
    // Prefer PWD environment variable over cwd if available to better support symbolic links
    match env::var("PWD") {
        Ok(v) => v,
        Err(_) => match env::current_dir() {
            Ok(dir) => dir.display().to_string(),
            Err(_) => String::from(""),
        },
    }
}

pub fn broken_symlink<P: Into<PathBuf>>(path: P) -> bool {
    let path = path.into();
    path.is_symlink() && !path.exists()
}

pub fn unquote(s: &str) -> Result<String> {
    if s.chars().count() < 2 {
        return Err(eyre!("not enough chars"));
    }

    let quote = s.chars().next().unwrap();

    // not quoted, do nothing
    if quote != '"' && quote != '\'' && quote != '`' {
        return Ok(s.to_string());
    }

    if s.chars().last().unwrap() != quote {
        return Err(eyre!("unexpected eof, quotes do not match"));
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

    use super::*;

    #[cfg(not(windows))]
    #[test]
    fn test_dirs() {
        // these tests need to be run sequentially to prevent race condition
        test_config_dir_xdg();
        test_config_dir();
        test_data_dir_xdg();
        test_data_dir();
    }

    #[cfg(not(windows))]
    fn test_config_dir_xdg() {
        // TODO: Audit that the environment access only happens in single-threaded code.
        unsafe { env::remove_var("HOME") };
        // TODO: Audit that the environment access only happens in single-threaded code.
        unsafe { env::set_var("XDG_CONFIG_HOME", "/home/user/custom_config") };
        assert_eq!(
            config_dir(),
            PathBuf::from("/home/user/custom_config/atuin")
        );
        // TODO: Audit that the environment access only happens in single-threaded code.
        unsafe { env::remove_var("XDG_CONFIG_HOME") };
    }

    #[cfg(not(windows))]
    fn test_config_dir() {
        // TODO: Audit that the environment access only happens in single-threaded code.
        unsafe { env::set_var("HOME", "/home/user") };
        // TODO: Audit that the environment access only happens in single-threaded code.
        unsafe { env::remove_var("XDG_CONFIG_HOME") };

        assert_eq!(config_dir(), PathBuf::from("/home/user/.config/atuin"));

        // TODO: Audit that the environment access only happens in single-threaded code.
        unsafe { env::remove_var("HOME") };
    }

    #[cfg(not(windows))]
    fn test_data_dir_xdg() {
        // TODO: Audit that the environment access only happens in single-threaded code.
        unsafe { env::remove_var("HOME") };
        // TODO: Audit that the environment access only happens in single-threaded code.
        unsafe { env::set_var("XDG_DATA_HOME", "/home/user/custom_data") };
        assert_eq!(data_dir(), PathBuf::from("/home/user/custom_data/atuin"));
        // TODO: Audit that the environment access only happens in single-threaded code.
        unsafe { env::remove_var("XDG_DATA_HOME") };
    }

    #[cfg(not(windows))]
    fn test_data_dir() {
        // TODO: Audit that the environment access only happens in single-threaded code.
        unsafe { env::set_var("HOME", "/home/user") };
        // TODO: Audit that the environment access only happens in single-threaded code.
        unsafe { env::remove_var("XDG_DATA_HOME") };
        assert_eq!(data_dir(), PathBuf::from("/home/user/.local/share/atuin"));
        // TODO: Audit that the environment access only happens in single-threaded code.
        unsafe { env::remove_var("HOME") };
    }

    #[cfg(not(windows))]
    #[test]
    fn in_git_repo_regular() {
        // regular git repo should resolve to the directory containing .git
        let tmp = std::env::temp_dir().join("atuin-test-regular-git");
        let _ = std::fs::remove_dir_all(&tmp);
        let subdir = tmp.join("src").join("deep");
        std::fs::create_dir_all(&subdir).unwrap();
        std::fs::create_dir_all(tmp.join(".git")).unwrap();

        let result = in_git_repo(subdir.to_str().unwrap());
        assert_eq!(result, Some(tmp.clone()));

        std::fs::remove_dir_all(&tmp).unwrap();
    }

    #[cfg(not(windows))]
    #[test]
    fn in_git_repo_worktree_resolves_to_main_repo() {
        // worktree .git is a file pointing back to the main repo —
        // in_git_repo should follow it so all worktrees share a workspace
        let tmp = std::env::temp_dir().join("atuin-test-worktree-git");
        let _ = std::fs::remove_dir_all(&tmp);

        // main repo at tmp/main with a real .git directory
        let main_repo = tmp.join("main");
        let worktree_git_dir = main_repo.join(".git").join("worktrees").join("feature");
        std::fs::create_dir_all(&worktree_git_dir).unwrap();

        // worktree at tmp/worktree with a .git file
        let worktree = tmp.join("worktree");
        let worktree_subdir = worktree.join("src");
        std::fs::create_dir_all(&worktree_subdir).unwrap();
        std::fs::write(
            worktree.join(".git"),
            format!("gitdir: {}", worktree_git_dir.to_str().unwrap()),
        )
        .unwrap();

        // should resolve to the main repo root, not the worktree root
        let result = in_git_repo(worktree_subdir.to_str().unwrap());
        assert_eq!(result, Some(main_repo));

        std::fs::remove_dir_all(&tmp).unwrap();
    }

    #[test]
    fn dumb_random_test() {
        // Obviously not a test of randomness, but make sure we haven't made some
        // catastrophic error

        assert_ne!(crypto_random_string::<8>(), crypto_random_string::<8>());
        assert_ne!(crypto_random_string::<16>(), crypto_random_string::<16>());
        assert_ne!(crypto_random_string::<32>(), crypto_random_string::<32>());
    }
}
