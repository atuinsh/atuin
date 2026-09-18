//! Tools for interacting and operating on different AI harnesses.

use std::path::PathBuf;

use enum_dispatch::enum_dispatch;

pub mod ccode;
pub mod codex;
mod json_hooks;
pub mod opencode;
pub mod pi;
pub mod session;

use ccode::Ccode;
use codex::Codex;
use opencode::Opencode;
use pi::Pi;

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
}
