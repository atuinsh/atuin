//! The real [`SessionSource`]: the ai-session sidecar database, opened read-only beside the daemon
//! that writes it, so the picker's first frame never waits on the daemon.
//!
//! Sessions record their host by id. Names come from the record store (the newest command each
//! host synced), which costs a decrypt per host: they are read once, off the UI thread, when the
//! picker first asks, and rows show a short form of the id until then, or for good when the store
//! or key can't be read.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use async_trait::async_trait;
use atuin_client::ai_session::{
    AiSessionDatabase, HarnessSession, HostSet, Session, SessionFilter as DbFilter, SessionRelation,
};
use atuin_client::history::store::HistoryStore;
use atuin_client::record::sqlite_store::SqliteStore;
use atuin_client::settings::Settings;
use atuin_common::encryption::paseto_v4;
use atuin_common::harnesstools::rehydrate::RehydrateSession;
use atuin_common::harnesstools::session::{Content, Role};
use atuin_common::string::highlighted::HighlightedString;
use atuin_common::utils::in_git_repo;
use atuin_domain::record::HostId;
use eyre::{Result, WrapErr};
use futures::TryStreamExt;
use parking_lot::Mutex;
use tokio::sync::OnceCell;
use uuid::Uuid;

use super::ResumeContext;
use super::source::{Relation, SessionFilter, SessionPreview, SessionRow, SessionSource, Snippet};

/// Where host names are read from: the record store and the key its history is encrypted with.
/// Both are only read: the store is opened read-only (no migrations), and a missing key is not
/// generated.
#[derive(Clone, Debug)]
pub struct HostNameSource {
    pub record_store: PathBuf,
    pub key: PathBuf,
    pub timeout: Duration,
}

impl HostNameSource {
    pub fn new(settings: &Settings) -> Self {
        Self {
            record_store: settings.record_store_path.clone(),
            key: settings.key_path.clone(),
            timeout: settings.local_timeout,
        }
    }

    /// Each host's name, by id (simple form).
    async fn load(&self) -> Result<HashMap<String, String>> {
        let key = paseto_v4::Key::try_load_from_path(&self.key)
            .wrap_err_with(|| format!("could not read the key {}", self.key.display()))?;
        let store = SqliteStore::open_read_only(&self.record_store, self.timeout).await?;
        // The host id only matters for writing; this store never writes.
        let names = HistoryStore::new(store, HostId(Uuid::nil()), key).host_names().await?;
        Ok(names.into_iter().map(|(id, name)| (id.0.as_simple().to_string(), name)).collect())
    }
}

pub struct SidecarSource {
    db: AiSessionDatabase,
    /// This host's id (simple form) and name.
    host_id: String,
    hostname: String,
    /// The git repository each session directory is in, looked up once per picker: finding it
    /// walks the filesystem, and every search would otherwise repeat it for each row.
    git_roots: Mutex<HashMap<PathBuf, Option<PathBuf>>>,
    host_names_from: Option<HostNameSource>,
    /// Other hosts' names by id (simple form), read once for the picker's lifetime. Empty when
    /// they can't be read.
    host_names: OnceCell<HashMap<String, String>>,
}

impl SidecarSource {
    /// Open the sidecar at `path`. Host names are read from `host_names` when the picker asks for
    /// them (see [`SessionSource::host_names`]); without it, hosts show by id.
    pub async fn open(
        path: &Path,
        context: &ResumeContext,
        host_names: Option<HostNameSource>,
    ) -> Result<Self> {
        let db = AiSessionDatabase::open_read_only(path).await.wrap_err_with(|| {
            format!("could not open the AI session database {}", path.display())
        })?;
        Ok(Self::new(db, context, host_names))
    }

    fn new(
        db: AiSessionDatabase,
        context: &ResumeContext,
        host_names_from: Option<HostNameSource>,
    ) -> Self {
        let host_id = super::simple_host_id(&context.host_id);
        Self {
            db,
            host_id,
            hostname: context.hostname.clone(),
            git_roots: Mutex::new(HashMap::new()),
            host_names_from,
            host_names: OnceCell::new(),
        }
    }

