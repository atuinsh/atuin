//! The creation and the removal of the certificate.

use std::fmt::Write as _;
use std::path::Path;

use atuin_common::os::private;

use super::copy::user_helper_name;
use super::{
    CODESIGN, FINGERPRINT, IoAction, NAME, PASSWORD, SCRATCH, SECURITY, Signing, SigningError,
    remove_file, run,
};

/// The common name of the certificate, which Keychain Access shows.
const COMMON_NAME: &str = "atuin desktop capture";

/// The system's LibreSSL. An OpenSSL 3 that comes first in `PATH`, such as Homebrew's, encrypts
/// PKCS#12 files in a way that older versions of `security import` reject.
const OPENSSL: &str = "/usr/bin/openssl";

/// The extensions of the certificate, which `openssl req` reads.
const REQUEST_CONFIG: &str = "\
[req]
distinguished_name = dn
x509_extensions = ext
prompt = no
[dn]
CN = atuin desktop capture
[ext]
keyUsage = critical, digitalSignature
extendedKeyUsage = critical, codeSigning
basicConstraints = critical, CA:false
";

impl Signing {
    /// Creates the certificate, in place of any earlier one.
    ///
    /// A new certificate needs a new grant of the permission.
    ///
    /// # Errors
    ///
    /// Fails when the earlier certificate can't be removed, or when a step fails. A failed
    /// creation removes the certificate files that it wrote.
    pub async fn create_certificate(&self) -> Result<(), SigningError> {
        private::create_dir_all(&self.dir)
            .await
            .map_err(SigningError::io(IoAction::Create, &self.dir))?;
        // A copy that the earlier certificate signed would stay behind under its own name.
        if let Ok(old) = self.signed_helper().await {
            let _ = remove_file(old).await;
        }
        // A keychain that stays behind would make `security create-keychain` fail.
        self.delete_certificate().await?;
        let scratch = self.dir.join(SCRATCH);
        let _ = tokio::fs::remove_dir_all(&scratch).await;
        private::create_dir_all(&scratch)
            .await
            .map_err(SigningError::io(IoAction::Create, &scratch))?;
        let created = self.create_in(&scratch).await;
        let _ = tokio::fs::remove_dir_all(&scratch).await;
        if created.is_err() {
            let _ = self.delete_certificate().await;
        }
        created
    }

    async fn create_in(&self, scratch: &Path) -> Result<(), SigningError> {
        let config = scratch.join("request.cnf");
        tokio::fs::write(&config, REQUEST_CONFIG)
            .await
            .map_err(SigningError::io(IoAction::Write, &config))?;
        let key = scratch.join("key.pem");
        let certificate = scratch.join("certificate.pem");
        let bundle = scratch.join("identity.p12");
        let password = random_password();

        run(OPENSSL, "req", |openssl| {
            openssl
                .args(["req", "-x509", "-newkey", "rsa:2048", "-nodes", "-days", "36500"])
                .arg("-config")
                .arg(&config)
                .arg("-keyout")
                .arg(&key)
                .arg("-out")
                .arg(&certificate)
        })
        .await?;
        let mut passout = String::from("pass:");
        passout.push_str(&password);
        run(OPENSSL, "pkcs12", |openssl| {
            openssl
                .args(["pkcs12", "-export", "-name", COMMON_NAME])
                .args(["-passout", &passout])
                .arg("-inkey")
                .arg(&key)
                .arg("-in")
                .arg(&certificate)
                .arg("-out")
                .arg(&bundle)
        })
        .await?;

        let password_file = self.dir.join(PASSWORD);
        private::write(&password_file, &password)
            .await
            .map_err(SigningError::io(IoAction::Write, password_file))?;
        let keychain = self.keychain();
        run(SECURITY, "create-keychain", |security| {
            security.args(["create-keychain", "-p", &password]).arg(&keychain)
        })
        .await?;
        // Without options, the keychain stays unlocked until the session ends.
        run(SECURITY, "set-keychain-settings", |security| {
            security.arg("set-keychain-settings").arg(&keychain)
        })
        .await?;
        // `-x` makes the key non-extractable, and `-T` lets `codesign` use it without a prompt.
        run(SECURITY, "import", |security| {
            security
                .arg("import")
                .arg(&bundle)
                .arg("-k")
                .arg(&keychain)
                .args(["-P", &password, "-x", "-T", CODESIGN])
        })
        .await?;
        run(SECURITY, "set-key-partition-list", |security| {
            security
                .args(["set-key-partition-list", "-S", "apple-tool:,apple:", "-s", "-k", &password])
                .arg(&keychain)
        })
        .await?;
        let found = run(SECURITY, "find-certificate", |security| {
            security.args(["find-certificate", "-Z", "-c", COMMON_NAME]).arg(&keychain)
        })
        .await?;
        let fingerprint = sha1_fingerprint(&String::from_utf8_lossy(&found.stdout))
            .ok_or(SigningError::NoFingerprint)?
            .to_owned();
        Self::ensure_in_search_list(&keychain).await?;
        let name = self.dir.join(NAME);
        private::write(&name, user_helper_name())
            .await
            .map_err(SigningError::io(IoAction::Write, name))?;
        let path = self.dir.join(FINGERPRINT);
        tokio::fs::write(&path, fingerprint).await.map_err(SigningError::io(IoAction::Write, path))
    }

