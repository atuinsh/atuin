use std::collections::BTreeMap;
use std::future::Future;

use futures::TryStreamExt;
use itertools::{EitherOrBoth, Itertools};
use sqlx::sqlite::SqliteConnection;
use sqlx::{AssertSqlSafe, Sqlite};
use tokio::sync::mpsc;

use super::event::{Appended, Change};
use super::schema::{Diffable, Tailable, TableSchema};
use super::{ObserveConfig, ObserveError, Replay};

pub(super) enum DeliverError {
    Sqlx(sqlx::Error),
    ConsumerGone,
}

pub(super) trait Strategy: Send + 'static {
    type Event: Send + 'static;

    fn seed(
        &mut self,
        conn: &mut SqliteConnection,
        replay: Replay,
        tx: &mpsc::Sender<Result<Self::Event, ObserveError>>,
    ) -> impl Future<Output = Result<(), DeliverError>> + Send;

    fn poll(
        &mut self,
        conn: &mut SqliteConnection,
        tx: &mpsc::Sender<Result<Self::Event, ObserveError>>,
    ) -> impl Future<Output = Result<(), DeliverError>> + Send;
}

pub(super) struct AppendStrategy<T: Tailable> {
    cursor: Option<T::Cursor>,
}

impl<T: Tailable> AppendStrategy<T> {
    pub(super) fn new() -> Self {
        Self { cursor: None }
    }
}

impl<T: Tailable> Strategy for AppendStrategy<T> {
    type Event = Appended<T>;

    async fn seed(
        &mut self,
        conn: &mut SqliteConnection,
        replay: Replay,
        _tx: &mpsc::Sender<Result<Self::Event, ObserveError>>,
    ) -> Result<(), DeliverError> {
        self.cursor = match replay {
            Replay::All => None,
            Replay::FromNow => {
                let sql = format!("SELECT max({}) FROM {}", T::CURSOR_COLUMN, T::TABLE);
                crate::db::query_scalar::<Sqlite, Option<T::Cursor>>(AssertSqlSafe(sql))
                    .fetch_one(conn)
                    .await
                    .map_err(DeliverError::Sqlx)?
            }
        };
        Ok(())
    }

    async fn poll(
        &mut self,
        conn: &mut SqliteConnection,
        tx: &mpsc::Sender<Result<Self::Event, ObserveError>>,
    ) -> Result<(), DeliverError> {
        let cols = T::COLUMNS.join(", ");
        let sql = match self.cursor {
            Some(_) => format!(
                "SELECT {cols} FROM {} WHERE {} > ?1 ORDER BY {} ASC",
                T::TABLE,
                T::CURSOR_COLUMN,
                T::CURSOR_COLUMN
            ),
            None => format!("SELECT {cols} FROM {} ORDER BY {} ASC", T::TABLE, T::CURSOR_COLUMN),
        };

        let query = crate::db::query_as::<Sqlite, T>(AssertSqlSafe(sql));
        let query = match &self.cursor {
            Some(cursor) => query.bind(cursor.clone()),
            None => query,
        };

        let next = query
            .fetch(conn)
            .map_err(DeliverError::Sqlx)
            .try_fold(self.cursor.clone(), |_, row| {
                let cursor = row.cursor();
                async move {
                    tx.send(Ok(Appended(row))).await.map_err(|_| DeliverError::ConsumerGone)?;
                    Ok(Some(cursor))
                }
            })
            .await?;

        self.cursor = next;
        Ok(())
    }
}

pub(super) async fn run<S: Strategy>(
    mut conn: SqliteConnection,
    mut strategy: S,
    cfg: ObserveConfig,
    tx: mpsc::Sender<Result<S::Event, ObserveError>>,
) {
    match strategy.seed(&mut conn, cfg.replay, &tx).await {
        Ok(()) => {}
        Err(DeliverError::ConsumerGone) => return,
        Err(DeliverError::Sqlx(e)) => {
            let _ = tx.send(Err(ObserveError::Seed(e))).await;
            return;
        }
    }

    let mut ticker = tokio::time::interval(cfg.poll_interval);
    let mut last: Option<i64> = None;
    loop {
        ticker.tick().await;

        let version = match crate::db::query_scalar::<Sqlite, i64>("PRAGMA data_version")
            .fetch_one(&mut conn)
            .await
        {
            Ok(version) => version,
            Err(e) => {
                let _ = tx.send(Err(ObserveError::Query(e))).await;
                return;
            }
        };
        if last == Some(version) {
            continue;
        }

        match strategy.poll(&mut conn, &tx).await {
            Ok(()) => last = Some(version),
            Err(DeliverError::ConsumerGone) => return,
            Err(DeliverError::Sqlx(e)) => {
                let _ = tx.send(Err(ObserveError::Query(e))).await;
                return;
            }
        }
    }
}

