use std::env;
use std::sync::LazyLock;

use atuin_common::filter::OrFilter;
use atuin_common::rmp::decode::{self, Bytes, DecodeError};
use atuin_common::rmp::encode::{self, ByteBuf, EncodeError};
use atuin_common::time::OffsetDateTimeExt;
use atuin_common::utils::{normalize_optional_string, uuid_v7};
use atuin_domain::record::{CmdOrigin, DecryptedData, UNKNOWN_USER};
use eyre::{Result, bail};
use time::OffsetDateTime;

use crate::secrets::SECRET_PATTERNS_RE;
use crate::settings::Settings;

pub(crate) mod builder;
pub mod store;

/// Known AI agent author values. Used by [`History::is_agent`] to guess who ran a command when the
/// entry does not state it, and so when matching against [`AuthorPattern::AllAgent`] and
/// [`AuthorPattern::AllUser`].
pub const KNOWN_AGENTS: &[&str] = &["claude-code", "codex", "copilot", "opencode", "pi"];

/// The spelling of [`AuthorPattern::AllUser`] on the command line and in the MCP tool schema.
pub const AUTHOR_FILTER_ALL_USER: &str = "$all-user";

/// The spelling of [`AuthorPattern::AllAgent`] on the command line and in the MCP tool schema.
pub const AUTHOR_FILTER_ALL_AGENT: &str = "$all-agent";

#[must_use]
pub fn is_known_agent(author: &str) -> bool {
    KNOWN_AGENTS.contains(&author)
}

/// Who wrote a history entry, as declared by whatever captured it.
///
/// This is stored as a small integer, both on the wire and in the database, and is optional: an
/// entry captured before this field existed, or by an integration that does not set it, has no
/// kind, and [`History::is_agent`] falls back to inspecting the author name. A stored value we
/// don't recognise (written by a future version with more kinds) decodes as "not stated" too:
/// [`Self::from_repr`] returns `None` rather than an error.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash, clap::ValueEnum, strum_macros::FromRepr)]
#[repr(u8)]
pub enum AuthorKind {
    /// A human ran this command.
    User = 1,
    /// An AI agent ran this command.
    Agent = 2,
}

impl AuthorKind {
    /// Every recognised kind. The SQL author filter derives its recognised-value list from this,
    /// so it stays in lockstep with [`Self::from_repr`] (a test pins the two together).
    pub const VARIANTS: [Self; 2] = [Self::User, Self::Agent];

    #[must_use]
    pub const fn as_u8(self) -> u8 {
        self as u8
    }

    /// The kind stated by the invoking integration's environment (`ATUIN_HISTORY_AUTHOR_KIND`).
    #[must_use]
    pub fn probe_current() -> Option<Self> {
        let value = env::var(HISTORY_AUTHOR_KIND_ENV).ok()?;
        clap::ValueEnum::from_str(&value, true).ok()
    }
}

/// An element of an author filter.
///
/// In addition to a plain string, this type can also be the special pattern `AllUser` or
/// `AllAgent`, which matches agent-run commands or everything that is not one.
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum AuthorPattern {
    /// Matches every entry that is not an agent's (see [`History::is_agent`]).
    AllUser,
    /// Matches every entry that is an agent's (see [`History::is_agent`]).
    AllAgent,
    /// Matches exactly one author name.
    Name(String),
}

impl From<String> for AuthorPattern {
    fn from(value: String) -> Self {
        match value.as_str() {
            AUTHOR_FILTER_ALL_USER => Self::AllUser,
            AUTHOR_FILTER_ALL_AGENT => Self::AllAgent,
            _ => Self::Name(value),
        }
    }
}

impl From<&str> for AuthorPattern {
    fn from(value: &str) -> Self {
        match value {
            AUTHOR_FILTER_ALL_USER => Self::AllUser,
            AUTHOR_FILTER_ALL_AGENT => Self::AllAgent,
            _ => Self::Name(value.to_owned()),
        }
    }
}

