//! The organizations a user's config had their threads shared with, how far each got, and the
//! directories each thread's rows went up from, which keep it to those they all share with.

use std::path::{Path, PathBuf};
use std::time::Duration;

use atuin_client::lua_config::OrgSlug;
use atuin_common::db;

use super::{PAGE_SIZE, QueueError, UploadQueue};
use crate::hub::{HubCallError, HubLogin, UserId};
use crate::pb;

/// How far one of a thread's organizations got, as `thread_org_shares.state` stores it.
#[derive(Debug, Clone, Copy)]
enum ShareState {
    /// Queued to be opened.
    Opening = 0,
    Open = 1,
    /// Queued to be closed, as the thread went where the organization's members mustn't follow.
    Closing = 2,
    /// Never opened again.
    Closed = 3,
}

impl UploadQueue {
    /// `user`'s `thread`, as the queue knows it.
    #[must_use]
    pub const fn thread<'q>(
        &'q self,
        user: &'q UserId,
        thread: &'q pb::HarnessSession,
    ) -> QueuedThread<'q> {
        QueuedThread {
            queue: self,
            user,
            thread,
        }
    }

    /// The first page of `login`'s threads queued to be opened to organizations, the longest
    /// queued first.
    pub async fn pending_opens<'a>(
        &'a self,
        login: &'a HubLogin,
    ) -> Result<Vec<PendingOpen<'a>>, QueueError> {
        let pending = self.pending_in(login, ShareState::Opening).await?;
        Ok(pending.into_iter().map(PendingOpen).collect())
    }

    /// The first page of `login`'s threads queued to be closed to organizations, the longest
    /// queued first.
    pub async fn pending_closes<'a>(
        &'a self,
        login: &'a HubLogin,
    ) -> Result<Vec<PendingClose<'a>>, QueueError> {
        let pending = self.pending_in(login, ShareState::Closing).await?;
        Ok(pending.into_iter().map(PendingClose).collect())
    }

    /// Each of `user`'s threads queued to be opened to an organization.
    pub async fn opening_threads(
        &self,
        user: &UserId,
    ) -> Result<Vec<pb::HarnessSession>, QueueError> {
        let rows: Vec<(String, String)> = db::query_as(
            "SELECT DISTINCT harness, session_id FROM thread_org_shares WHERE user_id = ?1 AND \
             state = ?2",
        )
        .bind(user.as_str())
        .bind(ShareState::Opening as i64)
        .fetch_all(self.db.pool())
        .await?;
        Ok(rows
            .into_iter()
            .map(|(harness, session_id)| pb::HarnessSession {
                harness,
                session_id,
            })
            .collect())
    }

    /// Queues the closing of each of `user`'s threads to each organization it's open to that one
    /// of its directories doesn't share with, returning how many. Those it's queued to be opened
    /// to wait for [`Self::narrow_shares`], once their directories are looked up again.
    pub async fn narrow_open_shares(&self, user: &UserId) -> Result<u64, QueueError> {
        self.narrow(user, &[ShareState::Open]).await
    }

    /// [`Self::narrow_open_shares`], and those queued to be opened too, which it cancels.
    pub async fn narrow_shares(&self, user: &UserId) -> Result<u64, QueueError> {
        self.narrow(user, &[ShareState::Opening, ShareState::Open]).await
    }

    /// Notes that `user` belongs to no organization with the slug `org_slug`, returning whether
    /// that's news.
    pub async fn note_skipped_org(
        &self,
        user: &UserId,
        org_slug: &str,
    ) -> Result<bool, QueueError> {
        let noted = db::query(
            "INSERT OR IGNORE INTO skipped_share_orgs (user_id, org_slug) VALUES (?1, ?2)",
        )
        .bind(user.as_str())
        .bind(org_slug)
        .execute(self.db.pool())
        .await?
        .rows_affected();
        Ok(noted == 1)
    }

    async fn pending_in<'a>(
        &'a self,
        login: &'a HubLogin,
        state: ShareState,
    ) -> Result<Vec<Pending<'a>>, QueueError> {
        let rows: Vec<(String, String, String)> = db::query_as(
            "SELECT harness, session_id, org_slug FROM thread_org_shares WHERE user_id = ?1 AND \
             state = ?2 ORDER BY queued_at, harness, session_id LIMIT ?3",
        )
        .bind(login.user_id().as_str())
        .bind(state as i64)
        .bind(PAGE_SIZE.get())
        .fetch_all(self.db.pool())
        .await?;

        let mut pending: Vec<Pending<'a>> = Vec::new();
        for (harness, session_id, slug) in rows {
            // Written as a slug.
            let Ok(org) = slug.parse() else {
                continue;
            };
            match pending.last_mut() {
                Some(last)
                    if last.thread.harness == harness && last.thread.session_id == session_id =>
                {
                    last.orgs.push(org);
                }
                _ => pending.push(Pending {
                    queue: self,
                    login,
                    thread: pb::HarnessSession {
                        harness,
                        session_id,
                    },
                    orgs: vec![org],
                }),
            }
        }
        Ok(pending)
    }

    async fn narrow(&self, user: &UserId, states: &[ShareState]) -> Result<u64, QueueError> {
        let states: Vec<i64> = states.iter().map(|&state| state as i64).collect();
        // `NOT IN` compares by the slug column's collation, so whatever the case.
        let narrowed = db::query(
            "UPDATE thread_org_shares AS s SET state = ?2, queued_at = unixepoch() WHERE \
             s.user_id = ?1 AND s.state IN (SELECT value FROM json_each(?3)) AND EXISTS (SELECT 1 \
             FROM thread_dirs d WHERE d.user_id = s.user_id AND d.harness = s.harness AND \
             d.session_id = s.session_id AND s.org_slug NOT IN (SELECT value FROM \
             json_each(d.org_slugs)))",
        )
        .bind(user.as_str())
        .bind(ShareState::Closing as i64)
        .bind(serde_json::to_string(&states).expect("a list of numbers serializes"))
        .execute(self.db.pool())
        .await?
        .rows_affected();
        Ok(narrowed)
    }
}

