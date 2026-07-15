//! A newtype wrapper for shell command strings.
//!
//! [`Command`] is generic over the storage `S` that holds the command text, so one
//! wrapper covers borrowed, owned and clone-on-write commands:
//!
//! | Alias | Storage | Plays the role of |
//! |---|---|---|
//! | [`CommandStr<'a>`] | `&'a str` | `&'a str` |
//! | [`CommandString`] | `String` | `String` |
//! | [`CommandCow<'a>`] | `Cow<'a, str>` | `Cow<'a, str>` |
//!
//! Any other storage that is `AsRef<str>` (`Arc<str>`, `Box<str>`, ...) works with no
//! extra code: `Command<Arc<str>>` is a command too.
//!
//! The wrapper stores its string verbatim. The one structural invariant it can
//! check is that a command contains no NUL byte — use [`CommandString::try_from`]
//! at the boundary where an untrusted string becomes a command.
//!
//! ```
//! use std::borrow::Cow;
//!
//! use atuin_common::command_str::{CommandCow, CommandStr, CommandString};
//!
//! let borrowed: CommandStr<'_> = CommandStr::new("cargo test");
//! let owned: CommandString = borrowed.to_command_string();
//! let cow: CommandCow<'_> = CommandCow::new(Cow::Borrowed("cargo test"));
//!
//! // The specialisations compare by command text, whatever they are stored in.
//! assert_eq!(borrowed, owned);
//! assert_eq!(owned, cow);
//!
//! // Borrow any of them back down to a `CommandStr`, or read the text directly.
//! assert_eq!(owned.as_command_str(), borrowed);
//! assert_eq!(cow.as_str(), "cargo test");
//!
//! // Validated construction rejects a NUL byte but accepts multi-line commands.
//! assert!(CommandString::try_from("git commit -m 'a\nb'").is_ok());
//! assert!(CommandString::try_from("oops\0nul").is_err());
//! ```

use std::{
    borrow::{Borrow, Cow},
    cmp::Ordering,
    fmt,
    hash::{Hash, Hasher},
    ops::Deref,
};

use serde::{Deserialize, Serialize};

/// A shell command, generic over the string storage `S`.
///
/// Use the [`CommandStr`], [`CommandString`] and [`CommandCow`] aliases rather than
/// naming this type directly.
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize)]
#[serde(transparent)]
#[repr(transparent)]
pub struct Command<S>(S);

/// A borrowed command. Plays the role of `&str`.
pub type CommandStr<'a> = Command<&'a str>;

/// An owned command. Plays the role of `String`.
pub type CommandString = Command<String>;

/// A clone-on-write command. Plays the role of `Cow<'_, str>`.
pub type CommandCow<'a> = Command<Cow<'a, str>>;

/// The reason a raw string could not be used as a [`Command`].
///
/// Returned by the validating `TryFrom` conversions (see [`CommandString`]'s
/// `TryFrom<&str>` impl). It is deliberately narrow: the only structural
/// invariant a shell command must hold — beyond being valid UTF-8, which the
/// `str`/`String` storage already guarantees — is that it contains no NUL byte.
/// Newlines (multi-line commands), tabs, other control characters and an empty
/// string are all valid commands and are **not** rejected.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum InvalidCommand {
    /// The string contains a NUL byte (`\0`), at the given byte index.
    ///
    /// A shell command can never legitimately contain a NUL: it cannot be
    /// captured through `argv` or an environment variable (both are
    /// NUL-terminated), and it is silently truncated when replayed through the
    /// shell's `$(...)` command substitution — so a stored NUL corrupts the
    /// command rather than representing anything runnable.
    #[error("command contains a NUL byte at index {index}")]
    ContainsNul {
        /// Byte index of the first NUL in the rejected string.
        index: usize,
    },
}

impl<S> Command<S> {
    /// Wrap `inner` as a command.
    pub const fn new(inner: S) -> Self {
        Self(inner)
    }

    /// Borrow the storage this command is held in.
    pub const fn inner(&self) -> &S {
        &self.0
    }

    /// Unwrap this command into the storage it is held in.
    pub fn into_inner(self) -> S {
        self.0
    }
}

impl<S: AsRef<str>> Command<S> {
    /// The command text.
    pub fn as_str(&self) -> &str {
        self.0.as_ref()
    }

    /// Borrow this command as a [`CommandStr`], whatever storage it is held in.
    pub fn as_command_str(&self) -> CommandStr<'_> {
        Command(self.0.as_ref())
    }

    /// Copy this command into an owned [`CommandString`].
    pub fn to_command_string(&self) -> CommandString {
        Command(self.0.as_ref().to_owned())
    }
}

impl<S: AsRef<str>> AsRef<str> for Command<S> {
    fn as_ref(&self) -> &str {
        self.0.as_ref()
    }
}

impl<S: AsRef<str>> Borrow<str> for Command<S> {
    fn borrow(&self) -> &str {
        self.0.as_ref()
    }
}

impl<S: AsRef<str>> Deref for Command<S> {
    type Target = str;

    fn deref(&self) -> &str {
        self.0.as_ref()
    }
}

