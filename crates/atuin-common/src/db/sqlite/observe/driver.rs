use std::collections::BTreeMap;
use std::future::Future;

use async_stream::try_stream;
use futures::Stream;
use itertools::{EitherOrBoth, Itertools};
use sqlx::sqlite::{SqliteConnectOptions, SqliteConnection};
use sqlx::{AssertSqlSafe, Connection, Sqlite};

use super::event::{Appended, Change};
use super::schema::{Diffable, TableSchema, Tailable};
use super::{ObserveConfig, ObserveError, Replay};

const PAGE_SIZE: usize = 1024;

fn quote_ident(ident: &str) -> String {
    format!("\"{}\"", ident.replace('"', "\"\""))
}

pub(super) struct Batch<E> {
    events: Vec<E>,
    drained: bool,
}

pub(super) trait Strategy: Send + 'static {
    type Event: Send + 'static;

    fn seed(
        &mut self,
        conn: &mut SqliteConnection,
        replay: Replay,
    ) -> impl Future<Output = Result<(), sqlx::Error>> + Send;

    fn poll(
        &mut self,
        conn: &mut SqliteConnection,
    ) -> impl Future<Output = Result<Batch<Self::Event>, sqlx::Error>> + Send;
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
    ) -> Result<(), sqlx::Error> {
        self.cursor = match replay {
            Replay::All => None,
            Replay::FromNow => {
                let sql = format!(
                    "SELECT max({}) FROM {}",
                    quote_ident(T::CURSOR_COLUMN),
                    quote_ident(T::TABLE)
                );
                crate::db::query_scalar::<Sqlite, Option<T::Cursor>>(AssertSqlSafe(sql))
                    .fetch_one(conn)
                    .await?
            }
        };
        Ok(())
    }

    async fn poll(
        &mut self,
        conn: &mut SqliteConnection,
    ) -> Result<Batch<Self::Event>, sqlx::Error> {
        let cols = T::COLUMNS.iter().map(|&c| quote_ident(c)).collect::<Vec<_>>().join(", ");
        let table = quote_ident(T::TABLE);
        let cursor_col = quote_ident(T::CURSOR_COLUMN);
        let sql = match self.cursor {
            Some(_) => format!(
                "SELECT {cols} FROM {table} WHERE {cursor_col} > ?1 ORDER BY {cursor_col} ASC \
                 LIMIT {PAGE_SIZE}"
            ),
            None => {
                format!("SELECT {cols} FROM {table} ORDER BY {cursor_col} ASC LIMIT {PAGE_SIZE}")
            }
        };

        let query = crate::db::query_as::<Sqlite, T>(AssertSqlSafe(sql));
        let query = match &self.cursor {
            Some(cursor) => query.bind(cursor.clone()),
            None => query,
        };

        let rows: Vec<T> = query.fetch_all(conn).await?;
        let drained = rows.len() < PAGE_SIZE;
        if let Some(row) = rows.last() {
            self.cursor = Some(row.cursor());
        }
        Ok(Batch {
            events: rows.into_iter().map(Appended).collect(),
            drained,
        })
    }
}

fn is_transient(e: &sqlx::Error) -> bool {
    matches!(e, sqlx::Error::Io(_))
        || e.as_database_error()
            .and_then(|db| db.code())
            .is_some_and(|code| crate::db::sqlite::TransientResultCode::from_code(&code).is_some())
}

pub(super) fn run<S: Strategy>(
    opts: SqliteConnectOptions,
    first: SqliteConnection,
    mut strategy: S,
    cfg: ObserveConfig,
) -> impl Stream<Item = Result<S::Event, ObserveError>> + Send {
    try_stream! {
        let mut pending = Some(first);
        loop {
            let mut conn = match pending.take() {
                Some(conn) => conn,
                None => {
                    let connected = cfg
                        .reconnect
                        .retry_forever(|| async {
                            match SqliteConnection::connect_with(&opts).await {
                                Ok(conn) => std::ops::ControlFlow::Break(Ok(conn)),
                                Err(e) if is_transient(&e) => std::ops::ControlFlow::Continue(()),
                                Err(e) => std::ops::ControlFlow::Break(Err(e)),
                            }
                        })
                        .await;
                    match connected {
                        Ok(conn) => conn,
                        Err(e) => Err(ObserveError::Connect(e))?,
                    }
                }
            };

            let mut ticker = tokio::time::interval(cfg.poll_interval);
            let mut last: Option<i64> = None;
            'gate: loop {
                ticker.tick().await;

                let version = match crate::db::query_scalar::<Sqlite, i64>("PRAGMA data_version")
                    .fetch_one(&mut conn)
                    .await
                {
                    Ok(version) => version,
                    Err(e) if is_transient(&e) => break 'gate,
                    Err(e) => Err(ObserveError::Query(e))?,
                };
                if last == Some(version) {
                    continue;
                }

                loop {
                    let batch = match strategy.poll(&mut conn).await {
                        Ok(batch) => batch,
                        Err(e) if is_transient(&e) => break 'gate,
                        Err(e) => Err(ObserveError::Query(e))?,
                    };
                    let drained = batch.drained;
                    for event in batch.events {
                        yield event;
                    }
                    if drained {
                        break;
                    }
                }
                last = Some(version);
            }

            tokio::time::sleep(cfg.poll_interval).await;
        }
    }
}

