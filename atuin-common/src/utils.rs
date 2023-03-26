use std::env;
use std::path::PathBuf;

use chrono::{Months, NaiveDate};
use uuid::Uuid;

pub fn uuid_v4() -> String {
    Uuid::new_v4().as_simple().to_string()
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
}
