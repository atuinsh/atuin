use std::num::NonZeroU32;
use std::path::Path;
use std::time::Duration;

use atuin_client::history::HistoryId;
use atuin_common::db;
use atuin_common::db::sqlite::{Sqlite, SqliteOpenOrCreateError};
use atuin_common::futures::stream::ChunkedStream;
use atuin_domain::record::RecordId;
use futures::stream;
use sqlx::migrate::MigrateError;
use sqlx::sqlite::SqliteRow;
use thiserror::Error;

use crate::hub::UserId;

const PAGE_SIZE: NonZeroU32 = NonZeroU32::new(100).unwrap();

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
        self.insert_ids("history_uploads", user, ids).await
    }

    /// Queues the deletion of `user`'s entries `ids` from Octavo, returning how many were not
    /// queued already.
    pub async fn push_deletions(
        &self,
        user: &UserId,
        ids: impl IntoIterator<Item = HistoryId>,
    ) -> Result<u64, QueueError> {
        self.insert_ids("history_deletions", user, ids).await
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

    async fn insert_ids(
        &self,
        table: &'static str,
        user: &UserId,
        ids: impl IntoIterator<Item = HistoryId>,
    ) -> Result<u64, QueueError> {
        let rows_per_insert = (self.db.info().await.variable_number_limit() / 2).max(1);
        let mut ids = ids.into_iter().peekable();
        let mut tx = self.db.pool().begin().await?;
        let mut queued = 0;

        while ids.peek().is_some() {
            let mut insert = sqlx::QueryBuilder::new(format!(
                "INSERT OR IGNORE INTO {table} (user_id, history_id) "
            ));
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
            "SELECT history_id, record_id FROM history_uploads WHERE user_id = ?1 AND (?2 IS NULL \
             OR history_id > ?2) ORDER BY history_id LIMIT ?3",
            |upload: &PendingHistoryUpload| upload.history_id,
        )
    }

    pub async fn remove_history(&self, user: &UserId, id: HistoryId) -> Result<(), QueueError> {
        db::query("DELETE FROM history_uploads WHERE user_id = ?1 AND history_id = ?2")
            .bind(user.as_str())
            .bind(id)
            .execute(self.db.pool())
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

    /// `user`'s rows that `sql` selects a page at a time, in id order.
    ///
    /// `sql` binds `?1` to the user, `?2` to the last id of the page before, if any, and `?3` to
    /// the page size, and orders by the id `last_id` reads off a row.
    fn paged<T>(
        &self,
        user: &UserId,
        sql: &'static str,
        last_id: fn(&T) -> HistoryId,
    ) -> ChunkedStream<Result<T, QueueError>>
    where
        T: for<'r> sqlx::FromRow<'r, SqliteRow> + Send + Unpin + 'static,
    {
        let pool = self.db.pool().clone();
        let user = user.clone();

        ChunkedStream::new(stream::unfold(Some(None), move |after: Option<Option<HistoryId>>| {
            let pool = pool.clone();
            let user = user.clone();
            async move {
                let after = after?;
                let page = match db::query_as::<_, T>(sql)
                    .bind(user.as_str())
                    .bind(after)
                    .bind(PAGE_SIZE.get())
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

#[cfg(test)]
mod tests {
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
}