/// One of a user's threads, as the queue knows it.
#[derive(Debug, Clone, Copy)]
pub struct QueuedThread<'q> {
    queue: &'q UploadQueue,
    user: &'q UserId,
    thread: &'q pb::HarnessSession,
}

impl QueuedThread<'_> {
    /// Queues the thread to be opened to each of `orgs` it never was queued for, returning how
    /// many that was.
    pub async fn push_shares(&self, orgs: &[OrgSlug]) -> Result<u64, QueueError> {
        let mut tx = self.queue.db.pool().begin().await?;
        let mut queued = 0;
        for org in orgs {
            queued += db::query(
                "INSERT OR IGNORE INTO thread_org_shares (user_id, harness, session_id, org_slug) \
                 VALUES (?1, ?2, ?3, ?4)",
            )
            .bind(self.user.as_str())
            .bind(&self.thread.harness)
            .bind(&self.thread.session_id)
            .bind(org.as_ref())
            .execute(&mut *tx)
            .await?
            .rows_affected();
        }
        tx.commit().await?;
        Ok(queued)
    }

    /// Whether it's queued to be closed to an organization.
    pub async fn is_closing(&self) -> Result<bool, QueueError> {
        let closing: Option<i64> = db::query_scalar(
            "SELECT 1 FROM thread_org_shares WHERE user_id = ?1 AND harness = ?2 AND session_id = \
             ?3 AND state = ?4 LIMIT 1",
        )
        .bind(self.user.as_str())
        .bind(&self.thread.harness)
        .bind(&self.thread.session_id)
        .bind(ShareState::Closing as i64)
        .fetch_optional(self.queue.db.pool())
        .await?;
        Ok(closing.is_some())
    }

    /// Whether it had a row uploaded from `dir` already.
    pub async fn knows_dir(&self, dir: &Path) -> Result<bool, QueueError> {
        let known: Option<i64> = db::query_scalar(
            "SELECT 1 FROM thread_dirs WHERE user_id = ?1 AND harness = ?2 AND session_id = ?3 \
             AND dir = ?4",
        )
        .bind(self.user.as_str())
        .bind(&self.thread.harness)
        .bind(&self.thread.session_id)
        .bind(dir.to_string_lossy())
        .fetch_optional(self.queue.db.pool())
        .await?;
        Ok(known.is_some())
    }

    /// Notes that it had a row uploaded from `dir`, whose config shares threads with `orgs`,
    /// unless it had one already: the first look stands.
    pub async fn record_dir(&self, dir: &Path, orgs: &[OrgSlug]) -> Result<(), QueueError> {
        db::query(
            "INSERT OR IGNORE INTO thread_dirs (user_id, harness, session_id, dir, org_slugs) \
             VALUES (?1, ?2, ?3, ?4, ?5)",
        )
        .bind(self.user.as_str())
        .bind(&self.thread.harness)
        .bind(&self.thread.session_id)
        .bind(dir.to_string_lossy())
        .bind(json(orgs))
        .execute(self.queue.db.pool())
        .await?;
        Ok(())
    }

    /// The directories it had rows uploaded from.
    pub async fn dirs(&self) -> Result<Vec<PathBuf>, QueueError> {
        let dirs: Vec<String> = db::query_scalar(
            "SELECT dir FROM thread_dirs WHERE user_id = ?1 AND harness = ?2 AND session_id = ?3",
        )
        .bind(self.user.as_str())
        .bind(&self.thread.harness)
        .bind(&self.thread.session_id)
        .fetch_all(self.queue.db.pool())
        .await?;
        Ok(dirs.into_iter().map(PathBuf::from).collect())
    }

    /// Replaces the organizations `dir`'s config shares it with by `orgs`, what it says now: a
    /// config may name an organization after the thread's rows went up, or drop one.
    pub async fn refresh_dir(&self, dir: &Path, orgs: &[OrgSlug]) -> Result<(), QueueError> {
        db::query(
            "UPDATE thread_dirs SET org_slugs = ?5 WHERE user_id = ?1 AND harness = ?2 AND \
             session_id = ?3 AND dir = ?4",
        )
        .bind(self.user.as_str())
        .bind(&self.thread.harness)
        .bind(&self.thread.session_id)
        .bind(dir.to_string_lossy())
        .bind(json(orgs))
        .execute(self.queue.db.pool())
        .await?;
        Ok(())
    }

    async fn settle(
        &self,
        orgs: &[OrgSlug],
        from: ShareState,
        to: ShareState,
    ) -> Result<(), QueueError> {
        let mut tx = self.queue.db.pool().begin().await?;
        for org in orgs {
            db::query(
                "UPDATE thread_org_shares SET state = ?5 WHERE user_id = ?1 AND harness = ?2 AND \
                 session_id = ?3 AND org_slug = ?4 AND state = ?6",
            )
            .bind(self.user.as_str())
            .bind(&self.thread.harness)
            .bind(&self.thread.session_id)
            .bind(org.as_ref())
            .bind(to as i64)
            .bind(from as i64)
            .execute(&mut *tx)
            .await?;
        }
        tx.commit().await?;
        Ok(())
    }

    async fn expire_opens(&self, after: Duration) -> Result<u64, QueueError> {
        let after = i64::try_from(after.as_secs()).unwrap_or(i64::MAX);
        let expired = db::query(
            "DELETE FROM thread_org_shares WHERE user_id = ?1 AND harness = ?2 AND session_id = \
             ?3 AND state = ?5 AND queued_at <= unixepoch() - ?4",
        )
        .bind(self.user.as_str())
        .bind(&self.thread.harness)
        .bind(&self.thread.session_id)
        .bind(after)
        .bind(ShareState::Opening as i64)
        .execute(self.queue.db.pool())
        .await?
        .rows_affected();
        Ok(expired)
    }
}

