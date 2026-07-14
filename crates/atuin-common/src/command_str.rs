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
//! The wrapper adds no behaviour to the string it holds.

use std::{
    borrow::{Borrow, Cow},
    ops::Deref,
};

/// A shell command, generic over the string storage `S`.
///
/// Use the [`CommandStr`], [`CommandString`] and [`CommandCow`] aliases rather than
/// naming this type directly.
#[derive(Clone, Copy, Debug, Default)]
#[repr(transparent)]
pub struct Command<S>(S);

/// A borrowed command. Plays the role of `&str`.
pub type CommandStr<'a> = Command<&'a str>;

/// An owned command. Plays the role of `String`.
pub type CommandString = Command<String>;

/// A clone-on-write command. Plays the role of `Cow<'_, str>`.
pub type CommandCow<'a> = Command<Cow<'a, str>>;

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

#[cfg(test)]
mod tests {
    use std::{borrow::Cow, sync::Arc};

    use pretty_assertions::assert_eq;

    use super::{Command, CommandCow, CommandStr, CommandString};

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
}