    /// The git repository root `cwd` is in, if any.
    fn git_root(&self, cwd: &Path) -> Option<PathBuf> {
        let mut roots = self.git_roots.lock();
        roots.entry(cwd.to_owned()).or_insert_with(|| in_git_repo(&cwd.to_string_lossy())).clone()
    }

    /// Other hosts' names, read on first use. Reading them never fails the picker: without the
    /// record store or key (not logged in, say), there are none.
    async fn names(&self) -> &HashMap<String, String> {
        self.host_names
            .get_or_init(|| async {
                let Some(from) = &self.host_names_from else {
                    return HashMap::new();
                };
                from.load().await.unwrap_or_else(|e| {
                    tracing::debug!("no host names, showing host ids: {e:#}");
                    HashMap::new()
                })
            })
            .await
    }

    /// How a host shows in rows: this host by its current name, another by the name it last
    /// synced under, or a short, stable form of its id when that isn't known (yet).
    fn hostname_of(&self, host_id: &str) -> String {
        if host_id == self.host_id {
            return self.hostname.clone();
        }
        self.host_names
            .get()
            .and_then(|names| names.get(host_id))
            .cloned()
            .unwrap_or_else(|| short_host_id(host_id))
    }

    fn local_host(&self) -> Option<HostId> {
        Uuid::try_parse(&self.host_id).ok().map(HostId)
    }

    /// The hosts `filter` keeps, or `None` for all. `@name` stands for every host showing as a
    /// name starting with it, and this host's sessions from before hosts were recorded are its
    /// own.
    async fn host_set(&self, filter: &SessionFilter) -> Result<Option<HostSet>> {
        let local = self.local_host();
        let mut ids = if let Some(name) = &filter.host_name {
            // Match names, not the short ids rows show until the names are read.
            self.names().await;
            let mut named: Vec<HostId> = self
                .db
                .session_hosts()
                .await?
                .into_iter()
                .filter(|h| {
                    self.hostname_of(&h.0.as_simple().to_string()).starts_with(name.as_str())
                })
                .collect();
            if let Some(local) = local
                && self.hostname.starts_with(name.as_str())
                && !named.contains(&local)
            {
                named.push(local);
            }
            Some(named)
        } else {
            None
        };
        if let Some(host) = &filter.host {
            let host = Uuid::try_parse(host).ok().map(HostId);
            ids = Some(match (ids, host) {
                (Some(ids), Some(host)) => ids.into_iter().filter(|&h| h == host).collect(),
                (None, Some(host)) => vec![host],
                (_, None) => Vec::new(),
            });
        }
        Ok(ids.map(|ids| HostSet {
            unrecorded: local.is_some_and(|l| ids.contains(&l)),
            ids,
        }))
    }

