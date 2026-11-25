//! The directories that setup created for its login files, such as `~/.config/autostart`.
//! Removal takes out the ones that are empty once setup's files are gone.

use std::io;
use std::path::{Path, PathBuf};

use atuin_common::path::PathBufExt as _;

use super::SessionError;

/// The file in Atuin's `desktop-capture` directory that names each directory that setup created,
/// one per line.
const RECORD: &str = "login-directories";

/// Creates `dir` and its missing parents, as a shell of the user creates a directory, and notes
/// each directory that it created.
pub fn create(dir: &Path) -> Result<(), SessionError> {
    let created = create_dirs(dir).map_err(|source| SessionError::Write {
        path: dir.to_path_buf(),
        source,
    })?;
    if created.is_empty() {
        return Ok(());
    }
    let record = record_path()?;
    note(&record, &created).map_err(|source| SessionError::Write {
        path: record,
        source,
    })
}

/// Removes each directory that [`create`] created and that is empty now, innermost first, and
/// forgets every directory that it noted. A directory that holds anything stays.
pub fn remove_empty() -> Result<(), SessionError> {
    let record = record_path()?;
    let content = match std::fs::read_to_string(&record) {
        Ok(content) => content,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(source) => {
            return Err(SessionError::Read {
                path: record,
                source,
            });
        }
    };
    let mut dirs: Vec<&Path> =
        content.lines().filter(|line| !line.is_empty()).map(Path::new).collect();
    // A deeper directory has to go before its parent can be empty.
    dirs.sort_by_key(|dir| std::cmp::Reverse(dir.components().count()));
    for dir in dirs {
        // A directory that isn't empty, or that is gone already, fails to go, and stays.
        let _ = std::fs::remove_dir(dir);
    }
    match std::fs::remove_file(&record) {
        Err(source) if source.kind() != io::ErrorKind::NotFound => Err(SessionError::Write {
            path: record,
            source,
        }),
        _ => Ok(()),
    }
}

fn record_path() -> Result<PathBuf, SessionError> {
    let home = atuin_common::dirs::try_atuin_home()?;
    Ok(PathBuf::from(home).joined("desktop-capture").joined(RECORD))
}

/// Appends `dirs` to the record, which only the user can read. A directory whose path isn't
/// UTF-8 stays out of the record, so removal leaves that directory.
fn note(record: &Path, dirs: &[PathBuf]) -> io::Result<()> {
    use std::io::Write as _;

    if let Some(parent) = record.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut file = std::fs::OpenOptions::new().create(true).append(true).open(record)?;
    for dir in dirs.iter().filter_map(|dir| dir.to_str()) {
        writeln!(file, "{dir}")?;
    }
    Ok(())
}

/// Creates the directories with the permissions that the user's umask leaves, because Atuin
/// itself runs with a umask that leaves only the owner any access.
#[cfg(unix)]
fn create_dirs(dir: &Path) -> io::Result<Vec<PathBuf>> {
    use atuin_common::os::umask::Umask;

    /// The umask of a process that didn't install one of its own, such as a test.
    const USUAL_UMASK: u32 = 0o022;
    let umask = Umask::original().map_or(USUAL_UMASK, Umask::bits);
    atuin_common::fs::create_dir_all_with_mode(dir, 0o777 & !umask)
}

/// Creates the directories without noting them, because desktop capture runs only on Unix.
#[cfg(not(unix))]
fn create_dirs(dir: &Path) -> io::Result<Vec<PathBuf>> {
    std::fs::create_dir_all(dir).map(|()| Vec::new())
}
