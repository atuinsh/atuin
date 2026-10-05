//! The real [`SessionSource`]: the ai-session sidecar database, opened read-only beside the daemon
//! that writes it, so the picker's first frame never waits on the daemon.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use async_trait::async_trait;
use atuin_client::ai_session::{AiSessionDatabase, Analysis, HarnessSession, SearchTerms, Session};
use atuin_common::harnesstools::rehydrate::RehydrateSession;
use atuin_common::harnesstools::session::{Content, ParentKind};
use atuin_common::string::highlighted::HighlightedString;
use atuin_common::utils::in_git_repo;
use eyre::{Result, WrapErr};
use futures::TryStreamExt;
use parking_lot::Mutex;

use super::source::{Relation, SessionFilter, SessionPreview, SessionRow, SessionSource, Snippet};
use super::{ResumeContext, title};

pub struct SidecarSource {
    db: AiSessionDatabase,
    /// This host's id (simple form).
    host_id: String,
    /// The git repository each session directory is in, looked up once per picker: finding it
    /// walks the filesystem, and every search would otherwise repeat it for each row.
    git_roots: Mutex<HashMap<PathBuf, Option<PathBuf>>>,
}

impl SidecarSource {
    /// Open the sidecar at `path`.
    pub async fn open(path: &Path, context: &ResumeContext) -> Result<Self> {
        let db = AiSessionDatabase::open_read_only(path).await.wrap_err_with(|| {
            format!("could not open the AI session database {}", path.display())
        })?;
        Ok(Self::new(db, context))
    }

    fn new(db: AiSessionDatabase, context: &ResumeContext) -> Self {
        Self {
            db,
            host_id: super::simple_host_id(&context.host_id),
            git_roots: Mutex::new(HashMap::new()),
        }
    }

    /// The git repository root `cwd` is in, if any.
    fn git_root(&self, cwd: &Path) -> Option<PathBuf> {
        let mut roots = self.git_roots.lock();
        roots.entry(cwd.to_owned()).or_insert_with(|| in_git_repo(&cwd.to_string_lossy())).clone()
    }

    /// A picker row for `s`.
    fn row(&self, s: Session) -> SessionRow {
        let relation = match s.inferred_parent_kind() {
            Some(ParentKind::Subagent) => Relation::Subagent,
            Some(ParentKind::Fork) => Relation::Fork,
            Some(ParentKind::Continuation) => Relation::Continuation,
            None if s.parent.is_some() => Relation::Child,
            None => Relation::Root,
        };
        // A session with no recorded host predates host tracking: it can only be this host's.
        let host_id = s.host.map_or_else(|| self.host_id.clone(), |h| h.0.as_simple().to_string());
        let git_root = s.cwd.as_deref().and_then(|cwd| self.git_root(cwd));
        let title = titled(&s)
            .map_or_else(|| title::derive(s.preview.as_deref().unwrap_or_default()), str::to_owned);
        SessionRow {
            atuin_id: s.atuin_id,
            handle: s.handle,
            // A copy (a Claude Code `--resume` fork) is forked from the session it copies.
            parent: s.parent.or(s.copy_of),
            relation,
            title: Snippet::plain(title),
            cwd: s.cwd,
            git_root,
            branch: s.git_branch,
            model: s.model,
            host_id,
            started_at: s.started_at,
            updated_at: s.group_updated_at.unwrap_or(s.updated_at),
            active_at: s.updated_at,
            messages: s.message_count,
            usage: s.usage,
            children: u32::try_from(s.child_count).unwrap_or(u32::MAX),
            matched: None,
        }
    }
}

/// The title the harness (or the user) gave `s`, if any.
fn titled(s: &Session) -> Option<&str> {
    s.title.as_deref().filter(|t| !t.trim().is_empty())
}

fn snippet(h: &HighlightedString) -> Snippet {
    let plain = h.to_plain();
    Snippet {
        text: plain.text.into_owned(),
        highlights: plain.ranges,
    }
}

fn conversation_text(c: &Content) -> Option<&str> {
    match c {
        Content::Text(t) | Content::Summary(t) if !t.trim().is_empty() => Some(t.as_str()),
        _ => None,
    }
}

