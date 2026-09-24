use std::collections::BTreeMap;
use std::future::Future;

use async_stream::stream;
use futures::Stream;
use itertools::{EitherOrBoth, Itertools};
use sqlx::sqlite::SqliteConnection;
use sqlx::{AssertSqlSafe, Sqlite};

use super::{
    Diffable, ObserveConfig, ObserveError, ReplayBehavior, RowAppendedEvent, RowChangedEvent,
    Tailable,
};
use crate::db::sqlite::{TransientResultCode, quote_ident};

/// The most rows one query reads, so a large backlog is delivered in bounded pages.
const PAGE_SIZE: usize = 1024;

/// The events one [`Strategy::poll`] read.
pub struct Batch<E> {
    /// The events, in the order they are yielded.
    events: Vec<E>,
    /// Whether nothing is left to read until the database changes again.
    drained: bool,
}

/// How an observation turns the observed table into events.
pub trait Strategy: Send + 'static {
    /// The item the observation yields.
    type Event: Send + 'static;

    /// Sets the position the first poll reads from, according to `replay`.
    fn seed(
        &mut self,
        conn: &mut SqliteConnection,
        replay: ReplayBehavior,
    ) -> impl Future<Output = Result<(), sqlx::Error>> + Send;

    /// Reads the next batch of events.
    ///
    /// This generally gets called whenever we are notified that there is new data in the database.
    fn poll(
        &mut self,
        conn: &mut SqliteConnection,
    ) -> impl Future<Output = Result<Batch<Self::Event>, sqlx::Error>> + Send;

    /// Tails `conn`, yielding what this strategy reports.
    fn run(
        mut self,
        mut conn: SqliteConnection,
        cfg: ObserveConfig,
    ) -> impl Stream<Item = Result<Self::Event, ObserveError>> + Send
    where
        Self: Sized,
    {
        stream! {
            let mut ticker = tokio::time::interval(cfg.poll_interval);
            let mut last: Option<i64> = None;
            'tick: loop {
                ticker.tick().await;
                let version =
                    match crate::db::query_scalar::<Sqlite, i64>("PRAGMA data_version")
                        .fetch_one(&mut conn)
                        .await
                    {
                        Ok(version) => version,
                        Err(e) => {
                            if TransientResultCode::from_error(&e).is_none() {
                                yield Err(ObserveError::Query(e));
                            }
                            continue;
                        }
                    };
                if last == Some(version) {
                    continue;
                }

                loop {
                    let batch = match self.poll(&mut conn).await {
                        Ok(batch) => batch,
                        Err(e) => {
                            if TransientResultCode::from_error(&e).is_none() {
                                yield Err(ObserveError::Query(e));
                            }
                            continue 'tick;
                        }
                    };
                    let drained = batch.drained;
                    for event in batch.events {
                        yield Ok(event);
                    }
                    if drained {
                        break;
                    }
                }
                last = Some(version);
            }
        }
    }
}

/// Tails `T`'s table by its cursor column (see [`Tailable`]).
pub struct AppendStrategy<T: Tailable> {
    /// The last delivered row: where the next page starts and, when it has an identity, the row
    /// that must still head that page.
    last: Option<T>,
}

impl<T: Tailable> AppendStrategy<T> {
    /// A tail that has delivered nothing yet.
    pub const fn new() -> Self {
        Self { last: None }
    }

    /// Records `row` as the last delivered row.
    fn remember(&mut self, row: &T) {
        self.last = Some(row.clone());
    }

    /// Whether the last delivered row still heads the page that was read from its cursor. When it
    /// is gone or has been replaced, every cursor value at or above it may have been recycled by
    /// rows a `> cursor` page would skip (see [`Tailable::identity`]), so the tail rewinds to the
    /// start of the table.
    fn verify_anchor(&mut self, page: &[T]) -> bool {
        let Some(last) = self.last.as_ref().filter(|last| last.identity().is_some()) else {
            return true;
        };

        let found = page
            .first()
            .is_some_and(|row| row.cursor() == last.cursor() && row.identity() == last.identity());

        if !found {
            tracing::warn!(
                table = T::TABLE,
                cursor = ?last.cursor(),
                "the last delivered row is gone or was replaced; rewinding to the start of the \
                 table"
            );
            self.last = None;
        }

        found
    }
}

impl<T: Tailable> Strategy for AppendStrategy<T> {
    type Event = RowAppendedEvent<T>;

