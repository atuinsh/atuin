use core::fmt::Formatter;
use rmp::decode::ValueReadError;
use rmp::{decode::Bytes, Marker};
use std::env;
use std::fmt::Display;

use atuin_common::record::DecryptedData;
use atuin_common::utils::uuid_v7;

use eyre::{bail, eyre, Result};
use regex::RegexSet;

use crate::utils::get_host_user;
use crate::{secrets::SECRET_PATTERNS, settings::Settings};
use time::OffsetDateTime;

mod builder;
pub mod store;

const HISTORY_VERSION: &str = "v0";
const HISTORY_TAG: &str = "history";

#[derive(Clone, Debug, Eq, PartialEq, Hash)]
pub struct HistoryId(pub String);

impl Display for HistoryId {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl From<String> for HistoryId {
    fn from(s: String) -> Self {
        Self(s)
    }
}

/// Client-side history entry.
///
/// Client stores data unencrypted, and only encrypts it before sending to the server.
///
/// To create a new history entry, use one of the builders:
/// - [`History::import()`] to import an entry from the shell history file
/// - [`History::capture()`] to capture an entry via hook
/// - [`History::from_db()`] to create an instance from the database entry
//
// ## Implementation Notes
//
// New fields must should be added to `encryption::{encode, decode}` in a backwards
// compatible way. (eg sensible defaults and updating the nfields parameter)
#[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow)]
pub struct History {
    /// A client-generated ID, used to identify the entry when syncing.
    ///
    /// Stored as `client_id` in the database.
    pub id: HistoryId,
    /// When the command was run.
    pub timestamp: OffsetDateTime,
    /// How long the command took to run.
    pub duration: i64,
    /// The exit code of the command.
    pub exit: i64,
    /// The command that was run.
    pub command: String,
    /// The current working directory when the command was run.
    pub cwd: String,
    /// The session ID, associated with a terminal session.
    pub session: String,
    /// The hostname of the machine the command was run on.
    pub hostname: String,
    /// Timestamp, which is set when the entry is deleted, allowing a soft delete.
    pub deleted_at: Option<OffsetDateTime>,
}

#[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow)]
pub struct HistoryStats {
    /// The command that was ran after this one in the session
    pub next: Option<History>,
    ///
    /// The command that was ran before this one in the session
    pub previous: Option<History>,

    /// How many times has this command been ran?
    pub total: u64,

    pub average_duration: u64,

    pub exits: Vec<(i64, i64)>,

    pub day_of_week: Vec<(String, i64)>,

    pub duration_over_time: Vec<(String, i64)>,
}

impl History {
    #[allow(clippy::too_many_arguments)]
    fn new(
        timestamp: OffsetDateTime,
        command: String,
        cwd: String,
        exit: i64,
        duration: i64,
        session: Option<String>,
        hostname: Option<String>,
        deleted_at: Option<OffsetDateTime>,
    ) -> Self {
        let session = session
            .or_else(|| env::var("ATUIN_SESSION").ok())
            .unwrap_or_else(|| uuid_v7().as_simple().to_string());
        let hostname = hostname.unwrap_or_else(get_host_user);

        Self {
            id: uuid_v7().as_simple().to_string().into(),
            timestamp,
            command,
            cwd,
            exit,
            duration,
            session,
            hostname,
            deleted_at,
        }
    }