/// The conversation text of a message: text and summaries, never tool calls or reasoning.
fn text_of(content: &[Content]) -> Option<String> {
    let text: Vec<&str> = content.iter().filter_map(conversation_text).collect();
    (!text.is_empty()).then(|| text.join("\n"))
}

#[async_trait]
impl SessionSource for SidecarSource {
    async fn search(&self, filter: &SessionFilter) -> Result<Vec<SessionRow>> {
        let limit = u32::try_from(filter.limit).unwrap_or(u32::MAX);
        let matches: Vec<_> = self
            .db
            .search(&filter.text, SearchTerms::Typed, &filter.db, limit)
            .try_collect()
            .await?;

        let has_text = !filter.text.trim().is_empty();
        let mut rows = Vec::with_capacity(matches.len());
        for m in matches {
            let has_title = titled(&m.session).is_some();
            let mut row = self.row(m.session);
            // Grouped, a subagent's match is already its root's; this is one ungrouped, or one
            // whose parent isn't stored yet.
            if !row.relation.is_listed() {
                continue;
            }
            if has_text {
                // The search highlights stored titles; a derived one is highlighted here.
                let title = snippet(&m.title);
                if has_title && !title.text.is_empty() {
                    row.title = title;
                } else {
                    row.title.highlights = title::highlights(&row.title.text, &filter.text);
                }
                let preview = snippet(&m.preview);
                row.matched = (!preview.text.is_empty()).then_some(preview);
            }
            rows.push(row);
        }
        Ok(rows)
    }

    async fn find_by_id(&self, id: &str) -> Result<Vec<SessionRow>> {
        let sessions = self.db.sessions_with_id_prefix(id).await?;
        Ok(sessions.into_iter().map(|s| self.row(s)).collect())
    }

    async fn preview(&self, session: &HarnessSession) -> Result<SessionPreview> {
        let parts = self.db.preview_parts(session).await?;
        Ok(SessionPreview {
            first_prompt: parts.first_user.as_deref().and_then(text_of),
            last_assistant: parts.last_assistant.as_deref().and_then(text_of),
        })
    }

    async fn children(&self, session: &HarnessSession) -> Result<Vec<SessionRow>> {
        // Newest first, from the query.
        Ok(self
            .db
            .children(session)
            .await?
            .into_iter()
            .map(|s| self.row(s))
            .filter(|r| r.relation.is_listed())
            .collect())
    }

    async fn rehydrate(&self, session: &HarnessSession, cwd: &Path) -> Result<RehydrateSession> {
        self.db
            .rehydrate_session(session, cwd.to_owned())
            .await?
            .ok_or_else(|| eyre::eyre!("the session isn't in the AI session database"))
    }

    async fn analyse(&self, session: &HarnessSession) -> Result<Option<Analysis>> {
        Ok(Some(self.db.analyse(session).await?))
    }
}

#[cfg(test)]
mod tests {
    use atuin_client::ai_session::{HarnessKind, Message, NativeSessionId, SourceId};
    use atuin_common::harnesstools::session::Role;
    use atuin_common::utils::uuid_v7;
    use atuin_domain::record::{HostId, RecordId};
    use rstest::{fixture, rstest};
    use time::{Duration, OffsetDateTime};
    use uuid::Uuid;

    use super::*;

    const HOST: &str = "0190aaaa0000700080000000000000aa";

    fn handle(id: &str) -> HarnessSession {
        HarnessSession {
            harness: HarnessKind::ClaudeCode,
            session: NativeSessionId::from(id.to_owned()),
        }
    }

    fn message(id: &str, parent: Option<&str>, role: Role, text: &str, minutes: i64) -> Message {
        Message::builder()
            .id(RecordId(uuid_v7()))
            .session(handle(id))
            .source_id(SourceId::from(format!("{id}-{minutes}")))
            .parent(parent.map(handle))
            .timestamp(OffsetDateTime::UNIX_EPOCH + Duration::minutes(minutes))
            .role(role)
            .content(vec![Content::Text(text.to_owned())])
            .cwd(Some(PathBuf::from("/tmp")))
            .host(Some(HostId(Uuid::try_parse(HOST).unwrap())))
            .build()
    }

