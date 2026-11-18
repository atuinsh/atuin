//! Queues the threads captured as they happen to be shared with the organizations their config
//! files name (see [`atuin_octavo::shares`]), for the upload engine to send after their messages.
//!
//! Capture only drops a thread's name in a channel, as looking at it must never hold capture up.

use std::collections::{HashMap, HashSet, VecDeque};
use std::future::Future;
use std::hash::Hash;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use atuin_client::ai_session::{AiSessionDatabase, DbError, HarnessSession};
use atuin_client::lua_config::OrgSlug;
use atuin_common::harnesstools::session::ParentKind;
use atuin_octavo::hub::UserId;
use atuin_octavo::pb;
use atuin_octavo::queue::{QueueError, UploadQueue};
use atuin_octavo::shares::{Configs, LookupError};
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tokio::time::Instant;

use super::DirectUploader;

/// How long a thread goes between looks at its config: a change made before a row the thread
/// writes reaches it within this of the row, without a walk up the disk for each line a busy agent
/// writes.
const RECHECK_AFTER: Duration = Duration::from_secs(60);

/// How many threads may wait for a look. One dropped for want of room gets another with its next
/// line.
const BACKLOG: usize = 256;

#[derive(Debug)]
pub(super) struct AutoSharer {
    threads: mpsc::Sender<LiveThread>,
    task: JoinHandle<()>,
}

impl AutoSharer {
    pub(super) fn spawn(uploader: Arc<DirectUploader>) -> Self {
        let (threads, heard) = mpsc::channel(BACKLOG);
        let sharer = Sharer {
            uploader,
            looks: Looks::default(),
            warned_of: HashSet::new(),
        };
        Self {
            threads,
            task: tokio::spawn(sharer.run(heard)),
        }
    }

    pub(super) fn consider(
        &self,
        user: UserId,
        session: HarnessSession,
        sidecar: &AiSessionDatabase,
    ) {
        let thread = LiveThread {
            user,
            session,
            sidecar: sidecar.clone(),
        };
        if self.threads.try_send(thread).is_err() {
            tracing::debug!("too many threads waiting to be considered for sharing; dropping one");
        }
    }
}

impl Drop for AutoSharer {
    fn drop(&mut self) {
        self.task.abort();
    }
}

/// The task that looks at the threads capture hears of.
struct Sharer {
    uploader: Arc<DirectUploader>,
    looks: Looks<(UserId, HarnessSession), LiveThread>,
    /// The config files atuin couldn't run that it warned of, as each is run again at each look.
    warned_of: HashSet<PathBuf>,
}

impl Sharer {
    async fn run(mut self, mut heard: mpsc::Receiver<LiveThread>) {
        loop {
            let thread = tokio::select! {
                thread = heard.recv() => {
                    let Some(thread) = thread else { return };
                    let key = (thread.user.clone(), thread.session.clone());
                    self.looks.heard(key, thread, Instant::now())
                }
                () = self.looks.until_next_end() => self.looks.ended(Instant::now()),
            };
            if let Some(thread) = thread {
                self.look_at(thread).await;
            }
        }
    }

    /// Queues the shares `thread`'s config files ask for.
    async fn look_at(&mut self, thread: LiveThread) {
        if !self.uploader.handle.settings().await.teams.enabled {
            return;
        }
        let candidate = match thread.read().await {
            Ok(Some(candidate)) => candidate,
            Ok(None) => return,
            Err(err) => {
                tracing::warn!(?err, "failed to read a thread to consider sharing it");
                return;
            }
        };
        let proposal = match candidate.look_up(&self.uploader.configs).await {
            Ok(proposal) => proposal,
            Err(LookupError::Config(err)) => {
                if self.warned_of.insert(err.path().to_owned()) {
                    tracing::warn!(
                        ?err,
                        "not sharing the threads a config file atuin can't run covers"
                    );
                }
                return;
            }
            Err(err) => {
                tracing::warn!(?err, "failed to look up a thread's config files");
                return;
            }
        };
        match proposal.queue(&self.uploader.queue).await {
            Ok(true) => self.uploader.wake.notify_one(),
            Ok(false) => {}
            Err(err) => tracing::warn!(?err, "failed to queue a thread share for octavo"),
        }
    }
}