async fn fetch_all<T: TableSchema>(conn: &mut SqliteConnection) -> Result<Vec<T>, sqlx::Error> {
    let cols = T::COLUMNS.iter().map(|&c| quote_ident(c)).collect::<Vec<_>>().join(", ");
    let sql = format!("SELECT {cols} FROM {}", quote_ident(T::TABLE));
    crate::db::query_as::<Sqlite, T>(AssertSqlSafe(sql)).fetch_all(conn).await
}

pub(super) struct MutateStrategy<T: Diffable> {
    snapshot: BTreeMap<T::Key, T>,
}

impl<T: Diffable> MutateStrategy<T> {
    pub(super) fn new() -> Self {
        Self {
            snapshot: BTreeMap::new(),
        }
    }
}

impl<T: Diffable> Strategy for MutateStrategy<T> {
    type Event = Change<T>;

    async fn seed(
        &mut self,
        conn: &mut SqliteConnection,
        replay: Replay,
    ) -> Result<(), sqlx::Error> {
        self.snapshot = match replay {
            Replay::All => BTreeMap::new(),
            Replay::FromNow => {
                fetch_all::<T>(conn).await?.into_iter().map(|row| (row.key(), row)).collect()
            }
        };
        Ok(())
    }

    async fn poll(
        &mut self,
        conn: &mut SqliteConnection,
    ) -> Result<Batch<Self::Event>, sqlx::Error> {
        let fresh: BTreeMap<T::Key, T> =
            fetch_all::<T>(conn).await?.into_iter().map(|row| (row.key(), row)).collect();

        let events = self
            .snapshot
            .iter()
            .merge_join_by(fresh.iter(), |(a, _), (b, _)| a.cmp(b))
            .filter_map(|joined| match joined {
                EitherOrBoth::Left((_, old)) => Some(Change::Deleted(old.clone())),
                EitherOrBoth::Right((_, new)) => Some(Change::Inserted(new.clone())),
                EitherOrBoth::Both((_, old), (_, new)) => (old != new).then(|| Change::Updated {
                    old: old.clone(),
                    new: new.clone(),
                }),
            })
            .collect();

        self.snapshot = fresh;
        Ok(Batch {
            events,
            drained: true,
        })
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
    use crate::db::sqlite::observe::{
        ObserveConfig, ObserveError, Replay, SqliteObserver, TableSchema,
    };

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

    // A schema whose table and cursor column are SQLite keywords, exercising identifier quoting.
    #[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow)]
    struct Reserved {
        id: i64,
        order: i64,
    }
    impl TableSchema for Reserved {
        const TABLE: &'static str = "transaction";
        const COLUMNS: &'static [&'static str] = &["id", "order"];
    }
    impl Tailable for Reserved {
        type Cursor = i64;
        const CURSOR_COLUMN: &'static str = "order";
        fn cursor(&self) -> i64 {
            self.order
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
            .arg(format!("PRAGMA busy_timeout=10000; {sql}"))
            .stdout(std::process::Stdio::null())
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
        ObserveConfig::builder().replay(replay).poll_interval(Duration::from_millis(10)).build()
    }

    fn item(id: i64, name: &str) -> Item {
        Item {
            id,
            name: name.into(),
        }
    }

    fn sqlite3_available() -> bool {
        std::process::Command::new("sqlite3")
            .arg("-version")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .is_ok()
    }