    async fn seed(
        &mut self,
        conn: &mut SqliteConnection,
        replay: ReplayBehavior,
    ) -> Result<(), sqlx::Error> {
        self.last = None;
        if replay == ReplayBehavior::FromNow {
            let sql =
                T::select(&format!("ORDER BY {} DESC LIMIT 1", quote_ident(T::CURSOR_COLUMN)));
            let newest: Option<T> =
                crate::db::query_as::<Sqlite, T>(AssertSqlSafe(sql)).fetch_optional(conn).await?;
            if let Some(row) = &newest {
                self.remember(row);
            }
        }
        Ok(())
    }

    async fn poll(
        &mut self,
        conn: &mut SqliteConnection,
    ) -> Result<Batch<Self::Event>, sqlx::Error> {
        let cursor_col = quote_ident(T::CURSOR_COLUMN);
        // With an identity to check, the page starts at the last delivered row itself: one
        // statement, so the check and the rows it vouches for come from the same snapshot.
        let anchored = self.last.as_ref().is_some_and(|last| last.identity().is_some());
        let sql = match (&self.last, anchored) {
            (Some(_), true) => T::select(&format!(
                "WHERE {cursor_col} >= ?1 ORDER BY {cursor_col} ASC LIMIT {}",
                PAGE_SIZE + 1
            )),
            (Some(_), false) => T::select(&format!(
                "WHERE {cursor_col} > ?1 ORDER BY {cursor_col} ASC LIMIT {PAGE_SIZE}"
            )),
            (None, _) => T::select(&format!("ORDER BY {cursor_col} ASC LIMIT {PAGE_SIZE}")),
        };

        let query = crate::db::query_as::<Sqlite, T>(AssertSqlSafe(sql));
        let query = match &self.last {
            Some(last) => query.bind(last.cursor()),
            None => query,
        };

        let rows: Vec<T> = query.fetch_all(conn).await?;
        if !self.verify_anchor(&rows) {
            // rewound: the next poll pages from the start
            return Ok(Batch {
                events: Vec::new(),
                drained: false,
            });
        }
        let skip = usize::from(anchored);
        let drained = rows.len() < PAGE_SIZE + skip;
        if let Some(row) = rows.last() {
            self.remember(row);
        }
        Ok(Batch {
            events: rows.into_iter().skip(skip).map(RowAppendedEvent).collect(),
            drained,
        })
    }
}

/// Diffs successive snapshots of `T`'s table (see [`Diffable`]).
pub struct MutateStrategy<T: Diffable> {
    /// The table as of the last poll, by key.
    snapshot: BTreeMap<T::Key, T>,
}

impl<T: Diffable> MutateStrategy<T> {
    /// A strategy whose snapshot is still empty.
    pub const fn new() -> Self {
        Self {
            snapshot: BTreeMap::new(),
        }
    }
}

impl<T: Diffable> Strategy for MutateStrategy<T> {
    type Event = RowChangedEvent<T>;

    async fn seed(
        &mut self,
        conn: &mut SqliteConnection,
        replay: ReplayBehavior,
    ) -> Result<(), sqlx::Error> {
        self.snapshot = match replay {
            ReplayBehavior::All => BTreeMap::new(),
            ReplayBehavior::FromNow => {
                crate::db::query_as::<Sqlite, T>(AssertSqlSafe(T::select("")))
                    .fetch_all(conn)
                    .await?
                    .into_iter()
                    .map(|row| (row.key(), row))
                    .collect()
            }
        };
        Ok(())
    }

