//! Finds the names of an application in its desktop entry.

use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};

use atuin_common::env::var_nonempty;
use atuin_common::path::PathBufExt as _;

/// The directory under the home directory that holds the user's exported Flatpak applications.
///
/// Flatpak adds it and [`FLATPAK_SYSTEM_DATA_DIR`] to `XDG_DATA_DIRS` in a login shell, but the
/// daemon that starts the helper may run without them.
const FLATPAK_USER_DATA_DIR: &str = ".local/share/flatpak/exports/share";

/// The directory that holds the exported Flatpak applications of the system.
const FLATPAK_SYSTEM_DATA_DIR: &str = "/var/lib/flatpak/exports/share";

/// The names in the desktop entry whose id is `id`: its name, and then its translations.
/// Empty when no entry has this id, or when `id` can't name an entry.
pub fn names(id: &str) -> Vec<String> {
    let mut file_name = OsString::from(id);
    file_name.push(".desktop");
    if id.is_empty() || Path::new(&file_name).file_name() != Some(file_name.as_os_str()) {
        return Vec::new();
    }
    read_entry(&file_name).map_or_else(Vec::new, |entry| names_in(&entry))
}

/// Reads the desktop entry in the file `file_name`, from the first data directory that holds
/// one.
///
/// The data directories come in the order of their precedence, as the XDG base directory
/// specification lists them, followed by the directories of Flatpak's exports.
fn read_entry(file_name: &OsStr) -> Option<String> {
    let home = atuin_common::dirs::try_home_dir().ok();
    let data_home = var_nonempty("XDG_DATA_HOME")
        .map(PathBuf::from)
        .or_else(|| home.as_ref().map(|home| home.join(".local/share")));
    let data_dirs = var_nonempty("XDG_DATA_DIRS")
        .unwrap_or_else(|| OsString::from("/usr/local/share:/usr/share"));
    let flatpak_user = home.map(|home| home.joined(FLATPAK_USER_DATA_DIR));
    data_home
        .into_iter()
        .chain(std::env::split_paths(&data_dirs))
        .chain(flatpak_user)
        .chain([PathBuf::from(FLATPAK_SYSTEM_DATA_DIR)])
        .filter(|dir| dir.is_absolute())
        .find_map(|dir| std::fs::read_to_string(dir.joined("applications").joined(file_name)).ok())
}

/// The `Name` key of the `[Desktop Entry]` group of `entry`, and then its translations, such
/// as `Name[de]`.
fn names_in(entry: &str) -> Vec<String> {
    let mut names = Vec::new();
    let mut in_entry = false;
    for line in entry.lines() {
        let line = line.trim();
        if line.starts_with('[') {
            in_entry = line == "[Desktop Entry]";
            continue;
        }
        if !in_entry {
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let key = key.trim_end();
        let value = value.trim_start();
        let is_name = key == "Name";
        let is_translation = key.strip_prefix("Name[").is_some_and(|rest| rest.ends_with(']'));
        if value.is_empty() || !(is_name || is_translation) {
            continue;
        }
        if is_name {
            names.insert(0, value.to_owned());
        } else {
            names.push(value.to_owned());
        }
    }
    names
}

#[cfg(test)]
mod tests {
    use rstest::rstest;

    use super::*;

    #[rstest]
    fn an_entry_gives_its_name_first_and_then_its_translations() {
        let entry = "\
[Desktop Entry]
Name[de]=Flare (Signal)
Type=Application
Name = Flare
GenericName=Chat
Name[fr]=Flare

[Desktop Action new-window]
Name=New Window
";
        assert_eq!(names_in(entry), ["Flare", "Flare (Signal)", "Flare"]);
    }

    #[rstest]
    #[case::path("../../etc/passwd")]
    #[case::absolute("/etc/passwd")]
    #[case::subdirectory("kde/org.kde.dolphin")]
    #[case::empty("")]
    fn an_id_that_is_no_file_name_names_no_entry(#[case] id: &str) {
        assert!(names(id).is_empty());
    }
}