/// An author filter that only allows non-agent commands (i.e., [`AuthorPattern::AllUser`]).
///
/// This function uses a [`LazyLock`] to avoid building the filter every time.
pub fn all_user_author_filter() -> OrFilter<&'static [AuthorPattern]> {
    static FILTER: LazyLock<OrFilter<Vec<AuthorPattern>>> = LazyLock::new(|| {
        OrFilter::from_list(vec![AuthorPattern::AllUser]).expect("the vector is not empty")
    });
    FILTER.as_slice_filter()
}

const HISTORY_AUTHOR_ENV: &str = "ATUIN_HISTORY_AUTHOR";
const HISTORY_AUTHOR_KIND_ENV: &str = "ATUIN_HISTORY_AUTHOR_KIND";
const HISTORY_INTENT_ENV: &str = "ATUIN_HISTORY_INTENT";

/// The author identity exported by the invoking integration (`ATUIN_HISTORY_AUTHOR`).
#[must_use]
pub fn probe_author() -> Option<String> {
    normalize_optional_string(env::var(HISTORY_AUTHOR_ENV).ok())
}

#[derive(Clone, Copy, Eq, PartialEq, Ord, PartialOrd, derive_more::Display)]
#[display("{}", self.name())]
#[repr(u16)]
pub enum Version {
    Zero = 0,
    One = 1,
    Two = 2,
}

impl Version {
    pub const VARIANTS: [Self; 3] = [Self::Zero, Self::One, Self::Two];
    pub const LATEST: Self = Self::Two;

    #[must_use]
    pub fn from_name(s: &str) -> Option<Self> {
        match s {
            "v0" => Some(Self::Zero),
            "v1" => Some(Self::One),
            "v2" => Some(Self::Two),
            _ => None,
        }
    }

    #[must_use]
    pub const fn name(&self) -> &'static str {
        match self {
            Self::Zero => "v0",
            Self::One => "v1",
            Self::Two => "v2",
        }
    }

    #[must_use]
    pub const fn as_int(&self) -> u16 {
        *self as u16
    }

    #[must_use]
    pub fn min_fields(&self) -> u32 {
        match self {
            Self::Zero => 9,
            Self::One => 10,
            Self::Two => 12,
        }
    }

    #[must_use]
    pub fn max_fields(&self) -> Option<u32> {
        match self {
            Self::Zero => Some(9),
            Self::One => Some(11),
            Self::Two => None,
        }
    }
}

/// Number of fields [`History::serialize`] writes.
///
/// This is deliberately not [`Version::min_fields`]: fields appended to the latest version grow
/// this count, while `min_fields` stays at the 12 fields V2 launched with so that entries written
/// before the new fields existed still decode.
const LATEST_SERIALIZED_FIELDS: u32 = 13;

/// A V2 record contains `author_kind` iff it has at least this many fields.
///
/// Frozen forever at the position `author_kind` was appended at; do not grow it alongside
/// [`LATEST_SERIALIZED_FIELDS`].
const V2_AUTHOR_KIND_FIELD_NUMBER: u32 = 13;

