//! The signed copy of the helper, which the daemon signs again when the shipped helper or the
//! certificate changes.

use std::ffi::OsStr;
use std::fmt::{self, Write as _};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::SystemTime;

use tracing::{info, warn};

use super::{
    CODESIGN, HELPER_NAME, IoAction, STAMP, Signing, SigningError, default_name, run,
    with_exe_extension,
};

/// The identifier that the signature gives the helper.
const IDENTIFIER: &str = "sh.atuin.desktop-capture";

/// The end of the name of a [`TempFile`].
const TEMP_SUFFIX: &str = ".tmp";

/// The hash of the shipped helper, and the file's state when it was read.
#[derive(Debug, Clone)]
pub struct ShippedHash {
    path: PathBuf,
    len: u64,
    modified: SystemTime,
    hash: blake3::Hash,
}

impl Signing {
    /// Returns the helper to run in place of `shipped`.
    ///
    /// Without a certificate, that is `shipped` itself. With a certificate, it is the signed
    /// copy. The call first signs the copy again when `shipped` or the certificate has changed
    /// since the last signing.
    ///
    /// # Errors
    ///
    /// Fails when `shipped` cannot be read, or when signing fails.
    pub async fn helper<P>(&self, shipped: P) -> Result<PathBuf, SigningError>
    where
        P: AsRef<Path> + Into<PathBuf>,
    {
        let Some(certificate) = self.fingerprint().await? else {
            return Ok(shipped.into());
        };
        if !self.swept.swap(true, Ordering::Relaxed) {
            self.remove_abandoned_temp_files().await;
        }
        let shipped = shipped.as_ref();
        let stamp = Stamp {
            source: self.hash_shipped(shipped).await?,
            certificate: &certificate,
        };
        let copy = self.signed_helper().await?;
        let stamp_path = self.dir.join(STAMP);
        let current =
            tokio::fs::read_to_string(&stamp_path).await.is_ok_and(|stored| stamp.matches(&stored))
                && tokio::fs::try_exists(&copy).await.unwrap_or(false);
        if current {
            return Ok(copy);
        }
        let stamp = Stamp {
            source: self.sign_copy(shipped, &copy, &certificate).await?,
            certificate: &certificate,
        };
        tokio::fs::write(&stamp_path, stamp.to_string())
            .await
            .map_err(SigningError::io(IoAction::Write, stamp_path))?;
        info!(from = %shipped.display(), to = %copy.display(), "signed a new copy of the helper");
        Ok(copy)
    }

    /// Removes the temporary files of the processes that have ended, such as a daemon that was
    /// killed while the daemon was copying the helper. A failure to remove a file is logged.
    ///
    /// The files of a process that is still running stay, because that process can still be
    /// signing.
    async fn remove_abandoned_temp_files(&self) {
        let Ok(mut entries) = tokio::fs::read_dir(&self.dir).await else {
            return;
        };
        while let Ok(Some(entry)) = entries.next_entry().await {
            let abandoned = TempFile::creator(&entry.file_name())
                .is_some_and(|pid| pid != std::process::id() && !process_runs(pid));
            if abandoned && let Err(err) = tokio::fs::remove_file(entry.path()).await {
                warn!(%err, path = %entry.path().display(), "could not remove a temporary file of signing");
            }
        }
    }

    /// The hash of the helper at `shipped`. The file is read again only when its length or its
    /// modification time has changed since the last read.
    async fn hash_shipped(&self, shipped: &Path) -> Result<blake3::Hash, SigningError> {
        // The metadata comes before the hash, so that a file that changes in between is hashed
        // again on the next call.
        let metadata = tokio::fs::metadata(shipped)
            .await
            .map_err(SigningError::io(IoAction::Read, shipped))?;
        let len = metadata.len();
        let modified = metadata.modified().map_err(SigningError::io(IoAction::Read, shipped))?;
        if let Some(known) = &*self.shipped.lock()
            && known.path == shipped
            && known.len == len
            && known.modified == modified
        {
            return Ok(known.hash);
        }
        let path = shipped.to_owned();
        let known = tokio::task::spawn_blocking(move || {
            let hash = hash_file(&path)?;
            Ok::<_, SigningError>(ShippedHash {
                path,
                len,
                modified,
                hash,
            })
        })
        .await
        .expect("a hashing task panicked")?;
        let hash = known.hash;
        *self.shipped.lock() = Some(known);
        Ok(hash)
    }

