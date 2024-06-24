use std::borrow::Cow;
use std::env;
use std::path::PathBuf;

use eyre::{eyre, Result};

use base64::prelude::{Engine, BASE64_URL_SAFE_NO_PAD};
use getrandom::getrandom;
use uuid::Uuid;

/// Generate N random bytes, using a cryptographically secure source
pub fn crypto_random_bytes<const N: usize>() -> [u8; N] {
    // rand say they are in principle safe for crypto purposes, but that it is perhaps a better
    // idea to use getrandom for things such as passwords.
    let mut ret = [0u8; N];

    getrandom(&mut ret).expect("Failed to generate random bytes!");

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
        return Some(gitdir);
    }

    None
}

// TODO: more reliable, more tested
// I don't want to use ProjectDirs, it puts config in awkward places on
// mac. Data too. Seems to be more intended for GUI apps.

#[cfg(not(target_os = "windows"))]
pub fn home_dir() -> PathBuf {
    let home = std::env::var("HOME").expect("$HOME not found");
    PathBuf::from(home)
}

#[cfg(target_os = "windows")]
pub fn home_dir() -> PathBuf {
    let home = std::env::var("USERPROFILE").expect("%userprofile% not found");
    PathBuf::from(home)
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

pub fn is_zsh() -> bool {
    // only set on zsh
    env::var("ATUIN_SHELL_ZSH").is_ok()
}

pub fn is_fish() -> bool {
    // only set on fish
    env::var("ATUIN_SHELL_FISH").is_ok()
}

pub fn is_bash() -> bool {
    // only set on bash
    env::var("ATUIN_SHELL_BASH").is_ok()
}

pub fn is_xonsh() -> bool {
    // only set on xonsh
    env::var("ATUIN_SHELL_XONSH").is_ok()
}

/// Extension trait for anything that can behave like a string to make it easy to escape control
/// characters.
///
/// Intended to help prevent control characters being printed and interpreted by the terminal when
/// printing history as well as to ensure the commands that appear in the interactive search
/// reflect the actual command run rather than just the printable characters.
pub trait Escapable: AsRef<str> {
    fn escape_control(&self) -> Cow<str> {
        if !self.as_ref().contains(|c: char| c.is_ascii_control()) {
            self.as_ref().into()
        } else {
            let mut remaining = self.as_ref();
            // Not a perfect way to reserve space but should reduce the allocations
            let mut buf = String::with_capacity(remaining.as_bytes().len());
            while let Some(i) = remaining.find(|c: char| c.is_ascii_control()) {
                // safe to index with `..i`, `i` and `i+1..` as part[i] is a single byte ascii char
                buf.push_str(&remaining[..i]);
                buf.push('^');
                buf.push(match remaining.as_bytes()[i] {
                    0x7F => '?',
                    code => char::from_u32(u32::from(code) + 64).unwrap(),
                });
                remaining = &remaining[i + 1..];
            }
            buf.push_str(remaining);
            buf.into()
        }
    }
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

impl<T: AsRef<str>> Escapable for T {}

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_ne;
    use time::Month;

    use super::*;
    use std::env;

    use std::collections::HashSet;

    #[cfg(not(windows))]
    #[test]
    fn test_dirs() {
        // these tests need to be run sequentially to prevent race condition
        test_config_dir_xdg();
        test_config_dir();
        test_data_dir_xdg();
        test_data_dir();
    }

    fn test_config_dir_xdg() {
        env::remove_var("HOME");
        env::set_var("XDG_CONFIG_HOME", "/home/user/custom_config");
        assert_eq!(
            config_dir(),
            PathBuf::from("/home/user/custom_config/atuin")
        );
        env::remove_var("XDG_CONFIG_HOME");
    }

    fn test_config_dir() {
        env::set_var("HOME", "/home/user");
        env::remove_var("XDG_CONFIG_HOME");

        assert_eq!(config_dir(), PathBuf::from("/home/user/.config/atuin"));

        env::remove_var("HOME");
    }

    fn test_data_dir_xdg() {
        env::remove_var("HOME");
        env::set_var("XDG_DATA_HOME", "/home/user/custom_data");
        assert_eq!(data_dir(), PathBuf::from("/home/user/custom_data/atuin"));
        env::remove_var("XDG_DATA_HOME");
    }

    fn test_data_dir() {
        env::set_var("HOME", "/home/user");
        env::remove_var("XDG_DATA_HOME");
        assert_eq!(data_dir(), PathBuf::from("/home/user/.local/share/atuin"));
        env::remove_var("HOME");
    }

    #[test]
    fn days_from_month() {
        assert_eq!(time::util::days_in_year_month(2023, Month::January), 31);
        assert_eq!(time::util::days_in_year_month(2023, Month::February), 28);
        assert_eq!(time::util::days_in_year_month(2023, Month::March), 31);
        assert_eq!(time::util::days_in_year_month(2023, Month::April), 30);
        assert_eq!(time::util::days_in_year_month(2023, Month::May), 31);
        assert_eq!(time::util::days_in_year_month(2023, Month::June), 30);
        assert_eq!(time::util::days_in_year_month(2023, Month::July), 31);
        assert_eq!(time::util::days_in_year_month(2023, Month::August), 31);
        assert_eq!(time::util::days_in_year_month(2023, Month::September), 30);
        assert_eq!(time::util::days_in_year_month(2023, Month::October), 31);
        assert_eq!(time::util::days_in_year_month(2023, Month::November), 30);
        assert_eq!(time::util::days_in_year_month(2023, Month::December), 31);

        // leap years
        assert_eq!(time::util::days_in_year_month(2024, Month::February), 29);
    }

    #[test]
    fn uuid_is_unique() {
        let how_many: usize = 1000000;

        // for peace of mind
        let mut uuids: HashSet<Uuid> = HashSet::with_capacity(how_many);

        // there will be many in the same millisecond
        for _ in 0..how_many {
            let uuid = uuid_v7();
            uuids.insert(uuid);
        }

        assert_eq!(uuids.len(), how_many);
    }

    #[test]
    fn escape_control_characters() {
        use super::Escapable;
        // CSI colour sequence
        assert_eq!("\x1b[31mfoo".escape_control(), "^[[31mfoo");

        // Tabs count as control chars
        assert_eq!("foo\tbar".escape_control(), "foo^Ibar");

        // space is in control char range but should be excluded
        assert_eq!("two words".escape_control(), "two words");

        // unicode multi-byte characters
        let s = "🐢\x1b[32m🦀";
        assert_eq!(s.escape_control(), s.replace("\x1b", "^["));
    }

    #[test]
    fn escape_no_control_characters() {
        use super::Escapable as _;
        assert!(matches!(
            "no control characters".escape_control(),
            Cow::Borrowed(_)
        ));
        assert!(matches!(
            "with \x1b[31mcontrol\x1b[0m characters".escape_control(),
            Cow::Owned(_)
        ));
    }

    #[test]
    fn dumb_random_test() {
        // Obviously not a test of randomness, but make sure we haven't made some
        // catastrophic error

        assert_ne!(crypto_random_string::<1>(), crypto_random_string::<1>());
        assert_ne!(crypto_random_string::<2>(), crypto_random_string::<2>());
        assert_ne!(crypto_random_string::<4>(), crypto_random_string::<4>());
        assert_ne!(crypto_random_string::<8>(), crypto_random_string::<8>());
        assert_ne!(crypto_random_string::<16>(), crypto_random_string::<16>());
        assert_ne!(crypto_random_string::<32>(), crypto_random_string::<32>());
    }
}
