//! The certificate of Atuin's own that signs the helper on macOS, and the signed copy of the
//! helper that Atuin runs.
//!
//! macOS ties the Accessibility permission to the helper's code signature and to its path. A
//! copy of the helper at a fixed path, signed again with the same certificate after every
//! update, keeps the permission across updates.
//!
//! The certificate's key lives in a keychain of its own, and a password file next to the
//! keychain unlocks it, so Atuin signs without asking the user. Any program that runs as the
//! user can therefore sign itself as the helper and inherit the permission. Three measures
//! narrow that risk. The key cannot be exported. Only `codesign` may use the key without a
//! prompt. The hardened runtime keeps other code from loading into the signed helper.
//!
//! - [`certificate`]: the creation and the removal of the certificate, which `atuin setup` runs.
//! - [`copy`]: the signed copy of the helper, which the daemon keeps up to date.

mod certificate;
mod copy;

use std::io;
use std::path::{Path, PathBuf};
use std::process::Output;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use atuin_common::path::{AbsPathBuf, PathBufExt as _};
use atuin_common::string::TrimExt as _;
use parking_lot::Mutex;
use tokio::process::Command;

use self::copy::{ShippedHash, TempFile};
use super::HELPER_NAME;

const SECURITY: &str = "/usr/bin/security";
const CODESIGN: &str = "/usr/bin/codesign";

const KEYCHAIN: &str = "signing.keychain-db";
const PASSWORD: &str = "signing.password";
/// Holds the SHA-1 fingerprint of the certificate. Creation writes it last, so the file marks a
/// complete certificate.
const FINGERPRINT: &str = "signing.fingerprint";
/// Holds the stamp of the signed copy, which names what the copy was made from.
const STAMP: &str = "atuin-desktop-capture.stamp";
/// Holds the file name of the signed copy, which creation picks for the user. A certificate
/// without the file signs a copy named [`HELPER_NAME`].
const NAME: &str = "signing.name";
/// The scratch directory of certificate creation.
const SCRATCH: &str = "signing.tmp";

#[derive(Debug, thiserror::Error)]
pub enum SigningError {
    #[error("could not {action} {path}: {source}")]
    Io {
        action: IoAction,
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("could not run {program}: {source}")]
    Spawn {
        program: &'static str,
        #[source]
        source: io::Error,
    },
    #[error("`{program} {step}` failed: {message}")]
    Failed {
        program: &'static str,
        step: &'static str,
        message: String,
    },
    #[error("`security find-certificate` did not print a SHA-1 fingerprint")]
    NoFingerprint,
    #[error("{path} must hold one file name, but it holds {name:?}")]
    BadName {
        path: PathBuf,
        name: String,
    },
}

/// What a [`SigningError::Io`] failed to do with its file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, derive_more::Display)]
pub enum IoAction {
    #[display("read")]
    Read,
    #[display("write")]
    Write,
    #[display("create")]
    Create,
    #[display("remove")]
    Remove,
    /// Replace the signed copy with a newly signed file.
    #[display("replace")]
    Replace,
    /// Copy the shipped helper to the file.
    #[display("copy the helper to")]
    CopyTo,
}

impl SigningError {
    fn io(action: IoAction, path: impl Into<PathBuf>) -> impl FnOnce(io::Error) -> Self {
        move |source| Self::Io {
            action,
            path: path.into(),
            source,
        }
    }
}

/// The signing files in one directory: the certificate, and the signed copy of the helper.
#[derive(Debug, Clone)]
pub struct Signing {
    dir: PathBuf,
    /// The hash of the shipped helper that [`helper`](Self::helper) last read.
    shipped: Arc<Mutex<Option<ShippedHash>>>,
    /// Whether [`helper`](Self::helper) has removed the temporary files that the signings of
    /// ended processes left behind.
    swept: Arc<AtomicBool>,
}

impl Signing {
    #[must_use]
    pub fn in_dir(dir: PathBuf) -> Self {
        Self {
            dir,
            shipped: Arc::default(),
            swept: Arc::default(),
        }
    }

    /// The signing files of Atuin's home on macOS. `None` on other platforms, because they don't
    /// tie permissions to signatures.
    #[must_use]
    pub fn for_platform() -> Option<Self> {
        cfg!(target_os = "macos").then(|| {
            Self::in_dir(PathBuf::from(atuin_common::dirs::atuin_home()).joined("desktop-capture"))
        })
    }