/// A thread capture stored a row of as it happened.
#[derive(Debug)]
struct LiveThread {
    /// Who the thread's messages go up as, read as they were captured.
    user: UserId,
    session: HarnessSession,
    sidecar: AiSessionDatabase,
}

impl LiveThread {
    /// The thread to look up by each directory its rows name: none for a subagent, which its
    /// parent's share holds, nor for one of a harness Octavo doesn't know.
    async fn read(self) -> Result<Option<Candidate>, DbError> {
        let row = self.sidecar.get_session(&self.session).await?;
        let Some(row) = row.filter(|row| row.parent_kind != Some(ParentKind::Subagent)) else {
            return Ok(None);
        };
        let mut dirs = self.sidecar.directories(&self.session).await?;
        if dirs.is_empty() {
            dirs.extend(row.cwd);
        }
        let Some(thread) = pb::HarnessSession::of_thread(self.session) else {
            return Ok(None);
        };
        Ok(Some(Candidate {
            user: self.user,
            thread,
            dirs,
        }))
    }
}

/// A thread to look up its config files for.
#[derive(Debug)]
struct Candidate {
    user: UserId,
    thread: pb::HarnessSession,
    dirs: Vec<PathBuf>,
}

impl Candidate {
    /// The organizations its directories' configs all share it with.
    async fn look_up(self, configs: &Configs) -> Result<Proposal, LookupError> {
        let orgs = configs.orgs_of_thread(self.dirs).await?;
        Ok(Proposal {
            user: self.user,
            thread: self.thread,
            orgs,
        })
    }
}

/// The organizations a thread's configs share it with.
#[derive(Debug)]
struct Proposal {
    user: UserId,
    thread: pb::HarnessSession,
    orgs: Vec<OrgSlug>,
}

impl Proposal {
    /// Queues it for the engine to send, returning whether that queued an organization anew.
    async fn queue(self, queue: &UploadQueue) -> Result<bool, QueueError> {
        let queued = queue.thread(&self.user, &self.thread).push_shares(&self.orgs).await?;
        Ok(queued > 0)
    }
}

/// When each thread was last looked at, so it is looked at once a [`RECHECK_AFTER`] however many
/// rows it writes, and again at the end of each in which it wrote one.
#[derive(Debug)]
struct Looks<K, T> {
    /// The threads looked at in the last [`RECHECK_AFTER`]: when, and the latest row's word of the
    /// thread since, if it wrote one.
    recent: HashMap<K, (Instant, Option<T>)>,
    /// When each look's [`RECHECK_AFTER`] ends, soonest first, as each lasts as long. A look
    /// superseded since leaves its end behind, which comes to nothing.
    ends: VecDeque<(Instant, K)>,
}

impl<K, T> Default for Looks<K, T> {
    fn default() -> Self {
        Self {
            recent: HashMap::new(),
            ends: VecDeque::new(),
        }
    }
}

impl<K: Clone + Eq + Hash, T> Looks<K, T> {
    /// Hears that `key`'s thread wrote a row at `now`, returning `thread` to look at now, unless it
    /// waits for its last look's [`RECHECK_AFTER`] to end.
    fn heard(&mut self, key: K, thread: T, now: Instant) -> Option<T> {
        if let Some((at, again)) = self.recent.get_mut(&key)
            && now < *at + RECHECK_AFTER
        {
            *again = Some(thread);
            return None;
        }
        self.look(key, now);
        Some(thread)
    }

    /// When the soonest look's [`RECHECK_AFTER`] ends, if any.
    fn next_end(&self) -> Option<Instant> {
        self.ends.front().map(|&(end, _)| end)
    }

    /// Waits for [`Self::next_end`], or forever without one.
    fn until_next_end(&self) -> impl Future<Output = ()> + use<K, T> {
        let end = self.next_end();
        async move {
            match end {
                Some(end) => tokio::time::sleep_until(end).await,
                None => std::future::pending().await,
            }
        }
    }

