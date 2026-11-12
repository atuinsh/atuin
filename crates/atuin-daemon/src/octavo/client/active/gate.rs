//! What keeps a thread to the organizations each directory it ran in shares with: a row waits
//! until its directory is known for its thread, and the thread is closed to those that directory
//! rules out, and a share opens only to those every one allows.

use std::collections::HashSet;

use atuin_client::ai_session::Message;
use atuin_domain::record::RecordId;
use atuin_octavo::hub::{HubCallError, HubLogin, UserId};
use atuin_octavo::pb;
use atuin_octavo::queue::{PendingOpen, QueueError, UploadQueue};
use atuin_octavo::shares::{Configs, LookupError};
use tokio::time::Instant;

use super::{clear_off, mark_off};

/// That the queued closes were sent, as far as they'd go: only [`Closes::send`] makes one.
#[derive(Debug)]
pub(super) struct ClosesSent(());

/// A login's queued thread closes.
#[derive(Debug)]
pub(super) struct Closes<'a> {
    queue: &'a UploadQueue,
    login: &'a HubLogin,
}

impl<'a> Closes<'a> {
    pub(super) const fn of(queue: &'a UploadQueue, login: &'a HubLogin) -> Self {
        Self { queue, login }
    }

    /// Sends them until `deadline`, settling each Octavo takes. One that fails waits, and the
    /// rows of its thread with it.
    pub(super) async fn send(self, deadline: Instant) -> Result<ClosesSent, CloseHalt> {
        let user = self.login.user_id();
        for close in self.queue.pending_closes(self.login).await? {
            let sent = match tokio::time::timeout_at(deadline, close.send()).await {
                Ok(Ok(sent)) => sent,
                Ok(Err(refused)) if matches!(refused.error, HubCallError::OctavoOff(_)) => {
                    tracing::debug!(
                        "octavo is off for the account; leaving its thread closes queued"
                    );
                    mark_off(self.queue, user).await;
                    return Err(CloseHalt::Off);
                }
                Ok(Err(refused)) => {
                    let error = &refused.error;
                    tracing::warn!(
                        ?error,
                        "failed to close a thread to organizations; retrying later"
                    );
                    break;
                }
                Err(_elapsed) => break,
            };
            clear_off(self.queue, user).await;
            sent.settle().await?;
        }
        Ok(ClosesSent(()))
    }
}

/// Why closes stopped short of a [`ClosesSent`].
#[derive(Debug)]
pub(super) enum CloseHalt {
    /// Octavo is off for the account.
    Off,
    Queue(QueueError),
}

impl From<QueueError> for CloseHalt {
    fn from(err: QueueError) -> Self {
        Self::Queue(err)
    }
}

/// A row on its way up, with the thread whose share holds it.
#[derive(Debug)]
struct Row {
    message: Message,
    thread: Option<pb::HarnessSession>,
}

/// A page of a user's thread rows to upload.
#[derive(Debug)]
pub(super) struct Rows<'a> {
    queue: &'a UploadQueue,
    user: &'a UserId,
    rows: Vec<Row>,
}

impl<'a> Rows<'a> {
    /// The rows that may go up, and the ids of those of the `waiting` threads.
    fn split(rows: Vec<Row>, waiting: &HashSet<pb::HarnessSession>) -> Cleared {
        let (waiting, ready): (Vec<_>, Vec<_>) = rows
            .into_iter()
            .partition(|row| row.thread.as_ref().is_some_and(|thread| waiting.contains(thread)));
        Cleared {
            ready: ready.into_iter().map(|row| row.message).collect(),
            waiting: waiting.into_iter().map(|row| row.message.id).collect(),
        }
    }

    pub(super) fn new(queue: &'a UploadQueue, user: &'a UserId, messages: Vec<Message>) -> Self {
        let rows = messages
            .into_iter()
            .map(|message| Row {
                thread: pb::HarnessSession::sharing(&message),
                message,
            })
            .collect();
        Self { queue, user, rows }
    }

