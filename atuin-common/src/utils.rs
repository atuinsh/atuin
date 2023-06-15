use std::env;
use std::path::{Path, PathBuf};

use chrono::{Months, NaiveDate};
use rand::RngCore;
use uuid::Uuid;

pub fn random_bytes<const N: usize>() -> [u8; N] {
    let mut ret = [0u8; N];

    rand::thread_rng().fill_bytes(&mut ret);

    ret
}

// basically just ripped from the uuid crate. they have it as unstable, but we can use it fine.
const fn encode_unix_timestamp_millis(millis: u64, random_bytes: &[u8; 10]) -> Uuid {
    let millis_high = ((millis >> 16) & 0xFFFF_FFFF) as u32;
    let millis_low = (millis & 0xFFFF) as u16;

    let random_and_version =
        (random_bytes[0] as u16 | ((random_bytes[1] as u16) << 8) & 0x0FFF) | (0x7 << 12);

    let mut d4 = [0; 8];

    d4[0] = (random_bytes[2] & 0x3F) | 0x80;
    d4[1] = random_bytes[3];
    d4[2] = random_bytes[4];
    d4[3] = random_bytes[5];
    d4[4] = random_bytes[6];
    d4[5] = random_bytes[7];
    d4[6] = random_bytes[8];
    d4[7] = random_bytes[9];

    Uuid::from_fields(millis_high, millis_low, random_and_version, &d4)
}

pub fn uuid_v7() -> Uuid {
    let bytes = random_bytes();
    let now: u64 = chrono::Utc::now().timestamp_millis() as u64;

    encode_unix_timestamp_millis(now, &bytes)
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

    return None;
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

pub fn get_days_from_month(year: i32, month: u32) -> i64 {
    let Some(start) = NaiveDate::from_ymd_opt(year, month, 1) else { return 30 };
    let Some(end) = start.checked_add_months(Months::new(1)) else { return 30 };
    end.signed_duration_since(start).num_days()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::env;

    use std::collections::HashSet;

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
        assert_eq!(get_days_from_month(2023, 1), 31);
        assert_eq!(get_days_from_month(2023, 2), 28);
        assert_eq!(get_days_from_month(2023, 3), 31);
        assert_eq!(get_days_from_month(2023, 4), 30);
        assert_eq!(get_days_from_month(2023, 5), 31);
        assert_eq!(get_days_from_month(2023, 6), 30);
        assert_eq!(get_days_from_month(2023, 7), 31);
        assert_eq!(get_days_from_month(2023, 8), 31);
        assert_eq!(get_days_from_month(2023, 9), 30);
        assert_eq!(get_days_from_month(2023, 10), 31);
        assert_eq!(get_days_from_month(2023, 11), 30);
        assert_eq!(get_days_from_month(2023, 12), 31);

        // leap years
        assert_eq!(get_days_from_month(2024, 2), 29);
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
}