fn json(orgs: &[OrgSlug]) -> String {
    let orgs: Vec<&str> = orgs.iter().map(AsRef::as_ref).collect();
    serde_json::to_string(&orgs).expect("a list of strings serializes")
}

/// A thread queued to be opened or closed to organizations, for the login it was read for.
#[derive(Debug)]
struct Pending<'a> {
    queue: &'a UploadQueue,
    login: &'a HubLogin,
    thread: pb::HarnessSession,
    orgs: Vec<OrgSlug>,
}

impl Pending<'_> {
    fn queued(&self) -> QueuedThread<'_> {
        self.queue.thread(self.login.user_id(), &self.thread)
    }
}

/// A thread queued to be opened to organizations.
#[derive(Debug)]
pub struct PendingOpen<'a>(Pending<'a>);

impl<'a> PendingOpen<'a> {
    #[must_use]
    pub fn thread(&self) -> &pb::HarnessSession {
        &self.0.thread
    }

    #[must_use]
    pub fn orgs(&self) -> &[OrgSlug] {
        &self.0.orgs
    }

    /// Opens the thread to its organizations.
    pub async fn send(self) -> Result<SentOpen<'a>, Box<NotSent<Self>>> {
        let Pending {
            login,
            thread,
            orgs,
            ..
        } = &self.0;
        match login.share_thread_with_orgs(thread, orgs).await {
            Ok(skipped) => Ok(SentOpen {
                open: self,
                skipped,
            }),
            Err(error) => Err(Box::new(NotSent {
                pending: self,
                error,
            })),
        }
    }

    /// Settles it unsent, as Octavo refused it for good, so it's never sent again.
    pub async fn give_up(self) -> Result<(), QueueError> {
        self.settle().await
    }

    /// Removes it if it was queued at least `after` ago, returning how many organizations that
    /// was, so the thread's next message queues it afresh.
    pub async fn expire(self, after: Duration) -> Result<u64, QueueError> {
        self.0.queued().expire_opens(after).await
    }

    async fn settle(&self) -> Result<(), QueueError> {
        self.0.queued().settle(&self.0.orgs, ShareState::Opening, ShareState::Open).await
    }
}

