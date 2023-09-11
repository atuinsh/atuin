use std::env;

use atuin_common::utils::uuid_v7;
use regex::RegexSet;

use crate::{secrets::SECRET_PATTERNS, settings::Settings};
use time::OffsetDateTime;

mod builder;

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
    pub id: String,
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
        let hostname = hostname.unwrap_or_else(|| {
            format!(
                "{}:{}",
                env::var("ATUIN_HOST_NAME").unwrap_or_else(|_| whoami::hostname()),
                env::var("ATUIN_HOST_USER").unwrap_or_else(|_| whoami::username())
            )
        });

        Self {
            id: uuid_v7().as_simple().to_string(),
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

    use crate::settings::Settings;

    use super::History;

    // Test that we don't save history where necessary
    #[test]
    fn privacy_test() {
        let mut settings = Settings::default();
        settings.cwd_filter = RegexSet::new(["^/supasecret"]).unwrap();
        settings.history_filter = RegexSet::new(["^psql"]).unwrap();

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
        let mut settings = Settings::default();
        settings.secrets_filter = false;

        let stripe_key: History = History::capture()
            .timestamp(time::OffsetDateTime::now_utc())
            .command("curl foo.com/bar?key=sk_test_1234567890abcdefghijklmnop")
            .cwd("/")
            .build()
            .into();

        assert!(stripe_key.should_save(&settings));
    }
}