    /// The keychain that holds the certificate.
    fn keychain(&self) -> PathBuf {
        self.dir.join(KEYCHAIN)
    }

    /// Where the signed copy of the helper lives.
    ///
    /// # Errors
    ///
    /// Fails when the name of the copy can't be read.
    pub async fn signed_helper(&self) -> Result<PathBuf, SigningError> {
        Ok(self.dir.join(self.helper_name().await?))
    }

    /// The file name of the signed copy, which System Settings shows in its list of the
    /// programs that may use the Accessibility API.
    ///
    /// # Errors
    ///
    /// Fails when the file that holds the name can't be read, or holds more than a file name.
    pub async fn helper_name(&self) -> Result<String, SigningError> {
        let path = self.dir.join(NAME);
        let mut name = match tokio::fs::read_to_string(&path).await {
            Ok(name) => name,
            Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(default_name()),
            Err(err) => return Err(SigningError::io(IoAction::Read, path)(err)),
        };
        name.trim_in_place();
        let is_file_name = Path::new(&name).file_name().is_some_and(|file| file == name.as_str());
        if !is_file_name || name.starts_with('.') {
            return Err(SigningError::BadName { path, name });
        }
        Ok(name)
    }

    /// Whether the certificate exists.
    pub async fn has_certificate(&self) -> bool {
        self.fingerprint().await.is_ok_and(|fingerprint| fingerprint.is_some())
    }

    async fn fingerprint(&self) -> Result<Option<String>, SigningError> {
        let path = self.dir.join(FINGERPRINT);
        match tokio::fs::read_to_string(&path).await {
            Ok(mut fingerprint) => {
                fingerprint.trim_in_place();
                Ok(Some(fingerprint))
            }
            Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(err) => Err(SigningError::io(IoAction::Read, path)(err)),
        }
    }

    /// Unlocks `keychain`, which macOS locks again when the session ends.
    ///
    /// The password goes to `security` as an argument. Another account's process could read
    /// the password from the list of processes, but that account cannot read the keychain in
    /// this directory. A process of this account can read the password file anyway.
    async fn unlock(&self, keychain: &Path) -> Result<(), SigningError> {
        let password = self.dir.join(PASSWORD);
        let password = tokio::fs::read_to_string(&password)
            .await
            .map_err(SigningError::io(IoAction::Read, password))?;
        run(SECURITY, "unlock-keychain", |security| {
            security.args(["unlock-keychain", "-p", password.trim()]).arg(keychain)
        })
        .await?;
        Ok(())
    }

    /// Adds `keychain` to the user's search list, unless the list has it.
    ///
    /// On macOS 14, `codesign --keychain` finds the certificate only in a keychain on the
    /// search list.
    async fn ensure_in_search_list(keychain: &Path) -> Result<(), SigningError> {
        let listed = run(SECURITY, "list-keychains", |security| {
            security.args(["list-keychains", "-d", "user"])
        })
        .await?;
        let listed = String::from_utf8_lossy(&listed.stdout);
        // `security` may list the path with its symbolic links resolved.
        let canonical =
            AbsPathBuf::canonicalize_async(keychain).await.ok().map(AbsPathBuf::into_path_buf);
        let is_ours = |path: &str| {
            Path::new(path) == keychain || canonical.as_deref() == Some(Path::new(path))
        };
        if search_list(&listed).any(is_ours) {
            return Ok(());
        }
        run(SECURITY, "list-keychains -s", |security| {
            security
                .args(["list-keychains", "-d", "user", "-s"])
                .args(search_list(&listed))
                .arg(keychain)
        })
        .await?;
        Ok(())
    }

