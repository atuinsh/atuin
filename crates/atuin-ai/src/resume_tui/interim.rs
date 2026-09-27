//! A stopgap [`SessionSource`] over the current sidecar API, so `atuin ai resume` works before the
//! read-only sidecar layer (`SessionFilter`, prefix search, highlights, host and root grouping)
//! lands. Replace it with that layer at merge.
//!
//! Limitations, all fixed by the real layer:
//! - opens the database read-write through `AiSessionDatabase::open` (which migrates);
//! - no host tracking, so every session counts as this host's;
//! - full-text search matches whole words only and has no highlight spans;
//! - filters and fork grouping run in memory over every session.

use std::path::{Path, PathBuf};

use async_trait::async_trait;
use atuin_client::ai_session::{AiSessionDatabase, HarnessSession, Session};
use atuin_client::settings::Settings;
use atuin_common::harnesstools::session::{Content, Role};
use atuin_common::utils::in_git_repo;
use eyre::{Result, bail};
use futures::TryStreamExt;

use super::ResumeContext;
use super::source::{Relation, SessionFilter, SessionPreview, SessionRow, SessionSource, Snippet};

/// The sidecar database the daemon writes.
pub fn sidecar_path() -> PathBuf {
    Settings::effective_data_dir().join("ai_harness_sessions.db")
}

pub struct SidecarSource {
    db: AiSessionDatabase,
    host_id: String,
    hostname: String,
}

impl SidecarSource {
    pub async fn open(path: &Path, context: &ResumeContext) -> Result<Self> {
        if !path.exists() {
            bail!(
                "no AI sessions have been captured yet ({} does not exist). Enable \
                 `ai.capture_sessions` and run the daemon to record them.",
                path.display()
            );
        }
        Ok(Self {
            db: AiSessionDatabase::open(path).await?,
            host_id: context.host_id.clone(),
            hostname: context.hostname.clone(),
        })
    }

    fn row(&self, s: Session, relation: Relation) -> SessionRow {
        let git_root = s.cwd.as_deref().and_then(|c| in_git_repo(&c.to_string_lossy()));
        let title = s.title.clone().or(s.preview.clone()).unwrap_or_default();
        SessionRow {
            handle: s.handle,
            relation,
            title: Snippet::plain(title),
            cwd: s.cwd,
            git_root,
            branch: s.git_branch,
            model: s.model,
            host_id: self.host_id.clone(),
            hostname: self.hostname.clone(),
            started_at: s.started_at,
            updated_at: s.updated_at,
            message_count: s.message_count,
            children: 0,
            matched: None,
        }
    }

    fn keep(row: &SessionRow, filter: &SessionFilter) -> bool {
        let cwd = row.cwd.as_deref();
        (filter.harnesses.is_empty() || filter.harnesses.contains(&row.handle.harness))
            && filter.model.as_ref().is_none_or(|m| {
                row.model.as_ref().is_some_and(|rm| rm.to_lowercase().contains(&m.to_lowercase()))
            })
            && filter.branch.as_ref().is_none_or(|b| row.branch.as_ref() == Some(b))
            && filter.host_id.as_ref().is_none_or(|h| &row.host_id == h)
            && filter.hostname.as_ref().is_none_or(|h| row.hostname.starts_with(h.as_str()))
            && filter.cwd_prefix.as_deref().is_none_or(|p| cwd.is_some_and(|c| c.starts_with(p)))
            && filter.cwd.as_deref().is_none_or(|p| cwd == Some(p))
    }

    /// Walk parents up to the root.
    fn root_of<'a>(sessions: &'a [Session], mut s: &'a Session) -> &'a Session {
        for _ in 0..32 {
            let Some(parent) = &s.parent else {
                break;
            };
            match sessions.iter().find(|p| &p.handle == parent) {
                Some(p) => s = p,
                None => break,
            }
        }
        s
    }
}

fn text_of(content: &[Content]) -> Option<String> {
    let text: Vec<&str> = content
        .iter()
        .filter_map(|c| match c {
            Content::Text(t) | Content::Summary(t) => Some(t.as_str()),
            _ => None,
        })
        .collect();
    (!text.is_empty()).then(|| text.join("\n"))
}

#[async_trait]
impl SessionSource for SidecarSource {
    async fn search(&self, filter: &SessionFilter) -> Result<Vec<SessionRow>> {
        let all = self.db.list_sessions(None).await?;

        // With a query, rank roots by their best-matching session (theirs or a child's).
        let matches: Option<Vec<(HarnessSession, Option<String>)>> = if filter.text.is_empty() {
            None
        } else {
            let found: Vec<_> = self.db.search(&filter.text, None, 0).try_collect().await?;
            Some(
                found
                    .into_iter()
                    .map(|m| {
                        let preview = m.preview.display_plain().to_string();
                        (m.session.handle, (!preview.is_empty()).then_some(preview))
                    })
                    .collect(),
            )
        };

        let mut rows: Vec<SessionRow> = Vec::new();
        let order: Vec<&Session> = match &matches {
            None => all.iter().collect(),
            Some(m) => m.iter().filter_map(|(h, _)| all.iter().find(|s| &s.handle == h)).collect(),
        };
        for s in order {
            let root = if filter.group_forks {
                Self::root_of(&all, s)
            } else {
                s
            };
            if rows.iter().any(|r| r.handle == root.handle) {
                continue;
            }
            let mut row = self.row(root.clone(), Relation::Root);
            if !Self::keep(&row, filter) {
                continue;
            }
            let children: Vec<&Session> = all
                .iter()
                .filter(|c| c.handle != root.handle && Self::root_of(&all, c).handle == root.handle)
                .collect();
            if filter.group_forks {
                row.children = u32::try_from(children.len()).unwrap_or(u32::MAX);
                if let Some(newest) = children.iter().map(|c| c.updated_at).max() {
                    row.updated_at = row.updated_at.max(newest);
                }
            }
            if let Some(m) = &matches {
                row.matched = m
                    .iter()
                    .find(|(h, _)| h == &s.handle)
                    .and_then(|(_, p)| p.clone())
                    .map(Snippet::plain);
            }
            rows.push(row);
            if filter.limit != 0 && rows.len() >= filter.limit {
                break;
            }
        }
        Ok(rows)
    }

    async fn find_by_id(&self, id: &str) -> Result<Vec<SessionRow>> {
        Ok(self
            .db
            .list_sessions(None)
            .await?
            .into_iter()
            .filter(|s| s.handle.session.as_ref().starts_with(id))
            .map(|s| self.row(s, Relation::Root))
            .collect())
    }

    async fn preview(&self, session: &HarnessSession) -> Result<SessionPreview> {
        let messages: Vec<_> = self.db.messages(session).try_collect().await?;
        let first_prompt =
            messages.iter().filter(|m| m.role == Role::User).find_map(|m| text_of(&m.content));
        let last_assistant = messages
            .iter()
            .rev()
            .filter(|m| m.role == Role::Assistant)
            .find_map(|m| text_of(&m.content));
        Ok(SessionPreview {
            first_prompt,
            last_assistant,
        })
    }

    async fn children(
        &self,
        session: &HarnessSession,
        _include_subagents: bool,
    ) -> Result<Vec<SessionRow>> {
        let all = self.db.list_sessions(None).await?;
        Ok(all
            .iter()
            .filter(|c| &c.handle != session && &Self::root_of(&all, c).handle == session)
            .map(|c| self.row(c.clone(), Relation::Fork))
            .collect())
    }
}
