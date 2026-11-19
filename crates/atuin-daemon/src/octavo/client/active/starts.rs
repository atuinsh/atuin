//! Which threads Atuin for Teams uploads: those that the config files covering where each started
//! let go (see [`atuin_octavo::shares`]).
//!
//! Where a thread started is the first directory its rows in the sidecar name. Rows as captured
//! fill in what the sidecar doesn't hold yet, as it holds each earlier row of their threads. Rows
//! read back from the record store can't, as the sidecar may not hold their threads' first rows
//! yet, while it recovers or before it projects what sync downloaded, and their threads wait.
//! Without a sidecar they wait for one.
//!
//! An answer is remembered with the start it was for, so a start the sidecar moves earlier, as it
//! recovers, is looked up again.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use atuin_client::ai_session::{AiSessionDatabase, HarnessSession, Message};
use atuin_client::lua_config::Thread;
use atuin_domain::record::RecordId;
use atuin_octavo::pb;
use atuin_octavo::shares::Configs;
use jiff::Timestamp;
use parking_lot::Mutex;
use tokio::time::Instant as Deadline;

/// How long what the config files covering a thread's start say of it is remembered. Live capture
/// queues a thread's messages as they come, under the locks that save them, and a lookup reads the
/// disk and runs the config files, so it runs about once a minute a thread. A config added or
/// changed counts once the answer is out of date.
const REMEMBERED_FOR: Duration = Duration::from_mins(1);

/// How many threads' answers are kept before those out of date go.
const PRUNE_AT: usize = 1024;

/// Where threads started, as `configs` say of each, with the answers of late.
#[derive(Clone, Debug)]
pub(super) struct Starts {
    configs: Configs,
    /// Where a thread started, as stored. Without it, its messages say.
    sidecar: Option<AiSessionDatabase>,
    remembered_for: Duration,
    started: Arc<Mutex<HashMap<HarnessSession, Remembered>>>,
}

impl Starts {
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

    /// The configs that say who threads are shared with, and what goes up.
    pub(super) const fn configs(&self) -> &Configs {
        &self.configs
    }

    /// Those of `messages`, as captured, whose threads go up.
    pub(super) async fn uploads(&self, messages: Vec<Message>) -> Vec<Message> {
        let goes = self.sort(Rows::Captured, &messages).await;
        let uploads = |message: &Message| goes.get(&message.session) == Some(&Goes::Up);
        messages.into_iter().filter(uploads).collect()
    }

    /// Those of `messages`, as stored, whose threads go up, and the ids of those that wait: those
    /// whose threads' starts the sidecar can't say yet, and those not looked up by `until`.
    pub(super) async fn uploads_stored(&self, messages: Vec<Message>, until: Deadline) -> Sorted {
        let goes = self.sort(Rows::Stored { until }, &messages).await;
        let mut sorted = Sorted::default();
        for message in messages {
            match goes.get(&message.session) {
                Some(Goes::Up) => sorted.going.push(message),
                Some(Goes::Later) => sorted.waiting.push(message.id),
                Some(Goes::Overdue) => {
                    sorted.waiting.push(message.id);
                    sorted.overdue = true;
                }
                Some(Goes::Here) | None => {}
            }
        }
        sorted
    }

    /// What goes of `thread`'s queued share, as its stored messages would, looked up by `until`.
    /// One of a harness this build doesn't know stays queued, as its config files can't be asked.
    pub(super) async fn shares(&self, thread: &pb::HarnessSession, until: Deadline) -> Goes {
        match thread.thread() {
            Some(session) => self.goes(Rows::Stored { until }, &session, &[]).await,
            None => Goes::Here,
        }
    }

    /// What goes of each thread of `messages`, which come from `rows`.
    async fn sort(&self, rows: Rows, messages: &[Message]) -> HashMap<HarnessSession, Goes> {
        let mut goes = HashMap::new();
        for message in messages {
            if goes.contains_key(&message.session) {
                continue;
            }
            let thread = self.goes(rows, &message.session, messages).await;
            goes.insert(message.session.clone(), thread);
        }
        goes
    }