impl<S: AsRef<str>, T: AsRef<str>> PartialEq<Command<T>> for Command<S> {
    fn eq(&self, other: &Command<T>) -> bool {
        self.as_str() == other.as_str()
    }
}

impl<S: AsRef<str>> Eq for Command<S> {}

impl<S: AsRef<str>, T: AsRef<str>> PartialOrd<Command<T>> for Command<S> {
    fn partial_cmp(&self, other: &Command<T>) -> Option<Ordering> {
        Some(self.as_str().cmp(other.as_str()))
    }
}

impl<S: AsRef<str>> Ord for Command<S> {
    fn cmp(&self, other: &Self) -> Ordering {
        self.as_str().cmp(other.as_str())
    }
}

impl<S: AsRef<str>> Hash for Command<S> {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.as_str().hash(state);
    }
}

impl<S: AsRef<str>> fmt::Display for Command<S> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.pad(self.as_str())
    }
}

impl<S> From<S> for Command<S> {
    fn from(inner: S) -> Self {
        Self(inner)
    }
}

/// Validates an untrusted string slice into an owned [`CommandString`],
/// rejecting any command that contains a NUL byte (see [`InvalidCommand`]).
///
/// This is the checked entry point at the raw-string boundary. The infallible
/// constructors ([`Command::new`] and the `From` conversions between storages)
/// still exist and do **not** validate, so validation here is opt-in rather than
/// an enforced invariant — reach for `try_from` when the string comes from an
/// untrusted source such as an imported history file.
impl TryFrom<&str> for CommandString {
    type Error = InvalidCommand;

    fn try_from(command: &str) -> Result<Self, Self::Error> {
        if let Some(index) = command.find('\0') {
            return Err(InvalidCommand::ContainsNul { index });
        }
        Ok(Command(command.to_owned()))
    }
}

impl From<CommandStr<'_>> for CommandString {
    fn from(command: CommandStr<'_>) -> Self {
        Command(command.0.to_owned())
    }
}

impl From<CommandCow<'_>> for CommandString {
    fn from(command: CommandCow<'_>) -> Self {
        Command(command.0.into_owned())
    }
}

impl<'a> From<CommandStr<'a>> for CommandCow<'a> {
    fn from(command: CommandStr<'a>) -> Self {
        Command(Cow::Borrowed(command.0))
    }
}

impl From<CommandString> for CommandCow<'_> {
    fn from(command: CommandString) -> Self {
        Command(Cow::Owned(command.0))
    }
}

#[cfg(test)]
mod tests {
    use std::{borrow::Cow, collections::HashMap, sync::Arc};

    use pretty_assertions::assert_eq;

    use super::{Command, CommandCow, CommandStr, CommandString, InvalidCommand};

    #[test]
    fn wraps_borrowed_owned_and_cow_storage() {
        let borrowed = CommandStr::new("ls -la");
        let owned = CommandString::new(String::from("ls -la"));
        let cow = CommandCow::new(Cow::Borrowed("ls -la"));

        assert_eq!(*borrowed.inner(), "ls -la");
        assert_eq!(owned.into_inner(), String::from("ls -la"));
        assert_eq!(cow.into_inner(), Cow::Borrowed("ls -la"));
    }

    #[test]
    fn supports_any_string_storage() {
        let cmd: Command<Arc<str>> = Command::new(Arc::from("git status"));

        assert_eq!(&*cmd.into_inner(), "git status");
    }

    #[test]
    fn borrowed_commands_are_copy() {
        let cmd = CommandStr::new("echo hi");
        let copied = cmd;

        // `cmd` is still usable after the move above, which only compiles if it is `Copy`.
        assert_eq!(*cmd.inner(), *copied.inner());
    }

    #[test]
    fn owned_commands_default_to_empty() {
        assert_eq!(CommandString::default().into_inner(), String::new());
    }

    #[test]
    fn as_str_exposes_the_command_text_for_every_storage() {
        assert_eq!(CommandStr::new("ls").as_str(), "ls");
        assert_eq!(CommandString::new(String::from("ls")).as_str(), "ls");
        assert_eq!(
            CommandCow::new(Cow::Owned(String::from("ls"))).as_str(),
            "ls"
        );
        assert_eq!(Command::new(Arc::<str>::from("ls")).as_str(), "ls");
    }

    #[test]
    fn converts_between_borrowed_and_owned() {
        let owned = CommandString::new(String::from("cargo test"));

        let borrowed: CommandStr<'_> = owned.as_command_str();
        assert_eq!(borrowed.as_str(), "cargo test");

        let round_tripped: CommandString = borrowed.to_command_string();
        assert_eq!(round_tripped.as_str(), "cargo test");
    }

    #[test]
    fn any_storage_can_be_borrowed_as_a_command_str() {
        let cow = CommandCow::new(Cow::Borrowed("git push"));

        assert_eq!(cow.as_command_str().as_str(), "git push");
    }

    #[test]
    fn derefs_and_as_refs_to_str() {
        let cmd = CommandString::new(String::from("git commit -m wip"));

        // `Deref<Target = str>` gives us the `str` API without re-implementing any of it.
        assert_eq!(cmd.len(), 17);
        assert!(cmd.starts_with("git"));

        let as_ref: &str = cmd.as_ref();
        assert_eq!(as_ref, "git commit -m wip");
    }

