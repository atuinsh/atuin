//! The threads Atuin for Teams uploads while `teams.upload_threads` is `workspaces`: those started
//! in a workspace, at or under a project's config file.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use atuin_client::ai_session::{AiSessionDatabase, HarnessSession, Message};
use atuin_client::settings::ThreadUploads;
use atuin_octavo::pb;
use atuin_octavo::shares::Configs;
use parking_lot::Mutex;

/// How long whether a thread started in a workspace is remembered. Live capture queues a thread's
/// messages as they come, under the locks that save them, and a lookup reads the disk and runs the
/// project's config, so it runs about once a minute a thread. A config added or changed counts
/// once the answer is out of date.
const REMEMBERED_FOR: Duration = Duration::from_mins(1);

/// How many threads' answers are kept before those out of date go.
const PRUNE_AT: usize = 1024;

/// Which threads started in a workspace, as `configs` say, with the answers of late.
#[derive(Clone, Debug)]
pub(super) struct Workspaces {
    configs: Configs,
    /// Where a thread started, as stored. Without it, its messages say.
    sidecar: Option<AiSessionDatabase>,
    remembered_for: Duration,
    started: Arc<Mutex<HashMap<HarnessSession, (bool, Instant)>>>,
}

impl Workspaces {
    pub(super) fn new(configs: Configs, sidecar: Option<AiSessionDatabase>) -> Self {
        Self::remembering(configs, sidecar, REMEMBERED_FOR)
    }

    fn remembering(
        configs: Configs,
        sidecar: Option<AiSessionDatabase>,
        remembered_for: Duration,
    ) -> Self {
        Self {
            configs,
            sidecar,
            remembered_for,
            started: Arc::default(),
        }
    }

    /// The configs that say who threads are shared with, and where workspaces are.
    pub(super) const fn configs(&self) -> &Configs {
        &self.configs
    }

    /// Those of `messages` that go up as `uploads` says. While it's `workspaces`, those whose
    /// threads started in one, as [`Self::started_in`] says.
    pub(super) async fn uploads(
        &self,
        uploads: ThreadUploads,
        messages: Vec<Message>,
    ) -> Vec<Message> {
        match uploads {
            ThreadUploads::All => messages,
            ThreadUploads::Workspaces => self.started_in(&messages).await,
            ThreadUploads::Off => Vec::new(),
        }
    }

    /// Whether `thread`'s queued share goes to organizations as `uploads` says. While it's
    /// `workspaces`, only that of a thread that started in one goes.
    pub(super) async fn shares(&self, uploads: ThreadUploads, thread: &pb::HarnessSession) -> bool {
        match uploads {
            ThreadUploads::All => true,
            ThreadUploads::Workspaces => match thread.thread() {
                Some(session) => self.is_from_one(&session, &[]).await,
                None => false,
            },
            ThreadUploads::Off => false,
        }
    }

    /// Those of `messages` whose threads started in a workspace. A thread whose start neither the
    /// sidecar nor `messages` says, or whose config can't be read, stays here.
    async fn started_in(&self, messages: &[Message]) -> Vec<Message> {
        let mut started = HashMap::new();
        for message in messages {
            if started.contains_key(&message.session) {
                continue;
            }
            let in_workspace = self.is_from_one(&message.session, messages).await;
            started.insert(message.session.clone(), in_workspace);
        }
        let uploads = |message: &&Message| started.get(&message.session) == Some(&true);
        messages.iter().filter(uploads).cloned().collect()
    }

    /// Whether `session` started in a workspace. Not remembered while its start can't be told, as
    /// its next messages may say.
    async fn is_from_one(&self, session: &HarnessSession, messages: &[Message]) -> bool {
        if let Some(in_workspace) = self.remembered(session) {
            return in_workspace;
        }
        let Some(dir) = start(session, messages, self.sidecar.as_ref()).await else {
            return false;
        };
        let in_workspace = self.configs.in_workspace(dir).await.unwrap_or_else(|err| {
            tracing::warn!(?err, "keeping a thread here, as its workspace can't be told");
            false
        });
        self.remember(session, in_workspace);
        in_workspace
    }

    fn remembered(&self, session: &HarnessSession) -> Option<bool> {
        let (in_workspace, at) = *self.started.lock().get(session)?;
        (at.elapsed() < self.remembered_for).then_some(in_workspace)
    }

    fn remember(&self, session: &HarnessSession, in_workspace: bool) {
        let mut started = self.started.lock();
        if started.len() >= PRUNE_AT {
            started.retain(|_, (_, at)| at.elapsed() < self.remembered_for);
        }
        started.insert(session.clone(), (in_workspace, Instant::now()));
    }
}