    /// Records each directory a row came from that its thread had none uploaded from, with the
    /// organizations its config shares threads with. A config that can't be run shares with none,
    /// as it may be what keeps the thread to itself.
    pub(super) async fn record_dirs(self, configs: &Configs) -> Result<Recorded<'a>, QueueError> {
        let mut unsure = HashSet::new();
        let mut seen = HashSet::new();
        for row in &self.rows {
            let (Some(thread), Some(dir)) = (&row.thread, &row.message.cwd) else {
                continue;
            };
            if !seen.insert((thread, dir)) {
                continue;
            }
            let queued = self.queue.thread(self.user, thread);
            if queued.knows_dir(dir).await? {
                continue;
            }
            match configs.orgs_of_dir(dir.clone()).await {
                Ok(orgs) => queued.record_dir(dir, &orgs).await?,
                Err(LookupError::Config(err)) => {
                    tracing::warn!(
                        ?err,
                        "a config file atuin can't run covers a thread; not sharing it"
                    );
                    queued.record_dir(dir, &[]).await?;
                }
                Err(err) => {
                    tracing::warn!(
                        ?err,
                        "failed to look up a thread directory's config; retrying later"
                    );
                    unsure.insert(thread.clone());
                }
            }
        }
        Ok(Recorded { rows: self, unsure })
    }
}

/// Rows whose directories are recorded, but for the `unsure` threads'.
#[derive(Debug)]
pub(super) struct Recorded<'a> {
    rows: Rows<'a>,
    unsure: HashSet<pb::HarnessSession>,
}

impl<'a> Recorded<'a> {
    /// Queues the closing of each thread to the organizations it's open to that its directories
    /// rule out. Those it's queued to be opened to wait for [`Opening`], which looks again first.
    pub(super) async fn narrow(self) -> Result<Closing<'a>, QueueError> {
        self.rows.queue.narrow_open_shares(self.rows.user).await?;
        Ok(Closing(self))
    }
}

/// Rows whose threads' closes are queued, to [`Closing::clear`] once they're sent.
#[derive(Debug)]
pub(super) struct Closing<'a>(Recorded<'a>);

impl Closing<'_> {
    /// Splits the rows into those that may go up and those that wait: an unsure thread's, and
    /// those of a thread still closing.
    pub(super) async fn clear(self, _sent: ClosesSent) -> Result<Cleared, QueueError> {
        let Recorded {
            rows: Rows { queue, user, rows },
            mut unsure,
        } = self.0;
        let mut asked = HashSet::new();
        for thread in rows.iter().filter_map(|row| row.thread.as_ref()) {
            if !unsure.contains(thread)
                && asked.insert(thread)
                && queue.thread(user, thread).is_closing().await?
            {
                unsure.insert(thread.clone());
            }
        }

        Ok(Rows::split(rows, &unsure))
    }
}

/// The rows that may go up now, and the ids of those that wait.
#[derive(Debug)]
pub(super) struct Cleared {
    pub(super) ready: Vec<Message>,
    pub(super) waiting: Vec<RecordId>,
}

/// A login's threads queued to be opened to organizations.
#[derive(Debug)]
pub(super) struct Opening<'a> {
    queue: &'a UploadQueue,
    login: &'a HubLogin,
    threads: Vec<pb::HarnessSession>,
}

impl<'a> Opening<'a> {
    pub(super) async fn of(
        queue: &'a UploadQueue,
        login: &'a HubLogin,
    ) -> Result<Self, QueueError> {
        let threads = queue.opening_threads(login.user_id()).await?;
        Ok(Self {
            queue,
            login,
            threads,
        })
    }

    /// Looks up each of their directories' configs again, as one may have named an organization
    /// since a thread's rows went up, which is what queues it, or dropped one. A thread with a
    /// config that can't be run, or a directory that can't be looked up, waits.
    pub(super) async fn look_again(self, configs: &Configs) -> Result<LookedAgain<'a>, QueueError> {
        let mut unsure = HashSet::new();
        for thread in &self.threads {
            let queued = self.queue.thread(self.login.user_id(), thread);
            for dir in queued.dirs().await? {
                match configs.orgs_of_dir(dir.clone()).await {
                    Ok(orgs) => queued.refresh_dir(&dir, &orgs).await?,
                    Err(err) => {
                        tracing::warn!(
                            ?err,
                            "failed to look up a thread directory's config; not sharing it yet"
                        );
                        unsure.insert(thread.clone());
                    }
                }
            }
        }
        Ok(LookedAgain {
            queue: self.queue,
            login: self.login,
            unsure,
        })
    }
}

