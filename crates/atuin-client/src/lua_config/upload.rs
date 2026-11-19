//! What a config file lets go up to Atuin for Teams, as its `upload` says: `threads` rules on
//! each thread, by where it started, and `history` on each command. Either is a boolean, or a
//! function that takes the [`Thread`] or [`Command`] and returns one.
//!
//! The user's own file turns uploading on, so there an unset rule keeps everything here. A
//! project's file can only keep things here, so there an unset rule lets everything go.
//!
//! A function runs in the VM its file ran in, so it sees what the file left there, under the same
//! memory limit and a time limit of its own.

use std::path::{Path, PathBuf};
use std::time::Duration;

use atuin_common::harnesstools::Harness as _;
use mlua::serde::SerializeOptions;
use mlua::{Function, Lua, LuaSerdeExt as _, Table, Value};
use serde::Serialize;
use thiserror::Error;

use super::{LoadError, run, vm};
use crate::ai_session::HarnessSession;
use crate::history::History;

/// The longest one call of a rule may run: a rule compares a few strings, so this only stops one
/// that never ends, without holding up the capture that asks it for long.
const CALL_TIME_LIMIT: Duration = Duration::from_millis(50);

/// A config file's `upload`, with the VM its functions run in.
///
/// It holds the VM, which can't leave the thread it was made on: ask it, then drop it.
#[derive(Debug)]
pub struct Upload {
    path: PathBuf,
    lua: Lua,
    rules: Rules,
}

impl Upload {
    /// The `upload` of the config file in `dir`, if it has one. Blocks as
    /// [`ConfigFile::in_dir`](super::ConfigFile::in_dir) does.
    pub fn in_dir(dir: &Path) -> Result<Option<Self>, LoadError> {
        Ok(run(dir)?.map(|(_, upload)| upload))
    }

    /// The `upload`s of the config files of the projects at and above `dir`, the nearest first,
    /// each run as it's reached.
    pub fn projects(dir: &Path) -> impl Iterator<Item = Result<Self, LoadError>> {
        dir.ancestors().filter_map(|ancestor| Self::in_dir(ancestor).transpose())
    }

    pub(super) const fn new(path: PathBuf, lua: Lua, rules: Rules) -> Self {
        Self { path, lua, rules }
    }

    /// The config file it's the `upload` of.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Whether the file lets `subject` go up, or `None` when it sets no rule for it. A function
    /// that fails, or returns anything but a boolean, fails it.
    pub fn allows<S: Subject>(&self, subject: &S) -> Result<Option<bool>, LoadError> {
        let predicate = match self.rule(S::RULE) {
            None => return Ok(None),
            Some(Rule::Flag(allows)) => return Ok(Some(*allows)),
            Some(Rule::Predicate(predicate)) => predicate,
        };
        let allows = self.call(predicate, subject).map_err(|source| LoadError::Rule {
            path: self.path.clone(),
            rule: S::RULE.key(),
            source,
        })?;
        Ok(Some(allows))
    }

    /// Whether the file's rule `name` may let anything go up: `true` or a function.
    #[must_use]
    pub fn switches_on(&self, name: RuleName) -> bool {
        match self.rule(name) {
            None | Some(Rule::Flag(false)) => false,
            Some(Rule::Flag(true) | Rule::Predicate(_)) => true,
        }
    }

    const fn rule(&self, name: RuleName) -> Option<&Rule> {
        match name {
            RuleName::Threads => self.rules.threads.as_ref(),
            RuleName::History => self.rules.history.as_ref(),
        }
    }

    fn call(&self, predicate: &Function, subject: &impl Serialize) -> mlua::Result<bool> {
        // Lua reads a field that's absent as `nil`, which `null` isn't.
        let options = SerializeOptions::new().serialize_none_to_null(false);
        let subject = self.lua.to_value_with(subject, options)?;
        vm::limit_to(&self.lua, CALL_TIME_LIMIT);
        match predicate.call(subject)? {
            Value::Boolean(allows) => Ok(allows),
            other => Err(mlua::Error::external(RuleError::NotABoolean(other.type_name()))),
        }
    }
}

/// What the `upload` of a file sets.
#[derive(Debug, Default)]
pub(super) struct Rules {
    threads: Option<Rule>,
    history: Option<Rule>,
}