    /// Removes every file of signing: the certificate with its keychain, the signed copy of the
    /// helper, and what an interrupted creation or signing left behind.
    ///
    /// The Accessibility permission belongs to the signed copy, so a copy that a later
    /// certificate signs needs a new grant.
    ///
    /// # Errors
    ///
    /// Fails when a file can't be removed, or when `security` can't delete the keychain.
    pub async fn remove(&self) -> Result<SigningFiles, SigningError> {
        let found = self.has_files().await;
        // The certificate's files hold the name of the copy, so the name comes first.
        let copy = self.signed_helper().await;
        self.delete_certificate().await?;
        let scratch = self.dir.join(SCRATCH);
        match tokio::fs::remove_dir_all(&scratch).await {
            Err(err) if err.kind() != io::ErrorKind::NotFound => {
                return Err(SigningError::io(IoAction::Remove, scratch)(err));
            }
            _ => {}
        }
        if let Ok(copy) = copy {
            remove_file(copy).await?;
        }
        remove_file(self.dir.join(default_name())).await?;
        remove_file(self.dir.join(STAMP)).await?;
        self.remove_temp_files().await?;
        Ok(if found {
            SigningFiles::Removed
        } else {
            SigningFiles::Absent
        })
    }

    /// Whether any file of signing exists.
    async fn has_files(&self) -> bool {
        let mut path = self.dir.clone();
        for name in [FINGERPRINT, KEYCHAIN, PASSWORD, NAME, SCRATCH, STAMP] {
            path.push(name);
            let exists = tokio::fs::try_exists(&path).await.unwrap_or(false);
            path.pop();
            if exists {
                return true;
            }
        }
        path.push(default_name());
        tokio::fs::try_exists(&path).await.unwrap_or(false) || self.has_temp_file().await
    }

    /// Whether the directory holds a temporary file that [`TempFile`] names. A killed signing
    /// leaves these files behind.
    async fn has_temp_file(&self) -> bool {
        let Ok(mut entries) = tokio::fs::read_dir(&self.dir).await else {
            return false;
        };
        while let Ok(Some(entry)) = entries.next_entry().await {
            if TempFile::is_named(&entry.file_name()) {
                return true;
            }
        }
        false
    }

    /// Removes the temporary files that [`TempFile`] names.
    async fn remove_temp_files(&self) -> Result<(), SigningError> {
        let Ok(mut entries) = tokio::fs::read_dir(&self.dir).await else {
            return Ok(());
        };
        while let Ok(Some(entry)) = entries.next_entry().await {
            if TempFile::is_named(&entry.file_name()) {
                remove_file(entry.path()).await?;
            }
        }
        Ok(())
    }
}

/// The name of a signed copy for a certificate that predates [`NAME`].
fn default_name() -> String {
    with_exe_extension(HELPER_NAME.to_owned())
}

/// `name` with the extension of executables on this platform, if it has one.
fn with_exe_extension(mut name: String) -> String {
    let extension = std::env::consts::EXE_EXTENSION;
    if !extension.is_empty() {
        name.push('.');
        name.push_str(extension);
    }
    name
}

/// What [`Signing::remove`] did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SigningFiles {
    Removed,
    /// There were no files to remove.
    Absent,
}

/// Removes the file at `path`, unless it is absent.
async fn remove_file(path: PathBuf) -> Result<(), SigningError> {
    match tokio::fs::remove_file(&path).await {
        Err(err) if err.kind() != io::ErrorKind::NotFound => {
            Err(SigningError::io(IoAction::Remove, path)(err))
        }
        _ => Ok(()),
    }
}

/// Runs `program` with the arguments that `configure` adds, and returns its output when it
/// succeeds.
///
/// Dropping the returned future kills the program.
async fn run(
    program: &'static str,
    step: &'static str,
    configure: impl FnOnce(&mut Command) -> &mut Command,
) -> Result<Output, SigningError> {
    let mut command = Command::new(program);
    configure(&mut command);
    let output = command
        .stdin(std::process::Stdio::null())
        .kill_on_drop(true)
        .output()
        .await
        .map_err(|source| SigningError::Spawn { program, source })?;
    if output.status.success() {
        return Ok(output);
    }
    let stderr = String::from_utf8_lossy(&output.stderr);
    let message = match stderr.trim() {
        "" => output.status.to_string(),
        stderr => stderr.to_owned(),
    };
    Err(SigningError::Failed {
        program,
        step,
        message,
    })
}

/// The keychains in the output of `security list-keychains`, one quoted path per line.
fn search_list(listed: &str) -> impl Iterator<Item = &str> {
    listed.lines().map(|line| line.trim().trim_matches('"')).filter(|path| !path.is_empty())
}

#[cfg(test)]
mod tests {
    use rstest::rstest;

    use super::*;