    /// What goes of `session`'s messages, as the config files covering where it started say.
    /// They wait while where it started can't be told, and, for stored rows past their deadline,
    /// while what the files say can't. A thread whose rows name nowhere is up to the user's own
    /// alone, and isn't remembered, as its next rows may name somewhere. One with a config file
    /// that can't be run stays here.
    async fn goes(&self, rows: Rows, session: &HarnessSession, messages: &[Message]) -> Goes {
        let dir = match self.start(rows, session, messages).await {
            Start::At(dir) => Some(dir),
            Start::Nowhere => None,
            Start::Unknown => return Goes::Later,
        };
        if let Some(dir) = &dir
            && let Some(uploads) = self.remembered(session, dir)
        {
            return Goes::of(uploads);
        }
        // A lookup can take seconds on a hung network mount, and the rest of the pass waits.
        if let Rows::Stored { until } = rows
            && Deadline::now() >= until
        {
            return Goes::Overdue;
        }
        let uploads = self.configs.uploads_thread(Thread::new(session, dir.clone())).await;
        let uploads = uploads.unwrap_or_else(|err| {
            tracing::warn!(
                ?err,
                "keeping a thread here, as a config file covering it can't be run"
            );
            false
        });
        if let Some(dir) = dir {
            self.remember(session, dir, uploads);
        }
        Goes::of(uploads)
    }

    /// Where `session` started: the first directory its rows in the sidecar name, or that the
    /// first of `messages` to name one does, as `rows` allow.
    async fn start(&self, rows: Rows, session: &HarnessSession, messages: &[Message]) -> Start {
        let named = first_named(session, messages);
        let Some(sidecar) = &self.sidecar else {
            return match rows {
                Rows::Captured => Start::of(named),
                Rows::Stored { .. } => Start::Unknown,
            };
        };
        let stored = match sidecar.start(session).await {
            Ok(stored) => stored,
            Err(err) => {
                tracing::warn!(?err, "failed to read where a thread started");
                return match rows {
                    Rows::Captured => Start::of(named),
                    Rows::Stored { .. } => Start::Unknown,
                };
            }
        };
        match (rows, stored, named) {
            // The sidecar holds each earlier row, so the earlier of the two is the first.
            (Rows::Captured, Some(stored), Some(named)) => Start::At(earlier(stored, named).0),
            (Rows::Captured, stored, named) => Start::of(stored.or(named)),
            // Before the first the sidecar holds, so it's behind, and an earlier one may be missing
            // from both.
            (Rows::Stored { .. }, Some((_, stored_at)), Some((_, named_at)))
                if named_at.as_millisecond() < stored_at.as_millisecond() =>
            {
                Start::Unknown
            }
            (Rows::Stored { .. }, Some((dir, _)), _) => Start::At(dir),
            (Rows::Stored { .. }, None, Some(_)) => Start::Unknown,
            (Rows::Stored { .. }, None, None) => match sidecar.get_session(session).await {
                Ok(Some(_)) => Start::Nowhere,
                Ok(None) => Start::Unknown,
                Err(err) => {
                    tracing::warn!(?err, "failed to read whether a thread is stored");
                    Start::Unknown
                }
            },
        }
    }

    /// The answer remembered for `session` starting in `start`, while it's fresh.
    fn remembered(&self, session: &HarnessSession, start: &Path) -> Option<bool> {
        let fresh = |remembered: &&Remembered| {
            remembered.start == start && remembered.at.elapsed() < self.remembered_for
        };
        self.started.lock().get(session).filter(fresh).map(|remembered| remembered.uploads)
    }