impl Rules {
    /// Reads the `upload` of `atuin.config`, where `atuin` is the `atuin` global as a file left
    /// it, and the rest of `atuin` with `rest`, which serde does and can't for a function.
    ///
    /// `upload` is out of `atuin.config` while `rest` runs, and back after, as the file's
    /// functions may read it. An `atuin` that isn't a table, or whose `config` isn't, has none,
    /// and fails `rest` instead.
    pub(super) fn read_beside<T>(
        atuin: Value,
        rest: impl FnOnce(Value) -> mlua::Result<T>,
    ) -> mlua::Result<(T, Self)> {
        let config = match &atuin {
            Value::Table(atuin) => atuin.raw_get::<Value>("config")?,
            _ => Value::Nil,
        };
        let Value::Table(config) = config else {
            return Ok((rest(atuin)?, Self::default()));
        };
        let upload = config.raw_get::<Value>("upload")?;
        config.raw_set("upload", Value::Nil)?;
        let read = rest(atuin);
        config.raw_set("upload", upload.clone())?;
        let rules = match upload {
            Value::Nil => Self::default(),
            Value::Table(upload) => Self::read(&upload)?,
            other => return Err(mlua::Error::external(RuleError::NotATable(other.type_name()))),
        };
        Ok((read?, rules))
    }

    fn read(upload: &Table) -> mlua::Result<Self> {
        let mut rules = Self::default();
        upload.for_each(|key: String, value: Value| {
            let name = match key.as_str() {
                "threads" => RuleName::Threads,
                "history" => RuleName::History,
                _ => return Err(mlua::Error::external(RuleError::UnknownKey(key))),
            };
            let rule = Some(Rule::read(name, value)?);
            match name {
                RuleName::Threads => rules.threads = rule,
                RuleName::History => rules.history = rule,
            }
            Ok(())
        })?;
        Ok(rules)
    }
}

#[derive(Debug)]
enum Rule {
    Flag(bool),
    Predicate(Function),
}

impl Rule {
    fn read(name: RuleName, value: Value) -> mlua::Result<Self> {
        match value {
            Value::Boolean(flag) => Ok(Self::Flag(flag)),
            Value::Function(predicate) => Ok(Self::Predicate(predicate)),
            other => Err(mlua::Error::external(RuleError::NotARule {
                key: name.key(),
                type_name: other.type_name(),
            })),
        }
    }
}

/// The key of `upload` that rules on a [`Subject`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RuleName {
    Threads,
    History,
}

impl RuleName {
    #[must_use]
    pub const fn key(self) -> &'static str {
        match self {
            Self::Threads => "threads",
            Self::History => "history",
        }
    }
}

#[derive(Debug, Error)]
enum RuleError {
    #[error("`upload` must be a table, not a {0}")]
    NotATable(&'static str),
    #[error("`upload` has no `{0}`; it takes `threads` and `history`")]
    UnknownKey(String),
    #[error("`upload.{key}` must be a boolean or a function, not a {type_name}")]
    NotARule {
        key: &'static str,
        type_name: &'static str,
    },
    #[error("the function returned a {0}, not a boolean")]
    NotABoolean(&'static str),
}

mod sealed {
    pub trait Sealed {}
}

/// What an `upload` rule decides on, which reaches the rule's function as a table of its fields.
pub trait Subject: Serialize + sealed::Sealed {
    const RULE: RuleName;

    /// The directory whose config files rule on it, if it names one.
    fn dir(&self) -> Option<&Path>;

    /// Sets the workspace its directory is in: that of the nearest project's config file.
    fn set_workspace(&mut self, workspace: PathBuf);
}

/// A thread, as `upload.threads` sees it: what it is, and where it started.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Thread {
    /// The harness's name, as `claude-code`, or `unknown` for one atuin doesn't know.
    harness: &'static str,
    /// The harness's own id for it.
    id: String,
    dir: Option<PathBuf>,
    /// The directory of the nearest project's config file at or above `dir`, if there's one.
    workspace: Option<PathBuf>,
}

impl Thread {
    #[must_use]
    pub fn new(session: &HarnessSession, dir: Option<PathBuf>) -> Self {
        let harness = session.harness.harness();
        Self {
            harness: harness.as_ref().map_or("unknown", |harness| harness.name()),
            id: session.session.to_string(),
            dir,
            workspace: None,
        }
    }
}

impl sealed::Sealed for Thread {}

impl Subject for Thread {
    const RULE: RuleName = RuleName::Threads;