    /// Copies `shipped` to a temporary file, signs the file, and moves it to `copy`. Returns the
    /// hash of the content that was copied.
    ///
    /// The move replaces `copy` at once, because macOS kills a signed program whose file changes
    /// while the program is running.
    async fn sign_copy(
        &self,
        shipped: &Path,
        copy: &Path,
        certificate: &str,
    ) -> Result<blake3::Hash, SigningError> {
        let temp = TempFile::new(&self.dir);
        let shipped = shipped.to_owned();
        // The blocking copy owns the temporary file until the copy ends, so that a signing that
        // is abandoned during the copy still removes the file.
        let Copied { temp, hash } = tokio::task::spawn_blocking(move || copy_to(&shipped, temp))
            .await
            .expect("a copying task panicked")?;
        self.sign(&temp.0, certificate).await?;
        tokio::fs::rename(&temp.0, copy)
            .await
            .map_err(SigningError::io(IoAction::Replace, copy))?;
        Ok(hash)
    }

    /// Signs `program` with the certificate whose fingerprint is `certificate`.
    async fn sign(&self, program: &Path, certificate: &str) -> Result<(), SigningError> {
        let keychain = self.keychain();
        self.unlock(&keychain).await?;
        Self::ensure_in_search_list(&keychain).await?;
        run(CODESIGN, "--sign", |codesign| {
            codesign
                .args(["--force", "--sign", certificate, "--identifier", IDENTIFIER])
                .args(["--options", "runtime", "--keychain"])
                .arg(&keychain)
                .arg(program)
        })
        .await?;
        Ok(())
    }
}

/// The temporary file that holds a copy of the shipped helper, and the hash of that copy.
struct Copied {
    temp: TempFile,
    hash: blake3::Hash,
}

/// Copies `shipped` to `temp`, and hashes the copy.
///
/// The hash of the copy describes the content that is signed, even when `shipped` changes during
/// the copy.
fn copy_to(shipped: &Path, temp: TempFile) -> Result<Copied, SigningError> {
    std::fs::copy(shipped, &temp.0).map_err(SigningError::io(IoAction::CopyTo, &temp.0))?;
    let hash = hash_file(&temp.0)?;
    Ok(Copied { temp, hash })
}

/// The BLAKE3 hash of the file at `path`. The call blocks while it reads the file.
fn hash_file(path: &Path) -> Result<blake3::Hash, SigningError> {
    let mut hasher = blake3::Hasher::new();
    let hashed = std::fs::File::open(path).and_then(|file| hasher.update_reader(file));
    hashed.map_err(SigningError::io(IoAction::Read, path))?;
    Ok(hasher.finalize())
}

/// The name of the signed copy for the user of this process, such as
/// `atuin-desktop-capture (dev1)`.
///
/// System Settings lists one copy for each account that turned capture on, and the name shows each
/// user which copy belongs to that user.
pub fn user_helper_name() -> String {
    let user = atuin_common::env::var_os("USER").and_then(|user| user.into_string().ok());
    let fits = |user: &String| {
        !user.is_empty() && !user.contains('/') && !user.chars().any(char::is_control)
    };
    match user.filter(fits) {
        Some(user) => with_exe_extension(format!("{HELPER_NAME} ({user})")),
        None => default_name(),
    }
}

/// What the signed copy was made from. A copy whose stamp differs is out of date.
struct Stamp<'a> {
    /// The BLAKE3 hash of the shipped helper.
    source: blake3::Hash,
    /// The fingerprint of the certificate.
    certificate: &'a str,
}