    /// Removes the certificate and its keychain, and takes the keychain off the search list.
    pub async fn delete_certificate(&self) -> Result<(), SigningError> {
        remove_file(self.dir.join(FINGERPRINT)).await?;
        let keychain = self.keychain();
        if tokio::fs::try_exists(&keychain).await.unwrap_or(false) {
            // `security delete-keychain` also takes the keychain off the search list.
            run(SECURITY, "delete-keychain", |security| {
                security.arg("delete-keychain").arg(&keychain)
            })
            .await?;
            remove_file(keychain).await?;
        }
        remove_file(self.dir.join(PASSWORD)).await?;
        remove_file(self.dir.join(NAME)).await
    }
}

/// The SHA-1 fingerprint in the output of `security find-certificate -Z`.
fn sha1_fingerprint(found: &str) -> Option<&str> {
    found.lines().find_map(|line| line.trim().strip_prefix("SHA-1 hash:")).map(str::trim)
}

fn random_password() -> String {
    rand::random::<[u8; 32]>().iter().fold(String::with_capacity(64), |mut hex, byte| {
        let _ = write!(hex, "{byte:02x}");
        hex
    })
}

#[cfg(test)]
mod tests {
    use atuin_common::path::PathBufExt as _;
    use rstest::rstest;

    use super::*;

    #[rstest]
    #[case::found(
        "SHA-256 hash: 0A1B\nSHA-1 hash: 73C51BC4BE620CC33DEBD9BF506E79D4DEF22DBE\nkeychain: \
         \"x\"\n",
        Some("73C51BC4BE620CC33DEBD9BF506E79D4DEF22DBE")
    )]
    #[case::missing("keychain: \"x\"\n", None)]
    fn reads_the_fingerprint(#[case] found: &str, #[case] expected: Option<&str>) {
        assert_eq!(sha1_fingerprint(found), expected);
    }

    /// An earlier certificate that can't be removed fails the creation with the error of the
    /// removal, before the creation runs a step that the leftover would break.
    #[rstest]
    #[tokio::test]
    async fn creation_fails_when_the_earlier_certificate_stays() {
        let dir = tempfile::tempdir().expect("tempdir");
        let signing = Signing::in_dir(dir.path().to_owned());
        // A directory that isn't empty, in the place of the fingerprint, can't be removed as a
        // file.
        std::fs::create_dir(dir.path().join(FINGERPRINT)).expect("mkdir");
        std::fs::write(dir.path().join(FINGERPRINT).joined("file"), b"").expect("write");

        let created = signing.create_certificate().await;

        assert!(
            matches!(
                created,
                Err(SigningError::Io {
                    action: IoAction::Remove,
                    ..
                })
            ),
            "{created:?}"
        );
        assert!(!dir.path().join(PASSWORD).exists());
    }

    #[rstest]
    fn password_is_64_hex_digits() {
        let password = random_password();
        assert_eq!(password.len(), 64);
        assert!(password.bytes().all(|b| b.is_ascii_hexdigit()));
    }
}