/// A thread Octavo opened to its organizations.
#[derive(Debug)]
pub struct SentOpen<'a> {
    open: PendingOpen<'a>,
    skipped: Vec<String>,
}

impl SentOpen<'_> {
    /// Those the user belongs to no organization with, which Octavo left out.
    #[must_use]
    pub fn skipped(&self) -> &[String] {
        &self.skipped
    }

    /// Settles it, so the thread is never queued to be opened to them again.
    pub async fn settle(self) -> Result<(), QueueError> {
        self.open.settle().await
    }
}

/// A thread queued to be closed to organizations.
#[derive(Debug)]
pub struct PendingClose<'a>(Pending<'a>);

impl<'a> PendingClose<'a> {
    #[must_use]
    pub fn thread(&self) -> &pb::HarnessSession {
        &self.0.thread
    }

    #[must_use]
    pub fn orgs(&self) -> &[OrgSlug] {
        &self.0.orgs
    }

    /// Closes the thread to its organizations: done too for a thread Octavo hasn't stored, as
    /// nothing of it opens to anyone.
    pub async fn send(self) -> Result<SentClose<'a>, Box<NotSent<Self>>> {
        let Pending {
            login,
            thread,
            orgs,
            ..
        } = &self.0;
        match login.close_thread_to_orgs(thread, orgs).await {
            Ok(()) | Err(HubCallError::ThreadNotStored(_)) => Ok(SentClose(self)),
            Err(error) => Err(Box::new(NotSent {
                pending: self,
                error,
            })),
        }
    }
}

/// A thread Octavo closed to its organizations.
#[derive(Debug)]
pub struct SentClose<'a>(PendingClose<'a>);

impl SentClose<'_> {
    /// Settles it, so the thread is never opened to them again.
    pub async fn settle(self) -> Result<(), QueueError> {
        let pending = &self.0.0;
        pending.queued().settle(&pending.orgs, ShareState::Closing, ShareState::Closed).await
    }
}

/// An open or close Octavo didn't take, and why.
#[derive(Debug)]
pub struct NotSent<T> {
    pub pending: T,
    pub error: HubCallError,
}

#[cfg(test)]
mod tests {
    use rstest::{fixture, rstest};
    use tempfile::TempDir;

    use super::*;

    struct TempQueue {
        queue: UploadQueue,
        _dir: TempDir,
    }

    #[fixture]
    async fn queue() -> TempQueue {
        let dir = tempfile::tempdir().expect("a temp dir");
        let queue = UploadQueue::open(dir.path().join("octavo.db")).await.expect("queue opens");
        TempQueue { queue, _dir: dir }
    }

    fn thread(session_id: &str) -> pb::HarnessSession {
        pb::HarnessSession {
            harness: "claude-code".to_owned(),
            session_id: session_id.to_owned(),
        }
    }

    fn slugs(slugs: &[&str]) -> Vec<OrgSlug> {
        slugs.iter().map(|slug| slug.parse().expect("a slug")).collect()
    }

    fn named(orgs: &[OrgSlug]) -> Vec<String> {
        orgs.iter().map(ToString::to_string).collect()
    }

    /// Each thread queued to be opened, by its session id, with its orgs.
    async fn opens(queue: &UploadQueue, login: &HubLogin) -> Vec<(String, Vec<String>)> {
        let opens = queue.pending_opens(login).await.expect("read");
        opens.iter().map(|open| (open.thread().session_id.clone(), named(open.orgs()))).collect()
    }