impl Stamp<'_> {
    /// Whether `stored`, the content of a stamp file, is this stamp.
    fn matches(&self, stored: &str) -> bool {
        stored.strip_suffix('\n').and_then(|stored| stored.split_once(' '))
            == Some((self.source.to_hex().as_str(), self.certificate))
    }
}

impl fmt::Display for Stamp<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(f, "{} {}", self.source, self.certificate)
    }
}

/// A temporary file in the signing directory, which is removed when the `TempFile` drops.
///
/// Each one has a name of its own, so that the copy of an abandoned attempt, which can still
/// be running on a blocking thread, never writes to the file of a later attempt.
pub struct TempFile(PathBuf);

impl TempFile {
    fn new(dir: &Path) -> Self {
        static ATTEMPTS: AtomicU64 = AtomicU64::new(0);
        let attempt = ATTEMPTS.fetch_add(1, Ordering::Relaxed);
        // Pushing "." adds the separator and the dot that starts the name, and the rest of the
        // name follows the dot.
        let mut path = dir.to_owned();
        path.push(".");
        let name = path.as_mut_os_string();
        let _ = write!(name, "{HELPER_NAME}.{}.{attempt}{TEMP_SUFFIX}", std::process::id());
        Self(path)
    }

    /// The id of the process that created the temporary file `name`. `None` when `name` isn't
    /// the name of a temporary file of this form.
    fn creator(name: &OsStr) -> Option<u32> {
        let rest = name.to_str()?.strip_prefix('.')?.strip_prefix(HELPER_NAME)?;
        let (pid, _attempt) = rest.strip_prefix('.')?.strip_suffix(TEMP_SUFFIX)?.split_once('.')?;
        pid.parse().ok()
    }

    /// Whether `name` is the file name of a temporary file.
    pub fn is_named(name: &OsStr) -> bool {
        name.to_str().and_then(|name| name.strip_prefix('.')).is_some_and(|name| {
            name.strip_prefix(HELPER_NAME)
                .is_some_and(|rest| rest.starts_with('.') && rest.ends_with(TEMP_SUFFIX))
        })
    }
}

impl Drop for TempFile {
    fn drop(&mut self) {
        // After a successful signing, the file was moved to the copy already.
        let _ = std::fs::remove_file(&self.0);
    }
}

/// Whether the process `pid` is running.
#[cfg(unix)]
fn process_runs(pid: u32) -> bool {
    atuin_common::os::unix::process::is_u32_alive(pid)
}