    fn remember(&self, session: &HarnessSession, start: PathBuf, uploads: bool) {
        let mut started = self.started.lock();
        if started.len() >= PRUNE_AT {
            started.retain(|_, remembered| remembered.at.elapsed() < self.remembered_for);
        }
        let remembered = Remembered {
            start,
            uploads,
            at: Instant::now(),
        };
        started.insert(session.clone(), remembered);
    }
}

/// What the config files said of a thread that started in `start`, and when.
#[derive(Debug)]
struct Remembered {
    start: PathBuf,
    uploads: bool,
    at: Instant,
}

/// Stored messages, by whether their threads go up now.
#[derive(Debug, Default)]
pub(super) struct Sorted {
    pub(super) going: Vec<Message>,
    /// The ids of those that wait, for the sidecar or for a lookup.
    pub(super) waiting: Vec<RecordId>,
    /// Whether a thread wasn't looked up by the deadline, which a pass soon after would look up.
    pub(super) overdue: bool,
}

/// Where the messages a thread's start is looked up with come from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Rows {
    /// As captured, under the locks that store them, with each earlier row of their threads in
    /// the sidecar.
    Captured,
    /// Read back from the record store, as queued or backfilled, to be looked up by `until`.
    Stored {
        until: Deadline,
    },
}

/// What goes of a thread's messages.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Goes {
    Up,
    Here,
    /// They wait for the sidecar to say where the thread started.
    Later,
    /// They wait for a lookup the deadline cut off.
    Overdue,
}

impl Goes {
    const fn of(uploads: bool) -> Self {
        if uploads {
            Self::Up
        } else {
            Self::Here
        }
    }
}

/// Where a thread started, as far as can be told.
#[derive(Debug)]
enum Start {
    At(PathBuf),
    /// Its rows name no directory.
    Nowhere,
    /// The sidecar doesn't hold the rows that say yet.
    Unknown,
}

impl Start {
    fn of(named: Option<(PathBuf, Timestamp)>) -> Self {
        named.map_or(Self::Nowhere, |(dir, _)| Self::At(dir))
    }
}

/// The first directory that those of `messages` of `session` name, and when, by time.
fn first_named(session: &HarnessSession, messages: &[Message]) -> Option<(PathBuf, Timestamp)> {
    let named = messages.iter().filter(|message| message.session == *session);
    let first = named.filter_map(|message| Some((message.cwd.as_ref()?, message.timestamp)));
    first.min_by_key(|&(_, at)| at).map(|(dir, at)| (dir.clone(), at))
}

