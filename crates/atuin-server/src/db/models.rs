use std::num::NonZeroUsize;
use std::str::FromStr;
use std::sync::LazyLock;

use argon2::password_hash::SaltString;
use argon2::{
    Algorithm, Argon2, Params, PasswordHash, PasswordHasher as _, PasswordVerifier as _, Version,
};
use atuin_common::sync::BlockingPool;
use atuin_domain::record::{
    EncryptedData, Host, HostId, Record, RecordIdx, RecordSeriesKey, RecordTag, RecordVersion,
};
use easy_cast::Conv;
use rand::rngs::OsRng;
use secrecy::{ExposeSecret as _, SecretString};
use sqlx::Row;

#[derive(sqlx::FromRow)]
pub struct User {
    pub id: i64,
    pub username: String,
    pub email: String,
    pub password: HashedPassword,
}

#[derive(sqlx::FromRow)]
pub struct Session {
    pub id: i64,
    pub user_id: i64,
    #[sqlx(try_from = "String")]
    pub token: SecretString,
}

pub struct NewUser {
    pub username: NewUsername,
    pub email: String,
    pub password: HashedPassword,
}

/// A name a new account may take: non-empty ASCII letters, digits and hyphens, such as `alice-2`.
///
/// Existing accounts keep plain `String` names, since accounts registered before this rule may hold
/// any name.
#[derive(Debug, Clone, derive_more::Deref, derive_more::Display)]
pub struct NewUsername(String);

impl FromStr for NewUsername {
    type Err = InvalidUsername;

    fn from_str(username: &str) -> Result<Self, Self::Err> {
        if username.is_empty() || !username.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') {
            return Err(InvalidUsername(username.to_owned()));
        }
        Ok(Self(username.to_owned()))
    }
}

#[derive(Debug, thiserror::Error)]
#[error("{0:?} is not a valid username; use only letters, digits and hyphens (-)")]
pub struct InvalidUsername(String);

// Each Argon2 hash holds 19 MiB, and the blocking pool alone would run up to 512 at once.
static ARGON2_POOL: LazyLock<BlockingPool> = LazyLock::new(|| {
    BlockingPool::new(std::thread::available_parallelism().unwrap_or(NonZeroUsize::MIN))
});

/// A user's password as stored: an Argon2id hash in PHC string format.
#[derive(sqlx::Type)]
#[sqlx(transparent)]
pub struct HashedPassword(String);

impl HashedPassword {
    /// Hash `password` with a fresh salt.
    pub async fn new(password: SecretString) -> Self {
        ARGON2_POOL
            .run(move || {
                let salt = SaltString::generate(&mut OsRng);
                let hash = Self::argon2()
                    .hash_password(password.expose_secret().as_bytes(), &salt)
                    .expect("Argon2id with default params hashes any password with a fresh salt");
                Self(hash.to_string())
            })
            .await
            .expect("the blocking pool runs the hash")
    }

    /// Whether `password` is the one this was hashed from.
    pub async fn verify(&self, password: SecretString) -> bool {
        let hash = self.0.clone();
        ARGON2_POOL
            .run(move || {
                let Ok(hash) = PasswordHash::new(&hash) else {
                    return false;
                };
                Self::argon2().verify_password(password.expose_secret().as_bytes(), &hash).is_ok()
            })
            .await
            .expect("the blocking pool runs the check")
    }

    pub(crate) fn as_str(&self) -> &str {
        &self.0
    }

    fn argon2() -> Argon2<'static> {
        Argon2::new(Algorithm::Argon2id, Version::V0x13, Params::default())
    }
}

pub struct NewSession {
    pub user_id: i64,
    pub token: SecretString,
}

#[derive(derive_more::Into)]
pub(crate) struct DbRecord(pub Record<EncryptedData>);

macro_rules! impl_db_record_from_row {
    ($row:ty) => {
        impl<'a> ::sqlx::FromRow<'a, $row> for DbRecord {
            fn from_row(row: &'a $row) -> ::sqlx::Result<Self> {
                let idx: i64 = row.try_get("idx")?;
                let timestamp: i64 = row.try_get("timestamp")?;

                let data = EncryptedData {
                    raw: row.try_get("data")?,
                    cek: row.try_get("cek")?,
                };

                Ok(Self(Record {
                    id: row.try_get("client_id")?,
                    host: Host::new(row.try_get("host")?),
                    idx: u64::conv(idx),
                    timestamp: u64::conv(timestamp),
                    version: RecordVersion::from(row.try_get::<String, _>("version")?),
                    tag: RecordTag::from(row.try_get::<String, _>("tag")?),
                    data,
                }))
            }
        }
    };
}

impl_db_record_from_row!(sqlx::postgres::PgRow);
impl_db_record_from_row!(sqlx::sqlite::SqliteRow);
impl_db_record_from_row!(sqlx::mysql::MySqlRow);

#[derive(derive_more::Into)]
pub(crate) struct RecordSeriesPoint {
    pub(super) series: RecordSeriesKey,
    pub(super) idx: RecordIdx,
}

macro_rules! impl_record_series_point_from_row {
    ($row:ty) => {
        impl<'a> ::sqlx::FromRow<'a, $row> for RecordSeriesPoint {
            fn from_row(row: &'a $row) -> ::sqlx::Result<Self> {
                let host: HostId = row.try_get("host")?;
                let tag: String = row.try_get("tag")?;
                let idx: i64 = row.try_get("idx")?;

                Ok(Self {
                    series: RecordSeriesKey::new(host, RecordTag::from(tag)),
                    idx: u64::conv(idx),
                })
            }
        }
    };
}

impl_record_series_point_from_row!(sqlx::postgres::PgRow);
impl_record_series_point_from_row!(sqlx::sqlite::SqliteRow);
impl_record_series_point_from_row!(sqlx::mysql::MySqlRow);

#[cfg(test)]
mod tests {
    use rstest::rstest;

    use super::{HashedPassword, NewUsername};

    #[rstest]
    #[case::letters_digits_and_hyphens("Alice-2", true)]
    #[case::empty("", false)]
    #[case::space("al ice", false)]
    #[case::underscore("al_ice", false)]
    #[case::email("alice@example.com", false)]
    #[case::non_ascii_letter("ålice", false)]
    fn new_usernames_are_nonempty_ascii_letters_digits_and_hyphens(
        #[case] username: &str,
        #[case] valid: bool,
    ) {
        assert_eq!(username.parse::<NewUsername>().is_ok(), valid);
    }

    #[rstest]
    #[case::same_password("hunter2", true)]
    #[case::other_password("hunter3", false)]
    #[tokio::test]
    async fn a_hash_verifies_only_the_password_it_was_made_from(
        #[case] attempt: &str,
        #[case] verified: bool,
    ) {
        let hash = HashedPassword::new("hunter2".into()).await;
        assert_eq!(hash.verify(attempt.into()).await, verified);
    }

    #[rstest]
    #[tokio::test]
    async fn a_stored_password_that_is_not_a_hash_never_verifies() {
        assert!(!HashedPassword("hunter2".to_owned()).verify("hunter2".into()).await);
    }
}
