use std::ffi::OsString;
use std::path::PathBuf;

use base64::prelude::{BASE64_URL_SAFE_NO_PAD, Engine};
use eyre::{Result, eyre};
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

// TODO: more reliable, more tested
// I don't want to use ProjectDirs, it puts config in awkward places on
// mac. Data too. Seems to be more intended for GUI apps.

pub fn home_dir() -> PathBuf {
    directories::BaseDirs::new()
        .map(|d| d.home_dir().to_path_buf())
        .expect("could not determine home directory")
}

/// Read an environment variable that must be nonempty.
///
/// This function will never return an empty string: if the environment variable is set but empty,
/// [`None`] is returned.
pub fn env_nonempty(name: &str) -> Option<OsString> {
    std::env::var_os(name).filter(|value| !value.is_empty())
}

/// Read an environment variable that must be an absolute path.
///
/// This is usually done in the name of XDG-compliance which requires that paths given through
/// environment variables are absolute.
pub fn env_abspath(name: &str) -> Option<PathBuf> {
    env_nonempty(name).map(PathBuf::from).filter(|s| s.is_absolute())
}

pub fn config_dir() -> PathBuf {
    let config_dir: PathBuf =
        env_abspath("XDG_CONFIG_HOME").unwrap_or_else(|| home_dir().join(".config"));
    config_dir.join("atuin")
}

pub fn data_dir() -> PathBuf {
    let data_dir: PathBuf =
        env_abspath("XDG_DATA_HOME").unwrap_or_else(|| home_dir().join(".local").join("share"));
    data_dir.join("atuin")
}

pub fn logs_dir() -> PathBuf {
    home_dir().join(".atuin").join("logs")
}

pub fn dotfiles_cache_dir() -> PathBuf {
    // In most cases, this will be  ~/.local/share/atuin/dotfiles/cache
    data_dir().join("dotfiles").join("cache")
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
    use rstest::rstest;
    use std::env;

    use super::*;

    #[cfg(not(windows))]
    #[test]
    fn test_dirs() {
        // these tests need to be run sequentially to prevent race condition
        test_config_dir_xdg();
        test_config_dir_xdg_empty();
        test_config_dir();
        test_data_dir_xdg();
        test_data_dir_xdg_empty();
        test_data_dir();
    }

    #[cfg(not(windows))]
    fn test_config_dir_xdg() {
        // TODO: Audit that the environment access only happens in single-threaded code.
        unsafe { env::remove_var("HOME") };
        // TODO: Audit that the environment access only happens in single-threaded code.
        unsafe { env::set_var("XDG_CONFIG_HOME", "/home/user/custom_config") };
        assert_eq!(config_dir(), PathBuf::from("/home/user/custom_config/atuin"));
        // TODO: Audit that the environment access only happens in single-threaded code.
        unsafe { env::remove_var("XDG_CONFIG_HOME") };
    }

    /// An empty `XDG_CONFIG_HOME` has to be treated as unset: the alternative is a relative path.
    #[cfg(not(windows))]
    fn test_config_dir_xdg_empty() {
        // TODO: Audit that the environment access only happens in single-threaded code.
        unsafe { env::set_var("HOME", "/home/user") };
        // TODO: Audit that the environment access only happens in single-threaded code.
        unsafe { env::set_var("XDG_CONFIG_HOME", "") };
        assert_eq!(config_dir(), PathBuf::from("/home/user/.config/atuin"));
        // TODO: Audit that the environment access only happens in single-threaded code.
        unsafe { env::remove_var("XDG_CONFIG_HOME") };
        // TODO: Audit that the environment access only happens in single-threaded code.
        unsafe { env::remove_var("HOME") };
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

    /// An empty `XDG_DATA_HOME` has to be treated as unset: the alternative is a relative path.
    #[cfg(not(windows))]
    fn test_data_dir_xdg_empty() {
        // TODO: Audit that the environment access only happens in single-threaded code.
        unsafe { env::set_var("HOME", "/home/user") };
        // TODO: Audit that the environment access only happens in single-threaded code.
        unsafe { env::set_var("XDG_DATA_HOME", "") };
        assert_eq!(data_dir(), PathBuf::from("/home/user/.local/share/atuin"));
        // TODO: Audit that the environment access only happens in single-threaded code.
        unsafe { env::remove_var("XDG_DATA_HOME") };
        // TODO: Audit that the environment access only happens in single-threaded code.
        unsafe { env::remove_var("HOME") };
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

    #[rstest]
    fn dumb_random_test<const N: usize>(#[values([(); 8], [(); 16], [(); 32])] _n: [(); N]) {
        // Obviously not a test of randomness, but make sure we haven't made some
        // catastrophic error

        assert_ne!(crypto_random_string::<N>(), crypto_random_string::<N>());
    }
}
