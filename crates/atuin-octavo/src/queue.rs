use std::num::NonZeroU32;
use std::path::Path;

use atuin_client::history::HistoryId;
use atuin_common::db;
use atuin_common::db::sqlite::{Sqlite, SqliteOpenOrCreateError};
use atuin_common::futures::stream::ChunkedStream;
use atuin_domain::record::RecordId;
use futures::stream;
use sqlx::migrate::MigrateError;
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
        let pool = self.db.pool().clone();
        let user = user.clone();

        ChunkedStream::new(stream::unfold(Some(None), move |after: Option<Option<HistoryId>>| {
            let pool = pool.clone();
            let user = user.clone();
            async move {
                let after = after?;
                let page = match db::query_as::<_, PendingHistoryUpload>(
                    "SELECT history_id, record_id FROM history_uploads WHERE user_id = ?1 AND (?2 \
                     IS NULL OR history_id > ?2) ORDER BY history_id LIMIT ?3",
                )
                .bind(user.as_str())
                .bind(after)
                .bind(PAGE_SIZE.get())
                .fetch_all(&pool)
                .await
                {
                    Ok(page) => page,
                    Err(err) => return Some((vec![Err(err.into())], None)),
                };

                let last = page.last()?.history_id;
                Some((page.into_iter().map(Ok).collect(), Some(Some(last))))
            }
        }))
    }

    pub async fn remove_history(&self, user: &UserId, id: HistoryId) -> Result<(), QueueError> {
        db::query("DELETE FROM history_uploads WHERE user_id = ?1 AND history_id = ?2")
            .bind(user.as_str())
            .bind(id)
            .execute(self.db.pool())
            .await?;
        Ok(())
    }
}