    #[test]
    fn equality_is_content_based_across_storages() {
        let borrowed = CommandStr::new("ls -la");
        let owned = CommandString::new(String::from("ls -la"));
        let cow = CommandCow::new(Cow::Borrowed("ls -la"));

        assert_eq!(borrowed, owned);
        assert_eq!(owned, cow);
        assert_eq!(cow, borrowed);
        assert_ne!(borrowed, CommandStr::new("ls"));
    }

    #[test]
    fn ordering_is_content_based() {
        let mut cmds = [
            CommandString::new(String::from("git status")),
            CommandString::new(String::from("cargo test")),
        ];
        cmds.sort();

        assert_eq!(cmds[0].as_str(), "cargo test");
        assert_eq!(cmds[1].as_str(), "git status");
        assert!(CommandStr::new("a") < CommandStr::new("b"));
    }

    #[test]
    fn hashes_like_the_underlying_str() {
        let mut counts = HashMap::new();
        counts.insert(CommandString::new(String::from("ls -la")), 3_u64);

        // `Borrow<str>` + a str-consistent `Hash` let us probe the map with a plain `&str`.
        assert_eq!(counts.get("ls -la"), Some(&3));
        assert_eq!(counts.get("ls"), None);
    }

    #[test]
    fn displays_as_the_raw_command() {
        assert_eq!(CommandStr::new("echo hi").to_string(), "echo hi");
        assert_eq!(
            CommandString::new(String::from("echo hi")).to_string(),
            "echo hi"
        );

        // Width and alignment are forwarded to the underlying `str`.
        assert_eq!(format!("[{:>4}]", CommandStr::new("ls")), "[  ls]");
    }

    #[test]
    fn wraps_any_storage_via_from() {
        let borrowed: CommandStr<'_> = "ls".into();
        let owned: CommandString = String::from("ls").into();
        let cow: CommandCow<'_> = Cow::Borrowed("ls").into();

        assert_eq!(borrowed, owned);
        assert_eq!(owned, cow);
    }

    #[test]
    fn converts_between_the_specialisations_via_from() {
        let from_borrowed: CommandString = CommandStr::new("ls").into();
        let from_cow: CommandString = CommandCow::new(Cow::Owned(String::from("ls"))).into();

        let borrowed_to_cow: CommandCow<'_> = CommandStr::new("ls").into();
        let owned_to_cow: CommandCow<'_> = CommandString::new(String::from("ls")).into();

        assert_eq!(from_borrowed.as_str(), "ls");
        assert_eq!(from_cow.as_str(), "ls");

        // A borrowed command stays borrowed and an owned one stays owned: neither
        // conversion allocates or copies where it does not have to.
        assert_eq!(borrowed_to_cow.as_str(), "ls");
        assert!(matches!(borrowed_to_cow.into_inner(), Cow::Borrowed(_)));
        assert_eq!(owned_to_cow.as_str(), "ls");
        assert!(matches!(owned_to_cow.into_inner(), Cow::Owned(_)));
    }

    #[test]
    fn serialises_transparently_as_a_bare_string() {
        let cmd = CommandString::new(String::from("ls -la"));

        assert_eq!(serde_json::to_string(&cmd).unwrap(), "\"ls -la\"");
    }

    #[test]
    fn deserialises_from_a_bare_string() {
        let cmd: CommandString = serde_json::from_str("\"ls -la\"").unwrap();

        assert_eq!(cmd.as_str(), "ls -la");
    }

    #[test]
    fn deserialises_a_borrowed_command_from_the_input() {
        let json = String::from("\"ls -la\"");

        let cmd: CommandStr<'_> = serde_json::from_str(&json).unwrap();

        assert_eq!(cmd, CommandStr::new("ls -la"));
    }

    #[test]
    fn try_from_accepts_a_normal_command() {
        let cmd = CommandString::try_from("git status").unwrap();

        assert_eq!(cmd.as_str(), "git status");
    }

    #[test]
    fn try_from_accepts_newlines_tabs_and_empty() {
        // Multi-line commands, tabs and the empty string are all legitimate and
        // must not be rejected — only NUL is structurally invalid.
        assert_eq!(
            CommandString::try_from("git commit -m 'line one\nline two'")
                .unwrap()
                .as_str(),
            "git commit -m 'line one\nline two'"
        );
        assert_eq!(
            CommandString::try_from("echo\t123").unwrap().as_str(),
            "echo\t123"
        );
        assert_eq!(CommandString::try_from("").unwrap().as_str(), "");
    }

    #[test]
    fn try_from_rejects_a_nul_byte_and_reports_its_index() {
        assert_eq!(
            CommandString::try_from("echo a\0b"),
            Err(InvalidCommand::ContainsNul { index: 6 })
        );
        // A leading NUL is reported at index 0.
        assert_eq!(
            CommandString::try_from("\0"),
            Err(InvalidCommand::ContainsNul { index: 0 })
        );
    }
}