    #[rstest]
    fn reads_the_search_list() {
        let listed = "    \"/Users/a/Library/Keychains/login.keychain-db\"\n    \
                      \"/Users/a/.atuin/desktop-capture/signing.keychain-db\"\n";
        assert_eq!(search_list(listed).collect::<Vec<_>>(), [
            "/Users/a/Library/Keychains/login.keychain-db",
            "/Users/a/.atuin/desktop-capture/signing.keychain-db",
        ]);
    }

    /// Removal takes every file of signing, the leftovers of an interrupted run included, and
    /// leaves the other files of the directory alone.
    #[rstest]
    #[tokio::test]
    async fn removal_leaves_only_the_files_of_others() {
        let dir = tempfile::tempdir().expect("tempdir");
        let signing = Signing::in_dir(dir.path().to_owned());
        for name in [FINGERPRINT, PASSWORD, STAMP, "stopped"] {
            std::fs::write(dir.path().join(name), b"x").expect("write");
        }
        std::fs::write(signing.signed_helper().await.unwrap(), b"helper").expect("write");
        std::fs::create_dir(dir.path().join(SCRATCH)).expect("mkdir");
        std::fs::write(dir.path().join(SCRATCH).joined("key.pem"), b"key").expect("write");
        // A killed signing leaves its temporary file behind.
        std::fs::write(dir.path().join(".atuin-desktop-capture.99.0.tmp"), b"copy").expect("write");

        let first = signing.remove().await.expect("removal");
        let second = signing.remove().await.expect("removal");

        let left: Vec<_> = std::fs::read_dir(dir.path())
            .expect("read_dir")
            .map(|entry| entry.expect("entry").file_name())
            .collect();
        assert_eq!(left, ["stopped"]);
        assert_eq!((first, second), (SigningFiles::Removed, SigningFiles::Absent));
    }

    /// Any one file of signing counts as found, a temporary file of a killed signing included.
    #[rstest]
    #[case::fingerprint(FINGERPRINT)]
    #[case::keychain(KEYCHAIN)]
    #[case::copy("atuin-desktop-capture")]
    #[case::temporary(".atuin-desktop-capture.99.0.tmp")]
    #[tokio::test]
    async fn removal_finds_each_file_of_signing(#[case] name: &str) {
        let dir = tempfile::tempdir().expect("tempdir");
        let signing = Signing::in_dir(dir.path().to_owned());
        std::fs::write(dir.path().join(name), b"x").expect("write");

        assert!(signing.has_files().await);
    }

    /// A certificate from before the stored name signs a copy with the plain name, and a stored
    /// name holds exactly one file name.
    #[rstest]
    #[case::absent(None, Ok("atuin-desktop-capture"))]
    #[case::stored(Some("atuin-desktop-capture (dev1)\n"), Ok("atuin-desktop-capture (dev1)"))]
    #[case::dotted(Some("atuin-desktop-capture (j.doe)"), Ok("atuin-desktop-capture (j.doe)"))]
    #[case::path(Some("../atuin-desktop-capture"), Err(()))]
    #[case::hidden(Some(".atuin-desktop-capture"), Err(()))]
    #[case::empty(Some(""), Err(()))]
    #[tokio::test]
    async fn reads_the_name_of_the_signed_copy(
        #[case] stored: Option<&str>,
        #[case] expected: Result<&str, ()>,
    ) {
        let dir = tempfile::tempdir().expect("tempdir");
        if let Some(stored) = stored {
            std::fs::write(dir.path().join(NAME), stored).expect("write");
        }
        let signing = Signing::in_dir(dir.path().to_owned());

        let name = signing.helper_name().await;

        assert_eq!(name.as_deref().map_err(|_| ()), expected);
    }

    /// Removal takes the copy under its stored name, together with the name.
    #[rstest]
    #[tokio::test]
    async fn removal_takes_the_named_copy() {
        let dir = tempfile::tempdir().expect("tempdir");
        let signing = Signing::in_dir(dir.path().to_owned());
        std::fs::write(dir.path().join(NAME), "atuin-desktop-capture (dev1)").expect("write");
        std::fs::write(dir.path().join("atuin-desktop-capture (dev1)"), b"copy").expect("write");

        let removed = signing.remove().await.expect("removal");

        assert_eq!(removed, SigningFiles::Removed);
        assert_eq!(std::fs::read_dir(dir.path()).expect("read_dir").count(), 0);
    }
}
