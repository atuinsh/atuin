//! Tools for interacting and operating on different AI harnesses.

use std::path::PathBuf;

use enum_dispatch::enum_dispatch;

pub mod ccode;
pub mod codex;
mod json_hooks;
pub mod note;
pub mod opencode;
pub mod pi;
pub mod rehydrate;
pub mod resume;
pub mod session;

use ccode::Ccode;
use codex::Codex;
use opencode::Opencode;
use pi::Pi;
use rehydrate::{RehydrateError, RehydrateSession};
use resume::{ResumeError, ResumePlan, ResumeTarget};
use session::Observable;
use session::any::AnySessions;

use crate::sync::BlockingPool;

/// Defines a generic harness trait that all implementations need to implement.
#[enum_dispatch]
pub trait Harness: std::fmt::Debug {
    /// The well-defined name of this harness.
    fn name(&self) -> &'static str;

    /// Names in addition to [`Self::name`] which are considered to be the names this harness uses.
    ///
    /// There is a default implementation -- the empty set.
    fn alias_names(&self) -> &'static [&'static str] {
        &[]
    }

    /// Install this harness's hooks on the current user's machine, returning the path written.
    #[allow(async_fn_in_trait)]
    async fn install_hooks(&self) -> Result<PathBuf, InstallHookError>;

    /// How this harness itself reopens `target`: its own resume command, and whether that has to
    /// run from the session's directory. `Err` for a session the harness cannot reopen.
    fn resume_plan(&self, target: &ResumeTarget) -> Result<ResumePlan, ResumeError>;

    /// [`Self::resume_plan`], with the program and arguments replaced by the user's `template`
    /// when there is one (see [`ResumePlan::with_template`]). The harness's plan still decides
    /// whether the session can be resumed at all, and from where.
    fn resume(
        &self,
        target: &ResumeTarget,
        template: Option<&str>,
    ) -> Result<ResumePlan, ResumeError> {
        let plan = self.resume_plan(target)?;
        match template {
            Some(template) => plan.with_template(template, target),
            None => Ok(plan),
        }
    }

    /// Where the native record of session `id` is on this machine, `None` when it is not here:
    /// the transcript file, or for a harness that keeps its sessions in a database, the database
    /// holding it. Looks where the harness keeps sessions by default.
    #[allow(async_fn_in_trait)]
    async fn locate(&self, id: &str) -> Option<PathBuf>;

    /// Write `session` out as this harness's own transcript, where [`Self::locate`] (and the
    /// harness itself) will find it, and return where it went: a session recorded on another
    /// machine, or whose transcript is gone, can then be resumed like any other. Never replaces a
    /// transcript already there ([`RehydrateError::AlreadyExists`]).
    #[allow(async_fn_in_trait)]
    async fn rehydrate(&self, session: &RehydrateSession) -> Result<PathBuf, RehydrateError>;
}

#[enum_dispatch(Harness)]
#[derive(Debug, Clone, Copy)]
pub enum AnyHarness {
    ClaudeCode(Ccode),
    Codex(Codex),
    Opencode(Opencode),
    Pi(Pi),
}

#[derive(Debug, thiserror::Error)]
pub enum HarnessLookupError {
    #[error("unknown harness. known harnesses: {0:?}")]
    Unknown(Vec<&'static str>),
}

/// Error returned when installing a harness's hooks fails.
#[derive(Debug, thiserror::Error)]
pub enum InstallHookError {
    #[error("unexpected io error")]
    Io(
        #[from]
        #[source]
        std::io::Error,
    ),

    #[error("could not parse the existing config as JSON")]
    Json(
        #[from]
        #[source]
        serde_json::Error,
    ),

    #[error("the config has an unexpected shape: {0}")]
    Malformed(&'static str),

    #[error("the atuin executable path is not valid UTF-8: {}", .0.display())]
    NonUtf8Executable(PathBuf),

    #[error("could not shell-quote the atuin executable path {}", .path.display())]
    UnquotableExecutable {
        path: PathBuf,
        #[source]
        source: shlex::QuoteError,
    },

    #[error("hook already installed")]
    AlreadyInstalled,
}

impl AnyHarness {
    /// Try to create a [`Self`] object from the given name.
    pub fn from_name(name: &str) -> Result<Self, HarnessLookupError> {
        Self::all()
            .iter()
            .find(|harness| {
                std::iter::once(harness.name())
                    .chain(harness.alias_names().iter().copied())
                    .any(|candidate| candidate == name)
            })
            .copied()
            .ok_or_else(|| {
                HarnessLookupError::Unknown(Self::all().iter().map(Harness::name).collect())
            })
    }

    /// Every known harness.
    #[must_use]
    pub fn all() -> &'static [Self] {
        &[Self::ClaudeCode(Ccode), Self::Codex(Codex), Self::Opencode(Opencode), Self::Pi(Pi)]
    }

    /// The harness's sessions, whose file reads all run in `pool`.
    #[must_use]
    pub fn sessions(&self, pool: &BlockingPool) -> Option<AnySessions> {
        match self {
            Self::ClaudeCode(h) => Some(h.sessions(pool.clone()).into()),
            Self::Codex(h) => Some(h.sessions(pool.clone()).into()),
            Self::Opencode(h) => Some(h.sessions(pool.clone()).into()),
            Self::Pi(h) => Some(h.sessions(pool.clone()).into()),
        }
    }
}
