//! The `toolkit-accessibility` key in `org.gnome.desktop.interface`, which at-spi2-core reads on
//! every desktop.
//!
//! On at-spi2-core 2.60 and earlier, the key is the only switch that makes Qt applications and
//! Firefox readable.

use std::io;
use std::path::{Path, PathBuf};
use std::process::Output;

use atuin_common::path::PathBufExt as _;
use thiserror::Error;
use tokio::process::Command;

use crate::i18n::fl;

const SCHEMA: &str = "org.gnome.desktop.interface";
const KEY: &str = "toolkit-accessibility";

/// What `gsettings` prints when it falls back to keeping settings in memory, which lasts only as
/// long as the process.
const MEMORY_BACKEND: &str = "'memory' GSettings backend";

/// The state of the key.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Toolkit {
    /// The machine has no `gsettings`, or no schema with the key.
    Absent,
    /// `GSettings` doesn't have a dconf backend, so a value lasts only as long as the `gsettings`
    /// process.
    Unsaved,
    On,
    Off,
}

#[derive(Debug, Error)]
pub enum ToolkitError {
    #[error("{}", fl!("desktop-capture-toolkit-run-failed"))]
    Run(#[source] io::Error),

    #[error("{}", fl!("desktop-capture-toolkit-no-home"))]
    Home(#[source] atuin_common::dirs::HomeError),

    #[error("{}", fl!("desktop-capture-toolkit-set-failed", error = stderr.as_str()))]
    Set {
        stderr: String,
    },

    #[error("{}", fl!("desktop-capture-toolkit-record-failed", path = path.display().to_string()))]
    Record {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
}

/// Reads the key.
pub async fn read() -> Toolkit {
    if atuin_common::env::var_os("GSETTINGS_BACKEND").is_some_and(|backend| backend == "memory") {
        return Toolkit::Unsaved;
    }
    let Ok(output) = gsettings(&["get", SCHEMA, KEY]).await else {
        return Toolkit::Absent;
    };
    if String::from_utf8_lossy(&output.stderr).contains(MEMORY_BACKEND) {
        return Toolkit::Unsaved;
    }
    if !output.status.success() {
        return Toolkit::Absent;
    }
    match output.stdout.trim_ascii() {
        b"true" => Toolkit::On,
        b"false" => Toolkit::Off,
        _ => Toolkit::Absent,
    }
}

/// Sets the key to `true`. The first call keeps the old value, so that [`restore`] can put it
/// back.
pub async fn turn_on() -> Result<(), ToolkitError> {
    keep_off(&record_path()?)?;
    set("true").await
}

/// Puts back the value that [`turn_on`] kept, and forgets it.
pub async fn restore() -> Result<Restored, ToolkitError> {
    restore_from(&record_path()?).await
}

/// The file that keeps the value of the key from before setup turned it on.
///
/// The file lives in Atuin's `desktop-capture` directory, which also holds the signing
/// certificate on macOS. It stays outside the store, because the store's directory tells the MCP
/// server that capture has run.
fn record_path() -> Result<PathBuf, ToolkitError> {
    let home = atuin_common::dirs::try_atuin_home().map_err(ToolkitError::Home)?;
    Ok(PathBuf::from(home).joined("desktop-capture").joined("toolkit-accessibility-before"))
}

/// Records in `record` that the key was off, unless `record` exists.
fn keep_off(record: &Path) -> Result<(), ToolkitError> {
    if record.exists() {
        return Ok(());
    }
    let written = record
        .parent()
        .map_or(Ok(()), std::fs::create_dir_all)
        .and_then(|()| std::fs::write(record, "false\n"));
    written.map_err(|source| ToolkitError::Record {
        path: record.to_path_buf(),
        source,
    })
}

/// Puts back the value that `record` holds, and removes `record`.
async fn restore_from(record: &Path) -> Result<Restored, ToolkitError> {
    let restored = match std::fs::read_to_string(record) {
        Ok(value) if value.trim() == "false" => {
            set("false").await?;
            Restored::Off
        }
        Ok(_) => Restored::Nothing,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(Restored::Nothing),
        Err(source) => {
            return Err(ToolkitError::Record {
                path: record.to_path_buf(),
                source,
            });
        }
    };
    std::fs::remove_file(record).map_err(|source| ToolkitError::Record {
        path: record.to_path_buf(),
        source,
    })?;
    Ok(restored)
}

/// What [`restore`] did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Restored {
    /// Setup had not changed the key.
    Nothing,
    /// The key is `false` again.
    Off,
}

async fn set(value: &str) -> Result<(), ToolkitError> {
    let output = gsettings(&["set", SCHEMA, KEY, value]).await.map_err(ToolkitError::Run)?;
    if output.status.success() {
        return Ok(());
    }
    Err(ToolkitError::Set {
        stderr: String::from_utf8_lossy(&output.stderr).trim().to_owned(),
    })
}

async fn gsettings(args: &[&str]) -> io::Result<Output> {
    Command::new("gsettings").args(args).stdin(std::process::Stdio::null()).output().await
}

#[cfg(test)]
mod tests {
    use atuin_common::env::MockEnv;
    use rstest::rstest;

    use super::*;

    /// With the memory backend, a value set would be gone when `gsettings` exits.
    #[rstest]
    #[tokio::test]
    #[expect(clippy::future_not_send, reason = "the mock environment belongs to this thread")]
    async fn the_memory_backend_keeps_nothing() {
        let env = MockEnv::install();
        env.set("GSETTINGS_BACKEND", "memory");

        assert_eq!(read().await, Toolkit::Unsaved);
    }

    #[rstest]
    #[tokio::test]
    async fn restoring_without_a_record_changes_nothing() {
        let dir = tempfile::tempdir().unwrap();

        let restored =
            restore_from(&dir.path().join("toolkit-accessibility-before")).await.unwrap();

        assert_eq!(restored, Restored::Nothing);
    }

    /// The first record stays, so that a second setup keeps the value from before the first
    /// setup.
    #[rstest]
    fn the_first_record_stays() {
        let dir = tempfile::tempdir().unwrap();
        let record = dir.path().join("desktop-capture/toolkit-accessibility-before");
        keep_off(&record).unwrap();
        assert_eq!(std::fs::read_to_string(&record).unwrap(), "false\n");

        std::fs::write(&record, "true\n").unwrap();
        keep_off(&record).unwrap();

        assert_eq!(std::fs::read_to_string(&record).unwrap(), "true\n");
    }

    /// A record of `true` means the key was on before setup, so restoring only drops the record.
    #[rstest]
    #[tokio::test]
    async fn a_record_of_true_is_dropped_without_a_change() {
        let dir = tempfile::tempdir().unwrap();
        let record = dir.path().join("toolkit-accessibility-before");
        std::fs::write(&record, "true\n").unwrap();

        let restored = restore_from(&record).await.unwrap();

        assert_eq!(restored, Restored::Nothing);
        assert!(!record.exists());
    }
}