    async fn poll(
        &mut self,
        conn: &mut SqliteConnection,
    ) -> Result<Batch<Self::Event>, sqlx::Error> {
        let fresh: BTreeMap<T::Key, T> =
            crate::db::query_as::<Sqlite, T>(AssertSqlSafe(T::select("")))
                .fetch_all(conn)
                .await?
                .into_iter()
                .map(|row| (row.key(), row))
                .collect();

        let events = self
            .snapshot
            .iter()
            .merge_join_by(fresh.iter(), |(a, _), (b, _)| a.cmp(b))
            .filter_map(|joined| match joined {
                EitherOrBoth::Left((_, old)) => Some(RowChangedEvent::Deleted(old.clone())),
                EitherOrBoth::Right((_, new)) => Some(RowChangedEvent::Inserted(new.clone())),
                EitherOrBoth::Both((_, old), (_, new)) => {
                    (old != new).then(|| RowChangedEvent::Updated {
                        old: old.clone(),
                        new: new.clone(),
                    })
                }
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
    use sqlx::Connection;

    use super::*;
    use crate::db::query;
    use crate::db::sqlite::Sqlite;
    use crate::db::sqlite::observe::{
        ObserveConfig, ObserveError, ReplayBehavior, SqliteObserver, SqliteTableObserver,
        TableSchema,
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

    // A rowid table without AUTOINCREMENT (opencode's `event` shape): deleting the highest rows
    // recycles their rowids, which the never-reused `id` exposes.
    #[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow)]
    struct Event {
        rowid: i64,
        id: String,
    }
    impl TableSchema for Event {
        const TABLE: &'static str = "events";
        const COLUMNS: &'static [&'static str] = &["rowid", "id"];
    }
    impl Tailable for Event {
        type Cursor = i64;
        fn cursor(&self) -> i64 {
            self.rowid
        }
        fn identity(&self) -> Option<impl Eq> {
            Some(self.id.as_str())
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

    fn cfg(replay: ReplayBehavior) -> ObserveConfig {
        ObserveConfig::builder().replay(replay).poll_interval(Duration::from_millis(10)).build()
    }

    fn item(id: i64, name: &str) -> Item {
        Item {
            id,
            name: name.into(),
        }
    }

    async fn insert_events(path: &std::path::Path, ids: &[&str]) {
        let sql = ids.iter().map(|id| format!("INSERT INTO events (id) VALUES ('{id}');")).join("");
        exec_sql(path, &sql).await;
    }

    fn event(rowid: i64, id: &str) -> Event {
        Event {
            rowid,
            id: id.into(),
        }
    }

    /// The next `n` rows, failing instead of hanging when the tail never delivers them.
    async fn next_n<T: Tailable>(
        stream: &mut SqliteTableObserver<RowAppendedEvent<T>>,
        n: usize,
    ) -> Vec<T> {
        tokio::time::timeout(Duration::from_secs(5), stream.take(n).map(|r| r.unwrap().0).collect())
            .await
            .expect("rows were not delivered")
    }

    /// The tail's next item, row or failure, insisting that the tail is still there.
    async fn next_item<T: Tailable>(
        stream: &mut SqliteTableObserver<RowAppendedEvent<T>>,
    ) -> Result<T, ObserveError> {
        tokio::time::timeout(Duration::from_secs(5), stream.next())
            .await
            .expect("the tail yielded nothing")
            .expect("the tail ended")
            .map(|RowAppendedEvent(row)| row)
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
        let stream = observer.append::<Item>(cfg(ReplayBehavior::FromNow)).await.unwrap();

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
        let stream = observer.append::<Reserved>(cfg(ReplayBehavior::FromNow)).await.unwrap();

        exec_sql(&path, r#"INSERT INTO "transaction" ("id", "order") VALUES (1, 10), (2, 20);"#)
            .await;

        let got: Vec<i64> = stream.take(2).map(|r| r.unwrap().0.order).collect().await;
        assert_eq!(got, vec![10, 20]);
    }

    #[rstest]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn append_replay_all_crosses_page_boundary() {
        // ReplayBehavior::All drains pre-existing rows one PAGE_SIZE page at a time; a table larger than a
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
        let stream = observer.append::<Item>(cfg(ReplayBehavior::All)).await.unwrap();

        let got: Vec<i64> =
            stream.take(usize::try_from(n).unwrap()).map(|r| r.unwrap().0.id).collect().await;
        assert_eq!(got, (1..=n).collect::<Vec<_>>());
    }

    #[rstest]
    #[case::from_now(ReplayBehavior::FromNow, vec![item(3, "c")])]
    #[case::all(ReplayBehavior::All, vec![item(1, "a"), item(2, "b"), item(3, "c")])]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn replay_controls_preexisting_rows(
        #[case] replay: ReplayBehavior,
        #[case] expected: Vec<Item>,
    ) {
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
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn append_rewinds_when_delivered_rows_are_deleted_and_their_rowids_reused() {
        if !sqlite3_available() {
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("db.sqlite");
        exec_sql(&path, "CREATE TABLE events (id TEXT PRIMARY KEY);").await;
        insert_events(&path, &["e1", "e2", "e3", "e4"]).await;
        let observer = SqliteObserver::new(&path);
        let mut stream = observer.append::<Event>(cfg(ReplayBehavior::All)).await.unwrap();
        assert_eq!(next_n(&mut stream, 4).await, [
            event(1, "e1"),
            event(2, "e2"),
            event(3, "e3"),
            event(4, "e4")
        ]);

        // e5..e7 take rowids 3..5; a plain `rowid > 4` tail would deliver only e7.
        exec_sql(&path, "DELETE FROM events WHERE id IN ('e3', 'e4');").await;
        insert_events(&path, &["e5", "e6", "e7"]).await;

        assert_eq!(next_n(&mut stream, 5).await, [
            event(1, "e1"),
            event(2, "e2"),
            event(3, "e5"),
            event(4, "e6"),
            event(5, "e7")
        ]);
    }

    #[rstest]
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn append_checks_the_anchor_in_the_page_snapshot() {
        // A commit that deletes the last delivered row and recycles its rowid can land at any
        // point of a poll. Whatever the poll sees, the row on the recycled rowid must arrive no
        // later than the newer row of the same commit; when the anchor is checked in one snapshot
        // and the page read in another, the newer row alone advances the cursor past it. The
        // commit's offset from the poll-triggering insert walks the interleavings.
        async fn next(rx: &mut tokio::sync::mpsc::UnboundedReceiver<String>) -> String {
            tokio::time::timeout(Duration::from_secs(5), rx.recv())
                .await
                .expect("rows were not delivered")
                .unwrap()
        }

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("db.sqlite");
        let db = Sqlite::builder(path.as_os_str()).open().await.unwrap();
        let mut conn = db.pool().acquire().await.unwrap();
        query::<sqlx::Sqlite>("CREATE TABLE events (id TEXT PRIMARY KEY)")
            .execute(&mut *conn)
            .await
            .unwrap();
        query::<sqlx::Sqlite>("INSERT INTO events (id) VALUES ('e1'), ('e2')")
            .execute(&mut *conn)
            .await
            .unwrap();
        drop(conn);

        let observer = SqliteObserver::new(&path);
        let cfg = ObserveConfig::builder()
            .replay(ReplayBehavior::All)
            .poll_interval(Duration::from_millis(1))
            .build();
        let mut stream = observer.append::<Event>(cfg).await.unwrap();
        // the stream only polls while it is consumed, so a separate task keeps it in flight
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let consumer = tokio::spawn(async move {
            while let Some(row) = stream.next().await {
                if tx.send(row.unwrap().0.id).is_err() {
                    break;
                }
            }
        });
        assert_eq!([next(&mut rx).await, next(&mut rx).await], ["e1", "e2"]);

        let mut delivered = std::collections::HashSet::new();
        for trial in 0..400u64 {
            let mut conn = db.pool().acquire().await.unwrap();
            query::<sqlx::Sqlite>("INSERT INTO events (id) VALUES (?1)")
                .bind(format!("t{trial}_trigger"))
                .execute(&mut *conn)
                .await
                .unwrap();
            std::thread::sleep(Duration::from_micros(trial * 97 % 2000));
            // one transaction: the anchor and the trigger go, `k` takes the anchor's rowid and `w`
            // the trigger's
            let mut tx = conn.begin().await.unwrap();
            query::<sqlx::Sqlite>(
                "DELETE FROM events WHERE rowid >= (SELECT max(rowid) FROM events) - 1",
            )
            .execute(&mut *tx)
            .await
            .unwrap();
            let (k, w) = (format!("t{trial}_k"), format!("t{trial}_w"));
            query::<sqlx::Sqlite>("INSERT INTO events (id) VALUES (?1), (?2)")
                .bind(&k)
                .bind(&w)
                .execute(&mut *tx)
                .await
                .unwrap();
            tx.commit().await.unwrap();
            drop(conn);

            loop {
                let id = next(&mut rx).await;
                let done = id == w;
                delivered.insert(id);
                if done {
                    break;
                }
            }
            assert!(delivered.contains(&k), "trial {trial}: {k} was skipped");
        }
        consumer.abort();
    }

    #[rstest]
    #[case::rowid_reused(
        "DELETE FROM events WHERE id = 'e3';",
        &["e4"],
        vec![event(1, "e1"), event(2, "e2"), event(3, "e4")]
    )]
    #[case::wiped("DELETE FROM events;", &["e4", "e5"], vec![event(1, "e4"), event(2, "e5")])]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn from_now_anchors_on_the_newest_row(
        #[case] delete: &str,
        #[case] then_insert: &[&str],
        #[case] expected: Vec<Event>,
    ) {
        if !sqlite3_available() {
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("db.sqlite");
        exec_sql(&path, "CREATE TABLE events (id TEXT PRIMARY KEY);").await;
        insert_events(&path, &["e1", "e2", "e3"]).await;
        let observer = SqliteObserver::new(&path);
        let mut stream = observer.append::<Event>(cfg(ReplayBehavior::FromNow)).await.unwrap();

        exec_sql(&path, delete).await;
        insert_events(&path, then_insert).await;

        assert_eq!(next_n(&mut stream, expected.len()).await, expected);
    }

    #[rstest]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn append_without_identity_never_rewinds() {
        if !sqlite3_available() {
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("db.sqlite");
        let db = writer(dir.path()).await;
        insert(&db, 1, "a").await;
        insert(&db, 2, "b").await;
        let observer = SqliteObserver::new(&path);
        let mut stream = observer.append::<Item>(cfg(ReplayBehavior::All)).await.unwrap();
        assert_eq!(next_n(&mut stream, 2).await, [item(1, "a"), item(2, "b")]);

        delete_xproc(&path, 2).await;
        insert_xproc(&path, 3, "c").await;

        assert_eq!(next_n(&mut stream, 1).await, [item(3, "c")]);
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
        let stream = observer.append::<Item>(cfg(ReplayBehavior::FromNow)).await.unwrap();

        let writers = (0..4).map(|w| {
            let path = path.clone();
            tokio::spawn(async move {
                let inserts = (0..25)
                    .map(|i| format!("INSERT INTO items (name) VALUES ('w{w}_{i}');"))
                    .join("");
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
                let stream = observer.append::<Item>(cfg(ReplayBehavior::FromNow)).await.unwrap();

                let mut sorted: Vec<i64> = ids.iter().copied().collect();
                sorted.sort_unstable();
                let sql = sorted
                    .iter()
                    .map(|id| format!("INSERT INTO items (id, name) VALUES ({id}, 'x');"))
                    .join("");
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
        let mut stream = observer.mutate::<Item>(cfg(ReplayBehavior::FromNow)).await.unwrap();

        insert_xproc(&path, 2, "b").await;
        assert_eq!(stream.next().await.unwrap().unwrap(), RowChangedEvent::Inserted(item(2, "b")));

        update_xproc(&path, 1, "a2").await;
        assert_eq!(stream.next().await.unwrap().unwrap(), RowChangedEvent::Updated {
            old: item(1, "a"),
            new: item(1, "a2")
        });

        delete_xproc(&path, 2).await;
        assert_eq!(stream.next().await.unwrap().unwrap(), RowChangedEvent::Deleted(item(2, "b")));
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
        let mut stream = observer.mutate::<Item>(cfg(ReplayBehavior::FromNow)).await.unwrap();

        update_xproc(&path, 1, "a").await;
        insert_xproc(&path, 2, "b").await;

        assert_eq!(stream.next().await.unwrap().unwrap(), RowChangedEvent::Inserted(item(2, "b")));
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
                let mut stream = observer.mutate::<Item>(cfg(ReplayBehavior::All)).await.unwrap();

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
                            RowChangedEvent::Inserted(i) | RowChangedEvent::Updated { new: i, .. } => {
                                state.insert(i.id, i.name);
                            }
                            RowChangedEvent::Deleted(i) => {
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

        let inserts =
            (1..=20).map(|i| format!("INSERT INTO items (id, name) VALUES ({i}, 'x');")).join("");
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

    /// A query that fails for its own reason is an item, not the end of the tail: the table is
    /// dropped for longer than a poll (a reset migration between two of its statements) and the
    /// rows written once it is back are delivered all the same.
    #[rstest]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_query_error_surfaces_and_the_tail_takes_the_table_up_again() {
        if !sqlite3_available() {
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("db.sqlite");
        let _db = writer(dir.path()).await;
        let observer = SqliteObserver::new(&path);
        let mut stream = observer.append::<Item>(cfg(ReplayBehavior::FromNow)).await.unwrap();

        insert_xproc(&path, 1, "a").await;
        assert_eq!(next_n(&mut stream, 1).await, vec![item(1, "a")]);

        exec_sql(&path, "DROP TABLE items;").await;
        let surfaced = next_item(&mut stream).await.expect_err("the dropped table must surface");
        assert!(matches!(surfaced, ObserveError::Query(_)));

        exec_sql(
            &path,
            "CREATE TABLE items (id INTEGER PRIMARY KEY, name TEXT NOT NULL);
             INSERT INTO items (id, name) VALUES (2, 'b');",
        )
        .await;
        let delivered = tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                // the polls that still met no table report it; the tail retries after each
                if let Ok(row) = next_item(&mut stream).await {
                    return row;
                }
            }
        })
        .await
        .expect("the tail never took the table up again");
        assert_eq!(delivered, item(2, "b"));
    }
}