async fn fetch_all<T: TableSchema>(conn: &mut SqliteConnection) -> Result<Vec<T>, DeliverError> {
    let cols = T::COLUMNS.join(", ");
    let sql = format!("SELECT {cols} FROM {}", T::TABLE);
    crate::db::query_as::<Sqlite, T>(AssertSqlSafe(sql))
        .fetch_all(conn)
        .await
        .map_err(DeliverError::Sqlx)
}

async fn deliver<E>(
    tx: &mpsc::Sender<Result<E, ObserveError>>,
    events: Vec<E>,
) -> Result<(), DeliverError> {
    use futures::StreamExt as _;
    futures::stream::iter(events)
        .map(Ok::<E, DeliverError>)
        .try_for_each(|event| async move {
            tx.send(Ok(event)).await.map_err(|_| DeliverError::ConsumerGone)
        })
        .await
}

pub(super) struct MutateStrategy<T: Diffable> {
    snapshot: BTreeMap<T::Key, T>,
}

impl<T: Diffable> MutateStrategy<T> {
    pub(super) fn new() -> Self {
        Self { snapshot: BTreeMap::new() }
    }
}

impl<T: Diffable> Strategy for MutateStrategy<T> {
    type Event = Change<T>;

    async fn seed(
        &mut self,
        conn: &mut SqliteConnection,
        replay: Replay,
        tx: &mpsc::Sender<Result<Self::Event, ObserveError>>,
    ) -> Result<(), DeliverError> {
        let snapshot: BTreeMap<T::Key, T> =
            fetch_all::<T>(conn).await?.into_iter().map(|row| (row.key(), row)).collect();
        if matches!(replay, Replay::All) {
            let inserts: Vec<Change<T>> = snapshot.values().cloned().map(Change::Inserted).collect();
            deliver(tx, inserts).await?;
        }
        self.snapshot = snapshot;
        Ok(())
    }

    async fn poll(
        &mut self,
        conn: &mut SqliteConnection,
        tx: &mpsc::Sender<Result<Self::Event, ObserveError>>,
    ) -> Result<(), DeliverError> {
        let fresh: BTreeMap<T::Key, T> =
            fetch_all::<T>(conn).await?.into_iter().map(|row| (row.key(), row)).collect();

        let changes: Vec<Change<T>> = self
            .snapshot
            .iter()
            .merge_join_by(fresh.iter(), |(a, _), (b, _)| a.cmp(b))
            .filter_map(|joined| match joined {
                EitherOrBoth::Left((_, old)) => Some(Change::Deleted(old.clone())),
                EitherOrBoth::Right((_, new)) => Some(Change::Inserted(new.clone())),
                EitherOrBoth::Both((_, old), (_, new)) => {
                    (old != new).then(|| Change::Updated { old: old.clone(), new: new.clone() })
                }
            })
            .collect();

        deliver(tx, changes).await?;
        self.snapshot = fresh;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use futures::StreamExt;
    use rstest::rstest;

    use super::*;
    use crate::db::query;
    use crate::db::sqlite::Sqlite;
    use crate::db::sqlite::observe::{ObserveConfig, Replay, SqliteObserver, TableSchema};

    #[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow)]
    struct Item {
        id: i64,
        name: String,
    }
    impl TableSchema for Item {
        const TABLE: &'static str = "items";
        const COLUMNS: &'static [&'static str] = &["id", "name"];
    }
    impl Tailable for Item {
        type Cursor = i64;
        const CURSOR_COLUMN: &'static str = "id";
        fn cursor(&self) -> i64 {
            self.id
        }
    }
    impl Diffable for Item {
        type Key = i64;
        fn key(&self) -> i64 {
            self.id
        }
    }

    async fn writer(dir: &std::path::Path) -> Sqlite {
        let sqlite = Sqlite::builder(dir.join("db.sqlite").as_os_str()).open().await.unwrap();
        let mut conn = sqlite.pool().acquire().await.unwrap();
        query::<sqlx::Sqlite>("CREATE TABLE items (id INTEGER PRIMARY KEY, name TEXT NOT NULL)")
            .execute(&mut *conn)
            .await
            .unwrap();
        sqlite
    }

