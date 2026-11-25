//! The full-text search index over the text of desktop capture contents.

use std::collections::HashSet;
use std::path::Path;
use std::sync::Arc;

use atuin_common::db::sqlite::fts::FtsQueryExt as _;
use atuin_common::db::sqlite::{Sqlite, SqliteOpenOrCreateError};
use atuin_common::db::{self};
use atuin_common::string::StrExt as _;
use atuin_common::time::SystemTimeExt as _;
use atuin_desktop_capture::ContentHash;
use sqlx::Row as _;
use uuid::Uuid;

use super::store::{ContentKey, IndexItem, Kinds, SegmentFilter, SegmentKey, SegmentKind};

/// The version of the index's schema, which the index keeps in SQLite's `user_version`.
const VERSION: i64 = 1;

#[derive(Debug, thiserror::Error)]
pub enum IndexError {
    #[error("the desktop capture search index could not be opened: {0}")]
    Open(#[source] SqliteOpenOrCreateError),
    #[error("the desktop capture search index failed: {0}")]
    Storage(#[from] sqlx::Error),
    #[error("the desktop capture search index holds a malformed key")]
    MalformedKey,
    #[error("a reader of the desktop capture search index kept its journal from being emptied")]
    CheckpointBusy,
}

/// The most contents that one statement of [`DesktopIndex::missing`] looks up, which keeps the
/// statement's parameters well under SQLite's limit.
const LOOKUPS_PER_STATEMENT: usize = 256;

/// A full-text index over the text, application, and title of each distinct content.
///
/// The store can rebuild everything that the index holds. The index's full-text table is
/// contentless, so a search names the matching contents and the store reads their text.
///
/// Each connection zeroes the content that a deletion frees, so that [`Self::scrub`] can leave
/// the index's files without any copy of a removed content's text. A [`Self::remove`] notes in
/// its own transaction that the index owes a scrub, so that a failed scrub, or a daemon that
/// stopped before the scrub, still leads to one.
#[derive(Debug, Clone)]
pub struct DesktopIndex {
    db: Sqlite,
    /// Held by each change of the store that the index then follows, until the index has
    /// followed the change.
    following: Arc<tokio::sync::Mutex<()>>,
}

impl DesktopIndex {
    /// Opens the index at `path`, creating it or rebuilding it empty when its schema version
    /// differs.
    pub async fn open(path: &Path) -> Result<Self, IndexError> {
        let db = Sqlite::builder(path.as_os_str())
            .restrict_permissions()
            .secure_delete()
            .open()
            .await
            .map_err(IndexError::Open)?;
        let index = Self {
            db,
            following: Arc::default(),
        };
        index.migrate().await?;
        Ok(index)
    }

    async fn migrate(&self) -> Result<(), IndexError> {
        let pool = self.db.pool();
        let version: i64 = db::query_scalar("PRAGMA user_version").fetch_one(pool).await?;
        if version == VERSION {
            return Ok(());
        }

        for statement in [
            "DROP TABLE IF EXISTS scrub_owed",
            // The one row of `scrub_owed` counts the removals since the last scrub.
            "CREATE TABLE scrub_owed(id INTEGER PRIMARY KEY CHECK (id = 1), removals INTEGER NOT \
             NULL)",
            "DROP TABLE IF EXISTS text",
            "DROP TABLE IF EXISTS indexed",
            "CREATE TABLE indexed(rowid INTEGER PRIMARY KEY, segment BLOB NOT NULL, hash BLOB NOT \
             NULL, kind INTEGER NOT NULL, app_lowercase TEXT NOT NULL, started_ms INTEGER NOT \
             NULL, UNIQUE(segment, hash))",
            "DROP TABLE IF EXISTS reconciled",
            "CREATE TABLE reconciled(id INTEGER PRIMARY KEY CHECK (id = 1), record BLOB NOT NULL)",
            "CREATE VIRTUAL TABLE text USING fts5(body, app, title, content='', \
             contentless_delete=1)",
        ] {
            db::query(statement).execute(pool).await?;
        }
        db::query(sqlx::AssertSqlSafe(format!("PRAGMA user_version = {VERSION}")))
            .execute(pool)
            .await?;
        // A new file doesn't hold any text.
        if version == 0 {
            return Ok(());
        }
        // `secure_delete` zeroed the pages of the dropped tables in the journal, and the
        // checkpoint carries the zeroed pages into the main file.
        self.empty_journal().await
    }

    /// Waits until no other caller is between a change of the store and the index's update for
    /// that change, and keeps other callers out until the guard drops.
    ///
    /// Hold the guard from a read or a removal in the store until the index has followed that
    /// change.
    /// For example, an eviction removes segments from the store and then from the index, and a
    /// reconciliation reads contents from the store and then indexes them. Without the guard, a
    /// reconciliation could index a segment after an eviction had removed the segment from the
    /// index.
    pub async fn follow_store(&self) -> tokio::sync::MutexGuard<'_, ()> {
        self.following.lock().await
    }

    /// Indexes `items`, skipping each content that the index already holds.
    pub async fn add<I: IntoIterator<Item = IndexItem>>(&self, items: I) -> Result<(), IndexError> {
        let mut tx = self.db.pool().begin_with("BEGIN IMMEDIATE").await?;
        Self::insert(&mut tx, items).await?;
        tx.commit().await?;
        Ok(())
    }

    /// Indexes `items`, and then moves the reconciliation watermark to `through`, the last
    /// `Observed` record that the items cover, in one transaction.
    ///
    /// Only reconciliation moves the watermark, so that a failed [`Self::add`] of a live content
    /// cannot hide the content from later reconciliation.
    pub async fn add_reconciled<I: IntoIterator<Item = IndexItem>>(
        &self,
        items: I,
        through: Uuid,
    ) -> Result<(), IndexError> {
        let mut tx = self.db.pool().begin_with("BEGIN IMMEDIATE").await?;
        Self::insert(&mut tx, items).await?;
        db::query(
            "INSERT INTO reconciled(id, record) VALUES (1, ?) ON CONFLICT(id) DO UPDATE SET \
             record = excluded.record",
        )
        .bind(&through.as_bytes()[..])
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(())
    }

    /// Inserts the items that are new.
    async fn insert<I: IntoIterator<Item = IndexItem>>(
        tx: &mut sqlx::SqliteConnection,
        items: I,
    ) -> Result<(), IndexError> {
        for item in items {
            let rowid: Option<i64> = db::query_scalar(
                "INSERT INTO indexed(segment, hash, kind, app_lowercase, started_ms) VALUES (?, \
                 ?, ?, ?, ?) ON CONFLICT DO NOTHING RETURNING rowid",
            )
            .bind(&item.key.segment.0.as_bytes()[..])
            .bind(&item.key.hash.0[..])
            .bind(kind_bit(item.kind))
            .bind(lowercase(&item.app))
            .bind(item.started.saturating_unix_millis())
            .fetch_optional(&mut *tx)
            .await?;
            let Some(rowid) = rowid else {
                continue;
            };
            db::query("INSERT INTO text(rowid, body, app, title) VALUES (?, ?, ?, ?)")
                .bind(rowid)
                .bind(item.text)
                .bind(&*item.app)
                .bind(&*item.title)
                .execute(&mut *tx)
                .await?;
        }
        Ok(())
    }

    /// The contents of `keys` that the index lacks, in the order of `keys`.
    pub async fn missing(&self, keys: &[ContentKey]) -> Result<Vec<ContentKey>, IndexError> {
        let mut held = HashSet::new();
        for chunk in keys.chunks(LOOKUPS_PER_STATEMENT) {
            let mut statement =
                String::from("SELECT segment, hash FROM indexed WHERE (segment, hash) IN (VALUES ");
            for n in 0..chunk.len() {
                statement.push_str(if n == 0 {
                    "(?, ?)"
                } else {
                    ", (?, ?)"
                });
            }
            statement.push(')');
            let mut query = db::query(sqlx::AssertSqlSafe(statement));
            for key in chunk {
                query = query.bind(&key.segment.0.as_bytes()[..]).bind(&key.hash.0[..]);
            }
            for row in query.fetch_all(self.db.pool()).await? {
                held.insert(key_from_row(&row)?);
            }
        }
        Ok(keys.iter().filter(|key| !held.contains(key)).copied().collect())
    }

    /// Removes the rows of the contents `keys`.
    pub async fn remove_contents(&self, keys: &[ContentKey]) -> Result<(), IndexError> {
        let mut tx = self.db.pool().begin_with("BEGIN IMMEDIATE").await?;
        for key in keys {
            let segment = &key.segment.0.as_bytes()[..];
            let hash = &key.hash.0[..];
            db::query(
                "DELETE FROM text WHERE rowid IN (SELECT rowid FROM indexed WHERE segment = ? AND \
                 hash = ?)",
            )
            .bind(segment)
            .bind(hash)
            .execute(&mut *tx)
            .await?;
            db::query("DELETE FROM indexed WHERE segment = ? AND hash = ?")
                .bind(segment)
                .bind(hash)
                .execute(&mut *tx)
                .await?;
        }
        tx.commit().await?;
        Ok(())
    }

    /// Removes every content of `segments`, and notes that the index owes a
    /// [scrub](Self::scrub).
    pub async fn remove(&self, segments: &[SegmentKey]) -> Result<(), IndexError> {
        let mut tx = self.db.pool().begin_with("BEGIN IMMEDIATE").await?;
        db::query(
            "INSERT INTO scrub_owed(id, removals) VALUES (1, 1) ON CONFLICT(id) DO UPDATE SET \
             removals = removals + 1",
        )
        .execute(&mut *tx)
        .await?;
        Self::delete_segments(&mut tx, segments).await?;
        tx.commit().await?;
        Ok(())
    }

    /// Removes every content of the evicted `segments`.
    ///
    /// The removal doesn't note a scrub, because the scrub after the store's next rewrite takes
    /// the text of evicted segments out of the index's files.
    pub async fn remove_evicted(&self, segments: &[SegmentKey]) -> Result<(), IndexError> {
        let mut tx = self.db.pool().begin_with("BEGIN IMMEDIATE").await?;
        Self::delete_segments(&mut tx, segments).await?;
        tx.commit().await?;
        Ok(())
    }

    async fn delete_segments(
        tx: &mut sqlx::SqliteConnection,
        segments: &[SegmentKey],
    ) -> Result<(), IndexError> {
        for segment in segments {
            let segment = &segment.0.as_bytes()[..];
            db::query(
                "DELETE FROM text WHERE rowid IN (SELECT rowid FROM indexed WHERE segment = ?)",
            )
            .bind(segment)
            .execute(&mut *tx)
            .await?;
            db::query("DELETE FROM indexed WHERE segment = ?")
                .bind(segment)
                .execute(&mut *tx)
                .await?;
        }
        Ok(())
    }

    /// Removes every trace of the removed contents from the index's files. The files keep their
    /// size, and later writes reuse the freed pages.
    ///
    /// The call merges the full-text index and then empties the index's journal, so it takes
    /// time in proportion to the size of the index. When a write to the index fails because the
    /// merge holds the database, the reconciler's next pass indexes the write's contents. A scrub
    /// that succeeds settles the scrubs that the index owed before the scrub began.
    pub async fn scrub(&self) -> Result<(), IndexError> {
        let pool = self.db.pool();
        let removals = self.owed_removals().await?;
        // A removal from a contentless table leaves the row's tokens in the full-text segments
        // until a merge drops them.
        db::query("INSERT INTO text(text) VALUES('optimize')").execute(pool).await?;
        // The journal keeps the pages that held the text, from before the merge zeroed them.
        self.empty_journal().await?;
        // A removal during the scrub counts one more, so its note stays.
        if let Some(removals) = removals {
            db::query("DELETE FROM scrub_owed WHERE id = 1 AND removals = ?")
                .bind(removals)
                .execute(pool)
                .await?;
        }
        Ok(())
    }

    /// Whether a removal is waiting for a [scrub](Self::scrub).
    pub async fn owes_scrub(&self) -> Result<bool, IndexError> {
        Ok(self.owed_removals().await?.is_some())
    }

    /// How many removals the owed scrub follows, or `None` when the index doesn't owe a scrub.
    async fn owed_removals(&self) -> Result<Option<i64>, IndexError> {
        Ok(db::query_scalar("SELECT removals FROM scrub_owed WHERE id = 1")
            .fetch_optional(self.db.pool())
            .await?)
    }

    /// Every segment that the index holds a content of.
    pub async fn indexed_segments(&self) -> Result<Vec<SegmentKey>, IndexError> {
        let rows: Vec<Vec<u8>> = db::query_scalar("SELECT DISTINCT segment FROM indexed")
            .fetch_all(self.db.pool())
            .await?;
        rows.iter()
            .map(|bytes| {
                let segment = Uuid::from_slice(bytes).map_err(|_| IndexError::MalformedKey)?;
                Ok(SegmentKey(segment))
            })
            .collect()
    }

    /// Moves the journal's pages into the main file and truncates the journal to nothing.
    async fn empty_journal(&self) -> Result<(), IndexError> {
        let busy: i64 =
            db::query_scalar("PRAGMA wal_checkpoint(TRUNCATE)").fetch_one(self.db.pool()).await?;
        if busy != 0 {
            return Err(IndexError::CheckpointBusy);
        }
        Ok(())
    }

    /// The best content of each of the best `limit` segments whose contents hold every word of
    /// `query` in their text, application, or title.
    ///
    /// The search applies the application and the kinds of `filter`, and leaves out the segments
    /// that started after the filter's range. The index cannot tell when a segment ended, so the
    /// hits can include segments that ended before the range.
    pub async fn search(
        &self,
        query: &str,
        filter: &SegmentFilter,
        limit: u32,
    ) -> Result<Vec<ContentKey>, IndexError> {
        // The newest of two contents that rank the same comes first.
        let statement = db::query(
            "SELECT segment, hash FROM (SELECT segment, hash, rank, rowid, row_number() OVER \
             (PARTITION BY segment ORDER BY rank, rowid DESC) AS nth FROM (SELECT \
             indexed.segment, indexed.hash, bm25(text) AS rank, indexed.rowid AS rowid FROM text \
             JOIN indexed ON indexed.rowid = text.rowid WHERE text MATCH ? AND (? & indexed.kind) \
             != 0 AND (? IS NULL OR indexed.app_lowercase = ?) AND indexed.started_ms <= ?)) \
             WHERE nth = 1 ORDER BY rank, rowid DESC LIMIT ?",
        );
        let Some(statement) = statement.bind_match_query(query.split_whitespace()) else {
            return Ok(Vec::new());
        };
        let app = filter.app.as_deref().map(lowercase);
        let rows = statement
            .bind(kinds_mask(&filter.kinds))
            .bind(app.as_deref())
            .bind(app.as_deref())
            .bind(filter.to.saturating_unix_millis())
            .bind(i64::from(limit))
            .fetch_all(self.db.pool())
            .await?;
        rows.iter().map(key_from_row).collect()
    }

    /// The last `Observed` record that a reconciliation indexed.
    pub async fn reconciled_through(&self) -> Result<Option<Uuid>, IndexError> {
        let record: Option<Vec<u8>> =
            db::query_scalar("SELECT record FROM reconciled WHERE id = 1")
                .fetch_optional(self.db.pool())
                .await?;
        record
            .map(|record| Uuid::from_slice(&record).map_err(|_| IndexError::MalformedKey))
            .transpose()
    }
}

/// The form of an application's name that the index compares, which matches names as
/// [`SegmentFilter::matches_app`] does.
fn lowercase(app: &str) -> String {
    app.lowercase_chars().collect()
}

/// The bit of a segment kind in a mask of kinds.
const fn kind_bit(kind: SegmentKind) -> i64 {
    match kind {
        SegmentKind::Active => 1,
        SegmentKind::Dialog => 2,
        SegmentKind::Background => 4,
        SegmentKind::Notification => 8,
    }
}

/// The mask of the bits of `kinds`.
fn kinds_mask(kinds: &Kinds) -> i64 {
    match kinds {
        Kinds::All => -1,
        Kinds::Only(kinds) => kinds.iter().fold(0, |mask, &kind| mask | kind_bit(kind)),
    }
}

fn key_from_row(row: &sqlx::sqlite::SqliteRow) -> Result<ContentKey, IndexError> {
    let segment: &[u8] = row.try_get("segment")?;
    let hash: &[u8] = row.try_get("hash")?;
    Ok(ContentKey {
        segment: SegmentKey(Uuid::from_slice(segment).map_err(|_| IndexError::MalformedKey)?),
        hash: ContentHash(hash.try_into().map_err(|_| IndexError::MalformedKey)?),
    })
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::time::{Duration, SystemTime};

    use rstest::rstest;
    use tempfile::TempDir;

    use super::*;
    use crate::desktop_capture::testing::records::everything;
    use crate::desktop_capture::testing::{
        OpenedIndex, dir, files_holding, indexed_keys as found, opened_index,
    };

    fn item(n: u8, app: &str, title: &str, text: &str) -> IndexItem {
        IndexItem {
            key: ContentKey {
                segment: SegmentKey(Uuid::from_u128(n.into())),
                hash: ContentHash([n; 32]),
            },
            kind: SegmentKind::Active,
            app: Arc::from(app),
            title: Arc::from(title),
            started: SystemTime::UNIX_EPOCH + Duration::from_secs(n.into()),
            text: text.to_owned(),
        }
    }

    fn record(n: u128) -> Uuid {
        Uuid::from_u128(n)
    }

    /// The index lacks the contents that it never held and the contents that it removed, and
    /// lists them in the order that it was asked.
    #[rstest]
    #[tokio::test]
    async fn missing_lists_the_contents_that_the_index_lacks(
        #[future(awt)] opened_index: OpenedIndex,
    ) {
        let index = &opened_index.index;
        let held: Vec<_> = (0..20).map(|n| item(n, "kate", "a", "word")).collect();
        index.add(held.iter().cloned()).await.expect("add");
        let removed = held[3].key;
        index.remove_contents(&[removed]).await.expect("remove");
        let absent = |n| item(n, "kate", "a", "word").key;
        // More keys than one statement looks up.
        let asked: Vec<_> = (100..=255)
            .map(absent)
            .chain(held.iter().map(|item| item.key))
            .chain((0..200).map(|n| ContentKey {
                segment: absent(n).segment,
                hash: ContentHash([n.wrapping_add(1); 32]),
            }))
            .collect();

        let missing = index.missing(&asked).await.expect("missing");

        let expected: Vec<_> = asked
            .iter()
            .filter(|key| *key == &removed || !held.iter().any(|item| item.key == **key))
            .copied()
            .collect();
        assert_eq!(missing, expected);
        assert!(missing.contains(&removed));
        assert_eq!(index.missing(&[]).await.expect("missing"), []);
    }

    /// Removing a content keeps the other contents of its segment.
    #[rstest]
    #[tokio::test]
    async fn removing_a_content_keeps_the_rest_of_its_segment(
        #[future(awt)] opened_index: OpenedIndex,
    ) {
        let index = &opened_index.index;
        let gone = item(1, "kate", "a", "needle one");
        let kept = IndexItem {
            key: ContentKey {
                segment: gone.key.segment,
                hash: ContentHash([9; 32]),
            },
            ..item(1, "kate", "a", "needle two")
        };
        index.add([gone.clone(), kept.clone()]).await.expect("add");

        index.remove_contents(&[gone.key]).await.expect("remove");

        assert_eq!(found(index, "needle").await, [kept.key]);
        let rows: i64 = db::query_scalar("SELECT count(*) FROM text")
            .fetch_one(index.db.pool())
            .await
            .expect("count");
        assert_eq!(rows, 1);
    }

    #[rstest]
    #[tokio::test]
    async fn search_finds_text_app_and_title(#[future(awt)] opened_index: OpenedIndex) {
        let index = &opened_index.index;
        let one = item(1, "firefox", "Release notes", "the quarterly forecast");
        index.add([one.clone()]).await.expect("add");

        assert_eq!(found(index, "forecast").await, [one.key]);
        assert_eq!(found(index, "firefox").await, [one.key]);
        assert_eq!(found(index, "release").await, [one.key]);
        assert_eq!(found(index, "quarterly forecast").await, [one.key]);
        assert!(found(index, "absent").await.is_empty());
        assert!(found(index, "   ").await.is_empty());
        for query in ["\"unbalanced", "a OR", "path:", "(", "*", "-x"] {
            index
                .search(query, &everything(), 10)
                .await
                .unwrap_or_else(|err| panic!("{query:?}: {err}"));
        }
    }

    /// A search names each segment once, by its best content, and keeps the order of the
    /// segments' best contents.
    #[rstest]
    #[tokio::test]
    async fn search_names_each_segment_once(#[future(awt)] opened_index: OpenedIndex) {
        let index = &opened_index.index;
        let in_segment = |segment: u8, n: u8, text: &str| IndexItem {
            key: ContentKey {
                segment: SegmentKey(Uuid::from_u128(segment.into())),
                hash: ContentHash([n; 32]),
            },
            ..item(n, "app", "title", text)
        };
        let weak = in_segment(1, 1, "needle in a long haystack of many other words here");
        let strong = in_segment(1, 2, "needle needle");
        let other = in_segment(2, 3, "a needle among words");
        index.add([weak, strong.clone(), other.clone()]).await.expect("add");

        assert_eq!(found(index, "needle").await, [strong.key, other.key]);
        let one = index.search("needle", &everything(), 1).await.expect("search");
        assert_eq!(one, [strong.key]);
    }

    /// A search keeps the segments of the filter's application and kinds, and leaves out the
    /// segments that started after the filter's range.
    #[rstest]
    #[case::everything(None, Kinds::All, 100, &[1, 2, 3, 4])]
    #[case::app(Some("kate"), Kinds::All, 100, &[2, 3])]
    #[case::app_in_any_case(Some("KaTe"), Kinds::All, 100, &[2, 3])]
    #[case::non_ascii_app_in_any_case(Some("éCRIRE"), Kinds::All, 100, &[4])]
    #[case::kind(None, Kinds::Only(vec![SegmentKind::Dialog]), 100, &[3])]
    #[case::kinds(None, Kinds::Only(vec![SegmentKind::Active, SegmentKind::Dialog]), 100, &[1, 3, 4])]
    #[case::started_by(None, Kinds::All, 2, &[1, 2])]
    #[case::none(Some("gedit"), Kinds::Only(vec![SegmentKind::Background]), 100, &[])]
    #[tokio::test]
    async fn search_applies_the_filter(
        #[future(awt)] opened_index: OpenedIndex,
        #[case] app: Option<&str>,
        #[case] kinds: Kinds,
        #[case] to_secs: u64,
        #[case] expected: &[u8],
    ) {
        let index = &opened_index.index;
        let with_kind = |n, app, kind| IndexItem {
            kind,
            ..item(n, app, "title", "needle")
        };
        let items = [
            with_kind(1, "gedit", SegmentKind::Active),
            with_kind(2, "kate", SegmentKind::Background),
            with_kind(3, "kate", SegmentKind::Dialog),
            with_kind(4, "Écrire", SegmentKind::Active),
        ];
        index.add(items.clone()).await.expect("add");
        let filter = SegmentFilter {
            from: SystemTime::UNIX_EPOCH,
            to: SystemTime::UNIX_EPOCH + Duration::from_secs(to_secs),
            app: app.map(str::to_owned),
            kinds,
        };

        let hits = index.search("needle", &filter, 10).await.expect("search");

        let mut found = hits;
        found.sort_by_key(|key| key.segment);
        let wanted: Vec<_> = expected.iter().map(|&n| items[usize::from(n) - 1].key).collect();
        assert_eq!(found, wanted);
    }

    #[rstest]
    #[tokio::test]
    async fn adding_a_content_twice_keeps_one_row(#[future(awt)] opened_index: OpenedIndex) {
        let index = &opened_index.index;
        let one = item(1, "kate", "a", "needle");
        index.add([one.clone(), one.clone()]).await.expect("add");
        index.add([one.clone()]).await.expect("add again");

        assert_eq!(found(index, "needle").await, [one.key]);
    }

    /// A removed segment doesn't leave any row behind, so that its contents can be indexed
    /// again.
    #[rstest]
    #[tokio::test]
    async fn removing_a_segment_removes_its_contents(#[future(awt)] opened_index: OpenedIndex) {
        let index = &opened_index.index;
        let gone = item(1, "kate", "a", "needle one");
        let also_gone = IndexItem {
            key: ContentKey {
                segment: gone.key.segment,
                hash: ContentHash([9; 32]),
            },
            ..item(1, "kate", "a", "needle two")
        };
        let kept = item(2, "kate", "b", "needle three");
        index.add([gone.clone(), also_gone, kept.clone()]).await.expect("add");

        index.remove(&[gone.key.segment]).await.expect("remove");
        assert_eq!(found(index, "needle").await, [kept.key]);
        let rows: i64 = db::query_scalar("SELECT count(*) FROM text")
            .fetch_one(index.db.pool())
            .await
            .expect("count");
        assert_eq!(rows, 1);

        index.add([gone.clone()]).await.expect("add again");
        assert_eq!(found(index, "one").await, [gone.key]);
    }

    /// A removal owes a scrub until a scrub succeeds, also after the index opens again. The
    /// removal of evicted segments leaves the scrub to the store's next rewrite.
    #[rstest]
    #[tokio::test]
    async fn a_removal_owes_a_scrub_until_a_scrub_succeeds(dir: TempDir) {
        let path = dir.path().join("index.sqlite");
        let index = DesktopIndex::open(&path).await.expect("open");
        let evicted = item(1, "kate", "a", "first");
        let deleted = item(2, "kate", "b", "second");
        index.add([evicted.clone(), deleted.clone()]).await.expect("add");
        assert!(!index.owes_scrub().await.expect("read"));

        index.remove_evicted(&[evicted.key.segment]).await.expect("remove");
        assert!(!index.owes_scrub().await.expect("read"));
        index.remove(&[deleted.key.segment]).await.expect("remove");
        assert!(index.owes_scrub().await.expect("read"));
        index.db.close().await;
        let index = DesktopIndex::open(&path).await.expect("open again");
        assert!(index.owes_scrub().await.expect("read"));

        index.scrub().await.expect("scrub");
        assert!(!index.owes_scrub().await.expect("read"));
    }

    #[rstest]
    #[tokio::test]
    async fn a_scrub_after_a_removal_keeps_the_rest_searchable(
        #[future(awt)] opened_index: OpenedIndex,
    ) {
        let index = &opened_index.index;
        let first = item(1, "kate", "a", "zebra");
        let second = item(2, "kate", "b", "yak");
        index.add([first.clone(), second.clone()]).await.expect("add");

        index.remove(&[first.key.segment]).await.expect("remove");
        index.scrub().await.expect("scrub");

        assert_eq!(found(index, "zebra").await, []);
        assert_eq!(found(index, "yak").await, [second.key]);
        assert_eq!(index.indexed_segments().await.expect("indexed segments"), [second.key.segment]);
    }

    /// The words of a secret that the index's files must lose. The full-text segments hold
    /// each word on its own.
    const SECRET_WORDS: [&str; 3] = ["zanzibar", "quokka", "rhythmbox"];

    /// A word of [`SECRET_WORDS`], and the files that hold the word.
    #[derive(Debug, PartialEq, Eq)]
    struct Trace {
        word: &'static str,
        files: Vec<std::path::PathBuf>,
    }

    fn files_holding_secret_words(dir: &Path) -> Vec<Trace> {
        SECRET_WORDS
            .into_iter()
            .map(|word| Trace {
                word,
                files: files_holding(dir, word),
            })
            .filter(|trace| !trace.files.is_empty())
            .collect()
    }

    /// What a test removes from the index.
    #[derive(Debug, Clone, Copy)]
    enum Removal {
        Segment,
        Content,
    }

    /// A scrub after a removal doesn't leave any word of the removed content in the index's main
    /// file, journal, or shared memory file, also among many other contents.
    #[rstest]
    #[tokio::test]
    async fn a_scrub_leaves_no_trace_in_the_index_files(
        #[future(awt)] opened_index: OpenedIndex,
        #[values(Removal::Segment, Removal::Content)] removal: Removal,
    ) {
        let index = &opened_index.index;
        let filler = (10..200).map(|n| {
            let text: Vec<_> =
                (0..200).map(|word| format!("w{}", u32::from(n) * 200 + word)).collect();
            item(n, "kate", "notes", &text.join(" "))
        });
        index.add(filler).await.expect("add");
        let secret = item(1, "rhythmbox", "zanzibar", "quokka zanzibar");
        index.add([secret.clone()]).await.expect("add");
        let dir = opened_index.dir.path();
        assert_eq!(files_holding_secret_words(dir).len(), SECRET_WORDS.len());

        match removal {
            Removal::Segment => index.remove(&[secret.key.segment]).await,
            Removal::Content => index.remove_contents(&[secret.key]).await,
        }
        .expect("remove");
        index.scrub().await.expect("scrub");

        assert_eq!(files_holding_secret_words(dir), []);
        assert_eq!(found(index, "w10000").await.len(), 1);
    }

    /// A rebuild for a different schema version doesn't leave any of the old index's text in
    /// the index's files.
    #[rstest]
    #[tokio::test]
    async fn a_rebuild_leaves_no_text_of_the_old_index(#[future(awt)] opened_index: OpenedIndex) {
        let OpenedIndex { dir, index } = opened_index;
        let secret = item(1, "rhythmbox", "zanzibar", "quokka zanzibar");
        index.add([secret]).await.expect("add");
        db::query(sqlx::AssertSqlSafe(format!("PRAGMA user_version = {}", VERSION + 1)))
            .execute(index.db.pool())
            .await
            .expect("set the version");
        index.db.close().await;
        drop(index);
        assert_eq!(files_holding_secret_words(dir.path()).len(), SECRET_WORDS.len());

        let _reopened = DesktopIndex::open(&dir.path().join("index.sqlite")).await.expect("open");

        assert_eq!(files_holding_secret_words(dir.path()), []);
    }

    #[rstest]
    #[tokio::test]
    async fn only_reconciliation_moves_the_watermark(#[future(awt)] opened_index: OpenedIndex) {
        let index = &opened_index.index;
        assert_eq!(index.reconciled_through().await.expect("watermark"), None);
        index.add([item(1, "a", "a", "one")]).await.expect("add");
        assert_eq!(index.reconciled_through().await.expect("watermark"), None);

        let added = [item(2, "a", "a", "two"), item(3, "a", "a", "three")];
        index.add_reconciled(added, record(3)).await.expect("add");
        assert_eq!(index.reconciled_through().await.expect("watermark"), Some(record(3)));

        index.add([item(9, "a", "a", "nine")]).await.expect("add");
        assert_eq!(index.reconciled_through().await.expect("watermark"), Some(record(3)));
        index.add_reconciled([], record(4)).await.expect("add nothing");
        assert_eq!(index.reconciled_through().await.expect("watermark"), Some(record(4)));
    }

    #[rstest]
    #[tokio::test]
    async fn a_new_schema_version_rebuilds(#[future(awt)] opened_index: OpenedIndex) {
        let path = opened_index.dir.path().join("index.sqlite");
        opened_index
            .index
            .add_reconciled([item(1, "a", "b", "kept")], record(1))
            .await
            .expect("add");
        db::query("PRAGMA user_version = 0")
            .execute(opened_index.index.db.pool())
            .await
            .expect("set the version");

        let reopened = DesktopIndex::open(&path).await.expect("reopen");
        assert!(found(&reopened, "kept").await.is_empty());
        assert_eq!(reopened.reconciled_through().await.expect("watermark"), None);
        let version: i64 =
            db::query_scalar("PRAGMA user_version").fetch_one(reopened.db.pool()).await.unwrap();
        assert_eq!(version, VERSION);

        reopened.add([item(2, "a", "b", "fresh")]).await.expect("add");
        let again = DesktopIndex::open(&path).await.expect("open at the same version");
        assert_eq!(found(&again, "fresh").await.len(), 1);
    }
}