    /// Each thread queued to be closed, by its session id, with its orgs.
    async fn closes(queue: &UploadQueue, login: &HubLogin) -> Vec<(String, Vec<String>)> {
        let mut closes: Vec<_> = queue
            .pending_closes(login)
            .await
            .expect("read")
            .iter()
            .map(|close| (close.thread().session_id.clone(), named(close.orgs())))
            .collect();
        closes.sort();
        closes
    }

    /// Settles each queued open, as Octavo's answer to it would.
    async fn open_all(queue: &UploadQueue, login: &HubLogin) {
        for open in queue.pending_opens(login).await.expect("read") {
            let sent = SentOpen {
                open,
                skipped: Vec::new(),
            };
            sent.settle().await.expect("settle");
        }
    }

    /// Settles each queued close, as Octavo's answer to it would.
    async fn close_all(queue: &UploadQueue, login: &HubLogin) {
        for close in queue.pending_closes(login).await.expect("read") {
            SentClose(close).settle().await.expect("settle");
        }
    }

    fn pair(session_id: &str, orgs: &[&str]) -> (String, Vec<String>) {
        (session_id.to_owned(), orgs.iter().map(|&org| org.to_owned()).collect())
    }

    #[rstest]
    #[tokio::test]
    async fn pending_opens_group_each_users_own_by_thread(#[future(awt)] queue: TempQueue) {
        let (alice, bob) = (HubLogin::offline("alice"), HubLogin::offline("bob"));
        let queue = queue.queue;
        for (login, session, orgs) in [
            (&alice, "first", ["acme", "globex"]),
            (&alice, "second", ["acme", "ACME"]),
            (&bob, "first", ["initech", "initech"]),
        ] {
            let thread = thread(session);
            let queued = queue.thread(login.user_id(), &thread);
            queued.push_shares(&slugs(&orgs)).await.expect("push");
        }

        assert_eq!(opens(&queue, &alice).await, [
            pair("first", &["acme", "globex"]),
            pair("second", &["acme"]),
        ]);
        assert_eq!(queue.opening_threads(alice.user_id()).await.expect("read").len(), 2);
    }

    /// A settled row is what keeps a thread the user unshared from being shared by its next
    /// message, however the slug is cased, while an organization added to the config still goes.
    #[rstest]
    #[tokio::test]
    async fn a_settled_open_is_never_queued_again(#[future(awt)] queue: TempQueue) {
        let login = HubLogin::offline("alice");
        let queue = queue.queue;
        let session = thread("first");
        let queued = queue.thread(login.user_id(), &session);
        queued.push_shares(&slugs(&["acme"])).await.expect("push");
        open_all(&queue, &login).await;

        let pushed = queued.push_shares(&slugs(&["ACME", "globex"])).await;

        assert_eq!(pushed.expect("push"), 1);
        assert_eq!(opens(&queue, &login).await, [pair("first", &["globex"])]);
        assert_eq!(queue.opening_threads(login.user_id()).await.expect("read"), [session]);
    }

    #[rstest]
    #[case::too_soon(Duration::from_secs(3600), 0)]
    #[case::old_enough(Duration::ZERO, 1)]
    #[tokio::test]
    async fn expiring_an_open_removes_only_one_unsent_that_long(
        #[future(awt)] queue: TempQueue,
        #[case] after: Duration,
        #[case] expired: u64,
    ) {
        let login = HubLogin::offline("alice");
        let queue = queue.queue;
        let session = thread("first");
        let queued = queue.thread(login.user_id(), &session);
        queued.push_shares(&slugs(&["acme"])).await.expect("push");
        open_all(&queue, &login).await;
        queued.push_shares(&slugs(&["globex"])).await.expect("push");

        let open = queue.pending_opens(&login).await.expect("read").pop().expect("an open");

        assert_eq!(open.expire(after).await.expect("expire"), expired);
        // The opened row stays, so only the expired one can be queued afresh.
        let pushed = queued.push_shares(&slugs(&["acme", "globex"])).await.expect("push");
        assert_eq!(pushed, expired);
    }

