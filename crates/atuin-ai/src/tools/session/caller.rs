//! Identifying the calling agent's own session, so list, search and `latest` can leave it out:
//! it is live, already in the caller's context, and otherwise tops every recency-ordered result.

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use atuin_client::ai_session::{HarnessKind, Session};
use atuin_daemon::AiClient;
use time::OffsetDateTime;

/// How long before the server started a session may begin and still be the caller's. A harness
/// writes its session within a second or two of spawning MCP servers (Codex just before, opencode
/// and Claude Code after). Keep it tight: a wider window catches the previous run in the same
/// directory, which makes the match ambiguous and hides nothing.
const START_SLACK: Duration = Duration::from_secs(10);

/// Per-server state: when and where the server started.
pub struct OwnSession {
    started: SystemTime,
    cwd: Option<PathBuf>,
}

impl Default for OwnSession {
    fn default() -> Self {
        Self {
            started: SystemTime::now(),
            cwd: std::env::current_dir().ok(),
        }
    }
}

/// The calling agent, as far as the MCP handshake and environment tell us.
pub struct Caller<'a> {
    /// `clientInfo.name` from the MCP `initialize` request.
    pub client: Option<&'a str>,
    pub own: &'a OwnSession,
}

impl Caller<'_> {
    fn harness(&self) -> Option<HarnessKind> {
        match self.client? {
            "claude-code" => Some(HarnessKind::ClaudeCode),
            "codex-mcp-client" => Some(HarnessKind::Codex),
            "opencode" => Some(HarnessKind::Opencode),
            _ => None,
        }
    }

    /// The caller's session id, if it can be identified.
    ///
    /// Claude Code exports `CLAUDE_CODE_SESSION_ID` to MCP servers; it is only trusted when the
    /// client says it is Claude Code, because other harnesses (opencode) pass their whole
    /// environment through, so an agent launched from inside a Claude Code session would inherit
    /// that session's id. Other harnesses pass no id at all (Codex scrubs the environment), so
    /// fall back to the one session of the caller's harness, in the server's directory, that
    /// started after the server did. When that is ambiguous (parallel agents in one directory)
    /// nothing is hidden: showing the caller its own session wastes a call, hiding someone else's
    /// loses it.
    ///
    /// Decided afresh on every call, never remembered: before the daemon has captured the
    /// caller's own session, another agent's session started just after this server can be the
    /// lone candidate. Remembering that pick would hide the other agent's session for the life of
    /// the server; re-deciding lets the next call, once both sessions exist, see the ambiguity.
    pub async fn own_session_id(&self, client: &mut AiClient) -> Option<String> {
        if let Some(id) = self.env_session_id() {
            return Some(id);
        }
        let sessions = super::list_sessions(client, Some(self.harness()?)).await.ok()?;
        self.own_in(&sessions)
    }

    /// [`Self::own_session_id`] for a caller that already holds the session list.
    pub fn own_in(&self, sessions: &[Session]) -> Option<String> {
        self.env_session_id().or_else(|| self.pick(sessions, self.own.cwd.as_deref()?))
    }

    fn env_session_id(&self) -> Option<String> {
        (self.harness()? == HarnessKind::ClaudeCode)
            .then(|| std::env::var("CLAUDE_CODE_SESSION_ID").ok())
            .flatten()
            .filter(|id| !id.is_empty())
    }

    fn pick(&self, sessions: &[Session], cwd: &Path) -> Option<String> {
        let harness = self.harness()?;
        let started = OffsetDateTime::from(self.own.started);
        let earliest = OffsetDateTime::from(self.own.started.checked_sub(START_SLACK)?);
        // Still active since this server started: a run that finished just before it (the
        // previous session, which a "continue" is looking for) started in the window too, but
        // recorded nothing after.
        let mut candidates = sessions.iter().filter(|s| {
            s.handle.harness == harness
                && s.parent.is_none()
                && s.cwd.as_deref() == Some(cwd)
                && s.started_at >= earliest
                && s.updated_at >= started
        });
        let only = candidates.next()?;
        candidates.next().is_none().then(|| only.handle.session.to_string())
    }
}

pub(super) fn is_own(s: &Session, own: Option<&str>) -> bool {
    own.is_some_and(|own| s.handle.session.as_ref() == own)
}