    #[rstest]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn append_tail_emits_new_rows_in_order() {
        if !sqlite3_available() {
            return;
        }
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
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn append_tails_reserved_word_identifiers() {
        if !sqlite3_available() {
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("db.sqlite");
        let db = Sqlite::builder(path.as_os_str()).open().await.unwrap();
        query::<sqlx::Sqlite>(
            r#"CREATE TABLE "transaction" ("id" INTEGER PRIMARY KEY, "order" INTEGER NOT NULL)"#,
        )
        .execute(&mut *db.pool().acquire().await.unwrap())
        .await
        .unwrap();

        let observer = SqliteObserver::new(&path);
        let stream = observer.append::<Reserved>(cfg(Replay::FromNow)).await.unwrap();

        exec_sql(&path, r#"INSERT INTO "transaction" ("id", "order") VALUES (1, 10), (2, 20);"#)
            .await;

        let got: Vec<i64> = stream.take(2).map(|r| r.unwrap().0.order).collect().await;
        assert_eq!(got, vec![10, 20]);
    }

    #[rstest]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn append_replay_all_crosses_page_boundary() {
        // Replay::All drains pre-existing rows one PAGE_SIZE page at a time; a table larger than a
        // page must still emit every row across the boundary.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("db.sqlite");
        let db = writer(dir.path()).await;
        let n = i64::try_from(PAGE_SIZE).unwrap() + 76;
        query::<sqlx::Sqlite>(
            "INSERT INTO items (id, name) WITH RECURSIVE seq(n) AS (SELECT 1 UNION ALL SELECT n + \
             1 FROM seq WHERE n < ?1) SELECT n, 'x' FROM seq",
        )
        .bind(n)
        .execute(&mut *db.pool().acquire().await.unwrap())
        .await
        .unwrap();

        let observer = SqliteObserver::new(&path);
        let stream = observer.append::<Item>(cfg(Replay::All)).await.unwrap();

        let got: Vec<i64> =
            stream.take(usize::try_from(n).unwrap()).map(|r| r.unwrap().0.id).collect().await;
        assert_eq!(got, (1..=n).collect::<Vec<_>>());
    }

    #[rstest]
    #[case::from_now(Replay::FromNow, vec![item(3, "c")])]
    #[case::all(Replay::All, vec![item(1, "a"), item(2, "b"), item(3, "c")])]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn replay_controls_preexisting_rows(#[case] replay: Replay, #[case] expected: Vec<Item>) {
        if !sqlite3_available() {
            return;
        }
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
        if !sqlite3_available() {
            return;
        }
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
        assert_eq!(got, (1..=100).collect::<Vec<_>>());
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
                if !sqlite3_available() {
                    return Ok(());
                }
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
        if !sqlite3_available() {
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("db.sqlite");
        let db = writer(dir.path()).await;
        insert(&db, 1, "a").await;

        let observer = SqliteObserver::new(&path);
        let mut stream = observer.mutate::<Item>(cfg(Replay::FromNow)).await.unwrap();

        insert_xproc(&path, 2, "b").await;
        assert_eq!(stream.next().await.unwrap().unwrap(), Change::Inserted(item(2, "b")));

        update_xproc(&path, 1, "a2").await;
        assert_eq!(stream.next().await.unwrap().unwrap(), Change::Updated {
            old: item(1, "a"),
            new: item(1, "a2")
        });

        delete_xproc(&path, 2).await;
        assert_eq!(stream.next().await.unwrap().unwrap(), Change::Deleted(item(2, "b")));
    }

    #[rstest]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn mutate_ignores_value_preserving_write() {
        if !sqlite3_available() {
            return;
        }
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
                if !sqlite3_available() {
                    return Ok(());
                }
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

    #[rstest]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn slow_consumer_loses_no_rows() {
        if !sqlite3_available() {
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("db.sqlite");
        let _db = writer(dir.path()).await;
        let observer = SqliteObserver::new(&path);
        let cfg = ObserveConfig::builder().poll_interval(Duration::from_millis(10)).build();
        let stream = observer.append::<Item>(cfg).await.unwrap();

        let inserts: String =
            (1..=20).map(|i| format!("INSERT INTO items (id, name) VALUES ({i}, 'x');")).collect();
        exec_sql(&path, &inserts).await;

        let got: Vec<i64> = stream
            .take(20)
            .then(|r| async move {
                tokio::time::sleep(Duration::from_millis(5)).await;
                r.unwrap().0.id
            })
            .collect()
            .await;
        assert_eq!(got, (1..=20).collect::<Vec<_>>());
    }

    #[rstest]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn terminal_query_error_surfaces() {
        if !sqlite3_available() {
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("db.sqlite");
        let _db = writer(dir.path()).await;
        let observer = SqliteObserver::new(&path);
        let mut stream = observer.append::<Item>(cfg(Replay::FromNow)).await.unwrap();

        exec_sql(&path, "DROP TABLE items;").await;

        let surfaced = tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                match stream.next().await {
                    Some(Err(e)) => return Some(e),
                    Some(Ok(_)) => {}
                    None => return None,
                }
            }
        })
        .await
        .expect("observer must surface a terminal error rather than hang");
        assert!(matches!(surfaced, Some(ObserveError::Query(_))));
    }
}