#[derive(Clone, Debug, Eq, PartialEq, Hash, derive_more::Display, derive_more::From)]
#[display("{_0}")]
pub struct HistoryId(pub String);

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
// New fields must be added to `History::{serialize,deserialize}` in a backwards
// compatible way (sensible defaults and careful `nfields` handling).
#[derive(Debug, Clone, PartialEq, Eq)]
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
    pub cmd_origin: CmdOrigin,
    /// Who wrote this command (human user or automation/agent identity).
    pub author: String,
    /// Optional rationale for why the command was executed.
    pub intent: Option<String>,
    /// Timestamp, which is set when the entry is deleted, allowing a soft delete.
    pub deleted_at: Option<OffsetDateTime>,
    /// The shell used to run the command.
    pub shell: Option<String>,
    /// Whether a human or an agent wrote this command, if whatever captured it said so.
    ///
    /// When this is `None`, [`History::is_agent`] guesses from the author name.
    pub author_kind: Option<AuthorKind>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
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
        cmd_origin: Option<CmdOrigin>,
        author: Option<String>,
        intent: Option<String>,
        deleted_at: Option<OffsetDateTime>,
        shell: Option<String>,
        author_kind: Option<AuthorKind>,
    ) -> Self {
        let session = session
            .or_else(|| env::var("ATUIN_SESSION").ok())
            .unwrap_or_else(|| uuid_v7().as_simple().to_string());
        let cmd_origin = cmd_origin.unwrap_or_else(CmdOrigin::probe_current);
        let author = normalize_optional_string(author)
            .or_else(|| normalize_optional_string(env::var(HISTORY_AUTHOR_ENV).ok()))
            .unwrap_or_else(|| cmd_origin.user().to_string());
        let intent = normalize_optional_string(intent)
            .or_else(|| normalize_optional_string(env::var(HISTORY_INTENT_ENV).ok()));
        let shell = normalize_optional_string(shell);

        Self {
            id: uuid_v7().as_simple().to_string().into(),
            timestamp,
            command,
            cwd,
            exit,
            duration,
            session,
            cmd_origin,
            author,
            intent,
            deleted_at,
            shell,
            author_kind,
        }
    }

    /// Whether an AI agent, rather than a human, ran this command.
    ///
    /// [`Self::author_kind`] is authoritative when the integration that captured the entry set it.
    /// Otherwise we guess: a known agent name identifies an agent, unless it is also the username
    /// recorded in the entry's origin, in which case the author is just the default it fell back
    /// to and tells us nothing. That exception is what stops a user called `pi` from looking like
    /// the `pi` agent.
    #[must_use]
    pub fn is_agent(&self) -> bool {
        match self.author_kind {
            Some(kind) => kind == AuthorKind::Agent,
            None => {
                // The username the author would have defaulted to. `CmdOrigin::parse_lenient`
                // maps a legacy colonless hostname to a placeholder user ("unknown-user"), but
                // old writers defaulted the author to the whole hostname there — as does the SQL
                // author filter's user expression — so compare against the host in that case.
                let user = self.cmd_origin.user();
                let defaulted = if user.as_ref() == UNKNOWN_USER {
                    self.cmd_origin.host().into_inner()
                } else {
                    user.into_inner()
                };
                is_known_agent(&self.author) && self.author != defaulted
            }
        }
    }

    /// Serializes a history entry in the V2 format.
    ///
    /// Differences from V1:
    ///
    /// * `intent` is always written; if `None`, nil is written to the output.
    /// * Added new field `shell`.
    ///
    /// V2 is designed to allow new fields to be added without incrementing the version. V1 cannot
    /// accommodate this because its deserialization routine errors if more than 11 fields are
    /// provided.
    pub fn serialize(&self) -> Result<DecryptedData, EncodeError> {
        let mut output = ByteBuf::new();

        // write the version
        encode::write_u16(&mut output, Version::LATEST.as_int())?;
        encode::write_array_len(&mut output, LATEST_SERIALIZED_FIELDS)?;

        encode::write_str(&mut output, &self.id.0)?;
        encode::write_u64(&mut output, self.timestamp.unix_timestamp_nanos() as u64)?;
        encode::write_sint(&mut output, self.duration)?;
        encode::write_sint(&mut output, self.exit)?;
        encode::write_str(&mut output, &self.command)?;
        encode::write_str(&mut output, &self.cwd)?;
        encode::write_str(&mut output, &self.session)?;
        encode::write_str(&mut output, self.cmd_origin.as_str())?;

        encode::write_optional(
            &mut output,
            self.deleted_at.map(|d| d.unix_timestamp_nanos() as u64),
            encode::write_u64,
        )?;
        encode::write_str(&mut output, self.author.as_str())?;
        encode::write_optional(&mut output, self.intent.as_deref(), encode::write_str)?;
        encode::write_optional(&mut output, self.shell.as_deref(), encode::write_str)?;
        encode::write_optional(
            &mut output,
            self.author_kind.map(AuthorKind::as_u8),
            |output, kind| encode::write_uint8(output, kind).map(|_marker| ()),
        )?;
        Ok(DecryptedData(output.into_vec()))
    }

    pub fn deserialize(bytes: &[u8], version: &str) -> Result<Self> {
        let Some(version) = Version::from_name(version) else {
            bail!("unknown version {version:?}");
        };

        let mut bytes = Bytes::new(bytes);

        let real_version = decode::read_u16(&mut bytes).map_err(DecodeError::from)?;
        if real_version != version.as_int() {
            bail!("expected to decode {version} record, found v{real_version}");
        }

        let nfields = decode::read_array_len(&mut bytes).map_err(DecodeError::from)?;
        let min_fields = version.min_fields();
        if nfields < min_fields || version.max_fields().is_some_and(|max| nfields > max) {
            bail!("unexpected number of fields ({nfields}) for history version {version}");
        }

        let id = decode::read_string(&mut bytes)?;
        let timestamp = decode::read_u64(&mut bytes).map_err(DecodeError::from)?;
        let duration = decode::read_int(&mut bytes).map_err(DecodeError::from)?;
        let exit = decode::read_int(&mut bytes).map_err(DecodeError::from)?;

        let command = decode::read_string(&mut bytes)?;
        let cwd = decode::read_string(&mut bytes)?;
        let session = decode::read_string(&mut bytes)?;
        #[allow(deprecated)]
        let cmd_origin = CmdOrigin::parse_lenient(decode::read_string(&mut bytes)?);
        let deleted_at = decode::read_optional(&mut bytes, decode::read_u64)?;

        let author = if version >= Version::One {
            decode::read_optional(&mut bytes, decode::read_string)?
        } else {
            None
        };

        let intent = if match version {
            Version::Zero => false,
            Version::One => nfields > min_fields,
            Version::Two => true,
        } {
            decode::read_optional(&mut bytes, decode::read_string)?
        } else {
            None
        };

        let shell = if version >= Version::Two {
            decode::read_optional(&mut bytes, decode::read_string)?
        } else {
            None
        };

        let author_kind = if version >= Version::Two && nfields >= V2_AUTHOR_KIND_FIELD_NUMBER {
            decode::read_optional(&mut bytes, decode::read_int::<u8, _>)?
                .and_then(AuthorKind::from_repr)
        } else {
            None
        };

        if version < Version::Two && !bytes.remaining_slice().is_empty() {
            bail!("trailing bytes in encoded history. malformed");
        }

        Ok(Self {
            id: id.into(),
            timestamp: OffsetDateTime::from_unix_nanos_u64(timestamp),
            duration,
            exit,
            command,
            cwd,
            session,
            author: author.unwrap_or_else(|| cmd_origin.user().to_string()),
            cmd_origin,
            intent,
            deleted_at: deleted_at.map(OffsetDateTime::from_unix_nanos_u64),
            shell,
            author_kind,
        })
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

    /// Builder for a history entry that is captured via hook, and sent to the daemon.
    ///
    /// This builder is used only at the `start` step of the hook,
    /// so it doesn't have any fields which are known only after
    /// the command is finished, such as `exit` or `duration`.
    ///
    /// It does, however, include information that can usually be inferred.
    ///
    /// This is because the daemon we are sending a request to lacks the context of the command
    ///
    /// ## Examples
    /// ```rust
    /// use atuin_client::history::History;
    ///
    /// let history: History = History::daemon()
    ///     .timestamp(time::OffsetDateTime::now_utc())
    ///     .command("ls -la")
    ///     .cwd("/home/user")
    ///     .session("018deb6e8287781f9973ef40e0fde76b")
    ///     .cmd_origin(atuin_domain::record::CmdOrigin::try_from("computer:ellie").unwrap())
    ///     .build()
    ///     .into();
    /// ```
    ///
    /// Command without any required info cannot be captured, which is forced at compile time:
    ///
    /// ```compile_fail
    /// use atuin_client::history::History;
    ///
    /// // this will not compile because `hostname` is missing
    /// let history: History = History::daemon()
    ///     .timestamp(time::OffsetDateTime::now_utc())
    ///     .command("ls -la")
    ///     .cwd("/home/user")
    ///     .session("018deb6e8287781f9973ef40e0fde76b")
    ///     .build()
    ///     .into();
    /// ```
    pub fn daemon() -> builder::HistoryDaemonCaptureBuilder {
        builder::HistoryDaemonCapture::builder()
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
    ///     .author("user".to_string())
    ///     .intent(None)
    ///     .deleted_at(None)
    ///     .shell(None)
    ///     .build()
    ///     .into();
    /// ```
    pub fn from_db() -> builder::HistoryFromDbBuilder {
        builder::HistoryFromDb::builder()
    }

    #[must_use]
    pub fn success(&self) -> bool {
        self.exit == 0 || self.duration == -1
    }

    pub fn should_save(&self, settings: &Settings) -> bool {
        !(self.command.starts_with(' ')
            || self.command.is_empty()
            || settings.history_filter.is_match(&self.command)
            || settings.cwd_filter.is_match(&self.cwd)
            || (settings.secrets_filter && SECRET_PATTERNS_RE.is_match(&self.command)))
    }
}

#[cfg(test)]
mod tests {
    use atuin_common::filter::OrFilter;
    use atuin_domain::record::CmdOrigin;
    use regex::RegexSet;
    use rstest::*;
    use time::macros::datetime;

    use super::{AuthorKind, AuthorPattern, History, all_user_author_filter, is_known_agent};
    use crate::history::Version;
    use crate::settings::Settings;

    /// Whether an author filter permits `history`, mirroring the SQL that
    /// [`apply_author_filter`](crate::database::OptFilters::authors) builds.
    ///
    /// There are only three ways a filter can admit an author, so each is one binary search rather
    /// than a scan that reinterprets every element in turn. No guard against an author *named*
    /// `$all-agent` is needed: such an author is an [`AuthorPattern::Name`], a different value from
    /// [`AuthorPattern::AllAgent`].
    fn author_matches_filters(history: &History, filters: OrFilter<&[AuthorPattern]>) -> bool {
        // `contains` is true for an "all" filter, so that case needs no separate check.
        filters.contains(&AuthorPattern::Name(history.author.clone()))
            || (filters.contains(&AuthorPattern::AllUser) && !history.is_agent())
            || (filters.contains(&AuthorPattern::AllAgent) && history.is_agent())
    }

    fn entry(author: &str, origin: &str, author_kind: Option<AuthorKind>) -> History {
        #[allow(deprecated, reason = "the bare-hostname test case has no `:` separator")]
        History::import()
            .timestamp(time::OffsetDateTime::now_utc())
            .command("git status")
            .cmd_origin(CmdOrigin::parse_lenient(origin))
            .author(author)
            .author_kind(author_kind)
            .build()
            .into()
    }

    #[fixture]
    fn privacy_settings() -> Settings {
        Settings {
            cwd_filter: RegexSet::new(["^/supasecret"]).unwrap(),
            history_filter: RegexSet::new(["^psql"]).unwrap(),
            ..Settings::utc()
        }
    }

    // Test that we don't save history where necessary
    #[rstest]
    #[case::normal("echo foo", "/", true)]
    #[case::leading_space(" echo bar", "/", false)]
    #[case::empty("", "/", false)]
    #[case::stripe_key("curl foo.com/bar?key=sk_test_1234567890abcdefghijklmnop", "/", false)]
    #[case::secret_dir("echo ohno", "/supasecret", false)]
    #[case::psql("psql", "/supasecret", false)]
    fn should_save_respects_privacy(
        #[from(privacy_settings)] settings: Settings,
        #[case] command: &str,
        #[case] cwd: &str,
        #[case] expected: bool,
    ) {
        let history: History = History::capture()
            .timestamp(time::OffsetDateTime::now_utc())
            .command(command)
            .cwd(cwd)
            .build()
            .into();
        assert_eq!(history.should_save(&settings), expected);
    }

    /// The SQL author filter derives its recognised-kind list from [`AuthorKind::VARIANTS`] while
    /// Rust decoding goes through [`AuthorKind::from_repr`]; a value present in one but not the
    /// other would split the two classifiers, so pin them to agree over the whole u8 range.
    #[test]
    fn author_kind_variants_and_from_repr_agree() {
        for value in 0..=u8::MAX {
            assert_eq!(
                AuthorKind::from_repr(value),
                AuthorKind::VARIANTS.iter().copied().find(|kind| kind.as_u8() == value),
                "{value}"
            );
        }
    }

    /// The capture path treats an explicitly stated author as an authorship claim: a known agent
    /// name there marks the entry as an agent's — unless it is also the current username, which
    /// is what a human's author defaults to and so says nothing (the same exception
    /// [`History::is_agent`] applies). An explicit kind still wins over the inference.
    #[rstest]
    #[case::known_agent_name("pi", "raspberry:ellie", None, Some(AuthorKind::Agent))]
    #[case::agent_name_is_the_username("pi", "raspberry:pi", None, None)]
    #[case::human_name("ellie", "raspberry:ellie", None, None)]
    #[case::explicit_kind_wins(
        "pi",
        "raspberry:pi",
        Some(AuthorKind::Agent),
        Some(AuthorKind::Agent)
    )]
    fn capture_infers_agent_kind_from_an_explicit_author(
        #[case] author: &str,
        #[case] origin: &str,
        #[case] stated_kind: Option<AuthorKind>,
        #[case] expected: Option<AuthorKind>,
    ) {
        let history: History = History::capture()
            .timestamp(time::OffsetDateTime::now_utc())
            .command("git status")
            .cwd("/")
            .author(author)
            .cmd_origin(CmdOrigin::try_from(origin.to_owned()).unwrap())
            .author_kind_opt(stated_kind)
            .build()
            .into();
        assert_eq!(history.author_kind, expected);
    }

    #[rstest]
    fn known_agents_include_pi() {
        let agents = OrFilter::from_list(vec![AuthorPattern::AllAgent]).unwrap();
        let users = OrFilter::from_list(vec![AuthorPattern::AllUser]).unwrap();
        let pi = entry("pi", "raspberry:ellie", None);
        let ellie = entry("ellie", "raspberry:ellie", None);

        assert!(is_known_agent("pi"));
        assert!(author_matches_filters(&pi, agents.as_slice_filter()));
        assert!(!author_matches_filters(&pi, users.as_slice_filter()));
        assert!(!author_matches_filters(&ellie, agents.as_slice_filter()));
        assert!(author_matches_filters(&ellie, users.as_slice_filter()));
    }

    #[test]
    fn an_all_author_filter_matches_everyone() {
        let all = OrFilter::all();
        assert!(author_matches_filters(&entry("pi", "raspberry:ellie", None), all));
        assert!(author_matches_filters(&entry("ellie", "raspberry:ellie", None), all));
    }

    #[test]
    fn the_all_user_filter_excludes_agents() {
        let filter = all_user_author_filter();
        assert!(!author_matches_filters(&entry("pi", "raspberry:ellie", None), filter));
        assert!(author_matches_filters(&entry("ellie", "raspberry:ellie", None), filter));
    }

    /// An agent name is only evidence of an agent when it is not also the username: a user called
    /// `pi` gets `pi` as their author by default, and is not the `pi` agent.
    #[rstest]
    #[case::agent_name_on_another_users_machine("pi", "raspberry:ellie", None, true)]
    #[case::agent_name_is_the_username("pi", "raspberry:pi", None, false)]
    #[case::plain_user("ellie", "raspberry:ellie", None, false)]
    #[case::hostname_without_a_username("pi", "raspberry", None, true)]
    #[case::colonless_hostname_is_the_agent_name("pi", "pi", None, false)]
    // A stated kind is authoritative, which is the only way to tell the `pi` agent apart from the
    // `pi` user on their own machine.
    #[case::stated_agent("pi", "raspberry:pi", Some(AuthorKind::Agent), true)]
    #[case::stated_user("pi", "raspberry:ellie", Some(AuthorKind::User), false)]
    #[case::stated_agent_with_a_human_name(
        "ellie",
        "raspberry:ellie",
        Some(AuthorKind::Agent),
        true
    )]
    fn is_agent_uses_the_username_when_no_kind_was_stated(
        #[case] author: &str,
        #[case] origin: &str,
        #[case] author_kind: Option<AuthorKind>,
        #[case] expected: bool,
    ) {
        assert_eq!(entry(author, origin, author_kind).is_agent(), expected);
    }

    #[rstest]
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

    #[rstest]
    #[case::basic(History {
        id: "66d16cbee7cd47538e5c5b8b44e9006e".to_owned().into(),
        timestamp: datetime!(2023-05-28 18:35:40.633872 +00:00),
        duration: 49206000,
        exit: 0,
        command: "git status".to_owned(),
        cwd: "/Users/conrad.ludgate/Documents/code/atuin".to_owned(),
        session: "b97d9a306f274473a203d2eba41f9457".to_owned(),
        cmd_origin: CmdOrigin::try_from("fvfg936c0kpf:conrad.ludgate").unwrap(),
        author: "conrad.ludgate".to_owned(),
        intent: None,
        deleted_at: None,
        shell: None,
        author_kind: None,
    })]
    #[case::deleted(History {
        id: "66d16cbee7cd47538e5c5b8b44e9006e".to_owned().into(),
        timestamp: datetime!(2023-05-28 18:35:40.633872 +00:00),
        duration: 49206000,
        exit: 0,
        command: "git status".to_owned(),
        cwd: "/Users/conrad.ludgate/Documents/code/atuin".to_owned(),
        session: "b97d9a306f274473a203d2eba41f9457".to_owned(),
        cmd_origin: CmdOrigin::try_from("fvfg936c0kpf:conrad.ludgate").unwrap(),
        author: "conrad.ludgate".to_owned(),
        intent: None,
        deleted_at: Some(datetime!(2023-11-19 20:18 +00:00)),
        shell: Some("bash".into()),
        author_kind: Some(AuthorKind::User),
    })]
    #[case::with_author_and_intent(History {
        id: "66d16cbee7cd47538e5c5b8b44e9006e".to_owned().into(),
        timestamp: datetime!(2023-05-28 18:35:40.633872 +00:00),
        duration: 49206000,
        exit: 0,
        command: "git status".to_owned(),
        cwd: "/Users/conrad.ludgate/Documents/code/atuin".to_owned(),
        session: "b97d9a306f274473a203d2eba41f9457".to_owned(),
        cmd_origin: CmdOrigin::try_from("fvfg936c0kpf:conrad.ludgate").unwrap(),
        author: "claude".to_owned(),
        intent: Some("check repository status".to_owned()),
        deleted_at: None,
        shell: Some("fish".into()),
        author_kind: Some(AuthorKind::Agent),
    })]
    fn serialize_deserialize_roundtrip(#[case] history: History) {
        let serialized = history.serialize().expect("failed to serialize history");
        assert_eq!(&serialized.0[0..3], [205, 0, 2], "should encode as history v2");

        let deserialized = History::deserialize(&serialized.0, Version::LATEST.name())
            .expect("failed to deserialize history");
        assert_eq!(history, deserialized);
    }

    const BYTES_V0: &[u8] = &[
        205, 0, 0, 153, 217, 32, 54, 54, 100, 49, 54, 99, 98, 101, 101, 55, 99, 100, 52, 55, 53,
        51, 56, 101, 53, 99, 53, 98, 56, 98, 52, 52, 101, 57, 48, 48, 54, 101, 207, 23, 99, 98,
        117, 24, 210, 246, 128, 206, 2, 238, 210, 240, 0, 170, 103, 105, 116, 32, 115, 116, 97,
        116, 117, 115, 217, 42, 47, 85, 115, 101, 114, 115, 47, 99, 111, 110, 114, 97, 100, 46,
        108, 117, 100, 103, 97, 116, 101, 47, 68, 111, 99, 117, 109, 101, 110, 116, 115, 47, 99,
        111, 100, 101, 47, 97, 116, 117, 105, 110, 217, 32, 98, 57, 55, 100, 57, 97, 51, 48, 54,
        102, 50, 55, 52, 52, 55, 51, 97, 50, 48, 51, 100, 50, 101, 98, 97, 52, 49, 102, 57, 52, 53,
        55, 187, 102, 118, 102, 103, 57, 51, 54, 99, 48, 107, 112, 102, 58, 99, 111, 110, 114, 97,
        100, 46, 108, 117, 100, 103, 97, 116, 101, 192,
    ];

    const BYTES_V1: &[u8] = &[
        205, 0, 1, 155, 217, 32, 54, 54, 100, 49, 54, 99, 98, 101, 101, 55, 99, 100, 52, 55, 53,
        51, 56, 101, 53, 99, 53, 98, 56, 98, 52, 52, 101, 57, 48, 48, 54, 101, 207, 23, 99, 98,
        117, 24, 210, 246, 128, 206, 2, 238, 210, 240, 0, 170, 103, 105, 116, 32, 115, 116, 97,
        116, 117, 115, 217, 42, 47, 85, 115, 101, 114, 115, 47, 99, 111, 110, 114, 97, 100, 46,
        108, 117, 100, 103, 97, 116, 101, 47, 68, 111, 99, 117, 109, 101, 110, 116, 115, 47, 99,
        111, 100, 101, 47, 97, 116, 117, 105, 110, 217, 32, 98, 57, 55, 100, 57, 97, 51, 48, 54,
        102, 50, 55, 52, 52, 55, 51, 97, 50, 48, 51, 100, 50, 101, 98, 97, 52, 49, 102, 57, 52, 53,
        55, 187, 102, 118, 102, 103, 57, 51, 54, 99, 48, 107, 112, 102, 58, 99, 111, 110, 114, 97,
        100, 46, 108, 117, 100, 103, 97, 116, 101, 207, 24, 194, 83, 235, 108, 206, 10, 0, 174, 99,
        111, 110, 114, 97, 100, 46, 108, 117, 100, 103, 97, 116, 101, 173, 115, 97, 109, 112, 108,
        101, 32, 105, 110, 116, 101, 110, 116,
    ];

    fn expected_v2() -> History {
        History {
            id: "66d16cbee7cd47538e5c5b8b44e9006e".to_owned().into(),
            timestamp: datetime!(2023-05-28 18:35:40.633872 +00:00),
            duration: 49206000,
            exit: 0,
            command: "git status".to_owned(),
            cwd: "/Users/conrad.ludgate/Documents/code/atuin".to_owned(),
            session: "b97d9a306f274473a203d2eba41f9457".to_owned(),
            cmd_origin: CmdOrigin::try_from("fvfg936c0kpf:conrad.ludgate").unwrap(),
            author: "conrad.ludgate".to_owned(),
            intent: Some("sample intent".to_owned()),
            deleted_at: Some(time::OffsetDateTime::from_unix_timestamp(1784080673).unwrap()),
            shell: Some("zsh".into()),
            author_kind: None,
        }
    }

    fn expected_v1() -> History {
        History {
            shell: None,
            ..expected_v2()
        }
    }

    fn expected_v0() -> History {
        History {
            intent: None,
            deleted_at: None,
            ..expected_v1()
        }
    }

    /// A V2 record from before `author_kind` was appended: same version, one field short. New
    /// fields are only read when the encoded array is long enough to hold them, so this must still
    /// decode rather than error or misread the missing field.
    #[test]
    fn deserialize_v2_written_without_author_kind() {
        let history = History {
            author_kind: Some(AuthorKind::Agent),
            ..expected_v2()
        };
        let mut bytes = history.serialize().unwrap().0;

        assert_eq!(bytes[3], 0x90 | 13, "v2 should encode 13 fields");
        bytes[3] = 0x90 | 12;
        bytes.pop();

        assert_eq!(History::deserialize(&bytes, Version::Two.name()).unwrap(), History {
            author_kind: None,
            ..history
        });
    }

    #[rstest]
    #[case::from_v0(Version::Zero, BYTES_V0, expected_v0())]
    #[case::from_v1(Version::One, BYTES_V1, expected_v1())]
    #[case::from_v2(Version::Two, &expected_v2().serialize().unwrap(), expected_v2())]
    fn deserialize_across_versions(
        #[case] source: Version,
        #[case] bytes: &[u8],
        #[case] expected: History,
        #[values(Version::Zero, Version::One, Version::Two)] decode_as: Version,
    ) {
        let got = History::deserialize(bytes, decode_as.name());
        if decode_as == source {
            assert_eq!(got.unwrap(), expected, "{decode_as}");
        } else {
            assert!(got.is_err(), "unexpected success deserializing as {decode_as}");
        }
    }
}