    #[fixture]
    async fn source() -> SidecarSource {
        let db = AiSessionDatabase::in_memory().await.unwrap();
        for m in [
            message("root", None, Role::User, "fix the flaky sync test", 0),
            message("root", None, Role::Assistant, "switched to a fixed clock", 1),
            message("agent-1", Some("root"), Role::User, "look for wall clock use", 2),
            message("fork-1", Some("root"), Role::User, "try a mocked clock instead", 3),
        ] {
            db.append(&m).await.unwrap();
        }
        SidecarSource::new(db, &context())
    }

    fn context() -> ResumeContext {
        ResumeContext {
            host_id: HOST.to_owned(),
            ..ResumeContext::default()
        }
    }

    fn roots(text: &str) -> SessionFilter {
        let mut filter = SessionFilter {
            text: text.to_owned(),
            limit: 50,
            ..SessionFilter::default()
        };
        filter.db.roots_only = true;
        filter
    }

    #[rstest]
    #[tokio::test]
    async fn groups_children_under_their_root(#[future] source: SidecarSource) {
        let source = source.await;
        let rows = source.search(&roots("")).await.unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].handle, handle("root"));
        assert_eq!(rows[0].children, 1, "the fork, not the subagent: it never resumes");
        assert_eq!(rows[0].host_id, HOST);

        // Only the fork: subagents never resume, so the picker never lists them.
        let children = source.children(&handle("root")).await.unwrap();
        assert_eq!(children.len(), 1);
        assert_eq!(children[0].handle, handle("fork-1"));
        assert_eq!(children[0].relation, Relation::Fork);
    }

    /// Ungrouped, forks get rows of their own, subagents still don't.
    #[rstest]
    #[tokio::test]
    async fn ungrouped_lists_forks_but_not_subagents(#[future] source: SidecarSource) {
        let source = source.await;
        let mut filter = roots("");
        filter.db.roots_only = false;
        let rows = source.search(&filter).await.unwrap();
        let ids: Vec<&str> = rows.iter().map(|r| r.handle.session.as_ref()).collect();
        assert_eq!(ids, ["fork-1", "root"]);
    }

    /// Typed into the picker, the last word matches as a prefix until a space finishes it.
    #[rstest]
    #[case::still_typing("fla", 1)]
    #[case::finished("fla ", 0)]
    #[case::finished_whole_word("flaky ", 1)]
    #[tokio::test]
    async fn a_trailing_space_finishes_the_typed_word(
        #[future] source: SidecarSource,
        #[case] input: &str,
        #[case] hits: usize,
    ) {
        let source = source.await;
        let text = crate::resume_tui::query::parse(input).text;
        assert_eq!(source.search(&roots(&text)).await.unwrap().len(), hits);
    }

    #[rstest]
    #[tokio::test]
    async fn a_childs_match_finds_the_root_with_highlights(#[future] source: SidecarSource) {
        let source = source.await;
        let rows = source.search(&roots("wall")).await.unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].handle, handle("root"));
        let matched = rows[0].matched.as_ref().expect("a matched snippet");
        let hl = &matched.highlights[0];
        assert_eq!(&matched.text[hl.clone()], "wall");
    }

    /// A session without a title is titled from its first prompt, highlighted when the query
    /// matches it; a harness's title wins, with the search's own highlights.
    #[rstest]
    #[tokio::test]
    async fn untitled_sessions_are_titled_from_their_first_prompt(#[future] source: SidecarSource) {
        use atuin_common::harnesstools::session::{TitleChange, TitleSource};

        let source = source.await;
        let brief = message(
            "brief",
            None,
            Role::User,
            "You're working on the sync code. Please **rewrite** the `sync_down` loop.",
            5,
        );
        source.db.append(&brief).await.unwrap();
        let mut titled = message("titled", None, Role::User, "rewrite the flaky test", 6);
        titled.session_title = Some("Flaky rewrite".to_owned());
        titled.session_title_source = Some(TitleSource::Generated);
        titled.title_change = Some(TitleChange::new(TitleSource::Generated, "Flaky rewrite"));
        source.db.append(&titled).await.unwrap();

        let rows = source.search(&roots("")).await.unwrap();
        let title = |id: &str| {
            rows.iter().find(|r| r.handle == handle(id)).map(|r| r.title.text.clone()).unwrap()
        };
        assert_eq!(title("root"), "fix the flaky sync test");
        assert_eq!(title("brief"), "Rewrite the sync_down loop");
        assert_eq!(title("titled"), "Flaky rewrite");

        let rows = source.search(&roots("rewrite")).await.unwrap();
        for row in &rows {
            let hl: Vec<&str> =
                row.title.highlights.iter().map(|r| &row.title.text[r.clone()]).collect();
            assert_eq!(hl, [if row.handle == handle("brief") {
                "Rewrite"
            } else {
                "rewrite"
            }]);
        }
    }

    #[rstest]
    #[tokio::test]
    async fn previews_and_ids(#[future] source: SidecarSource) {
        let source = source.await;
        let preview = source.preview(&handle("root")).await.unwrap();
        assert_eq!(preview.first_prompt.as_deref(), Some("fix the flaky sync test"));
        assert_eq!(preview.last_assistant.as_deref(), Some("switched to a fixed clock"));
        assert_eq!(source.find_by_id("ro").await.unwrap().len(), 1);
        assert!(source.find_by_id("r%").await.unwrap().is_empty());
    }

    /// The preview skips an assistant tail of tool calls for the last reply with text.
    #[rstest]
    #[tokio::test]
    async fn the_preview_skips_a_tail_without_text(#[future] source: SidecarSource) {
        let source = source.await;
        let mut tail = message("root", None, Role::Assistant, "", 3);
        tail.content = vec![Content::Reasoning("thinking".to_owned())];
        source.db.append(&tail).await.unwrap();
        let mut tool = message("root", None, Role::Tool, "output", 4);
        tool.content = vec![Content::Text("tool output".to_owned())];
        source.db.append(&tool).await.unwrap();

        let preview = source.preview(&handle("root")).await.unwrap();
        assert_eq!(preview.last_assistant.as_deref(), Some("switched to a fixed clock"));
    }

    // --- hosts ----------------------------------------------------------------------------------

    /// The host filter mode keeps this host's sessions, those from before hosts were recorded
    /// (`old`) included; rows from another host keep its id.
    #[rstest]
    #[tokio::test]
    async fn the_host_mode_keeps_this_hosts_unrecorded_sessions() {
        const OTHER: &str = "0190bbbb0000700080000000000000b1";
        let host = |id: &str| HostId(Uuid::try_parse(id).unwrap());
        let db = AiSessionDatabase::in_memory().await.unwrap();
        for (i, (id, on)) in
            [("here", Some(HOST)), ("old", None), ("there", Some(OTHER))].into_iter().enumerate()
        {
            let mut m = message(id, None, Role::User, "shared words", i64::try_from(i).unwrap());
            m.host = on.map(host);
            db.append(&m).await.unwrap();
        }
        let source = SidecarSource::new(db, &context());
        let hosts = |rows: Vec<SessionRow>| {
            let mut hosts: Vec<_> =
                rows.into_iter().map(|r| (r.handle.session.to_string(), r.host_id)).collect();
            hosts.sort();
            hosts
        };
        let pairs = |pairs: &[(&str, &str)]| {
            pairs.iter().map(|&(a, b)| (a.to_owned(), b.to_owned())).collect::<Vec<_>>()
        };

        let all = source.search(&roots("")).await.unwrap();
        assert_eq!(hosts(all), pairs(&[("here", HOST), ("old", HOST), ("there", OTHER)]));
        for text in ["", "shared"] {
            let mut filter = roots(text);
            filter.db.host = Some(host(HOST));
            filter.db.or_unrecorded = true;
            let found = source.search(&filter).await.unwrap();
            assert_eq!(hosts(found), pairs(&[("here", HOST), ("old", HOST)]), "{text:?}");
        }
    }
}