    async fn insert(sqlite: &Sqlite, id: i64, name: &str) {
        let mut conn = sqlite.pool().acquire().await.unwrap();
        query::<sqlx::Sqlite>("INSERT INTO items (id, name) VALUES (?1, ?2)")
            .bind(id)
            .bind(name)
            .execute(&mut *conn)
            .await
            .unwrap();
    }

    async fn exec_sql(path: &std::path::Path, sql: &str) {
        let status = tokio::process::Command::new("sqlite3")
            .arg(path)
            .arg(sql)
            .status()
            .await
            .expect("observe tests require the `sqlite3` CLI on PATH");
        assert!(status.success(), "sqlite3 failed: {status}");
    }

    async fn insert_xproc(path: &std::path::Path, id: i64, name: &str) {
        exec_sql(path, &format!("INSERT INTO items (id, name) VALUES ({id}, '{name}');")).await;
    }

    async fn update_xproc(path: &std::path::Path, id: i64, name: &str) {
        exec_sql(path, &format!("UPDATE items SET name = '{name}' WHERE id = {id};")).await;
    }

    async fn delete_xproc(path: &std::path::Path, id: i64) {
        exec_sql(path, &format!("DELETE FROM items WHERE id = {id};")).await;
    }

    fn cfg(replay: Replay) -> ObserveConfig {
        ObserveConfig::builder()
            .replay(replay)
            .poll_interval(Duration::from_millis(10))
            .build()
    }

    fn item(id: i64, name: &str) -> Item {
        Item { id, name: name.into() }
    }

    #[rstest]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn append_tail_emits_new_rows_in_order() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("db.sqlite");
        let _db = writer(dir.path()).await;
        let observer = SqliteObserver::new(&path);
        let stream = observer.append::<Item>(cfg(Replay::FromNow)).await.unwrap();

        insert_xproc(&path, 1, "a").await;
        insert_xproc(&path, 2, "b").await;
        insert_xproc(&path, 3, "c").await;

