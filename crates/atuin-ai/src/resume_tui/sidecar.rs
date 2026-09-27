//! The real [`SessionSource`]: the ai-session sidecar database, opened read-only beside the daemon
//! that writes it, so the picker's first frame never waits on the daemon.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use async_trait::async_trait;
use atuin_client::ai_session::{
    AiSessionDatabase, HarnessSession, Session, SessionFilter as DbFilter, SessionRelation,
};
use atuin_common::harnesstools::session::{Content, Role};
use atuin_common::string::highlighted::HighlightedString;
use atuin_common::utils::in_git_repo;
use atuin_domain::record::HostId;
use eyre::{Result, WrapErr};
use futures::TryStreamExt;
use parking_lot::Mutex;
use uuid::Uuid;

use super::ResumeContext;
use super::source::{Relation, SessionFilter, SessionPreview, SessionRow, SessionSource, Snippet};

pub struct SidecarSource {
    db: AiSessionDatabase,
    host_id: String,
    hostname: String,
    /// The git repository each session directory is in, looked up once per picker: finding it
    /// walks the filesystem, and every search would otherwise repeat it for each row.
    git_roots: Mutex<HashMap<PathBuf, Option<PathBuf>>>,
}

impl SidecarSource {
    pub async fn open(path: &Path, context: &ResumeContext) -> Result<Self> {
        let db = AiSessionDatabase::open_read_only(path).await.wrap_err_with(|| {
            format!("could not open the AI session database {}", path.display())
        })?;
        Ok(Self::new(db, context))
    }

    fn new(db: AiSessionDatabase, context: &ResumeContext) -> Self {
        Self {
            db,
            host_id: context.host_id.clone(),
            hostname: context.hostname.clone(),
            git_roots: Mutex::new(HashMap::new()),
        }
    }

    /// The git repository root `cwd` is in, if any.
    fn git_root(&self, cwd: &Path) -> Option<PathBuf> {
        let mut roots = self.git_roots.lock();
        roots.entry(cwd.to_owned()).or_insert_with(|| in_git_repo(&cwd.to_string_lossy())).clone()
    }

    fn db_filter(filter: &SessionFilter) -> DbFilter {
        DbFilter {
            host: filter.host.as_deref().and_then(|h| Uuid::try_parse(h).ok()).map(HostId),
            workspace: filter.workspace.clone(),
            directory: filter.directory.clone(),
            branch: filter.branch.clone(),
            harness: filter.harness,
            model: filter.model.clone(),
            roots_only: filter.roots_only,
        }
    }

    /// A picker row for `s`.
    fn row(&self, s: Session) -> SessionRow {
        let relation = match s.relation() {
            SessionRelation::Root => Relation::Root,
            SessionRelation::Subagent => Relation::Subagent,
            SessionRelation::Fork => Relation::Fork,
            SessionRelation::Child => Relation::Child,
        };
        // A session with no recorded host predates host tracking: it can only be this host's.
        let host_id = s.host.map_or_else(|| self.host_id.clone(), |h| h.0.as_simple().to_string());
        let hostname = if host_id == self.host_id {
            self.hostname.clone()
        } else {
            // TODO: host ids have no names yet; show a short, stable form.
            host_id.chars().take(8).collect()
        };
        let git_root = s.cwd.as_deref().and_then(|cwd| self.git_root(cwd));
        let title = s.title.clone().or_else(|| s.preview.clone()).unwrap_or_default();
        SessionRow {
            handle: s.handle,
            parent: s.parent,
            relation,
            title: Snippet::plain(title),
            cwd: s.cwd,
            git_root,
            branch: s.git_branch,
            model: s.model,
            host_id,
            hostname,
            started_at: s.started_at,
            updated_at: s.group_updated_at.unwrap_or(s.updated_at),
            message_count: s.message_count,
            usage: s.usage,
            children: u32::try_from(s.child_count).unwrap_or(u32::MAX),
            matched: None,
        }
    }

