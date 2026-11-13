use atuin_client::history::ShellSession;
use lasso::{Rodeo, Spur};
use uuid::Uuid;

use crate::search::index::core::{CommandData, SearchIndex, SessionPreload};

/// Filter mode for search queries.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IndexFilterMode {
    /// No filtering - search all commands.
    Global,
    /// Filter to commands run in a specific directory.
    Directory(String),
    /// Filter to commands run in a workspace (directory prefix).
    Workspace(String),
    /// Filter to commands run on a specific host.
    Host(String),
    /// Filter to commands run in a specific session.
    Session(String),
    /// Filter to commands run in a specific session, plus all commands run before the start of the
    /// session.
    SessionPreload(String),
}

/// A "compiled" form of [`IndexFilterMode`], with most of the strings interned and parsed.
pub(super) enum CompiledFilter<'a> {
    All,
    Directory(Spur),
    Workspace(&'a str),
    Host(Spur),
    Session(Uuid),
    /// The session's own commands, plus those run before it started.
    SessionPreload(SessionPreload),
    /// Used when a target (host/dir/session) has never been seen by the index -- nothing can match.
    Nothing,
}

impl CompiledFilter<'_> {
    /// Check whether `data`, the command at `index`, matches this filter.
    pub fn matches(&self, index: u32, data: &CommandData, interner: &Rodeo) -> bool {
        match self {
            Self::All => true,
            Self::Nothing => false,
            Self::Directory(dir) => data.has_invocation_in_dir(*dir),
            Self::Workspace(prefix) => data.has_invocation_in_workspace(prefix, interner),
            Self::Host(hostname) => data.has_invocation_on_host(*hostname),
            Self::Session(session) => data.has_invocation_in_session(session),
            Self::SessionPreload(preload) => {
                preload.ran_before_session(index)
                    || data.has_invocation_in_session(&preload.session.id)
            }
        }
    }
}

impl IndexFilterMode {
    /// "Compile" this filter for `index` by interning and parsing its strings.
    ///
    /// A [`Self::SessionPreload`] filter needs a database query first if its session isn't cached.
    pub(super) fn compile(&self, index: &SearchIndex) -> Result<CompiledFilter<'_>, ShellSession> {
        let interner = &index.interner;
        Ok(match self {
            Self::Global => CompiledFilter::All,
            Self::Directory(dir) => {
                interner.get(dir).map_or(CompiledFilter::Nothing, CompiledFilter::Directory)
            }
            Self::Workspace(prefix) => CompiledFilter::Workspace(prefix),
            Self::Host(hostname) => {
                interner.get(hostname).map_or(CompiledFilter::Nothing, CompiledFilter::Host)
            }
            Self::Session(session) => {
                Uuid::parse_str(session).map_or(CompiledFilter::Nothing, CompiledFilter::Session)
            }
            Self::SessionPreload(session) => match ShellSession::parse(session) {
                Some(session) => match index.cached_session_preload(&session) {
                    Some(preload) => CompiledFilter::SessionPreload(preload),
                    None => return Err(session),
                },
                // A session without a start time can only find its own commands.
                None => Uuid::parse_str(session)
                    .map_or(CompiledFilter::Nothing, CompiledFilter::Session),
            },
        })
    }
}