    fn dir(&self) -> Option<&Path> {
        self.dir.as_deref()
    }

    fn set_workspace(&mut self, workspace: PathBuf) {
        self.workspace = Some(workspace);
    }
}

/// A command, as `upload.history` sees it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Command {
    command: String,
    dir: PathBuf,
    exit: i64,
    /// How long it ran, in nanoseconds.
    duration: i64,
    author: String,
    /// Whether an agent ran it.
    agent: bool,
    intent: Option<String>,
    shell: Option<String>,
    /// As [`Thread`]'s.
    workspace: Option<PathBuf>,
}

impl From<&History> for Command {
    fn from(history: &History) -> Self {
        Self {
            command: history.command.clone(),
            dir: PathBuf::from(&history.cwd),
            exit: history.exit,
            duration: history.duration,
            author: history.author.clone(),
            agent: history.is_agent(),
            intent: history.intent.clone(),
            shell: history.shell.clone(),
            workspace: None,
        }
    }
}

impl sealed::Sealed for Command {}

impl Subject for Command {
    const RULE: RuleName = RuleName::History;

    fn dir(&self) -> Option<&Path> {
        Some(&self.dir)
    }

    fn set_workspace(&mut self, workspace: PathBuf) {
        self.workspace = Some(workspace);
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use rstest::{fixture, rstest};
    use tempfile::TempDir;

    use super::*;
    use crate::ai_session::{HarnessKind, NativeSessionId};
    use crate::history::AuthorKind;

    #[fixture]
    fn root() -> TempDir {
        tempfile::tempdir().expect("a temp dir")
    }

    /// The `upload` of a file that sets `upload` to `rules`.
    fn upload(root: &TempDir, rules: &str) -> Result<Upload, LoadError> {
        let source = format!("-- atuin:version=1\natuin.config = {{ upload = {rules} }}");
        fs::write(root.path().join("atuin.lua"), source).expect("write");
        Ok(Upload::in_dir(root.path())?.expect("a config file"))
    }

    fn thread(harness: HarnessKind, dir: Option<&str>) -> Thread {
        let session = HarnessSession {
            harness,
            session: NativeSessionId::from("thread".to_owned()),
        };
        Thread::new(&session, dir.map(PathBuf::from))
    }

    fn command(command: &str, author: AuthorKind) -> Command {
        let history: History = History::import()
            .timestamp(jiff::Timestamp::UNIX_EPOCH)
            .command(command)
            .cwd("/work")
            .build()
            .into();
        let history = History {
            author_kind: Some(author),
            ..history
        };
        Command::from(&history)
    }

    #[rstest]
    #[case::unset("{}", None)]
    #[case::allowed("{ threads = true }", Some(true))]
    #[case::denied("{ threads = false }", Some(false))]
    #[case::unset_for_the_other("{ history = false }", None)]
    #[case::by_harness(r#"{ threads = function(t) return t.harness == "codex" end }"#, Some(false))]
    #[case::by_dir(
        r#"{ threads = function(t) return string.find(t.dir, "^/work") ~= nil end }"#,
        Some(true)
    )]
    #[case::by_id(r#"{ threads = function(t) return t.id ~= "thread" end }"#, Some(false))]
    fn a_rule_decides_a_thread(root: TempDir, #[case] rules: &str, #[case] expected: Option<bool>) {
        let upload = upload(&root, rules).expect("an upload");

        let allows = upload.allows(&thread(HarnessKind::ClaudeCode, Some("/work/src")));

        assert_eq!(allows.expect("a decision"), expected);
    }

    /// The user's own file turns uploading on with a rule that may let something go.
    #[rstest]
    #[case::unset("{}", false)]
    #[case::off("{ threads = false }", false)]
    #[case::on("{ threads = true }", true)]
    #[case::by_function("{ threads = function() return false end }", true)]
    #[case::only_the_other("{ history = true }", false)]
    fn a_rule_switches_on_when_it_may_let_something_go(
        root: TempDir,
        #[case] rules: &str,
        #[case] expected: bool,
    ) {
        let upload = upload(&root, rules).expect("an upload");

        assert_eq!(upload.switches_on(RuleName::Threads), expected);
    }

    /// A thread's harness is named as Octavo names it, so a rule matches what the user sees there.
    #[rstest]
    #[case::known(HarnessKind::ClaudeCode, "claude-code")]
    #[case::a_flavour(HarnessKind::Omp, "omp")]
    #[case::unknown(HarnessKind::Unknown, "unknown")]
    fn a_thread_names_its_harness(root: TempDir, #[case] harness: HarnessKind, #[case] name: &str) {
        let upload = upload(
            &root,
            &format!(r#"{{ threads = function(t) return t.harness == "{name}" end }}"#),
        );

        let allows = upload.expect("an upload").allows(&thread(harness, None));

        assert_eq!(allows.expect("a decision"), Some(true));
    }

    /// A field a thread doesn't have reads as `nil` in Lua, as an optional field does in its type.
    #[rstest]
    fn a_thread_that_names_no_directory_has_a_nil_dir(root: TempDir) {
        let upload = upload(&root, "{ threads = function(t) return t.dir == nil end }");

        let allows = upload.expect("an upload").allows(&thread(HarnessKind::Codex, None));

        assert_eq!(allows.expect("a decision"), Some(true));
    }

    #[rstest]
    #[case::unset("{}", "ls", AuthorKind::User, None)]
    #[case::denied("{ history = false }", "ls", AuthorKind::User, Some(false))]
    #[case::unset_for_the_other("{ threads = false }", "ls", AuthorKind::User, None)]
    #[case::by_command(
        r#"{ history = function(c) return string.find(c.command, "secret", 1, true) == nil end }"#,
        "cat secret.txt",
        AuthorKind::User,
        Some(false)
    )]
    #[case::by_author(
        "{ history = function(c) return not c.agent end }",
        "ls",
        AuthorKind::Agent,
        Some(false)
    )]
    #[case::by_dir(
        r#"{ history = function(c) return c.dir == "/work" end }"#,
        "ls",
        AuthorKind::User,
        Some(true)
    )]
    #[case::by_its_own_upload(
        "{ threads = false, history = function() return atuin.config.upload.threads end }",
        "ls",
        AuthorKind::User,
        Some(false)
    )]
    fn a_rule_decides_a_command(
        root: TempDir,
        #[case] rules: &str,
        #[case] run: &str,
        #[case] author: AuthorKind,
        #[case] expected: Option<bool>,
    ) {
        let upload = upload(&root, rules).expect("an upload");

        let allows = upload.allows(&command(run, author));

        assert_eq!(allows.expect("a decision"), expected);
    }

    /// A typo could be what keeps a thread here, so an `upload` atuin can't read fails the file.
    #[rstest]
    #[case::not_a_table("true")]
    #[case::unknown_key("{ thread = false }")]
    #[case::output_follows_history("{ output = false }")]
    #[case::not_a_rule(r#"{ threads = "no" }"#)]
    #[case::a_list("{ false }")]
    fn an_upload_atuin_cant_read_fails_the_file(root: TempDir, #[case] rules: &str) {
        let err = upload(&root, rules).expect_err("an invalid file");

        assert!(matches!(err, LoadError::Invalid { .. }), "{err:?}");
    }

    /// A function that can't answer keeps the thread here, as the file may be what keeps it.
    #[rstest]
    #[case::raises(r#"function() error("no") end"#)]
    #[case::returns_nothing("function() end")]
    #[case::returns_a_number("function() return 1 end")]
    #[case::runs_forever("function() while true do end end")]
    #[case::catches_its_stop(
        "function() while true do pcall(function() while true do end end) end end"
    )]
    fn a_function_that_cant_answer_fails(root: TempDir, #[case] function: &str) {
        let upload = upload(&root, &format!("{{ threads = {function} }}")).expect("an upload");

        let err = upload.allows(&thread(HarnessKind::Codex, None)).expect_err("no answer");

        assert!(
            matches!(&err, LoadError::Rule { rule: "threads", path, .. } if *path == root.path().join("atuin.lua")),
            "{err:?}"
        );
    }
}