/// Whether the process `pid` is running. Only macOS signs the helper, so other platforms keep every
/// file.
#[cfg(not(unix))]
fn process_runs(_pid: u32) -> bool {
    true
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use rstest::rstest;

    use super::super::FINGERPRINT;
    use super::*;

    #[rstest]
    #[case::user("dev1", "atuin-desktop-capture (dev1)")]
    #[case::dotted("j.doe", "atuin-desktop-capture (j.doe)")]
    #[case::slash("a/b", "atuin-desktop-capture")]
    #[case::empty("", "atuin-desktop-capture")]
    fn names_the_copy_after_the_user(#[case] user: &str, #[case] expected: &str) {
        let env = atuin_common::env::MockEnv::install();
        env.set("USER", user);
        assert_eq!(user_helper_name(), with_exe_extension(expected.to_owned()));
    }

    #[rstest]
    #[case::temp(".atuin-desktop-capture.12.0.tmp", true)]
    #[case::helper("atuin-desktop-capture", false)]
    #[case::stamp("atuin-desktop-capture.stamp", false)]
    #[case::other_program(".atuin-desktop-capturex.1.tmp", false)]
    fn knows_the_names_of_temporary_files(#[case] name: &str, #[case] temp: bool) {
        assert_eq!(TempFile::is_named(OsStr::new(name)), temp);
    }

    #[rstest]
    #[case::temp(".atuin-desktop-capture.12.0.tmp", Some(12))]
    #[case::malformed_pid(".atuin-desktop-capture.x.0.tmp", None)]
    #[case::no_attempt(".atuin-desktop-capture.12.tmp", None)]
    #[case::helper("atuin-desktop-capture", None)]
    fn reads_the_creator_of_a_temporary_file(#[case] name: &str, #[case] pid: Option<u32>) {
        assert_eq!(TempFile::creator(OsStr::new(name)), pid);
    }

    /// The first helper that a signing returns removes the temporary files of the processes
    /// that have ended, and keeps those of the processes that are still running.
    #[rstest]
    #[tokio::test]
    async fn the_first_helper_removes_the_temporary_files_of_ended_processes() {
        let dir = tempfile::tempdir().unwrap();
        let signing = Signing::in_dir(dir.path().join("signing"));
        std::fs::create_dir(&signing.dir).unwrap();
        std::fs::write(signing.dir.join(FINGERPRINT), "CERT\n").unwrap();
        let shipped = dir.path().join("helper");
        std::fs::write(&shipped, "one").unwrap();
        let stamp = Stamp {
            source: blake3::hash(b"one"),
            certificate: "CERT",
        };
        std::fs::write(signing.dir.join(STAMP), stamp.to_string()).unwrap();
        std::fs::write(signing.signed_helper().await.unwrap(), "signed").unwrap();
        let mut ended = std::process::Command::new("true").spawn().unwrap();
        ended.wait().unwrap();
        let temp_of = |pid: u32| signing.dir.join(format!(".{HELPER_NAME}.{pid}.0.tmp"));
        let [abandoned, own, running] = [ended.id(), std::process::id(), 1].map(temp_of);
        for temp in [&abandoned, &own, &running] {
            std::fs::write(temp, "copy").unwrap();
        }

        signing.helper(shipped.as_path()).await.unwrap();

        assert!(!abandoned.exists());
        assert!(own.exists());
        assert!(running.exists());
    }

    /// Each attempt gets a hidden file of its own in the directory, which is gone once the
    /// attempt drops it.
    #[rstest]
    fn each_attempt_has_a_temporary_file_of_its_own() {
        let dir = tempfile::tempdir().expect("tempdir");
        let first = TempFile::new(dir.path());
        let second = TempFile::new(dir.path());
        std::fs::write(&first.0, b"copy").expect("write");

        assert_ne!(first.0, second.0);
        for temp in [&first, &second] {
            assert_eq!(temp.0.parent(), Some(dir.path()));
            let name = temp.0.file_name().expect("a name").to_string_lossy();
            assert!(name.starts_with(".atuin-desktop-capture."), "{name}");
            assert!(name.ends_with(".tmp"), "{name}");
        }
        let path = first.0.clone();
        drop(first);
        assert!(!path.exists());
    }

    /// The temporary files in `dir`.
    fn temp_files(dir: &Path) -> Vec<PathBuf> {
        std::fs::read_dir(dir)
            .expect("read_dir")
            .map(|entry| entry.expect("entry").path())
            .filter(|path| TempFile::is_named(path.file_name().expect("a name")))
            .collect()
    }

    /// A signing that is abandoned before its copy of the shipped helper begins still removes
    /// the temporary file that the copy creates.
    ///
    /// The runtime has one blocking thread, which a task holds until the signing is abandoned,
    /// so that the copy runs after the signing is dropped.
    #[rstest]
    fn an_abandoned_copy_leaves_no_temporary_file() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .max_blocking_threads(1)
            .build()
            .expect("a runtime");
        let dir = tempfile::tempdir().expect("tempdir");
        let signing = Signing::in_dir(dir.path().to_owned());
        let shipped = dir.path().join("shipped");
        std::fs::write(&shipped, b"helper").expect("write");
        let copy = dir.path().join("copy");

        runtime.block_on(async {
            let (release, held) = std::sync::mpsc::channel::<()>();
            let holder = tokio::task::spawn_blocking(move || held.recv());
            let mut signed = Box::pin(signing.sign_copy(&shipped, &copy, "CERT"));
            assert!(futures::poll!(&mut signed).is_pending());
            drop(signed);
            release.send(()).expect("release the blocking thread");
            holder.await.expect("the holder").expect("the release");
            // The blocking thread runs its tasks in order, so the copy has ended once this runs.
            tokio::task::spawn_blocking(|| ()).await.expect("the last task");
        });

        assert_eq!(temp_files(dir.path()), Vec::<PathBuf>::new());
        assert!(!copy.exists());
    }

    /// Without a certificate, the helper runs from where it was shipped, and the helper isn't
    /// copied.
    #[rstest]
    #[tokio::test]
    async fn runs_the_shipped_helper_without_a_certificate() {
        let dir = tempfile::tempdir().unwrap();
        let signing = Signing::in_dir(dir.path().join("signing"));
        let shipped = dir.path().join("helper");

        assert_eq!(signing.helper(shipped.as_path()).await.unwrap(), shipped);
        assert!(!signing.has_certificate().await);
        assert!(!signing.signed_helper().await.unwrap().exists());
    }

    #[rstest]
    #[case::itself(b"helper", "CERT", true)]
    #[case::other_source(b"other", "CERT", false)]
    #[case::other_certificate(b"helper", "OTHER", false)]
    fn a_stamp_matches_only_its_own_text(
        #[case] source: &[u8],
        #[case] certificate: &str,
        #[case] matches: bool,
    ) {
        let stamp = Stamp {
            source: blake3::hash(b"helper"),
            certificate: "CERT",
        };
        let stored = Stamp {
            source: blake3::hash(source),
            certificate,
        };

        assert_eq!(stamp.matches(&stored.to_string()), matches);
    }

    /// The shipped helper is hashed again only when its length or its modification time changes.
    ///
    /// A changed hash signs a new copy, and a failed signing doesn't leave a temporary file behind.
    #[rstest]
    #[tokio::test]
    async fn hashes_the_shipped_helper_again_only_when_it_changes() {
        let dir = tempfile::tempdir().unwrap();
        let signing = Signing::in_dir(dir.path().join("signing"));
        std::fs::create_dir(&signing.dir).unwrap();
        std::fs::write(signing.dir.join(FINGERPRINT), "CERT\n").unwrap();
        let shipped = dir.path().join("helper");
        std::fs::write(&shipped, "one").unwrap();
        let stamp = Stamp {
            source: blake3::hash(b"one"),
            certificate: "CERT",
        };
        std::fs::write(signing.dir.join(STAMP), stamp.to_string()).unwrap();
        std::fs::write(signing.signed_helper().await.unwrap(), "signed").unwrap();
        let set_modified = |modified| {
            let file = std::fs::File::options().write(true).open(&shipped).unwrap();
            file.set_modified(modified).unwrap();
        };

        assert_eq!(
            signing.helper(shipped.as_path()).await.unwrap(),
            signing.signed_helper().await.unwrap()
        );

        let modified = std::fs::metadata(&shipped).unwrap().modified().unwrap();
        std::fs::write(&shipped, "two").unwrap();
        set_modified(modified);
        assert_eq!(
            signing.helper(shipped.as_path()).await.unwrap(),
            signing.signed_helper().await.unwrap()
        );

        set_modified(modified + Duration::from_secs(1));
        // Signing first unlocks the keychain, whose password file this test leaves out.
        let failed = signing.helper(shipped.as_path()).await;
        assert!(
            matches!(
                failed,
                Err(SigningError::Io {
                    action: IoAction::Read,
                    ..
                })
            ),
            "{failed:?}"
        );
        let mut left = std::fs::read_dir(&signing.dir)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().into_string().unwrap())
            .collect::<Vec<_>>();
        left.sort();
        assert_eq!(left, [HELPER_NAME, STAMP, FINGERPRINT]);
    }
}