    async fn all_sessions(&self) -> Result<Vec<Session>> {
        Ok(self.db.list_sessions(&DbFilter::default()).await?)
    }
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

/// Whether a message has any conversation text (see [`text_of`]).
fn has_text(content: &[Content]) -> bool {
    content.iter().any(|c| conversation_text(c).is_some())
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
        let matches: Vec<_> =
            self.db.search(&filter.text, &Self::db_filter(filter), limit).try_collect().await?;

        let has_text = !filter.text.trim().is_empty();
        let mut rows = Vec::with_capacity(matches.len());
        for m in matches {
            let mut row = self.row(m.session);
            if let Some(name) = &filter.host_name
                && !row.hostname.starts_with(name.as_str())
            {
                continue;
            }
            if has_text {
                let title = snippet(&m.title);
                if !title.text.is_empty() {
                    row.title = title;
                }
                let preview = snippet(&m.preview);
                row.matched = (!preview.text.is_empty()).then_some(preview);
            }
            rows.push(row);
        }
        Ok(rows)
    }

    async fn find_by_id(&self, id: &str) -> Result<Vec<SessionRow>> {
        Ok(self
            .all_sessions()
            .await?
            .into_iter()
            .filter(|s| s.handle.session.as_ref().starts_with(id))
            .map(|s| self.row(s))
            .collect())
    }

    async fn preview(&self, session: &HarnessSession) -> Result<SessionPreview> {
        // One pass, keeping only what the preview shows rather than the whole transcript.
        // TODO: every message is still decompressed; the data layer has no query for just the
        // timestamps and the first and last conversation text.
        let mut messages = std::pin::pin!(self.db.messages(session));
        let mut preview = SessionPreview::default();
        let mut last_assistant: Option<Vec<Content>> = None;
        while let Some(m) = messages.try_next().await? {
            match m.role {
                Role::User => {
                    preview.activity.push(m.timestamp);
                    if preview.first_prompt.is_none() {
                        preview.first_prompt = text_of(&m.content);
                    }
                }
                Role::Assistant => {
                    preview.activity.push(m.timestamp);
                    if has_text(&m.content) {
                        last_assistant = Some(m.content);
                    }
                }
                _ => {}
            }
        }
        preview.last_assistant = last_assistant.as_deref().and_then(text_of);
        Ok(preview)
    }

    async fn children(
        &self,
        session: &HarnessSession,
        include_subagents: bool,
    ) -> Result<Vec<SessionRow>> {
        let mut rows: Vec<SessionRow> = self
            .all_sessions()
            .await?
            .into_iter()
            .filter(|s| s.root.as_ref() == Some(session))
            .map(|s| self.row(s))
            .filter(|r| include_subagents || r.relation != Relation::Subagent)
            .collect();
        rows.sort_by_key(|r| std::cmp::Reverse(r.updated_at));
        Ok(rows)
    }
}

#[cfg(test)]
mod tests {
    use atuin_client::ai_session::{HarnessKind, Message, NativeSessionId, SourceId};
    use atuin_common::utils::uuid_v7;
    use atuin_domain::record::RecordId;
    use rstest::{fixture, rstest};
    use time::{Duration, OffsetDateTime};

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
        ] {
            db.append(&m).await.unwrap();
        }
        SidecarSource::new(db, &ResumeContext {
            host_id: HOST.to_owned(),
            hostname: "wintermute".to_owned(),
            ..ResumeContext::default()
        })
    }

    fn roots(text: &str) -> SessionFilter {
        SessionFilter {
            text: text.to_owned(),
            roots_only: true,
            limit: 50,
            ..SessionFilter::default()
        }
    }

    #[rstest]
    #[tokio::test]
    async fn groups_children_under_their_root(#[future] source: SidecarSource) {
        let source = source.await;
        let rows = source.search(&roots("")).await.unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].handle, handle("root"));
        assert_eq!(rows[0].children, 1);
        assert_eq!(rows[0].hostname, "wintermute");

        let children = source.children(&handle("root"), true).await.unwrap();
        assert_eq!(children.len(), 1);
        assert_eq!(children[0].relation, Relation::Subagent);
        assert!(source.children(&handle("root"), false).await.unwrap().is_empty());
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

    #[rstest]
    #[tokio::test]
    async fn previews_and_ids(#[future] source: SidecarSource) {
        let source = source.await;
        let preview = source.preview(&handle("root")).await.unwrap();
        assert_eq!(preview.first_prompt.as_deref(), Some("fix the flaky sync test"));
        assert_eq!(preview.last_assistant.as_deref(), Some("switched to a fixed clock"));
        assert_eq!(preview.activity.len(), 2);
        assert_eq!(source.find_by_id("ro").await.unwrap().len(), 1);
    }
}