    pub fn serialize(&self) -> Result<DecryptedData> {
        // This is pretty much the same as what we used for the old history, with one difference -
        // it uses integers for timestamps rather than a string format.

        use rmp::encode;

        let mut output = vec![];

        // write the version
        encode::write_u16(&mut output, 0)?;
        // INFO: ensure this is updated when adding new fields
        encode::write_array_len(&mut output, 9)?;

        encode::write_str(&mut output, &self.id.0)?;
        encode::write_u64(&mut output, self.timestamp.unix_timestamp_nanos() as u64)?;
        encode::write_sint(&mut output, self.duration)?;
        encode::write_sint(&mut output, self.exit)?;
        encode::write_str(&mut output, &self.command)?;
        encode::write_str(&mut output, &self.cwd)?;
        encode::write_str(&mut output, &self.session)?;
        encode::write_str(&mut output, &self.hostname)?;

        match self.deleted_at {
            Some(d) => encode::write_u64(&mut output, d.unix_timestamp_nanos() as u64)?,
            None => encode::write_nil(&mut output)?,
        }

        Ok(DecryptedData(output))
    }

    fn deserialize_v0(bytes: &[u8]) -> Result<History> {
        use rmp::decode;

        fn error_report<E: std::fmt::Debug>(err: E) -> eyre::Report {
            eyre!("{err:?}")
        }

        let mut bytes = Bytes::new(bytes);

        let version = decode::read_u16(&mut bytes).map_err(error_report)?;

        if version != 0 {
            bail!("expected decoding v0 record, found v{version}");
        }

        let nfields = decode::read_array_len(&mut bytes).map_err(error_report)?;

        if nfields != 9 {
            bail!("cannot decrypt history from a different version of Atuin");
        }

        let bytes = bytes.remaining_slice();
        let (id, bytes) = decode::read_str_from_slice(bytes).map_err(error_report)?;

        let mut bytes = Bytes::new(bytes);
        let timestamp = decode::read_u64(&mut bytes).map_err(error_report)?;
        let duration = decode::read_int(&mut bytes).map_err(error_report)?;
        let exit = decode::read_int(&mut bytes).map_err(error_report)?;

        let bytes = bytes.remaining_slice();
        let (command, bytes) = decode::read_str_from_slice(bytes).map_err(error_report)?;
        let (cwd, bytes) = decode::read_str_from_slice(bytes).map_err(error_report)?;
        let (session, bytes) = decode::read_str_from_slice(bytes).map_err(error_report)?;
        let (hostname, bytes) = decode::read_str_from_slice(bytes).map_err(error_report)?;

        // if we have more fields, try and get the deleted_at
        let mut bytes = Bytes::new(bytes);

        let (deleted_at, bytes) = match decode::read_u64(&mut bytes) {
            Ok(unix) => (Some(unix), bytes.remaining_slice()),
            // we accept null here
            Err(ValueReadError::TypeMismatch(Marker::Null)) => (None, bytes.remaining_slice()),
            Err(err) => return Err(error_report(err)),
        };

        if !bytes.is_empty() {
            bail!("trailing bytes in encoded history. malformed")
        }

        Ok(History {
            id: id.to_owned().into(),
            timestamp: OffsetDateTime::from_unix_timestamp_nanos(timestamp as i128)?,
            duration,
            exit,
            command: command.to_owned(),
            cwd: cwd.to_owned(),
            session: session.to_owned(),
            hostname: hostname.to_owned(),
            deleted_at: deleted_at
                .map(|t| OffsetDateTime::from_unix_timestamp_nanos(t as i128))
                .transpose()?,
        })
    }

    pub fn deserialize(bytes: &[u8], version: &str) -> Result<History> {
        match version {
            HISTORY_VERSION => Self::deserialize_v0(bytes),

            _ => bail!("unknown version {version:?}"),
        }
    }

