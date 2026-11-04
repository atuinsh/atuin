use std::num::NonZeroU32;
use std::path::Path;
use std::time::Duration;

use atuin_client::history::HistoryId;
use atuin_common::db;
use atuin_common::db::sqlite::{Sqlite, SqliteOpenOrCreateError};
use atuin_common::futures::stream::ChunkedStream;
use atuin_domain::record::{HostId, RecordId, RecordIdx};
use futures::stream;
use sqlx::QueryBuilder;
use sqlx::migrate::MigrateError;
use sqlx::sqlite::SqliteRow;
use thiserror::Error;

use crate::hub::UserId;
use crate::pb;

const PAGE_SIZE: NonZeroU32 = NonZeroU32::new(100).unwrap();
const HISTORY_PAGE_SIZE: NonZeroU32 = pb::HistoryBatch::MAX_ITEMS;
/// As many as one `InsertThreadMessages` takes, so a page of small messages is one request.
const THREAD_PAGE_SIZE: NonZeroU32 = pb::ThreadBatch::MAX_ITEMS;

#[derive(Debug, Error)]
pub enum QueueError {
    #[error("failed to open the upload queue")]
    Open(#[from] SqliteOpenOrCreateError),
    #[error("failed to migrate the upload queue")]
    Migrate(#[from] MigrateError),
    #[error("an upload queue query failed")]
    Query(#[from] sqlx::Error),
}

#[derive(Debug, Clone, Copy, sqlx::FromRow)]
pub struct PendingHistoryUpload {
    pub history_id: HistoryId,
    pub record_id: Option<RecordId>,
}

#[derive(Debug, Clone, Copy, sqlx::FromRow)]
pub struct PendingOutputUpload {
    pub history_id: HistoryId,
}

#[derive(Debug, Clone, Copy, sqlx::FromRow)]
pub struct PendingThreadMessageUpload {
    pub record_id: RecordId,
}

/// How far a user's upload of the thread records one host captured has got.
#[derive(Debug, Clone, Copy, PartialEq, Eq, sqlx::FromRow)]
pub struct ThreadBackfill {
    /// The host whose `ai-session` series this walks.
    pub host_id: HostId,
    /// The idx of the next record to upload.
    pub next_idx: RecordIdx,
    /// The idx past the last record to upload, or `None` until it is fixed.
    pub until_idx: Option<RecordIdx>,
}

#[derive(Debug, Clone)]
pub struct UploadQueue {
    db: Sqlite,
}

impl UploadQueue {
    pub async fn open(path: impl AsRef<Path>) -> Result<Self, QueueError> {
        let db = Sqlite::builder(path.as_ref().as_os_str()).restrict_permissions().open().await?;
        db::migrate!(db.pool(), "./migrations").await?;
        Ok(Self { db })
    }

    /// Queues `user`'s entry `id` for upload.
    pub async fn push_history(
        &self,
        user: &UserId,
        id: HistoryId,
        record_id: RecordId,
    ) -> Result<(), QueueError> {
        db::query(
            "INSERT OR IGNORE INTO history_uploads (user_id, history_id, record_id) VALUES (?1, \
             ?2, ?3)",
        )
        .bind(user.as_str())
        .bind(id)
        .bind(record_id)
        .execute(self.db.pool())
        .await?;
        Ok(())
    }

    pub async fn push_many_history(
        &self,
        user: &UserId,
        ids: impl IntoIterator<Item = HistoryId>,
    ) -> Result<u64, QueueError> {
        self.insert_ids("history_uploads", "history_id", user, ids).await
    }

    /// Queues the deletion of `user`'s entries `ids` from Octavo, returning how many were not
    /// queued already.
    pub async fn push_deletions(
        &self,
        user: &UserId,
        ids: impl IntoIterator<Item = HistoryId>,
    ) -> Result<u64, QueueError> {
        self.insert_ids("history_deletions", "history_id", user, ids).await
    }

    /// The first page of `user`'s queued deletions, in id order.
    pub async fn pending_deletions(&self, user: &UserId) -> Result<Vec<HistoryId>, QueueError> {
        let ids = db::query_scalar(
            "SELECT history_id FROM history_deletions WHERE user_id = ?1 ORDER BY history_id \
             LIMIT ?2",
        )
        .bind(user.as_str())
        .bind(PAGE_SIZE.get())
        .fetch_all(self.db.pool())
        .await?;
        Ok(ids)
    }

    pub async fn remove_deletion(&self, user: &UserId, id: HistoryId) -> Result<(), QueueError> {
        db::query("DELETE FROM history_deletions WHERE user_id = ?1 AND history_id = ?2")
            .bind(user.as_str())
            .bind(id)
            .execute(self.db.pool())
            .await?;
        Ok(())
    }

    async fn insert_ids<I>(
        &self,
        table: &'static str,
        column: &'static str,
        user: &UserId,
        ids: impl IntoIterator<Item = I>,
    ) -> Result<u64, QueueError>
    where
        I: for<'q> sqlx::Encode<'q, sqlx::Sqlite> + sqlx::Type<sqlx::Sqlite>,
    {
        let rows_per_insert = (self.db.info().await.variable_number_limit() / 2).max(1);
        let mut ids = ids.into_iter().peekable();
        let mut tx = self.db.pool().begin().await?;
        let mut queued = 0;

        while ids.peek().is_some() {
            let mut insert =
                QueryBuilder::new(format!("INSERT OR IGNORE INTO {table} (user_id, {column}) "));
            insert.push_values(ids.by_ref().take(rows_per_insert), |mut row, id| {
                row.push_bind(user.as_str()).push_bind(id);
            });
            queued += insert.build().execute(&mut *tx).await?.rows_affected();
        }

        tx.commit().await?;
        Ok(queued)
    }

    /// `user`'s queued uploads, in id order.
    pub fn pending_history(
        &self,
        user: &UserId,
    ) -> ChunkedStream<Result<PendingHistoryUpload, QueueError>> {
        self.paged(
            user,
            HISTORY_PAGE_SIZE,
            "SELECT history_id, record_id FROM history_uploads WHERE user_id = ?1 AND (?2 IS NULL \
             OR history_id > ?2) ORDER BY history_id LIMIT ?3",
            |upload: &PendingHistoryUpload| upload.history_id,
        )
    }

    pub async fn remove_history(
        &self,
        user: &UserId,
        ids: impl IntoIterator<Item = HistoryId>,
    ) -> Result<(), QueueError> {
        self.run_over_ids(user, "history_id", ids, |statement| {
            statement.push("DELETE FROM history_uploads WHERE");
        })
        .await?;
        Ok(())
    }

    /// Queues the upload of the output of `user`'s entry `id`.
    pub async fn push_output(&self, user: &UserId, id: HistoryId) -> Result<(), QueueError> {
        db::query("INSERT OR IGNORE INTO output_uploads (user_id, history_id) VALUES (?1, ?2)")
            .bind(user.as_str())
            .bind(id)
            .execute(self.db.pool())
            .await?;
        Ok(())
    }

    /// `user`'s queued output uploads, in id order.
    pub fn pending_outputs(
        &self,
        user: &UserId,
    ) -> ChunkedStream<Result<PendingOutputUpload, QueueError>> {
        self.paged(
            user,
            PAGE_SIZE,
            "SELECT history_id FROM output_uploads WHERE user_id = ?1 AND (?2 IS NULL OR \
             history_id > ?2) ORDER BY history_id LIMIT ?3",
            |upload: &PendingOutputUpload| upload.history_id,
        )
    }

    pub async fn remove_output(&self, user: &UserId, id: HistoryId) -> Result<(), QueueError> {
        db::query("DELETE FROM output_uploads WHERE user_id = ?1 AND history_id = ?2")
            .bind(user.as_str())
            .bind(id)
            .execute(self.db.pool())
            .await?;
        Ok(())
    }

    /// Removes `user`'s queued output upload for `id` if it was queued at least `after` ago, to
    /// the second, returning whether it did.
    pub async fn expire_output(
        &self,
        user: &UserId,
        id: HistoryId,
        after: Duration,
    ) -> Result<bool, QueueError> {
        let after = i64::try_from(after.as_secs()).unwrap_or(i64::MAX);
        let expired = db::query(
            "DELETE FROM output_uploads WHERE user_id = ?1 AND history_id = ?2 AND queued_at <= \
             unixepoch() - ?3",
        )
        .bind(user.as_str())
        .bind(id)
        .bind(after)
        .execute(self.db.pool())
        .await?
        .rows_affected();
        Ok(expired == 1)
    }

    /// Queues the upload of `user`'s thread messages stored as the records `ids`, returning how
    /// many were not queued already.
    pub async fn push_thread_messages(
        &self,
        user: &UserId,
        ids: impl IntoIterator<Item = RecordId>,
    ) -> Result<u64, QueueError> {
        self.insert_ids("thread_message_uploads", "record_id", user, ids).await
    }

    /// `user`'s queued thread message uploads that aren't held, in record id order.
    pub fn pending_thread_messages(
        &self,
        user: &UserId,
    ) -> ChunkedStream<Result<PendingThreadMessageUpload, QueueError>> {
        self.paged(
            user,
            THREAD_PAGE_SIZE,
            "SELECT record_id FROM thread_message_uploads WHERE user_id = ?1 AND held = 0 AND (?2 \
             IS NULL OR record_id > ?2) ORDER BY record_id LIMIT ?3",
            |upload: &PendingThreadMessageUpload| upload.record_id,
        )
    }

    pub async fn remove_thread_messages(
        &self,
        user: &UserId,
        ids: impl IntoIterator<Item = RecordId>,
    ) -> Result<(), QueueError> {
        self.run_over_ids(user, "record_id", ids, |statement| {
            statement.push("DELETE FROM thread_message_uploads WHERE");
        })
        .await?;
        Ok(())
    }

    /// Removes those of `user`'s queued thread message uploads `ids` that were queued at least
    /// `after` ago, to the second, returning how many it removed.
    pub async fn expire_thread_messages(
        &self,
        user: &UserId,
        ids: impl IntoIterator<Item = RecordId>,
        after: Duration,
    ) -> Result<u64, QueueError> {
        let after = i64::try_from(after.as_secs()).unwrap_or(i64::MAX);
        self.run_over_ids(user, "record_id", ids, |statement| {
            statement
                .push("DELETE FROM thread_message_uploads WHERE queued_at <= unixepoch() - ")
                .push_bind(after)
                .push(" AND");
        })
        .await
    }

    /// Leaves `user`'s queued thread message uploads `ids` out of
    /// [`Self::pending_thread_messages`] until [`Self::release_held_thread_messages`].
    pub async fn hold_thread_messages(
        &self,
        user: &UserId,
        ids: impl IntoIterator<Item = RecordId>,
    ) -> Result<(), QueueError> {
        self.run_over_ids(user, "record_id", ids, |statement| {
            statement.push("UPDATE thread_message_uploads SET held = 1 WHERE");
        })
        .await?;
        Ok(())
    }

    /// Releases every user's held thread message uploads.
    pub async fn release_held_thread_messages(&self) -> Result<(), QueueError> {
        db::query("UPDATE thread_message_uploads SET held = 0 WHERE held = 1")
            .execute(self.db.pool())
            .await?;
        Ok(())
    }

    /// Runs the statement `head` starts over `user`'s queued uploads whose `column` is one of
    /// `ids`, once per chunk SQLite can bind, in one transaction, returning the rows it changed.
    ///
    /// `head` ends where its `WHERE` clause can go on with `user_id = <user> AND <column> IN
    /// (<ids>)`.
    async fn run_over_ids<I>(
        &self,
        user: &UserId,
        column: &'static str,
        ids: impl IntoIterator<Item = I>,
        head: impl Fn(&mut QueryBuilder<sqlx::Sqlite>),
    ) -> Result<u64, QueueError>
    where
        I: for<'q> sqlx::Encode<'q, sqlx::Sqlite> + sqlx::Type<sqlx::Sqlite>,
    {
        // Leaves a parameter for the user and one for `head`.
        let ids_per_statement =
            self.db.info().await.variable_number_limit().saturating_sub(2).max(1);
        let mut ids = ids.into_iter().peekable();
        let mut tx = self.db.pool().begin().await?;
        let mut changed = 0;

        while ids.peek().is_some() {
            let mut statement = QueryBuilder::new("");
            head(&mut statement);
            statement
                .push(" user_id = ")
                .push_bind(user.as_str())
                .push(format!(" AND {column} IN ("));
            let mut list = statement.separated(", ");
            for id in ids.by_ref().take(ids_per_statement) {
                list.push_bind(id);
            }
            list.push_unseparated(")");
            changed += statement.build().execute(&mut *tx).await?.rows_affected();
        }

        tx.commit().await?;
        Ok(changed)
    }

    /// Marks `user`'s account as having Octavo off, until [`Self::clear_off`], returning whether
    /// it wasn't marked already.
    pub async fn mark_off(&self, user: &UserId) -> Result<bool, QueueError> {
        let marked = db::query("INSERT OR IGNORE INTO octavo_off (user_id) VALUES (?1)")
            .bind(user.as_str())
            .execute(self.db.pool())
            .await?
            .rows_affected();
        Ok(marked == 1)
    }

    /// Clears the mark [`Self::mark_off`] left on `user`'s account, returning whether there was
    /// one.
    pub async fn clear_off(&self, user: &UserId) -> Result<bool, QueueError> {
        // Read first, as a read takes no write lock, and almost every call finds no mark.
        if !self.is_off(user).await? {
            return Ok(false);
        }
        let cleared = db::query("DELETE FROM octavo_off WHERE user_id = ?1")
            .bind(user.as_str())
            .execute(self.db.pool())
            .await?
            .rows_affected();
        Ok(cleared == 1)
    }

    /// Whether [`Self::mark_off`] marked `user`'s account, and nothing cleared it since.
    pub async fn is_off(&self, user: &UserId) -> Result<bool, QueueError> {
        let marked: Option<i64> = db::query_scalar("SELECT 1 FROM octavo_off WHERE user_id = ?1")
            .bind(user.as_str())
            .fetch_optional(self.db.pool())
            .await?;
        Ok(marked.is_some())
    }

    /// Starts `user`'s backfill of each of `hosts` over from its first record, with the bound
    /// unfixed.
    pub async fn request_thread_backfill(
        &self,
        user: &UserId,
        hosts: impl IntoIterator<Item = HostId>,
    ) -> Result<(), QueueError> {
        let mut tx = self.db.pool().begin().await?;
        for host in hosts {
            db::query(
                "INSERT OR REPLACE INTO thread_backfills (user_id, host_id, next_idx, until_idx) \
                 VALUES (?1, ?2, 0, NULL)",
            )
            .bind(user.as_str())
            .bind(host)
            .execute(&mut *tx)
            .await?;
        }
        tx.commit().await?;
        Ok(())
    }

    /// Fixes each unfixed bound of `user`'s backfills just past its host's tail: `tails` maps a
    /// host to the idx of the last record of its series.
    ///
    /// A backfill whose host `tails` leaves out stays unfixed.
    pub async fn fix_thread_backfills(
        &self,
        user: &UserId,
        tails: impl IntoIterator<Item = (HostId, RecordIdx)>,
    ) -> Result<(), QueueError> {
        let mut tx = self.db.pool().begin().await?;
        for (host, tail) in tails {
            db::query(
                "UPDATE thread_backfills SET until_idx = ?3 WHERE user_id = ?1 AND host_id = ?2 \
                 AND until_idx IS NULL",
            )
            .bind(user.as_str())
            .bind(host)
            .bind(stored_idx(tail.saturating_add(1)))
            .execute(&mut *tx)
            .await?;
        }
        tx.commit().await?;
        Ok(())
    }

    /// Drops `user`'s backfills whose bound isn't fixed yet.
    ///
    /// A fixed backfill stays: all it walks was captured before the user asked for it.
    pub async fn drop_unfixed_thread_backfills(&self, user: &UserId) -> Result<(), QueueError> {
        db::query("DELETE FROM thread_backfills WHERE user_id = ?1 AND until_idx IS NULL")
            .bind(user.as_str())
            .execute(self.db.pool())
            .await?;
        Ok(())
    }

    /// `user`'s backfills, done or not, in host order.
    pub async fn thread_backfills(&self, user: &UserId) -> Result<Vec<ThreadBackfill>, QueueError> {
        let backfills = db::query_as(
            "SELECT host_id, next_idx, until_idx FROM thread_backfills WHERE user_id = ?1 ORDER \
             BY host_id",
        )
        .bind(user.as_str())
        .fetch_all(self.db.pool())
        .await?;
        Ok(backfills)
    }

    /// Moves `user`'s backfill of `host` on to the record at `next`.
    pub async fn advance_thread_backfill(
        &self,
        user: &UserId,
        host: HostId,
        next: RecordIdx,
    ) -> Result<(), QueueError> {
        db::query("UPDATE thread_backfills SET next_idx = ?3 WHERE user_id = ?1 AND host_id = ?2")
            .bind(user.as_str())
            .bind(host)
            .bind(stored_idx(next))
            .execute(self.db.pool())
            .await?;
        Ok(())
    }

    /// `user`'s rows that `sql` selects `page_size` at a time, in id order.
    ///
    /// `sql` binds `?1` to the user, `?2` to the last id of the page before, if any, and `?3` to
    /// the page size, and orders by the id `last_id` reads off a row.
    fn paged<T, I>(
        &self,
        user: &UserId,
        page_size: NonZeroU32,
        sql: &'static str,
        last_id: fn(&T) -> I,
    ) -> ChunkedStream<Result<T, QueueError>>
    where
        T: for<'r> sqlx::FromRow<'r, SqliteRow> + Send + Unpin + 'static,
        I: for<'q> sqlx::Encode<'q, sqlx::Sqlite>
            + sqlx::Type<sqlx::Sqlite>
            + Copy
            + Send
            + 'static,
    {
        let pool = self.db.pool().clone();
        let user = user.clone();

        ChunkedStream::new(stream::unfold(Some(None), move |after: Option<Option<I>>| {
            let pool = pool.clone();
            let user = user.clone();
            async move {
                let after = after?;
                let page = match db::query_as::<_, T>(sql)
                    .bind(user.as_str())
                    .bind(after)
                    .bind(page_size.get())
                    .fetch_all(&pool)
                    .await
                {
                    Ok(page) => page,
                    Err(err) => return Some((vec![Err(err.into())], None)),
                };

                let last = last_id(page.last()?);
                Some((page.into_iter().map(Ok).collect(), Some(Some(last))))
            }
        }))
    }
}

/// `idx` as SQLite binds it: the record store keeps an idx as an `i64`, so none is past
/// `i64::MAX`.
fn stored_idx(idx: RecordIdx) -> i64 {
    i64::try_from(idx).unwrap_or(i64::MAX)
}

#[cfg(test)]
mod tests {
    use std::iter;
    use std::time::Duration;

    use atuin_common::utils::uuid_v7;
    use futures::TryStreamExt as _;
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

    /// Ids in the order `uuid_v7` made them, which is their sort order.
    fn ids(count: usize) -> Vec<HistoryId> {
        (0..count).map(|_| HistoryId::from(uuid_v7())).collect()
    }

    async fn pending(queue: &UploadQueue, user: &UserId) -> Vec<HistoryId> {
        queue
            .pending_outputs(user)
            .items()
            .map_ok(|pending| pending.history_id)
            .try_collect()
            .await
            .expect("the queue reads")
    }

    #[rstest]
    #[tokio::test]
    async fn pending_outputs_are_each_users_own_in_id_order(#[future(awt)] queue: TempQueue) {
        let (alice, bob) = (UserId::new("alice"), UserId::new("bob"));
        let ids = ids(3);
        for (user, id) in [(&alice, ids[1]), (&bob, ids[2]), (&alice, ids[0]), (&alice, ids[1])] {
            queue.queue.push_output(user, id).await.expect("push");
        }

        assert_eq!(pending(&queue.queue, &alice).await, ids[..2]);
        assert_eq!(pending(&queue.queue, &bob).await, ids[2..]);
    }

    #[rstest]
    #[tokio::test]
    async fn pending_outputs_pages_through_more_than_a_page(#[future(awt)] queue: TempQueue) {
        let user = UserId::new("alice");
        let ids = ids(usize::try_from(PAGE_SIZE.get()).expect("fits") * 2 + 1);
        for &id in &ids {
            queue.queue.push_output(&user, id).await.expect("push");
        }

        assert_eq!(pending(&queue.queue, &user).await, ids);
    }

    #[rstest]
    #[tokio::test]
    async fn remove_output_takes_only_that_users_row(#[future(awt)] queue: TempQueue) {
        let (alice, bob) = (UserId::new("alice"), UserId::new("bob"));
        let id = HistoryId::from(uuid_v7());
        queue.queue.push_output(&alice, id).await.expect("push");
        queue.queue.push_output(&bob, id).await.expect("push");

        queue.queue.remove_output(&alice, id).await.expect("remove");

        assert_eq!(pending(&queue.queue, &alice).await, vec![]);
        assert_eq!(pending(&queue.queue, &bob).await, vec![id]);
    }

    #[rstest]
    #[case::fresh(Duration::from_secs(60 * 60), false)]
    #[case::due(Duration::ZERO, true)]
    #[tokio::test]
    async fn expire_output_removes_only_a_row_queued_that_long_ago(
        #[future(awt)] queue: TempQueue,
        #[case] after: Duration,
        #[case] expired: bool,
    ) {
        let user = UserId::new("alice");
        let id = HistoryId::from(uuid_v7());
        queue.queue.push_output(&user, id).await.expect("push");

        assert_eq!(queue.queue.expire_output(&user, id, after).await.expect("expire"), expired);
        assert_eq!(pending(&queue.queue, &user).await.is_empty(), expired);
    }

    /// Ids in the order `uuid_v7` made them, which is their sort order.
    fn record_ids(count: usize) -> Vec<RecordId> {
        (0..count).map(|_| RecordId(uuid_v7())).collect()
    }

    async fn pending_threads(queue: &UploadQueue, user: &UserId) -> Vec<RecordId> {
        queue
            .pending_thread_messages(user)
            .items()
            .map_ok(|pending| pending.record_id)
            .try_collect()
            .await
            .expect("the queue reads")
    }

    #[rstest]
    #[tokio::test]
    async fn pending_thread_messages_are_each_users_own_in_id_order(
        #[future(awt)] queue: TempQueue,
    ) {
        let (alice, bob) = (UserId::new("alice"), UserId::new("bob"));
        let ids = record_ids(3);
        for (user, pushed) in [(&alice, [ids[1], ids[0]]), (&bob, [ids[2], ids[2]])] {
            queue.queue.push_thread_messages(user, pushed).await.expect("push");
        }

        assert_eq!(pending_threads(&queue.queue, &alice).await, ids[..2]);
        assert_eq!(pending_threads(&queue.queue, &bob).await, ids[2..]);
    }

    #[rstest]
    #[tokio::test]
    async fn pending_thread_messages_pages_past_ids_left_queued(#[future(awt)] queue: TempQueue) {
        let user = UserId::new("alice");
        let ids = record_ids(usize::try_from(THREAD_PAGE_SIZE.get()).expect("fits") * 2 + 1);
        queue.queue.push_thread_messages(&user, ids.iter().copied()).await.expect("push");

        assert_eq!(pending_threads(&queue.queue, &user).await, ids);
    }

    #[rstest]
    #[tokio::test]
    async fn pending_thread_messages_passes_over_held_ids(#[future(awt)] queue: TempQueue) {
        let user = UserId::new("alice");
        let ids = record_ids(3);
        queue.queue.push_thread_messages(&user, ids.iter().copied()).await.expect("push");

        queue.queue.hold_thread_messages(&user, [ids[0], ids[2]]).await.expect("hold");

        assert_eq!(pending_threads(&queue.queue, &user).await, [ids[1]]);
    }

    #[rstest]
    #[tokio::test]
    async fn release_held_thread_messages_releases_every_users(#[future(awt)] queue: TempQueue) {
        let (alice, bob) = (UserId::new("alice"), UserId::new("bob"));
        let ids = record_ids(2);
        for (user, id) in [(&alice, ids[0]), (&bob, ids[1])] {
            queue.queue.push_thread_messages(user, [id]).await.expect("push");
            queue.queue.hold_thread_messages(user, [id]).await.expect("hold");
        }

        queue.queue.release_held_thread_messages().await.expect("release");

        assert_eq!(pending_threads(&queue.queue, &alice).await, [ids[0]]);
        assert_eq!(pending_threads(&queue.queue, &bob).await, [ids[1]]);
    }

    #[rstest]
    #[tokio::test]
    async fn remove_thread_messages_takes_only_that_users_rows(#[future(awt)] queue: TempQueue) {
        let (alice, bob) = (UserId::new("alice"), UserId::new("bob"));
        let ids = record_ids(3);
        queue.queue.push_thread_messages(&alice, ids.iter().copied()).await.expect("push");
        queue.queue.push_thread_messages(&bob, ids.iter().copied()).await.expect("push");

        queue.queue.remove_thread_messages(&alice, [ids[0], ids[2]]).await.expect("remove");

        assert_eq!(pending_threads(&queue.queue, &alice).await, [ids[1]]);
        assert_eq!(pending_threads(&queue.queue, &bob).await, ids);
    }

    #[rstest]
    #[case::fresh(Duration::from_secs(60 * 60), 0)]
    #[case::due(Duration::ZERO, 2)]
    #[tokio::test]
    async fn expire_thread_messages_removes_only_rows_queued_that_long_ago(
        #[future(awt)] queue: TempQueue,
        #[case] after: Duration,
        #[case] expired: u64,
    ) {
        let user = UserId::new("alice");
        let ids = record_ids(3);
        queue.queue.push_thread_messages(&user, ids.iter().copied()).await.expect("push");

        let removed = queue.queue.expire_thread_messages(&user, [ids[0], ids[1]], after).await;

        assert_eq!(removed.expect("expire"), expired);
        assert_eq!(
            pending_threads(&queue.queue, &user).await.len(),
            3 - usize::try_from(expired).expect("fits")
        );
    }

    /// What settles a backfill whose bound isn't fixed.
    enum Settle {
        /// [`UploadQueue::fix_thread_backfills`], at a tail of 4.
        Fix,
        /// [`UploadQueue::drop_unfixed_thread_backfills`].
        Drop,
    }

    #[rstest]
    #[case::fix(Settle::Fix, Some(5))]
    #[case::drop(Settle::Drop, None)]
    #[tokio::test]
    async fn settling_touches_only_unfixed_backfills(
        #[future(awt)] queue: TempQueue,
        #[case] settle: Settle,
        #[case] unfixed_until: Option<RecordIdx>,
    ) {
        let user = UserId::new("alice");
        let (fixed, unfixed) = (HostId(uuid_v7()), HostId(uuid_v7()));
        queue.queue.request_thread_backfill(&user, [fixed]).await.expect("request");
        queue.queue.fix_thread_backfills(&user, [(fixed, 9)]).await.expect("fix");
        queue.queue.request_thread_backfill(&user, [unfixed]).await.expect("request");

        let settled = match settle {
            Settle::Fix => {
                queue.queue.fix_thread_backfills(&user, [(fixed, 20), (unfixed, 4)]).await
            }
            Settle::Drop => queue.queue.drop_unfixed_thread_backfills(&user).await,
        };

        settled.expect("settle");
        let backfill = |host_id, until_idx| ThreadBackfill {
            host_id,
            next_idx: 0,
            until_idx,
        };
        let want: Vec<_> = iter::once(backfill(fixed, Some(10)))
            .chain(unfixed_until.map(|until| backfill(unfixed, Some(until))))
            .collect();
        assert_eq!(queue.queue.thread_backfills(&user).await.expect("read"), want);
    }

    #[rstest]
    #[tokio::test]
    async fn request_thread_backfill_starts_a_done_backfill_over(#[future(awt)] queue: TempQueue) {
        let user = UserId::new("alice");
        let host = HostId(uuid_v7());
        queue.queue.request_thread_backfill(&user, [host]).await.expect("request");
        queue.queue.fix_thread_backfills(&user, [(host, 9)]).await.expect("fix");
        queue.queue.advance_thread_backfill(&user, host, 10).await.expect("advance");

        queue.queue.request_thread_backfill(&user, [host]).await.expect("request");

        assert_eq!(queue.queue.thread_backfills(&user).await.expect("read"), [ThreadBackfill {
            host_id: host,
            next_idx: 0,
            until_idx: None,
        }]);
    }
}