/// The earlier of two directories named at their times, `a` on a tie.
fn earlier(a: (PathBuf, Timestamp), b: (PathBuf, Timestamp)) -> (PathBuf, Timestamp) {
    if b.1 < a.1 {
        b
    } else {
        a
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use atuin_client::ai_session::{HarnessKind, NativeSessionId, SourceId};
    use atuin_common::harnesstools::session::Role;
    use atuin_common::utils::uuid_v7;
    use rstest::{fixture, rstest};
    use tempfile::TempDir;

    use super::*;

    /// A thread's row, as `(thread, seconds in, directory under the root)`.
    type Row<'a> = (&'a str, i64, Option<&'a str>);

    /// The user's own config, as each of its `upload.threads` turns uploading on or not.
    const ON: &str = "atuin.config = { upload = { threads = true } }";
    const WORKSPACES: &str =
        "atuin.config = { upload = { threads = function(t) return t.workspace ~= nil end } }";
    const OFF: &str = "atuin.config = { upload = { threads = false } }";

    /// A root whose `project` has a config file, whose `kept` has one that keeps its threads
    /// here, and whose `elsewhere` has none.
    #[fixture]
    fn root() -> TempDir {
        let root = tempfile::tempdir().expect("a temp dir");
        write(&root.path().join("project"), "atuin.config = {}");
        write(&root.path().join("kept"), OFF);
        root
    }

    /// The configs of `root`, with the user's own config `users` in its `home`.
    fn configs(root: &Path, users: &str) -> Configs {
        let home = root.join("home");
        write(&home, users);
        Configs::new(Some(home))
    }

    fn write(dir: &Path, body: &str) {
        fs::create_dir_all(dir).expect("mkdir");
        fs::write(dir.join(".atuin.lua"), format!("-- atuin:version=1\n{body}")).expect("write");
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

    /// The names of `messages`' threads, once each.
    fn names(messages: &[Message]) -> Vec<String> {
        let name = |message: &Message| AsRef::<str>::as_ref(&message.session.session).to_owned();
        let mut names: Vec<_> = messages.iter().map(name).collect();
        names.dedup();
        names
    }

    /// The threads of `messages` that go up.
    async fn uploaded(starts: &Starts, messages: Vec<Message>) -> Vec<String> {
        names(&starts.uploads(messages).await)
    }

    /// A deadline a lookup never reaches.
    fn unhurried() -> Deadline {
        Deadline::now() + Duration::from_mins(1)
    }

    /// The threads of stored `messages` that go up, how many of them wait, and whether a lookup
    /// was cut off, looked up by `until`.
    async fn sorted(
        starts: &Starts,
        messages: Vec<Message>,
        until: Deadline,
    ) -> (Vec<String>, usize, bool) {
        let sorted = starts.uploads_stored(messages, until).await;
        (names(&sorted.going), sorted.waiting.len(), sorted.overdue)
    }

    async fn sidecar(root: &Path, rows: &[Row<'_>]) -> AiSessionDatabase {
        let sidecar = AiSessionDatabase::in_memory().await.expect("a sidecar");
        for row in messages(root, rows) {
            sidecar.append(&row).await.expect("an append");
        }
        sidecar
    }

    /// Rows read back from the record store may not hold where their thread started, as one
    /// page of a backfill may not, so their thread goes by the sidecar's rows, or waits for them:
    /// while the sidecar recovers, a later directory may be all the page names.
    #[rstest]
    #[case::not_held(&[], &[("a", 2, Some("kept"))], &[], 1)]
    #[case::held(&[("a", 1, Some("kept"))], &[("a", 2, Some("project"))], &[], 0)]
    #[case::held_from_a_project(&[("a", 1, Some("project"))], &[("a", 2, Some("kept"))], &["a"], 0)]
    #[case::held_from_a_later_row(&[("a", 5, Some("elsewhere"))], &[("a", 1, Some("kept"))], &[], 1)]
    #[case::held_without_its_directory(&[("a", 1, None)], &[("a", 2, Some("kept"))], &[], 1)]
    #[case::held_naming_nowhere(&[("a", 1, None)], &[("a", 2, None)], &["a"], 0)]
    #[case::not_held_naming_nowhere(&[], &[("a", 2, None)], &[], 1)]
    #[tokio::test]
    async fn a_stored_thread_goes_by_the_sidecar_or_waits(
        root: TempDir,
        #[case] held: &[Row<'_>],
        #[case] read: &[Row<'_>],
        #[case] going: &[&str],
        #[case] waiting: usize,
    ) {
        let sidecar = AiSessionDatabase::in_memory().await.expect("a sidecar");
        for row in messages(root.path(), held) {
            sidecar.append(&row).await.expect("an append");
        }
        let starts = Starts::new(configs(root.path(), ON), Some(sidecar));

        let sorted = sorted(&starts, messages(root.path(), read), unhurried()).await;

        // Waiting on the sidecar is no reason for a pass soon, which would only find it waiting.
        let going = going.iter().map(ToString::to_string).collect();
        assert_eq!(sorted, (going, waiting, false));
    }

    #[rstest]
    #[case::started_in_one(&[], &[("a", 1, Some("project/src")), ("a", 2, None)], &["a"])]
    #[case::started_elsewhere(&[], &[("a", 1, Some("elsewhere"))], &[])]
    #[case::wandered_out_of_one(&[], &[("a", 1, Some("project")), ("a", 2, Some("elsewhere"))], &["a"])]
    #[case::wandered_into_one(&[], &[("a", 1, Some("elsewhere")), ("a", 2, Some("project"))], &[])]
    #[case::says_nowhere(&[], &[("a", 1, None)], &[])]
    #[case::as_stored(&[("a", 1, Some("project"))], &[("a", 2, None)], &["a"])]
    #[case::stored_elsewhere(&[("a", 1, Some("elsewhere"))], &[("a", 2, Some("project"))], &[])]
    #[case::captured_before_its_stored_rows(&[("a", 5, Some("elsewhere"))], &[("a", 1, Some("project"))], &["a"])]
    #[case::each_by_its_own(
        &[],
        &[("a", 1, Some("project")), ("b", 1, Some("elsewhere")), ("a", 2, None)],
        &["a"],
    )]
    #[tokio::test]
    async fn a_thread_goes_by_the_workspace_it_started_in(
        root: TempDir,
        #[case] stored: &[Row<'_>],
        #[case] captured: &[Row<'_>],
        #[case] expected: &[&str],
    ) {
        let sidecar = AiSessionDatabase::in_memory().await.expect("a sidecar");
        for row in messages(root.path(), stored) {
            sidecar.append(&row).await.expect("an append");
        }
        let starts = Starts::new(configs(root.path(), WORKSPACES), Some(sidecar));
        let captured = messages(root.path(), captured);
        assert_eq!(uploaded(&starts, captured).await, expected);
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
        let starts = Starts::new(configs(root.path(), WORKSPACES), None);
        let queued = messages(root.path(), queued);
        assert_eq!(uploaded(&starts, queued).await, expected);
    }

    /// The user's own config says which threads may go, and of those, the projects' config files
    /// covering where each started say which do.
    #[rstest]
    #[case::on(ON, &["a", "b"])]
    #[case::workspaces(WORKSPACES, &["a"])]
    #[case::off(OFF, &[])]
    #[case::unset("atuin.config = {}", &[])]
    #[tokio::test]
    async fn a_thread_goes_as_the_users_own_and_its_config_files_say(
        root: TempDir,
        #[case] users: &str,
        #[case] expected: &[&str],
    ) {
        let starts = Starts::new(configs(root.path(), users), None);
        let rows = messages(root.path(), &[
            ("a", 1, Some("project")),
            ("b", 1, Some("elsewhere")),
            ("c", 1, Some("kept/src")),
        ]);
        assert_eq!(uploaded(&starts, rows).await, expected);
    }

    /// A thread whose start nothing says yet is up to the user's own config file, which sees no
    /// directory, until its next messages say.
    #[rstest]
    #[tokio::test]
    async fn an_unknown_start_is_up_to_the_users_own(root: TempDir) {
        let users = "atuin.config = { upload = { threads = function(t) return t.dir ~= nil end } }";
        let starts = Starts::new(configs(root.path(), users), None);

        let first = messages(root.path(), &[("a", 1, None)]);
        assert!(uploaded(&starts, first).await.is_empty());
        let next = messages(root.path(), &[("a", 2, Some("elsewhere"))]);
        assert_eq!(uploaded(&starts, next).await, ["a"]);
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
        let starts = Starts::remembering(configs(root.path(), WORKSPACES), None, remembered_for);
        let first = messages(root.path(), &[("a", 1, Some("project"))]);
        assert_eq!(uploaded(&starts, first).await, ["a"]);

        fs::remove_file(root.path().join("project/.atuin.lua")).expect("rm");
        let next = messages(root.path(), &[("a", 2, Some("project"))]);
        assert_eq!(uploaded(&starts, next).await, expected);
    }

    /// A queued share goes as its thread's messages would, by where the sidecar says it started.
    #[rstest]
    #[case::on_in_one(ON, "a", true)]
    #[case::on_elsewhere(ON, "b", true)]
    #[case::on_kept(ON, "k", false)]
    #[case::on_not_stored(ON, "c", false)]
    #[case::from_a_workspace(WORKSPACES, "a", true)]
    #[case::from_elsewhere(WORKSPACES, "b", false)]
    #[case::off(OFF, "a", false)]
    #[tokio::test]
    async fn a_share_goes_as_its_thread_would(
        root: TempDir,
        #[case] users: &str,
        #[case] name: &str,
        #[case] expected: bool,
    ) {
        let sidecar = AiSessionDatabase::in_memory().await.expect("a sidecar");
        let rows = [("a", 1, Some("project")), ("b", 1, Some("elsewhere")), ("k", 1, Some("kept"))];
        for row in messages(root.path(), &rows) {
            sidecar.append(&row).await.expect("an append");
        }
        let starts = Starts::new(configs(root.path(), users), Some(sidecar));
        let thread = pb::HarnessSession::of_thread(thread(name)).expect("a thread Octavo names");
        assert_eq!(starts.shares(&thread, unhurried()).await == Goes::Up, expected);
    }

    /// A thread whose start nothing says yet isn't remembered as kept here, so its next messages
    /// can say.
    #[rstest]
    #[tokio::test]
    async fn an_unknown_start_is_not_remembered(root: TempDir) {
        let starts = Starts::new(configs(root.path(), WORKSPACES), None);
        let first = messages(root.path(), &[("a", 1, None)]);
        assert!(uploaded(&starts, first).await.is_empty());
        let next = messages(root.path(), &[("a", 2, Some("project"))]);
        assert_eq!(uploaded(&starts, next).await, ["a"]);
    }

    /// Without a sidecar, a page of stored rows is all there is to say where their threads
    /// started, and it may not hold their first rows, so they wait.
    #[rstest]
    #[tokio::test]
    async fn stored_rows_wait_without_a_sidecar(root: TempDir) {
        let starts = Starts::new(configs(root.path(), ON), None);

        let rows = messages(root.path(), &[("a", 2, Some("elsewhere"))]);

        assert_eq!(sorted(&starts, rows, unhurried()).await, (vec![], 1, false));
    }

    /// A lookup can take seconds on a hung network mount, so past the deadline only the threads
    /// with an answer remembered go, and the rest wait.
    #[rstest]
    #[tokio::test]
    async fn past_the_deadline_only_remembered_threads_go(root: TempDir) {
        let held = [("a", 1, Some("project")), ("b", 1, Some("elsewhere"))];
        let starts = Starts::new(configs(root.path(), ON), Some(sidecar(root.path(), &held).await));
        let first = messages(root.path(), &[("a", 2, None)]);
        assert_eq!(sorted(&starts, first, unhurried()).await, (vec!["a".to_owned()], 0, false));

        let next = messages(root.path(), &[("a", 3, None), ("b", 2, None)]);

        assert_eq!(sorted(&starts, next, Deadline::now()).await, (vec!["a".to_owned()], 1, true));
    }

    /// As the sidecar recovers, it may take in a thread's earlier rows after a later page was let
    /// go, so the answer for the start it held then doesn't hold for the one it holds now.
    #[rstest]
    #[tokio::test]
    async fn a_start_the_sidecar_moves_is_looked_up_again(root: TempDir) {
        let sidecar = sidecar(root.path(), &[("a", 5, Some("elsewhere"))]).await;
        let starts = Starts::new(configs(root.path(), ON), Some(sidecar.clone()));
        let first = messages(root.path(), &[("a", 6, Some("elsewhere"))]);
        assert_eq!(sorted(&starts, first, unhurried()).await, (vec!["a".to_owned()], 0, false));

        for row in messages(root.path(), &[("a", 1, Some("kept"))]) {
            sidecar.append(&row).await.expect("an append");
        }
        let next = messages(root.path(), &[("a", 7, Some("elsewhere"))]);

        assert_eq!(sorted(&starts, next, unhurried()).await, (vec![], 0, false));
    }
}