    /// Builder for a history entry that is imported from shell history.
    ///
    /// The only two required fields are `timestamp` and `command`.
    ///
    /// ## Examples
    /// ```
    /// use atuin_client::history::History;
    ///
    /// let history: History = History::import()
    ///     .timestamp(time::OffsetDateTime::now_utc())
    ///     .command("ls -la")
    ///     .build()
    ///     .into();
    /// ```
    ///
    /// If shell history contains more information, it can be added to the builder:
    /// ```
    /// use atuin_client::history::History;
    ///
    /// let history: History = History::import()
    ///     .timestamp(time::OffsetDateTime::now_utc())
    ///     .command("ls -la")
    ///     .cwd("/home/user")
    ///     .exit(0)
    ///     .duration(100)
    ///     .build()
    ///     .into();
    /// ```
    ///
    /// Unknown command or command without timestamp cannot be imported, which
    /// is forced at compile time:
    ///
    /// ```compile_fail
    /// use atuin_client::history::History;
    ///
    /// // this will not compile because timestamp is missing
    /// let history: History = History::import()
    ///     .command("ls -la")
    ///     .build()
    ///     .into();
    /// ```
    pub fn import() -> builder::HistoryImportedBuilder {
        builder::HistoryImported::builder()
    }

    /// Builder for a history entry that is captured via hook.
    ///
    /// This builder is used only at the `start` step of the hook,
    /// so it doesn't have any fields which are known only after
    /// the command is finished, such as `exit` or `duration`.
    ///
    /// ## Examples
    /// ```rust
    /// use atuin_client::history::History;
    ///
    /// let history: History = History::capture()
    ///     .timestamp(time::OffsetDateTime::now_utc())
    ///     .command("ls -la")
    ///     .cwd("/home/user")
    ///     .build()
    ///     .into();
    /// ```
    ///
    /// Command without any required info cannot be captured, which is forced at compile time:
    ///
    /// ```compile_fail
    /// use atuin_client::history::History;
    ///
    /// // this will not compile because `cwd` is missing
    /// let history: History = History::capture()
    ///     .timestamp(time::OffsetDateTime::now_utc())
    ///     .command("ls -la")
    ///     .build()
    ///     .into();
    /// ```
    pub fn capture() -> builder::HistoryCapturedBuilder {
        builder::HistoryCaptured::builder()
    }

    /// Builder for a history entry that is imported from the database.
    ///
    /// All fields are required, as they are all present in the database.
    ///
    /// ```compile_fail
    /// use atuin_client::history::History;
    ///
    /// // this will not compile because `id` field is missing
    /// let history: History = History::from_db()
    ///     .timestamp(time::OffsetDateTime::now_utc())
    ///     .command("ls -la".to_string())
    ///     .cwd("/home/user".to_string())
    ///     .exit(0)
    ///     .duration(100)
    ///     .session("somesession".to_string())
    ///     .hostname("localhost".to_string())
    ///     .deleted_at(None)
    ///     .build()
    ///     .into();
    /// ```
    pub fn from_db() -> builder::HistoryFromDbBuilder {
        builder::HistoryFromDb::builder()
    }

    pub fn success(&self) -> bool {
        self.exit == 0 || self.duration == -1
    }

    pub fn should_save(&self, settings: &Settings) -> bool {
        let secret_regex = SECRET_PATTERNS.iter().map(|f| f.1);
        let secret_regex = RegexSet::new(secret_regex).expect("Failed to build secrets regex");

        !(self.command.starts_with(' ')
            || settings.history_filter.is_match(&self.command)
            || settings.cwd_filter.is_match(&self.cwd)
            || (secret_regex.is_match(&self.command)) && settings.secrets_filter)
    }
}

#[cfg(test)]
mod tests {
    use regex::RegexSet;
    use time::macros::datetime;

    use crate::{history::HISTORY_VERSION, settings::Settings};

    use super::History;