    /// The filter for everything but hosts (see [`Self::host_set`]).
    fn db_filter(filter: &SessionFilter) -> DbFilter {
        DbFilter {
            host: None,
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
        let hostname = self.hostname_of(&host_id);
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
}

/// A host id's short form, for a host whose name isn't known.
fn short_host_id(host_id: &str) -> String {
    host_id.chars().take(8).collect()
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
        let db_filter = Self::db_filter(filter);
        let matches: Vec<_> = match self.host_set(filter).await? {
            Some(hosts) => {
                self.db
                    .search_on_hosts(&filter.text, &db_filter, &hosts, limit)
                    .try_collect()
                    .await?
            }
            None => self.db.search(&filter.text, &db_filter, limit).try_collect().await?,
        };

        let has_text = !filter.text.trim().is_empty();
        let mut rows = Vec::with_capacity(matches.len());
        for m in matches {
            let mut row = self.row(m.session);
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
        let sessions = self.db.sessions_with_id_prefix(id).await?;
        Ok(sessions.into_iter().map(|s| self.row(s)).collect())
    }

    async fn preview(&self, session: &HarnessSession) -> Result<SessionPreview> {
        let parts = self.db.preview_parts(session).await?;
        Ok(SessionPreview {
            first_prompt: parts.first_user.as_deref().and_then(text_of),
            last_assistant: parts.last_assistant.as_deref().and_then(text_of),
            activity: parts
                .activity
                .into_iter()
                .filter(|(_, role)| matches!(role, Role::User | Role::Assistant))
                .map(|(at, _)| at)
                .collect(),
        })
    }

    async fn children(
        &self,
        session: &HarnessSession,
        include_subagents: bool,
    ) -> Result<Vec<SessionRow>> {
        // Newest first, from the query.
        Ok(self
            .db
            .children(session)
            .await?
            .into_iter()
            .map(|s| self.row(s))
            .filter(|r| include_subagents || r.relation != Relation::Subagent)
            .collect())
    }

    async fn rehydrate(&self, session: &HarnessSession, cwd: &Path) -> Result<RehydrateSession> {
        self.db
            .rehydrate_session(session, cwd.to_owned())
            .await?
            .ok_or_else(|| eyre::eyre!("the session isn't in the AI session database"))
    }

    async fn host_names(&self) -> Result<HashMap<String, String>> {
        Ok(self.names().await.clone())
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
        SidecarSource::new(db, &context(), None)
    }

    fn context() -> ResumeContext {
        ResumeContext {
            host_id: HOST.to_owned(),
            hostname: "wintermute".to_owned(),
            ..ResumeContext::default()
        }
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
        // User and assistant messages only.
        assert_eq!(preview.activity.len(), 3);
    }

    // --- hosts ----------------------------------------------------------------------------------

    const BUILD_1: &str = "0190bbbb0000700080000000000000b1";
    const BUILD_2: &str = "0190cccc0000700080000000000000b2";
    const LAPTOP: &str = "0190dddd0000700080000000000000dd";

    fn host(id: &str) -> HostId {
        HostId(Uuid::try_parse(id).unwrap())
    }

    /// Sessions on this host (`here`, and `old`, from before hosts were recorded), two hosts both
    /// named buildbox, and a laptop.
    async fn hosts_db() -> AiSessionDatabase {
        let db = AiSessionDatabase::in_memory().await.unwrap();
        for (i, (id, on)) in [
            ("here", Some(HOST)),
            ("old", None),
            ("build-1", Some(BUILD_1)),
            ("build-2", Some(BUILD_2)),
            ("laptop", Some(LAPTOP)),
        ]
        .into_iter()
        .enumerate()
        {
            let mut m = message(id, None, Role::User, "shared words", i64::try_from(i).unwrap());
            m.host = on.map(host);
            db.append(&m).await.unwrap();
        }
        db
    }

    /// A record store and key in `dir` naming the hosts as their newest commands do.
    async fn record_store(dir: &Path, names: &[(&str, &str)]) -> HostNameSource {
        use atuin_client::history::History;
        use atuin_domain::record::CmdOrigin;

        let from = HostNameSource {
            record_store: dir.join("records.db"),
            key: dir.join("key"),
            timeout: std::time::Duration::from_secs(5),
        };
        let key = paseto_v4::Key::from([7u8; 32]);
        key.try_write_path(&from.key).unwrap();
        let store = SqliteStore::new(&from.record_store, from.timeout).await.unwrap();
        for (id, name) in names {
            let history: History = History::daemon()
                .timestamp(time::OffsetDateTime::now_utc())
                .command("ls")
                .cwd("/")
                .session("018deb6e8287781f9973ef40e0fde76b")
                .cmd_origin(CmdOrigin::try_from(format!("{name}:me").as_str()).unwrap())
                .build()
                .into();
            let history_store = HistoryStore::new(store.clone(), host(id), [7u8; 32].into());
            history_store.push(history).await.unwrap();
        }
        from
    }

    fn hosts(rows: &[SessionRow]) -> Vec<(String, String)> {
        let mut hosts: Vec<_> =
            rows.iter().map(|r| (r.handle.session.to_string(), r.hostname.clone())).collect();
        hosts.sort();
        hosts
    }

    fn at_host(name: &str) -> SessionFilter {
        SessionFilter {
            host_name: Some(name.to_owned()),
            ..roots("")
        }
    }

    fn pairs(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
        pairs.iter().map(|&(a, b)| (a.to_owned(), b.to_owned())).collect()
    }

    #[rstest]
    #[tokio::test]
    async fn remote_rows_show_host_names_once_read() {
        let dir = tempfile::tempdir().unwrap();
        let from = record_store(dir.path(), &[
            (HOST, "old-name"),
            (BUILD_1, "buildbox"),
            (BUILD_2, "buildbox"),
            (LAPTOP, "laptop"),
        ])
        .await;
        let source = SidecarSource::new(hosts_db().await, &context(), Some(from));

        // Before the names are read (the first frame), rows show short ids.
        let rows = source.search(&roots("")).await.unwrap();
        assert_eq!(
            hosts(&rows),
            pairs(&[
                ("build-1", "0190bbbb"),
                ("build-2", "0190cccc"),
                ("here", "wintermute"),
                ("laptop", "0190dddd"),
                ("old", "wintermute"),
            ])
        );

        let names = source.host_names().await.unwrap();
        assert_eq!(names.get(BUILD_1).map(String::as_str), Some("buildbox"));
        let rows = source.search(&roots("")).await.unwrap();
        assert_eq!(
            hosts(&rows),
            pairs(&[
                ("build-1", "buildbox"),
                ("build-2", "buildbox"),
                // This host keeps its current name.
                ("here", "wintermute"),
                ("laptop", "laptop"),
                ("old", "wintermute"),
            ])
        );
    }

    /// `@name` stands for every host of that name, and this host's includes the sessions from
    /// before hosts were recorded. It waits for the names, which the first search didn't.
    #[rstest]
    #[case::two_hosts_one_name("build", &["build-1", "build-2"])]
    #[case::this_host("winter", &["here", "old"])]
    #[case::one("lap", &["laptop"])]
    #[case::none("nowhere", &[])]
    #[case::by_id_it_has_a_name("0190bbbb", &[])]
    #[tokio::test]
    async fn at_host_filters_by_name(#[case] name: &str, #[case] expected: &[&str]) {
        let dir = tempfile::tempdir().unwrap();
        let from = record_store(dir.path(), &[
            (BUILD_1, "buildbox"),
            (BUILD_2, "buildbox"),
            (LAPTOP, "laptop"),
        ])
        .await;
        let source = SidecarSource::new(hosts_db().await, &context(), Some(from));

        for text in ["", "shared"] {
            let filter = SessionFilter {
                text: text.to_owned(),
                ..at_host(name)
            };
            let mut found: Vec<_> = source
                .search(&filter)
                .await
                .unwrap()
                .into_iter()
                .map(|r| r.handle.session.to_string())
                .collect();
            found.sort();
            assert_eq!(found, expected, "{text:?}");
        }
    }

    /// The host filter mode keeps this host's sessions, those from before hosts were recorded
    /// included.
    #[rstest]
    #[tokio::test]
    async fn the_host_mode_keeps_this_hosts_unrecorded_sessions() {
        let source = SidecarSource::new(hosts_db().await, &context(), None);
        let filter = SessionFilter {
            host: Some(HOST.to_owned()),
            ..roots("")
        };
        assert_eq!(
            hosts(&source.search(&filter).await.unwrap()),
            pairs(&[("here", "wintermute"), ("old", "wintermute")])
        );
        let both = SessionFilter {
            host_name: Some("lap".to_owned()),
            ..filter
        };
        assert!(source.search(&both).await.unwrap().is_empty());
    }

    /// Without the record store or key (not logged in, say), hosts show and filter by short id.
    #[rstest]
    #[case::nothing(false, false)]
    #[case::no_store(true, false)]
    #[case::no_key(false, true)]
    #[case::another_key(true, true)]
    #[tokio::test]
    async fn without_names_hosts_show_by_id(#[case] key: bool, #[case] store: bool) {
        let dir = tempfile::tempdir().unwrap();
        let from = HostNameSource {
            record_store: dir.path().join("records.db"),
            key: dir.path().join("key"),
            timeout: std::time::Duration::from_secs(5),
        };
        if store {
            record_store(dir.path(), &[(BUILD_1, "buildbox")]).await;
            std::fs::remove_file(&from.key).unwrap();
        }
        if key {
            paseto_v4::Key::generate().try_write_path(&from.key).unwrap();
        }
        let source = SidecarSource::new(hosts_db().await, &context(), Some(from));

        assert!(source.host_names().await.unwrap().is_empty());
        let rows = source.search(&at_host("0190bb")).await.unwrap();
        assert_eq!(hosts(&rows), pairs(&[("build-1", "0190bbbb")]));
        assert_eq!(dir.path().join("records.db").exists(), store, "never created");
        assert_eq!(dir.path().join("key").exists(), key, "never generated");
    }
}