/// The directory `session` started in: the first its stored rows name, or else the first of
/// those in `messages` that name one.
async fn start(
    session: &HarnessSession,
    messages: &[Message],
    sidecar: Option<&AiSessionDatabase>,
) -> Option<PathBuf> {
    let stored = match sidecar {
        Some(sidecar) => sidecar.directories(session).await,
        None => Ok(Vec::new()),
    };
    match stored {
        Ok(dirs) if !dirs.is_empty() => return dirs.into_iter().next(),
        Ok(_) => {}
        // Its messages may still say.
        Err(err) => tracing::warn!(?err, "failed to read where a thread started"),
    }
    let named = messages.iter().filter(|message| message.session == *session);
    let first =
        named.filter(|message| message.cwd.is_some()).min_by_key(|message| message.timestamp);
    first.and_then(|message| message.cwd.clone())
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::Path;

    use atuin_client::ai_session::{HarnessKind, NativeSessionId, SourceId};
    use atuin_common::harnesstools::session::Role;
    use atuin_common::utils::uuid_v7;
    use atuin_domain::record::RecordId;
    use jiff::Timestamp;
    use rstest::{fixture, rstest};
    use tempfile::TempDir;

    use super::*;

    /// A thread's row, as `(thread, seconds in, directory under the root)`.
    type Row<'a> = (&'a str, i64, Option<&'a str>);

    /// A root whose `project` has a config file, and whose `elsewhere` has none.
    #[fixture]
    fn root() -> TempDir {
        let root = tempfile::tempdir().expect("a temp dir");
        let project = root.path().join("project");
        fs::create_dir_all(&project).expect("mkdir");
        fs::write(project.join(".atuin.lua"), "-- atuin:version=1\natuin.config = {}")
            .expect("write");
        root
    }

    fn thread(name: &str) -> HarnessSession {
        HarnessSession {
            harness: HarnessKind::ClaudeCode,
            session: NativeSessionId::from(name.to_owned()),
        }
    }

    /// The messages `rows` say, under `root`.
    fn messages(root: &Path, rows: &[Row<'_>]) -> Vec<Message> {
        let row = |&(name, at, dir): &Row<'_>| {
            Message::builder()
                .id(RecordId(uuid_v7()))
                .session(thread(name))
                .source_id(SourceId::from(format!("{name}-{at}")))
                .timestamp(Timestamp::UNIX_EPOCH + jiff::SignedDuration::from_secs(at))
                .role(Role::User)
                .content(Vec::new())
                .cwd(dir.map(|dir| root.join(dir)))
                .build()
        };
        rows.iter().map(row).collect()
    }

    /// The threads of `messages` that go up as `uploads` says.
    async fn uploaded(
        workspaces: &Workspaces,
        uploads: ThreadUploads,
        messages: Vec<Message>,
    ) -> Vec<String> {
        let kept = workspaces.uploads(uploads, messages).await;
        let name = |message: &Message| AsRef::<str>::as_ref(&message.session.session).to_owned();
        let mut names: Vec<_> = kept.iter().map(name).collect();
        names.dedup();
        names
    }

    #[rstest]
    #[case::started_in_one(&[], &[("a", 1, Some("project/src")), ("a", 2, None)], &["a"])]
    #[case::started_elsewhere(&[], &[("a", 1, Some("elsewhere"))], &[])]
    #[case::wandered_out_of_one(&[], &[("a", 1, Some("project")), ("a", 2, Some("elsewhere"))], &["a"])]
    #[case::wandered_into_one(&[], &[("a", 1, Some("elsewhere")), ("a", 2, Some("project"))], &[])]
    #[case::says_nowhere(&[], &[("a", 1, None)], &[])]
    #[case::as_stored(&[("a", 1, Some("project"))], &[("a", 2, None)], &["a"])]
    #[case::stored_elsewhere(&[("a", 1, Some("elsewhere"))], &[("a", 2, Some("project"))], &[])]
    #[case::each_by_its_own(
        &[],
        &[("a", 1, Some("project")), ("b", 1, Some("elsewhere")), ("a", 2, None)],
        &["a"],
    )]
    #[tokio::test]
    async fn a_thread_goes_up_from_a_workspace_it_started_in(
        root: TempDir,
        #[case] stored: &[Row<'_>],
        #[case] captured: &[Row<'_>],
        #[case] expected: &[&str],
    ) {
        let sidecar = AiSessionDatabase::in_memory().await.expect("a sidecar");
        for row in messages(root.path(), stored) {
            sidecar.append(&row).await.expect("an append");
        }
        let workspaces = Workspaces::new(Configs::new(None), Some(sidecar));
        let captured = messages(root.path(), captured);
        assert_eq!(uploaded(&workspaces, ThreadUploads::Workspaces, captured).await, expected);
    }

    /// Without a sidecar, as for queued messages, a thread's start is the first its messages
    /// name.
    #[rstest]
    #[case::in_one(&[("a", 2, Some("elsewhere")), ("a", 1, Some("project"))], &["a"])]
    #[case::elsewhere(&[("a", 1, Some("elsewhere"))], &[])]
    #[tokio::test]
    async fn without_a_sidecar_the_messages_say(
        root: TempDir,
        #[case] queued: &[Row<'_>],
        #[case] expected: &[&str],
    ) {
        let workspaces = Workspaces::new(Configs::new(None), None);
        let queued = messages(root.path(), queued);
        assert_eq!(uploaded(&workspaces, ThreadUploads::Workspaces, queued).await, expected);
    }

    #[rstest]
    #[case::all(ThreadUploads::All, &["a", "b"])]
    #[case::off(ThreadUploads::Off, &[])]
    #[tokio::test]
    async fn the_rest_go_up_or_stay_whole(
        root: TempDir,
        #[case] uploads: ThreadUploads,
        #[case] expected: &[&str],
    ) {
        let workspaces = Workspaces::new(Configs::new(None), None);
        let rows = messages(root.path(), &[("a", 1, Some("project")), ("b", 1, Some("elsewhere"))]);
        assert_eq!(uploaded(&workspaces, uploads, rows).await, expected);
    }

    /// An answer is remembered while it's fresh, so a thread's messages one at a time look it up
    /// once.
    #[rstest]
    #[case::fresh(REMEMBERED_FOR, &["a"])]
    #[case::out_of_date(Duration::ZERO, &[])]
    #[tokio::test]
    async fn an_answer_is_remembered_while_fresh(
        root: TempDir,
        #[case] remembered_for: Duration,
        #[case] expected: &[&str],
    ) {
        let workspaces = Workspaces::remembering(Configs::new(None), None, remembered_for);
        let first = messages(root.path(), &[("a", 1, Some("project"))]);
        assert_eq!(uploaded(&workspaces, ThreadUploads::Workspaces, first).await, ["a"]);

        fs::remove_file(root.path().join("project/.atuin.lua")).expect("rm");
        let next = messages(root.path(), &[("a", 2, Some("project"))]);
        assert_eq!(uploaded(&workspaces, ThreadUploads::Workspaces, next).await, expected);
    }

    /// A queued share goes under `workspaces` only for a thread that started in one, as the
    /// sidecar says.
    #[rstest]
    #[case::all_in_one(ThreadUploads::All, "a", true)]
    #[case::all_elsewhere(ThreadUploads::All, "b", true)]
    #[case::from_one(ThreadUploads::Workspaces, "a", true)]
    #[case::from_elsewhere(ThreadUploads::Workspaces, "b", false)]
    #[case::not_stored(ThreadUploads::Workspaces, "c", false)]
    #[case::off(ThreadUploads::Off, "a", false)]
    #[tokio::test]
    async fn a_share_goes_as_its_thread_would(
        root: TempDir,
        #[case] uploads: ThreadUploads,
        #[case] name: &str,
        #[case] expected: bool,
    ) {
        let sidecar = AiSessionDatabase::in_memory().await.expect("a sidecar");
        for row in messages(root.path(), &[("a", 1, Some("project")), ("b", 1, Some("elsewhere"))])
        {
            sidecar.append(&row).await.expect("an append");
        }
        let workspaces = Workspaces::new(Configs::new(None), Some(sidecar));
        let thread = pb::HarnessSession::of_thread(thread(name)).expect("a thread Octavo names");
        assert_eq!(workspaces.shares(uploads, &thread).await, expected);
    }

    /// A thread whose start nothing says yet isn't remembered as kept here, so its next messages
    /// can say.
    #[rstest]
    #[tokio::test]
    async fn an_unknown_start_is_not_remembered(root: TempDir) {
        let workspaces = Workspaces::new(Configs::new(None), None);
        let first = messages(root.path(), &[("a", 1, None)]);
        assert!(uploaded(&workspaces, ThreadUploads::Workspaces, first).await.is_empty());
        let next = messages(root.path(), &[("a", 2, Some("project"))]);
        assert_eq!(uploaded(&workspaces, ThreadUploads::Workspaces, next).await, ["a"]);
    }
}