    // Test that we don't save history where necessary
    #[test]
    fn privacy_test() {
        let settings = Settings {
            cwd_filter: RegexSet::new(["^/supasecret"]).unwrap(),
            history_filter: RegexSet::new(["^psql"]).unwrap(),
            ..Settings::utc()
        };

        let normal_command: History = History::capture()
            .timestamp(time::OffsetDateTime::now_utc())
            .command("echo foo")
            .cwd("/")
            .build()
            .into();

        let with_space: History = History::capture()
            .timestamp(time::OffsetDateTime::now_utc())
            .command(" echo bar")
            .cwd("/")
            .build()
            .into();

        let stripe_key: History = History::capture()
            .timestamp(time::OffsetDateTime::now_utc())
            .command("curl foo.com/bar?key=sk_test_1234567890abcdefghijklmnop")
            .cwd("/")
            .build()
            .into();

        let secret_dir: History = History::capture()
            .timestamp(time::OffsetDateTime::now_utc())
            .command("echo ohno")
            .cwd("/supasecret")
            .build()
            .into();

        let with_psql: History = History::capture()
            .timestamp(time::OffsetDateTime::now_utc())
            .command("psql")
            .cwd("/supasecret")
            .build()
            .into();

        assert!(normal_command.should_save(&settings));
        assert!(!with_space.should_save(&settings));
        assert!(!stripe_key.should_save(&settings));
        assert!(!secret_dir.should_save(&settings));
        assert!(!with_psql.should_save(&settings));
    }

    #[test]
    fn disable_secrets() {
        let settings = Settings {
            secrets_filter: false,
            ..Settings::utc()
        };

        let stripe_key: History = History::capture()
            .timestamp(time::OffsetDateTime::now_utc())
            .command("curl foo.com/bar?key=sk_test_1234567890abcdefghijklmnop")
            .cwd("/")
            .build()
            .into();

        assert!(stripe_key.should_save(&settings));
    }

    #[test]
    fn test_serialize_deserialize() {
        let bytes = [
            205, 0, 0, 153, 217, 32, 54, 54, 100, 49, 54, 99, 98, 101, 101, 55, 99, 100, 52, 55,
            53, 51, 56, 101, 53, 99, 53, 98, 56, 98, 52, 52, 101, 57, 48, 48, 54, 101, 207, 23, 99,
            98, 117, 24, 210, 246, 128, 206, 2, 238, 210, 240, 0, 170, 103, 105, 116, 32, 115, 116,
            97, 116, 117, 115, 217, 42, 47, 85, 115, 101, 114, 115, 47, 99, 111, 110, 114, 97, 100,
            46, 108, 117, 100, 103, 97, 116, 101, 47, 68, 111, 99, 117, 109, 101, 110, 116, 115,
            47, 99, 111, 100, 101, 47, 97, 116, 117, 105, 110, 217, 32, 98, 57, 55, 100, 57, 97,
            51, 48, 54, 102, 50, 55, 52, 52, 55, 51, 97, 50, 48, 51, 100, 50, 101, 98, 97, 52, 49,
            102, 57, 52, 53, 55, 187, 102, 118, 102, 103, 57, 51, 54, 99, 48, 107, 112, 102, 58,
            99, 111, 110, 114, 97, 100, 46, 108, 117, 100, 103, 97, 116, 101, 192,
        ];

        let history = History {
            id: "66d16cbee7cd47538e5c5b8b44e9006e".to_owned().into(),
            timestamp: datetime!(2023-05-28 18:35:40.633872 +00:00),
            duration: 49206000,
            exit: 0,
            command: "git status".to_owned(),
            cwd: "/Users/conrad.ludgate/Documents/code/atuin".to_owned(),
            session: "b97d9a306f274473a203d2eba41f9457".to_owned(),
            hostname: "fvfg936c0kpf:conrad.ludgate".to_owned(),
            deleted_at: None,
        };

        let serialized = history.serialize().expect("failed to serialize history");
        assert_eq!(serialized.0, bytes);

        let deserialized = History::deserialize(&serialized.0, HISTORY_VERSION)
            .expect("failed to deserialize history");
        assert_eq!(history, deserialized);

        // test the snapshot too
        let deserialized =
            History::deserialize(&bytes, HISTORY_VERSION).expect("failed to deserialize history");
        assert_eq!(history, deserialized);
    }

