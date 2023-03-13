use std::path::PathBuf;

use chrono::NaiveDate;
use uuid::Uuid;

pub fn uuid_v7() -> String {
    Uuid::now_v7().as_simple().to_string()
}

pub fn hash_str(string: &str) -> String {
    hash_bytes(string.as_bytes())
}

pub fn hash_bytes(buf: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(buf);
    hex::encode(hasher.finalize())
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

pub fn get_days_from_month(year: i32, month: u32) -> i64 {
    NaiveDate::from_ymd(
        match month {
            12 => year + 1,
            _ => year,
        },
        match month {
            12 => 1,
            _ => month + 1,
        },
        1,
    )
    .signed_duration_since(NaiveDate::from_ymd(year, month, 1))
    .num_days()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::env;

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
}
