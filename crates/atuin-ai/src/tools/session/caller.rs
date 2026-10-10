//! Identifying the calling agent's own session, so list, search and `latest` can leave it out:
//! it is live, already in the caller's context, and otherwise tops every recency-ordered result.

use std::path::PathBuf;
use std::time::{Duration, SystemTime};

use atuin_client::ai_session::{HarnessKind, HarnessSession, NativeSessionId, Session};
use atuin_common::env;
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

    /// The caller's session, if it can be identified.
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
    pub async fn own_session(&self, client: &mut AiClient) -> Option<HarnessSession> {
        if let Some(own) = self.env_session() {
            return Some(own);
        }
        // Only a session active since the server started can be the caller's; don't fetch the rest.
        let since = Some(OffsetDateTime::from(self.own.started));
        self.pick(&super::list_sessions(client, Some(self.harness()?), since).await.ok()?)
    }

    /// The earliest activity a listing needs for [`Self::own_in`] to find the caller's session.
    pub fn active_since(&self) -> OffsetDateTime {
        OffsetDateTime::from(self.own.started)
    }

    /// [`Self::own_session`] for a caller that already holds the session list.
    pub fn own_in(&self, sessions: &[Session]) -> Option<HarnessSession> {
        self.env_session().or_else(|| self.pick(sessions))
    }

    fn env_session(&self) -> Option<HarnessSession> {
        let harness = self.harness().filter(|h| *h == HarnessKind::ClaudeCode)?;
        let id = env::var("CLAUDE_CODE_SESSION_ID").ok().filter(|id| !id.is_empty())?;
        Some(HarnessSession {
            harness,
            session: NativeSessionId::from(id),
        })
    }

    fn pick(&self, sessions: &[Session]) -> Option<HarnessSession> {
        let harness = self.harness()?;
        let cwd = self.own.cwd.as_deref()?;
        let started = OffsetDateTime::from(self.own.started);
        let earliest = OffsetDateTime::from(self.own.started.checked_sub(START_SLACK)?);
        // Still active since this server started: a run that finished just before it (the
        // previous session, which a "continue" is looking for) started in the window too, but
        // recorded nothing after.
        let mut candidates = sessions.iter().filter(|s| {
            s.handle.harness == harness
                && !super::is_subagent(s)
                && s.cwd.as_deref() == Some(cwd)
                && s.started_at >= earliest
                && s.updated_at >= started
        });
        let only = candidates.next()?;
        candidates.next().is_none().then(|| only.handle.clone())
    }
}

pub(super) fn is_own(s: &Session, own: Option<&HarnessSession>) -> bool {
    own == Some(&s.handle)
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

    fn id(own: &HarnessSession) -> String {
        own.session.to_string()
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
        assert_eq!(caller.pick(&sessions).as_ref().map(id).as_deref(), Some("mine"));
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
        assert_eq!(caller.pick(&sessions).as_ref().map(id).as_deref(), Some("mine"));
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
            caller.pick(std::slice::from_ref(&other)).as_ref().map(id).as_deref(),
            Some("other")
        );
        let mine = session("mine", "/work/p", now + Duration::from_secs(1));
        assert_eq!(caller.pick(&[other, mine]), None);
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
        assert_eq!(caller.pick(std::slice::from_ref(&previous)), None);
        let mine = session("mine", "/work/p", now + Duration::from_secs(1));
        assert_eq!(caller.pick(&[previous, mine]).as_ref().map(id).as_deref(), Some("mine"));
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
        assert_eq!(caller.pick(&sessions), None);
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

    #[rstest]
    #[case::same_session(HarnessKind::Codex, "mine", true)]
    #[case::same_id_other_harness(HarnessKind::ClaudeCode, "mine", false)]
    #[case::other_id(HarnessKind::Codex, "other", false)]
    fn own_session_matches_harness_and_id(
        #[case] harness: HarnessKind,
        #[case] id: &str,
        #[case] own: bool,
    ) {
        let mine = session("mine", "/work/p", SystemTime::now());
        let candidate = HarnessSession {
            harness,
            session: NativeSessionId::from(id.to_owned()),
        };
        assert_eq!(is_own(&mine, Some(&candidate)), own);
        assert!(!is_own(&mine, None));
    }
}