    #[test]
    fn test_serialize_deserialize_deleted() {
        let history = History {
            id: "66d16cbee7cd47538e5c5b8b44e9006e".to_owned().into(),
            timestamp: datetime!(2023-05-28 18:35:40.633872 +00:00),
            duration: 49206000,
            exit: 0,
            command: "git status".to_owned(),
            cwd: "/Users/conrad.ludgate/Documents/code/atuin".to_owned(),
            session: "b97d9a306f274473a203d2eba41f9457".to_owned(),
            hostname: "fvfg936c0kpf:conrad.ludgate".to_owned(),
            deleted_at: Some(datetime!(2023-11-19 20:18 +00:00)),
        };

        let serialized = history.serialize().expect("failed to serialize history");

        let deserialized = History::deserialize(&serialized.0, HISTORY_VERSION)
            .expect("failed to deserialize history");

        assert_eq!(history, deserialized);
    }

    #[test]
    fn test_serialize_deserialize_version() {
        // v0
        let bytes_v0 = [
            205, 0, 0, 153, 217, 32, 54, 54, 100, 49, 54, 99, 98, 101, 101, 55, 99, 100, 52, 55,
            53, 51, 56, 101, 53, 99, 53, 98, 56, 98, 52, 52, 101, 57, 48, 48, 54, 101, 207, 23, 99,
            98, 117, 24, 210, 246, 128, 206, 2, 238, 210, 240, 0, 170, 103, 105, 116, 32, 115, 116,
            97, 116, 117, 115, 217, 42, 47, 85, 115, 101, 114, 115, 47, 99, 111, 110, 114, 97, 100,
            46, 108, 117, 100, 103, 97, 116, 101, 47, 68, 111, 99, 117, 109, 101, 110, 116, 115,
            47, 99, 111, 100, 101, 47, 97, 116, 117, 105, 110, 217, 32, 98, 57, 55, 100, 57, 97,
            51, 48, 54, 102, 50, 55, 52, 52, 55, 51, 97, 50, 48, 51, 100, 50, 101, 98, 97, 52, 49,
            102, 57, 52, 53, 55, 187, 102, 118, 102, 103, 57, 51, 54, 99, 48, 107, 112, 102, 58,
            99, 111, 110, 114, 97, 100, 46, 108, 117, 100, 103, 97, 116, 101, 192,
        ];

        // some other version
        let bytes_v1 = [
            205, 1, 0, 153, 217, 32, 54, 54, 100, 49, 54, 99, 98, 101, 101, 55, 99, 100, 52, 55,
            53, 51, 56, 101, 53, 99, 53, 98, 56, 98, 52, 52, 101, 57, 48, 48, 54, 101, 207, 23, 99,
            98, 117, 24, 210, 246, 128, 206, 2, 238, 210, 240, 0, 170, 103, 105, 116, 32, 115, 116,
            97, 116, 117, 115, 217, 42, 47, 85, 115, 101, 114, 115, 47, 99, 111, 110, 114, 97, 100,
            46, 108, 117, 100, 103, 97, 116, 101, 47, 68, 111, 99, 117, 109, 101, 110, 116, 115,
            47, 99, 111, 100, 101, 47, 97, 116, 117, 105, 110, 217, 32, 98, 57, 55, 100, 57, 97,
            51, 48, 54, 102, 50, 55, 52, 52, 55, 51, 97, 50, 48, 51, 100, 50, 101, 98, 97, 52, 49,
            102, 57, 52, 53, 55, 187, 102, 118, 102, 103, 57, 51, 54, 99, 48, 107, 112, 102, 58,
            99, 111, 110, 114, 97, 100, 46, 108, 117, 100, 103, 97, 116, 101, 192,
        ];

        let deserialized = History::deserialize(&bytes_v0, HISTORY_VERSION);
        assert!(deserialized.is_ok());

        let deserialized = History::deserialize(&bytes_v1, HISTORY_VERSION);
        assert!(deserialized.is_err());
    }
}