        let got: Vec<Item> = stream.take(3).map(|r| r.unwrap().0).collect().await;
        assert_eq!(got, vec![item(1, "a"), item(2, "b"), item(3, "c")]);
    }

    #[rstest]
    #[case::from_now(Replay::FromNow, vec![item(3, "c")])]
    #[case::all(Replay::All, vec![item(1, "a"), item(2, "b"), item(3, "c")])]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn replay_controls_preexisting_rows(#[case] replay: Replay, #[case] expected: Vec<Item>) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("db.sqlite");
        let db = writer(dir.path()).await;
        insert(&db, 1, "a").await;
        insert(&db, 2, "b").await;

        let observer = SqliteObserver::new(&path);
        let stream = observer.append::<Item>(cfg(replay)).await.unwrap();

        insert_xproc(&path, 3, "c").await;

        let got: Vec<Item> = stream.take(expected.len()).map(|r| r.unwrap().0).collect().await;
        assert_eq!(got, expected);
    }

    #[rstest]
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn concurrent_writers_no_loss() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("db.sqlite");
        let _db = writer(dir.path()).await;
        let observer = SqliteObserver::new(&path);
        let stream = observer.append::<Item>(cfg(Replay::FromNow)).await.unwrap();

        let writers = (0..4).map(|w| {
            let path = path.clone();
            tokio::spawn(async move {
                let inserts: String = (0..25)
                    .map(|i| format!("INSERT INTO items (name) VALUES ('w{w}_{i}');"))
                    .collect();
                exec_sql(
                    &path,
                    &format!("PRAGMA busy_timeout=10000; BEGIN IMMEDIATE; {inserts} COMMIT;"),
                )
                .await;
            })
        });
        futures::future::join_all(writers).await;

        let got: Vec<i64> = stream.take(100).map(|r| r.unwrap().0.id).collect().await;
        let mut sorted = got.clone();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(sorted.len(), 100, "every committed row delivered exactly once");
        let mut ascending = got.clone();
        ascending.sort_unstable();
        assert_eq!(got, ascending, "rows delivered in ascending cursor order");
    }

    proptest::proptest! {
        #![proptest_config(proptest::prelude::ProptestConfig::with_cases(16))]
        #[rstest]
        fn append_stream_is_a_strictly_ascending_run(ids in proptest::collection::hash_set(1i64..1000, 1..30)) {
            let rt = tokio::runtime::Builder::new_multi_thread()
                .worker_threads(2)
                .enable_all()
                .build()
                .unwrap();
            rt.block_on(async {
                let dir = tempfile::tempdir().unwrap();
                let path = dir.path().join("db.sqlite");
                let _db = writer(dir.path()).await;
                let observer = SqliteObserver::new(&path);
                let stream = observer.append::<Item>(cfg(Replay::FromNow)).await.unwrap();

                let mut sorted: Vec<i64> = ids.iter().copied().collect();
                sorted.sort_unstable();
                let sql: String = sorted
                    .iter()
                    .map(|id| format!("INSERT INTO items (id, name) VALUES ({id}, 'x');"))
                    .collect();
                exec_sql(&path, &sql).await;

                let got: Vec<i64> = stream.take(sorted.len()).map(|r| r.unwrap().0.id).collect().await;
                proptest::prop_assert_eq!(got, sorted);
                Ok(())
            })?;
        }
    }

    #[rstest]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn mutate_detects_insert_update_delete() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("db.sqlite");
        let db = writer(dir.path()).await;
        insert(&db, 1, "a").await;

        let observer = SqliteObserver::new(&path);
        let mut stream = observer.mutate::<Item>(cfg(Replay::FromNow)).await.unwrap();

        insert_xproc(&path, 2, "b").await;
        assert_eq!(stream.next().await.unwrap().unwrap(), Change::Inserted(item(2, "b")));

        update_xproc(&path, 1, "a2").await;
        assert_eq!(
            stream.next().await.unwrap().unwrap(),
            Change::Updated { old: item(1, "a"), new: item(1, "a2") }
        );

        delete_xproc(&path, 2).await;
        assert_eq!(stream.next().await.unwrap().unwrap(), Change::Deleted(item(2, "b")));
    }

    #[rstest]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn mutate_ignores_value_preserving_write() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("db.sqlite");
        let db = writer(dir.path()).await;
        insert(&db, 1, "a").await;

        let observer = SqliteObserver::new(&path);
        let mut stream = observer.mutate::<Item>(cfg(Replay::FromNow)).await.unwrap();

        update_xproc(&path, 1, "a").await;
        insert_xproc(&path, 2, "b").await;

        assert_eq!(stream.next().await.unwrap().unwrap(), Change::Inserted(item(2, "b")));
    }

    proptest::proptest! {
        #![proptest_config(proptest::prelude::ProptestConfig::with_cases(12))]
        #[rstest]
        fn mutate_stream_reconstructs_final_state(
            ops in proptest::collection::vec((1i64..8, 0u8..3, "[a-c]"), 1..24)
        ) {
            let rt = tokio::runtime::Builder::new_multi_thread()
                .worker_threads(2)
                .enable_all()
                .build()
                .unwrap();
            rt.block_on(async {
                let dir = tempfile::tempdir().unwrap();
                let path = dir.path().join("db.sqlite");
                let _db = writer(dir.path()).await;
                let observer = SqliteObserver::new(&path);
                let mut stream = observer.mutate::<Item>(cfg(Replay::All)).await.unwrap();

                let mut expected = std::collections::BTreeMap::<i64, String>::new();
                for (id, op, name) in &ops {
                    match op {
                        0 | 1 => {
                            exec_sql(&path, &format!("INSERT INTO items (id,name) VALUES ({id},'{name}') ON CONFLICT(id) DO UPDATE SET name='{name}';")).await;
                            expected.insert(*id, name.clone());
                        }
                        _ => {
                            exec_sql(&path, &format!("DELETE FROM items WHERE id={id};")).await;
                            expected.remove(id);
                        }
                    }
                }

                let mut state = std::collections::BTreeMap::<i64, String>::new();
                let deadline = tokio::time::Instant::now() + Duration::from_secs(3);
                while state != expected {
                    match tokio::time::timeout_at(deadline, stream.next()).await {
                        Ok(Some(Ok(change))) => match change {
                            Change::Inserted(i) | Change::Updated { new: i, .. } => {
                                state.insert(i.id, i.name);
                            }
                            Change::Deleted(i) => {
                                state.remove(&i.id);
                            }
                        },
                        _ => break,
                    }
                }
                proptest::prop_assert_eq!(state, expected);
                Ok(())
            })?;
        }
    }
}
