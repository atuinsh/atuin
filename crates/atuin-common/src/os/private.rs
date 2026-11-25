//! Files and directories that only the current user can open.
//!
//! On Unix, these set the mode when they create the file or directory. Other platforms get the
//! default permissions.

use std::io;
use std::path::Path;

/// Creates `dir` and its missing parents, each with mode `0700`.
///
/// A directory that exists already keeps its mode.
///
/// # Errors
///
/// Fails when a directory cannot be created.
pub async fn create_dir_all(dir: impl AsRef<Path>) -> io::Result<()> {
    let mut builder = tokio::fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    builder.mode(0o700);
    builder.create(dir).await
}

/// Creates `dir` and its missing parents as [`create_dir_all`] does, and gives `dir` mode `0700`
/// when it exists already.
///
/// The parents that exist already keep their modes.
///
/// # Errors
///
/// Fails when a directory cannot be created, or when the mode of `dir` cannot be set.
pub async fn ensure_private_dir(dir: impl AsRef<Path>) -> io::Result<()> {
    let dir = dir.as_ref();
    create_dir_all(dir).await?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;

        tokio::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700)).await?;
    }
    Ok(())
}

/// Writes `contents` to the file at `path`, which this creates with mode `0600`, or truncates.
///
/// A file that exists already keeps its mode.
///
/// # Errors
///
/// Fails when the file cannot be opened or written.
pub async fn write(path: impl AsRef<Path>, contents: impl AsRef<[u8]>) -> io::Result<()> {
    use tokio::io::AsyncWriteExt as _;

    let mut options = tokio::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    options.mode(0o600);
    let mut file = options.open(path).await?;
    file.write_all(contents.as_ref()).await?;
    file.flush().await
}

#[cfg(all(test, unix))]
mod tests {
    use std::os::unix::fs::PermissionsExt as _;

    use rstest::rstest;

    use super::*;
    use crate::path::PathBufExt as _;

    fn mode(path: &Path) -> u32 {
        std::fs::metadata(path).expect("metadata").permissions().mode() & 0o777
    }

    #[rstest]
    #[tokio::test]
    async fn creates_private_directories_and_files() {
        let root = tempfile::tempdir().expect("tempdir");
        let dir = root.path().join("a").joined("b");
        let file = dir.join("secret");

        create_dir_all(&dir).await.expect("create");
        write(&file, "one").await.expect("write");
        write(&file, "two").await.expect("write again");

        assert_eq!(mode(&root.path().join("a")), 0o700);
        assert_eq!(mode(&dir), 0o700);
        assert_eq!(mode(&file), 0o600);
        assert_eq!(std::fs::read_to_string(&file).expect("read"), "two");
    }

    #[rstest]
    #[case::new(None)]
    #[case::open_to_everyone(Some(0o755))]
    #[case::private(Some(0o700))]
    #[tokio::test]
    async fn ensure_private_dir_makes_the_directory_private(#[case] existing: Option<u32>) {
        let root = tempfile::tempdir().expect("tempdir");
        let dir = root.path().join("store");
        if let Some(existing) = existing {
            std::fs::create_dir(&dir).expect("create");
            std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(existing))
                .expect("chmod");
        }

        ensure_private_dir(&dir).await.expect("ensure");

        assert_eq!(mode(&dir), 0o700);
    }
}