    /// A thread opens, and stays open, to an org only while each directory it had a row uploaded
    /// from shares with it, whatever the slug's case. One closed to an org is never queued to be
    /// opened to it again.
    #[rstest]
    #[tokio::test]
    async fn a_thread_is_closed_to_an_org_one_of_its_directories_doesnt_share_with(
        #[future(awt)] queue: TempQueue,
    ) {
        let login = HubLogin::offline("alice");
        let user = login.user_id();
        let queue = queue.queue;
        let (opened, opening) = (thread("opened"), thread("opening"));
        for session in [&opened, &opening] {
            let queued = queue.thread(user, session);
            queued.push_shares(&slugs(&["acme", "globex"])).await.expect("push");
            queued
                .record_dir(Path::new("/work"), &slugs(&["ACME", "globex"]))
                .await
                .expect("record");
        }
        open_all(&queue, &login).await;
        queue.thread(user, &opening).push_shares(&slugs(&["acme", "initech"])).await.expect("push");
        assert_eq!(queue.narrow_open_shares(user).await.expect("narrow"), 0);

        for session in [&opened, &opening] {
            let queued = queue.thread(user, session);
            queued.record_dir(Path::new("/private"), &slugs(&["Globex"])).await.expect("record");
        }
        assert_eq!(queue.narrow_open_shares(user).await.expect("narrow"), 2);
        assert_eq!(opens(&queue, &login).await, [pair("opening", &["initech"])]);
        assert_eq!(queue.narrow_shares(user).await.expect("narrow"), 1);

        assert_eq!(opens(&queue, &login).await, []);
        assert_eq!(closes(&queue, &login).await, [
            pair("opened", &["acme"]),
            pair("opening", &["acme", "initech"]),
        ]);
        assert!(queue.thread(user, &opened).is_closing().await.expect("read"));

        close_all(&queue, &login).await;
        for session in [&opened, &opening] {
            let queued = queue.thread(user, session);
            assert!(!queued.is_closing().await.expect("read"));
            assert_eq!(queued.push_shares(&slugs(&["acme"])).await.expect("push"), 0);
        }
        assert_eq!(queue.narrow_shares(user).await.expect("narrow"), 0);
    }

    /// A row's directory keeps the orgs its config named as the row went up, so a share it went
    /// up without stays closed to it, until a look at its config now says otherwise: an org it
    /// names since, or an opt-out.
    #[rstest]
    #[tokio::test]
    async fn a_thread_directory_keeps_its_first_look_until_refreshed(
        #[future(awt)] queue: TempQueue,
    ) {
        let login = HubLogin::offline("alice");
        let user = login.user_id();
        let queue = queue.queue;
        let session = thread("first");
        let queued = queue.thread(user, &session);
        let dir = Path::new("/work");
        assert!(!queued.knows_dir(dir).await.expect("read"));

        queued.record_dir(dir, &[]).await.expect("record");
        queued.record_dir(dir, &slugs(&["acme"])).await.expect("record");
        queued.push_shares(&slugs(&["acme", "globex"])).await.expect("push");

        assert!(queued.knows_dir(dir).await.expect("read"));
        let bob = UserId::new("bob");
        assert!(!queue.thread(&bob, &session).knows_dir(dir).await.expect("read"));
        assert_eq!(queued.dirs().await.expect("read"), [dir]);
        assert_eq!(queue.narrow_open_shares(user).await.expect("narrow"), 0);

        queued.refresh_dir(dir, &slugs(&["acme", "globex"])).await.expect("refresh");
        assert_eq!(queue.narrow_shares(user).await.expect("narrow"), 0);
        assert_eq!(opens(&queue, &login).await, [pair("first", &["acme", "globex"])]);

        queued.refresh_dir(dir, &slugs(&["acme"])).await.expect("refresh");
        assert_eq!(queue.narrow_shares(user).await.expect("narrow"), 1);
        assert_eq!(opens(&queue, &login).await, [pair("first", &["acme"])]);
    }

    #[rstest]
    #[tokio::test]
    async fn a_skipped_org_is_news_once_per_user(#[future(awt)] queue: TempQueue) {
        let (alice, bob) = (UserId::new("alice"), UserId::new("bob"));
        let queue = queue.queue;

        assert!(queue.note_skipped_org(&alice, "acme").await.expect("note"));
        assert!(!queue.note_skipped_org(&alice, "ACME").await.expect("note"));
        assert!(queue.note_skipped_org(&bob, "acme").await.expect("note"));
    }
}