#[cfg(test)]
mod tests {
    use rstest::rstest;

    use super::*;

    fn session(id: &str, cwd: &str, started: SystemTime) -> Session {
        let mut s = super::super::fixtures::session(id, Some(cwd), OffsetDateTime::from(started));
        s.handle.harness = HarnessKind::Codex;
        s
    }

    fn server(started: SystemTime) -> OwnSession {
        OwnSession {
            started,
            cwd: Some(PathBuf::from("/work/p")),
        }
    }

    #[rstest]
    fn picks_the_single_session_started_with_the_server_in_its_directory() {
        let now = SystemTime::now();
        let own = server(now);
        let caller = Caller {
            client: Some("codex-mcp-client"),
            own: &own,
        };
        let hour_ago = now - Duration::from_secs(3600);
        let sessions = [
            session("old", "/work/p", hour_ago),
            session("elsewhere", "/work/q", now),
            session("mine", "/work/p", now + Duration::from_secs(30)),
        ];
        assert_eq!(caller.pick(&sessions, Path::new("/work/p")).as_deref(), Some("mine"));
    }

    #[rstest]
    fn a_run_just_before_this_one_is_not_a_candidate() {
        let now = SystemTime::now();
        let own = server(now);
        let caller = Caller {
            client: Some("codex-mcp-client"),
            own: &own,
        };
        let sessions = [
            session("previous", "/work/p", now - Duration::from_secs(25)),
            session("mine", "/work/p", now + Duration::from_secs(1)),
        ];
        assert_eq!(caller.pick(&sessions, Path::new("/work/p")).as_deref(), Some("mine"));
    }

    #[rstest]
    fn a_lone_other_session_stops_being_hidden_once_the_callers_own_appears() {
        let now = SystemTime::now();
        let own = server(now);
        let caller = Caller {
            client: Some("codex-mcp-client"),
            own: &own,
        };
        let other = session("other", "/work/p", now + Duration::from_secs(2));
        // The caller's own session is not captured yet, so the other agent's is the lone
        // candidate; nothing may carry that mistake into the next call.
        assert_eq!(
            caller.pick(std::slice::from_ref(&other), Path::new("/work/p")).as_deref(),
            Some("other")
        );
        let mine = session("mine", "/work/p", now + Duration::from_secs(1));
        assert_eq!(caller.pick(&[other, mine], Path::new("/work/p")), None);
    }

    #[rstest]
    fn a_run_that_finished_just_before_the_server_started_is_not_the_callers() {
        let now = SystemTime::now();
        let own = server(now);
        let caller = Caller {
            client: Some("codex-mcp-client"),
            own: &own,
        };
        let mut previous = session("previous", "/work/p", now - Duration::from_secs(5));
        previous.updated_at = OffsetDateTime::from(now - Duration::from_secs(2));
        // The caller's own session is not captured yet: nothing is its, so nothing is hidden.
        assert_eq!(caller.pick(std::slice::from_ref(&previous), Path::new("/work/p")), None);
        let mine = session("mine", "/work/p", now + Duration::from_secs(1));
        assert_eq!(caller.pick(&[previous, mine], Path::new("/work/p")).as_deref(), Some("mine"));
    }

    #[rstest]
    fn ambiguity_hides_nothing() {
        let now = SystemTime::now();
        let own = server(now);
        let caller = Caller {
            client: Some("codex-mcp-client"),
            own: &own,
        };
        let sessions = [session("a", "/work/p", now), session("b", "/work/p", now)];
        assert_eq!(caller.pick(&sessions, Path::new("/work/p")), None);
    }

    #[rstest]
    #[case::claude("claude-code", Some(HarnessKind::ClaudeCode))]
    #[case::codex("codex-mcp-client", Some(HarnessKind::Codex))]
    #[case::opencode("opencode", Some(HarnessKind::Opencode))]
    #[case::cursor("cursor-vscode", None)]
    fn maps_client_names_to_harnesses(#[case] name: &str, #[case] want: Option<HarnessKind>) {
        let own = OwnSession::default();
        assert_eq!(
            Caller {
                client: Some(name),
                own: &own
            }
            .harness(),
            want
        );
    }
}