/// A login's threads queued to be opened, their directories looked up again but for the `unsure`
/// threads'.
#[derive(Debug)]
pub(super) struct LookedAgain<'a> {
    queue: &'a UploadQueue,
    login: &'a HubLogin,
    unsure: HashSet<pb::HarnessSession>,
}

impl<'a> LookedAgain<'a> {
    /// Queues the closing, or cancels the opening, of each thread to the organizations its
    /// directories rule out.
    pub(super) async fn narrow(self) -> Result<OpensNarrowed<'a>, QueueError> {
        self.queue.narrow_shares(self.login.user_id()).await?;
        Ok(OpensNarrowed(self))
    }
}

/// A login's queued opens, each to organizations every directory of its thread allows.
#[derive(Debug)]
pub(super) struct OpensNarrowed<'a>(LookedAgain<'a>);

impl<'a> OpensNarrowed<'a> {
    /// The opens that may go, once the closes are sent: not an unsure thread's.
    pub(super) async fn pending(
        self,
        _sent: ClosesSent,
    ) -> Result<Vec<PendingOpen<'a>>, QueueError> {
        let LookedAgain {
            queue,
            login,
            unsure,
        } = self.0;
        let opens = queue.pending_opens(login).await?;
        Ok(opens.into_iter().filter(|open| !unsure.contains(open.thread())).collect())
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::Path;

    use atuin_client::ai_session::{HarnessKind, HarnessSession, NativeSessionId, SourceId};
    use atuin_client::lua_config::OrgSlug;
    use atuin_common::harnesstools::session::Role;
    use atuin_common::utils::uuid_v7;
    use rstest::rstest;
    use time::OffsetDateTime;

    use super::*;

    fn session(id: &str) -> HarnessSession {
        HarnessSession {
            harness: HarnessKind::ClaudeCode,
            session: NativeSessionId::from(id.to_owned()),
        }
    }

    fn row(thread: &str, cwd: Option<&Path>) -> Message {
        Message::builder()
            .id(RecordId(uuid_v7()))
            .session(session(thread))
            .source_id(SourceId::from(uuid_v7().to_string()))
            .timestamp(OffsetDateTime::UNIX_EPOCH)
            .role(Role::User)
            .content(Vec::new())
            .cwd(cwd.map(Into::into))
            .build()
    }

    fn write(dir: &Path, orgs: &str) {
        fs::create_dir_all(dir).expect("mkdir");
        let source = format!("-- atuin:version=1\natuin.config = {{ orgs = {orgs} }}");
        fs::write(dir.join("atuin.lua"), source).expect("write");
    }

    /// A row goes up once its directory is recorded for its thread, but not one of a thread
    /// queued to be closed to an org: here, as its private directory rules the org out.
    #[rstest]
    #[tokio::test]
    async fn a_row_waits_while_its_thread_is_closing() {
        let root = tempfile::tempdir().expect("a temp dir");
        let queue = UploadQueue::open(root.path().join("octavo.db")).await.expect("a queue");
        let (work, private) = (root.path().join("work"), root.path().join("private"));
        write(&work, r#"{ "acme" }"#);
        write(&private, "{}");
        let configs = Configs::new(None);
        let user = UserId::new("alice");
        let acme: Vec<OrgSlug> = vec!["acme".parse().expect("a slug")];
        for name in ["moved", "stayed"] {
            let thread = pb::HarnessSession::of_thread(session(name)).expect("a thread");
            queue.thread(&user, &thread).push_shares(&acme).await.expect("push");
        }
        let rows = vec![
            row("moved", Some(&work)),
            row("moved", Some(&private)),
            row("stayed", Some(&work)),
            row("unshared", None),
        ];
        let ids: Vec<_> = rows.iter().map(|row| row.id).collect();

        let recorded = Rows::new(&queue, &user, rows).record_dirs(&configs).await.expect("record");
        let closing = recorded.narrow().await.expect("narrow");
        // As `look_again` would, which queues closing the opens a directory rules out.
        queue.narrow_shares(&user).await.expect("narrow");
        let cleared = closing.clear(ClosesSent(())).await.expect("clear");

        let ready: Vec<_> = cleared.ready.iter().map(|row| row.id).collect();
        assert_eq!(ready, [ids[2], ids[3]]);
        assert_eq!(cleared.waiting, [ids[0], ids[1]]);
    }
}