    /// The thread to look at again at `now`, one that wrote a row since a look whose
    /// [`RECHECK_AFTER`] has ended, if any. Forgets those that wrote none.
    fn ended(&mut self, now: Instant) -> Option<T> {
        while let Some(&(end, _)) = self.ends.front()
            && end <= now
        {
            let Some((_, key)) = self.ends.pop_front() else {
                break;
            };
            let Some((at, again)) = self.recent.get_mut(&key) else {
                continue;
            };
            // Looked at since, by a row heard after this ended.
            if *at + RECHECK_AFTER != end {
                continue;
            }
            match again.take() {
                Some(thread) => {
                    self.look(key, now);
                    return Some(thread);
                }
                None => {
                    self.recent.remove(&key);
                }
            }
        }
        None
    }

    fn look(&mut self, key: K, now: Instant) {
        self.ends.push_back((now + RECHECK_AFTER, key.clone()));
        self.recent.insert(key, (now, None));
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::Path;

    use atuin_client::ai_session::{HarnessKind, Message, NativeSessionId, SourceId};
    use atuin_common::harnesstools::session::Role;
    use atuin_common::utils::uuid_v7;
    use atuin_domain::record::RecordId;
    use rstest::rstest;
    use time::OffsetDateTime;

    use super::*;

    const ACME: &str = r#"-- atuin:version=1
atuin.config = { orgs = { "acme" } }"#;

    fn handle() -> HarnessSession {
        HarnessSession {
            harness: HarnessKind::ClaudeCode,
            session: NativeSessionId::from("thread".to_owned()),
        }
    }

    /// The row a thread wrote `at` seconds in, in `cwd`.
    fn row(at: i64, cwd: Option<&Path>, parent_kind: Option<ParentKind>) -> Message {
        Message::builder()
            .id(RecordId(uuid_v7()))
            .session(handle())
            .source_id(SourceId::from(format!("line-{at}")))
            .parent(parent_kind.map(|_| HarnessSession {
                harness: HarnessKind::ClaudeCode,
                session: NativeSessionId::from("parent".to_owned()),
            }))
            .parent_kind(parent_kind)
            .timestamp(OffsetDateTime::UNIX_EPOCH + time::Duration::seconds(at))
            .role(Role::User)
            .content(Vec::new())
            .cwd(cwd.map(Path::to_path_buf))
            .build()
    }

    /// The orgs of a thread of `rows` in a new sidecar, for a user sharing with `user-org`.
    async fn orgs_of(rows: &[Message]) -> Vec<String> {
        let sidecar = AiSessionDatabase::in_memory().await.expect("a sidecar");
        for row in rows {
            sidecar.append(row).await.expect("an append");
        }

        let home = tempfile::tempdir().expect("a temp dir");
        fs::write(
            home.path().join("atuin.lua"),
            "-- atuin:version=1\natuin.config = { orgs = { \"user-org\" } }",
        )
        .expect("write");
        let thread = LiveThread {
            user: UserId::new("alice"),
            session: handle(),
            sidecar,
        };
        let Some(candidate) = thread.read().await.expect("a read") else {
            return Vec::new();
        };
        let configs = Configs::new(Some(home.path().to_path_buf()));
        let proposal = candidate.look_up(&configs).await.expect("a look");
        proposal.orgs.iter().map(ToString::to_string).collect()
    }

    fn project_with_config(root: &Path) -> PathBuf {
        let project = root.join("project");
        fs::create_dir_all(&project).expect("mkdir");
        fs::write(project.join("atuin.lua"), ACME).expect("write");
        project
    }

    /// A share holds the whole thread, so each directory it ran in must share it: what it said
    /// elsewhere must not reach a project's organizations, nor what a fork, which copied its
    /// parent's rows with their directories, says in a private project the parent's.
    #[rstest]
    #[case::in_its_project(&[None, Some("project"), Some("project/src")], &["acme"])]
    #[case::wandered_into_it(&[Some(""), Some("project")], &[])]
    #[case::wandered_out_of_it(&[Some("project"), Some("")], &[])]
    #[case::a_fork_in_a_private_project(&[Some("project"), Some("private")], &[])]
    #[tokio::test]
    async fn a_thread_is_shared_by_every_directory_its_rows_name(
        #[case] dirs: &[Option<&str>],
        #[case] expected: &[&str],
    ) {
        let root = tempfile::tempdir().expect("a temp dir");
        project_with_config(root.path());
        let private = root.path().join("private");
        fs::create_dir_all(&private).expect("mkdir");
        fs::write(private.join("atuin.lua"), "-- atuin:version=1\natuin.config = { orgs = {} }")
            .expect("write");
        let dirs: Vec<_> = dirs.iter().map(|dir| dir.map(|dir| root.path().join(dir))).collect();
        let rows: Vec<_> =
            (1..).zip(&dirs).map(|(at, dir)| row(at, dir.as_deref(), None)).collect();

        assert_eq!(orgs_of(&rows).await, expected);
    }

    #[rstest]
    #[tokio::test]
    async fn a_subagent_is_left_to_its_parents_share() {
        let root = tempfile::tempdir().expect("a temp dir");
        let project = project_with_config(root.path());

        let orgs = orgs_of(&[row(1, Some(&project), Some(ParentKind::Subagent))]).await;

        assert!(orgs.is_empty(), "shared a subagent with {orgs:?}");
    }

    #[rstest]
    #[case::fork(ParentKind::Fork)]
    #[case::continuation(ParentKind::Continuation)]
    #[tokio::test]
    async fn a_thread_from_another_goes_by_its_own_directory(#[case] kind: ParentKind) {
        let root = tempfile::tempdir().expect("a temp dir");
        let project = project_with_config(root.path());

        assert_eq!(orgs_of(&[row(1, Some(&project), Some(kind))]).await, ["acme"]);
    }

    #[rstest]
    #[tokio::test]
    async fn a_thread_naming_no_directory_is_not_shared() {
        assert!(orgs_of(&[row(1, None, None)]).await.is_empty());
    }

    /// The seconds at which a thread that writes a row at each of `rows` seconds is looked at.
    fn looked_at(rows: &[u64]) -> Vec<u64> {
        let start = Instant::now();
        let secs = |at: Instant| at.duration_since(start).as_secs();
        let mut looks = Looks::default();
        let mut looked = Vec::new();
        let mut rows = rows.iter().map(|&row| start + Duration::from_secs(row)).peekable();
        loop {
            let next_end = looks.next_end();
            let Some(row) = rows.next_if(|&row| next_end.is_none_or(|end| row < end)) else {
                let Some(end) = next_end else {
                    return looked;
                };
                looked.extend(looks.ended(end).map(|_| secs(end)));
                continue;
            };
            looked.extend(looks.heard("thread", row, row).map(secs));
        }
    }

    /// A config changed before a thread's last row reaches it within a minute of the row, however
    /// quiet the thread goes after: an agent waiting on its user writes nothing more.
    #[rstest]
    #[case::one_row(&[0], &[0])]
    #[case::rows_inside_a_window_look_again_at_its_end(&[0, 10, 30], &[0, 60])]
    #[case::a_busy_thread_looks_once_a_window(&[0, 30, 70, 100], &[0, 60, 120])]
    #[case::a_row_after_a_quiet_window_looks_at_once(&[0, 90], &[0, 90])]
    fn a_thread_is_looked_at_once_a_window_and_after_its_last_row(
        #[case] rows: &[u64],
        #[case] expected: &[u64],
    ) {
        assert_eq!(looked_at(rows), expected);
    }

    #[rstest]
    fn a_row_heard_after_its_window_ended_looks_at_once(
        #[values(Duration::from_secs(60), Duration::from_secs(90))] late: Duration,
    ) {
        let start = Instant::now();
        let mut looks = Looks::default();
        assert_eq!(looks.heard("thread", 0, start), Some(0));

        assert_eq!(looks.heard("thread", 1, start + late), Some(1));

        // The first look's window comes to nothing, as the second superseded it.
        assert_eq!(looks.ended(start + RECHECK_AFTER), None);
        assert_eq!(looks.next_end(), Some(start + late + RECHECK_AFTER));
    }

    #[rstest]
    fn each_thread_has_its_own_window() {
        let now = Instant::now();
        let mut looks = Looks::default();

        assert_eq!(looks.heard("one", 1, now), Some(1));
        assert_eq!(looks.heard("two", 2, now), Some(2));
        assert_eq!(looks.heard("one", 3, now), None);
    }
}
