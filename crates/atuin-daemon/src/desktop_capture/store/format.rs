//! The format of the stored data, which each database notes in `meta`.
//!
//! A daemon reads only the format that it writes, because it can't know what the records of a
//! different format mean.

use super::keyspaces::Keyspaces;
use super::model::StoreError;

/// The format that this daemon reads and writes.
pub const FORMAT: u32 = 1;

/// Notes [`FORMAT`] in a new database, and fails on a database of a different format.
pub fn check(keyspaces: &Keyspaces) -> Result<(), StoreError> {
    let format = keyspaces.read().format()?;
    match format {
        Some(FORMAT) => Ok(()),
        Some(found) => Err(StoreError::UnknownFormat { found }),
        None => {
            let mut writer = keyspaces.write()?;
            writer.set_format(FORMAT);
            writer.commit()
        }
    }
}

#[cfg(test)]
mod tests {
    use std::time::SystemTime;

    use rstest::rstest;
    use tempfile::TempDir;

    use super::FORMAT;
    use crate::desktop_capture::store::{DesktopStore, StoreError};
    use crate::desktop_capture::testing::records::{ended, everything, started, written};
    use crate::desktop_capture::testing::{HOST, dir, store_in};

    /// A new store notes its format, and opens again with its records.
    #[rstest]
    #[tokio::test]
    async fn a_new_store_opens_again(dir: TempDir) {
        let store = store_in(&dir).await;
        assert_eq!(store.format(), Some(FORMAT));
        let _ = written(&store, [
            started(1, "gedit", "Log", SystemTime::now()),
            ended(1, SystemTime::now()),
        ])
        .await;
        store.close().await.expect("close");

        for _ in 0..2 {
            let store = store_in(&dir).await;
            assert_eq!(store.format(), Some(FORMAT));
            assert_eq!(store.segments(everything()).await.expect("segments").len(), 1);
            store.close().await.expect("close");
        }
    }

    /// A store of a different format, or of a malformed one, fails every open.
    #[rstest]
    #[case::newer(&(FORMAT + 1).to_be_bytes(), "format 2")]
    #[case::older(&0_u32.to_be_bytes(), "format 0")]
    #[case::malformed(b"not a format", "malformed")]
    #[tokio::test]
    async fn a_store_of_a_different_format_fails_to_open(
        dir: TempDir,
        #[case] format: &[u8],
        #[case] message: &str,
    ) {
        let store = store_in(&dir).await;
        store.put_raw_format(format);
        store.close().await.expect("close");

        for _ in 0..2 {
            let opened = DesktopStore::open(dir.path().join("store"), HOST).await;

            let Err(err) = opened else {
                panic!("the store opened");
            };
            assert!(
                matches!(err, StoreError::UnknownFormat { .. } | StoreError::Corrupt(_)),
                "{err:?}"
            );
            assert!(err.to_string().contains(message), "{err}");
        }
    }
}
