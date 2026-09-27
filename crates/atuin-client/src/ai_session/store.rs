use atuin_common::encryption::paseto_v4::{EncryptedData, Key};
use atuin_domain::record::{
    DecryptedData, Host, HostId, Record, RecordId, RecordIdx, RecordSeriesKey, RecordTag,
    RecordVersion,
};
use tracing::warn;
use typed_builder::TypedBuilder;

use crate::ai_session::Message;
use crate::ai_session::database::{AiSessionDatabase, DbError, Watermark};
use crate::record::sqlite_store::SqliteStore;

/// Records read from the record store at a time while reprojecting.
const REPROJECT_PAGE: u64 = 512;

#[derive(Debug, Clone, TypedBuilder)]
pub struct AiSessionStore {
    store: SqliteStore,
    host_id: HostId,
    key: Key,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AiSessionRecord {
    Message(Message),
}

#[derive(Debug, thiserror::Error)]
pub enum DecodeError {
    #[error("empty ai-session record")]
    Empty,
    #[error("unknown ai-session record kind {0}")]
    UnknownKind(u8),
    #[error("failed to decode ai-session record body: {0}")]
    Body(#[from] rmp_serde::decode::Error),
}

#[derive(Debug, thiserror::Error)]
pub enum PushError {
    #[error(transparent)]
    Store(#[from] eyre::Report),
}

#[derive(Debug, thiserror::Error)]
pub enum BuildError {
    #[error(transparent)]
    Store(#[from] eyre::Report),
    #[error(transparent)]
    Db(#[from] DbError),
}

impl AiSessionRecord {
    const MESSAGE_KIND: u8 = 0;

    /// A kind byte, then the body as named-field msgpack: a reader ignores fields it does not
    /// know, so adding one never breaks hosts on an older build.
    #[must_use]
    pub fn serialize(&self) -> Vec<u8> {
        match self {
            Self::Message(msg) => {
                let mut out = vec![Self::MESSAGE_KIND];
                out.extend(rmp_serde::to_vec_named(msg).expect("Message is always serializable"));
                out
            }
        }
    }

    pub fn deserialize(bytes: &[u8]) -> Result<Self, DecodeError> {
        let (&kind, body) = bytes.split_first().ok_or(DecodeError::Empty)?;

        match kind {
            Self::MESSAGE_KIND => Ok(Self::Message(rmp_serde::from_slice(body)?)),
            n => Err(DecodeError::UnknownKind(n)),
        }
    }
}

impl AiSessionStore {
    /// The host this store pushes records as: the local host.
    #[must_use]
    pub const fn host_id(&self) -> HostId {
        self.host_id
    }

    pub async fn push(&self, msg: &Message) -> Result<RecordId, PushError> {
        let id = msg.id;

        let bytes = AiSessionRecord::Message(msg.clone()).serialize();
        let series = RecordSeriesKey::new(self.host_id, RecordTag::AiSession);

        loop {
            let idx = self.store.last(&series).await?.map_or(0, |r| r.idx + 1);

            let record = Record::builder()
                .id(id)
                .host(Host::new(self.host_id))
                .version(RecordVersion::V0)
                .tag(RecordTag::AiSession)
                .idx(idx)
                .data(DecryptedData(bytes.clone()))
                .build();

            if self.store.push_unique(&record.encrypt(&self.key)).await? {
                return Ok(id);
            }
        }
    }

    async fn decode_and_append(
        &self,
        record: Record<EncryptedData>,
        db: &AiSessionDatabase,
    ) -> Result<Projected, BuildError> {
        if record.tag != RecordTag::AiSession {
            return Ok(Projected::Skipped);
        }

        let id = record.id;
        let host = record.host.id;

        let decrypted = match record.decrypt(&self.key) {
            Ok(decrypted) => decrypted,
            Err(err) => {
                warn!(?err, id = %id.0, "failed to decrypt ai-session record, skipping");
                return Ok(Projected::Skipped);
            }
        };

        let AiSessionRecord::Message(msg) = match AiSessionRecord::deserialize(&decrypted.data.0) {
            Ok(record) => record,
            // Written by a newer build, which an upgrade will be able to project.
            Err(err @ DecodeError::UnknownKind(_)) => {
                warn!(?err, id = %id.0, "unknown ai-session record kind, skipping for now");
                return Ok(Projected::Deferred);
            }
            Err(err) => {
                warn!(?err, id = %id.0, "failed to deserialize ai-session record, skipping");
                return Ok(Projected::Skipped);
            }
        };

        // The record body never carries the host: its envelope does.
        let msg = Message {
            host: Some(host),
            ..msg
        };
        db.append(&msg).await?;
        Ok(Projected::Appended)
    }

    /// Replay every record into the sidecar, whatever it already holds.
    pub async fn build(&self, db: &AiSessionDatabase) -> Result<(), BuildError> {
        db.clear_reproject_watermarks().await?;
        self.reproject(db).await.map(|_| ())
    }

    /// Bring the sidecar up to date with the record store, replaying only the records past each
    /// series' [`Watermark`]. A series without one (a fresh sidecar, or watermarks cleared by a
    /// migration or maintenance command) is replayed from its start, as is one rewritten under
    /// its watermark. Run it before capture starts: over this host's series it would otherwise
    /// race the capture pipeline (see [`Self::reproject_remote`]).
    ///
    /// Continues past a failed series, but reports it so callers do not enable capture against a
    /// projection missing already-persisted messages; that series' watermark stays below the
    /// failure, so the next reprojection retries it.
    pub async fn reproject(&self, db: &AiSessionDatabase) -> Result<Reprojected, BuildError> {
        self.reproject_where(db, |_| true).await
    }

    /// [`Self::reproject`] over other hosts' series only: what sync downloads. This host's series
    /// is left to the startup reprojection, since capture projects it live and uses the sidecar
    /// as its dedup gate.
    pub async fn reproject_remote(
        &self,
        db: &AiSessionDatabase,
    ) -> Result<Reprojected, BuildError> {
        let local = self.host_id;
        self.reproject_where(db, |host| host != local).await
    }

    async fn reproject_where(
        &self,
        db: &AiSessionDatabase,
        include: impl Fn(HostId) -> bool,
    ) -> Result<Reprojected, BuildError> {
        let mut marks = db.reproject_watermarks().await?;
        let mut series: Vec<(RecordSeriesKey, RecordIdx)> = self
            .store
            .status()
            .await?
            .hosts
            .into_iter()
            .filter(|(host, _)| include(*host))
            .filter_map(|(host, tags)| {
                let last = *tags.get(&RecordTag::AiSession)?;
                Some((RecordSeriesKey::new(host, RecordTag::AiSession), last))
            })
            .collect();
        series.sort();

        let mut stats = Reprojected::default();
        let mut failure = None;
        for (series, last) in series {
            let mark = marks.remove(&series);
            if let Err(err) = self.reproject_series(db, &series, last, mark, &mut stats).await {
                warn!(?err, host = %series.host_id, "failed to reproject ai-session records");
                failure = Some(err);
            }
        }

        // Watermarks left over belong to series with no records at all any more: that host's
        // store was deleted. Forget them, so records arriving for it again replay from the start.
        for (series, _) in marks {
            if series.tag == RecordTag::AiSession && include(series.host_id) {
                warn!(host = %series.host_id, "ai-session records vanished from the record store");
                db.forget_host(series.host_id).await?;
                db.forget_reproject_watermark(&series).await?;
                stats.restarted += 1;
            }
        }

        failure.map_or(Ok(stats), Err)
    }

    async fn reproject_series(
        &self,
        db: &AiSessionDatabase,
        series: &RecordSeriesKey,
        last: RecordIdx,
        mark: Option<Watermark>,
        stats: &mut Reprojected,
    ) -> Result<(), BuildError> {
        let start = match mark {
            None => 0,
            Some(mark) => match self.store.idx(series, mark.idx).await? {
                // Still the record it was: everything up to it is projected.
                Some(record) if record.id == mark.record_id => mark.idx + 1,
                // Reset, truncated or rewritten under the watermark: what it covered is no longer
                // what the store holds, so start over.
                _ => {
                    warn!(
                        host = %series.host_id,
                        idx = mark.idx,
                        "ai-session records rewritten under the watermark, replaying the host"
                    );
                    // What the old series projected may no longer be in it.
                    db.forget_host(series.host_id).await?;
                    stats.restarted += 1;
                    0
                }
            },
        };
        if start > last {
            return Ok(());
        }

        let mut from = mark;
        let mut next = start;
        // A deferred record holds the watermark below it until a build that understands it.
        let mut held = false;
        loop {
            let page = self.store.next(series, next, REPROJECT_PAGE).await?;
            let Some(tail) = page.last() else {
                break;
            };
            next = tail.idx + 1;

            let mut to = None;
            for record in page {
                let (idx, record_id) = (record.idx, record.id);
                if self.decode_and_append(record, db).await? == Projected::Deferred {
                    held = true;
                }
                stats.replayed += 1;
                if !held {
                    to = Some(Watermark { idx, record_id });
                }
            }

            // Every append up to `to` has committed, in the same database: a watermark that
            // survives a crash implies the appends it covers do too.
            if let Some(to) = to {
                if !db.advance_reproject_watermark(series, from, to).await? {
                    warn!(host = %series.host_id, "ai-session watermark changed under reproject");
                    return Ok(());
                }
                from = Some(to);
            }
        }

        Ok(())
    }

    pub async fn incremental_build(&self, db: &AiSessionDatabase, ids: &[RecordId]) {
        for id in ids {
            let record = match self.store.get(*id).await {
                Ok(record) => record,
                Err(err) => {
                    warn!(?err, id = %id.0, "failed to load ai-session record, skipping");
                    continue;
                }
            };

            // A single bad record must not abort the rest of the batch: these records are already
            // downloaded and the sync cursor advances past them regardless, so propagating the
            // error here would strand every later record out of the sidecar permanently.
            if let Err(err) = self.decode_and_append(record, db).await {
                warn!(?err, id = %id.0, "failed to append ai-session record to sidecar, skipping");
            }
        }
    }
}

/// What [`AiSessionStore::reproject`] did.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Reprojected {
    /// Records read and replayed into the sidecar (duplicates included).
    pub replayed: u64,
    /// Series found rewritten or deleted under their watermark, and so replayed from the start.
    pub restarted: u64,
}

/// What replaying one record did to the sidecar.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Projected {
    Appended,
    /// Can never be projected (not ai-session, undecryptable or corrupt): pass over it.
    Skipped,
    /// Cannot be projected by this build: replay it again after an upgrade.
    Deferred,
}

#[cfg(test)]
mod tests {
    use std::ops::Range;

    use atuin_common::harnesstools::session::{Content, Role};
    use atuin_domain::record::HostId;
    use proptest::prelude::*;
    use rstest::*;
    use time::OffsetDateTime;

    use super::{
        AiSessionRecord, AiSessionStore, DecryptedData, Host, Key, Record, RecordId,
        RecordSeriesKey, RecordTag, RecordVersion, Reprojected, SqliteStore, Watermark,
    };
    use crate::ai_session::{
        AiSessionDatabase, HarnessKind, HarnessSession, Message, NativeSessionId, SourceId,
    };
    use crate::settings::test_local_timeout;

    fn hid() -> HostId {
        HostId(atuin_common::utils::uuid_v7())
    }

    fn key() -> Key {
        Key::from([0u8; 32])
    }

    fn sample_message() -> Message {
        Message::builder()
            .id(atuin_domain::record::RecordId(atuin_common::utils::uuid_v7()))
            .session(HarnessSession {
                harness: HarnessKind::ClaudeCode,
                session: NativeSessionId::from("native-session".to_owned()),
            })
            .source_id(SourceId::from("source-id".to_owned()))
            .timestamp(OffsetDateTime::UNIX_EPOCH)
            .role(Role::User)
            .content(vec![Content::Text("hello".to_owned())])
            .build()
    }

    fn message_in(session: &HarnessSession, index: i64, text: &str) -> Message {
        Message::builder()
            .id(atuin_domain::record::RecordId(atuin_common::utils::uuid_v7()))
            .session(session.clone())
            .source_id(SourceId::from(format!("source-{index}")))
            .timestamp(OffsetDateTime::UNIX_EPOCH + time::Duration::seconds(index))
            .role(Role::User)
            .content(vec![Content::Text(text.to_owned())])
            .build()
    }

    fn sample_handle() -> HarnessSession {
        HarnessSession {
            harness: HarnessKind::ClaudeCode,
            session: NativeSessionId::from("ordered-session".to_owned()),
        }
    }

    fn ordered_messages(session: &HarnessSession) -> Vec<Message> {
        (0..3).map(|i| message_in(session, i, &format!("message {i}"))).collect()
    }

    fn arb_content() -> impl Strategy<Value = Content> {
        "[a-zA-Z0-9 ]{0,16}".prop_map(Content::Text)
    }

    fn arb_message() -> impl Strategy<Value = Message> {
        ("[a-z0-9]{1,8}", "[a-z0-9]{1,8}", proptest::collection::vec(arb_content(), 0..3)).prop_map(
            |(native_session, source_id, content)| {
                Message::builder()
                    .id(atuin_domain::record::RecordId(atuin_common::utils::uuid_v7()))
                    .session(HarnessSession {
                        harness: HarnessKind::ClaudeCode,
                        session: NativeSessionId::from(native_session),
                    })
                    .source_id(SourceId::from(source_id))
                    .timestamp(OffsetDateTime::UNIX_EPOCH)
                    .role(Role::User)
                    .content(content)
                    .build()
            },
        )
    }

    #[rstest]
    #[tokio::test]
    async fn push_then_read_back_one_message() {
        let store = SqliteStore::in_memory(test_local_timeout()).await.unwrap();
        let s = AiSessionStore::builder().store(store.clone()).host_id(hid()).key(key()).build();
        let msg = sample_message();
        let id = s.push(&msg).await.unwrap();

        assert_eq!(id, msg.id, "push must return the message's own id, not a re-minted one");

        let recs = store.all_tagged(&RecordTag::AiSession).await.unwrap();
        assert_eq!(recs.len(), 1);
        assert_eq!(recs[0].id, msg.id, "record envelope id must match the message id");
        let decrypted = recs[0].decrypt(&key()).unwrap();
        let AiSessionRecord::Message(got) =
            AiSessionRecord::deserialize(&decrypted.data.0).unwrap();
        assert_eq!(got.id, msg.id, "stored record body id must match the message id");
        assert_eq!(got.session, msg.session);
    }

    proptest! {
        #[test]
        fn record_body_roundtrips(m in arb_message()) {
            let bytes = AiSessionRecord::Message(m.clone()).serialize();
            let AiSessionRecord::Message(back) = AiSessionRecord::deserialize(&bytes).unwrap();
            prop_assert_eq!(m, back);
        }
    }

    #[rstest]
    #[tokio::test]
    async fn build_reconstructs_sidecar_from_records() {
        let store = SqliteStore::in_memory(test_local_timeout()).await.unwrap();
        let s = AiSessionStore::builder().store(store).host_id(hid()).key(key()).build();

        let mut ids = vec![];
        for m in ordered_messages(&sample_handle()) {
            ids.push(s.push(&m).await.unwrap());
        }

        let db = AiSessionDatabase::in_memory().await.unwrap();
        s.incremental_build(&db, &ids).await;

        let sess = db.get_session(&sample_handle()).await.unwrap().unwrap();
        assert_eq!(usize::try_from(sess.message_count).unwrap(), ids.len());
    }

    #[rstest]
    #[tokio::test]
    async fn build_reconstructs_sidecar_from_all_records() {
        let store = SqliteStore::in_memory(test_local_timeout()).await.unwrap();
        let s = AiSessionStore::builder().store(store).host_id(hid()).key(key()).build();

        let messages = ordered_messages(&sample_handle());
        for m in &messages {
            s.push(m).await.unwrap();
        }

        let db = AiSessionDatabase::in_memory().await.unwrap();
        s.build(&db).await.unwrap();

        let sess = db.get_session(&sample_handle()).await.unwrap().unwrap();
        assert_eq!(usize::try_from(sess.message_count).unwrap(), messages.len());
    }

    #[rstest]
    #[tokio::test]
    async fn build_recovers_session_title_from_message_records() {
        // Titles live only on the Started event, which is never synced; denormalising them onto the
        // message record (Message::session_title) is what lets a reproject on another machine --
        // records only, no sidecar -- recover the title.
        let store = SqliteStore::in_memory(test_local_timeout()).await.unwrap();
        let s = AiSessionStore::builder().store(store).host_id(hid()).key(key()).build();

        let handle = sample_handle();
        let mut untitled = message_in(&handle, 0, "hello");
        untitled.session_title = None;
        let mut titled = message_in(&handle, 1, "world");
        titled.session_title = Some("My Session".to_owned());
        s.push(&untitled).await.unwrap();
        s.push(&titled).await.unwrap();

        let db = AiSessionDatabase::in_memory().await.unwrap();
        s.build(&db).await.unwrap();

        let sess = db.get_session(&handle).await.unwrap().unwrap();
        assert_eq!(sess.title.as_deref(), Some("My Session"));
    }

    /// A store of `hosts` ai-session writers sharing one record store.
    fn writers(store: &SqliteStore, hosts: usize) -> Vec<AiSessionStore> {
        (0..hosts)
            .map(|_| {
                AiSessionStore::builder().store(store.clone()).host_id(hid()).key(key()).build()
            })
            .collect()
    }

    fn session_named(name: &str) -> HarnessSession {
        HarnessSession {
            harness: HarnessKind::ClaudeCode,
            session: NativeSessionId::from(name.to_owned()),
        }
    }

    async fn push_range(s: &AiSessionStore, session: &HarnessSession, range: Range<i64>) {
        for i in range {
            s.push(&message_in(session, i, &format!("message {i}"))).await.unwrap();
        }
    }

    async fn count(db: &AiSessionDatabase, session: &HarnessSession) -> Option<u64> {
        db.get_session(session).await.unwrap().map(|s| s.message_count)
    }

    async fn mark(db: &AiSessionDatabase, s: &AiSessionStore) -> Option<Watermark> {
        let series = RecordSeriesKey::new(s.host_id, RecordTag::AiSession);
        db.reproject_watermarks().await.unwrap().remove(&series)
    }

    #[rstest]
    #[tokio::test]
    async fn reproject_replays_only_records_past_the_watermark() {
        let store = SqliteStore::in_memory(test_local_timeout()).await.unwrap();
        let [s] = <[_; 1]>::try_from(writers(&store, 1)).ok().unwrap();
        let handle = sample_handle();
        let db = AiSessionDatabase::in_memory().await.unwrap();

        // A fresh sidecar has no watermark: everything is replayed.
        push_range(&s, &handle, 0..3).await;
        assert_eq!(s.reproject(&db).await.unwrap().replayed, 3);
        assert_eq!(mark(&db, &s).await.map(|m| m.idx), Some(2));

        push_range(&s, &handle, 3..5).await;
        let stats = s.reproject(&db).await.unwrap();
        assert_eq!(stats, Reprojected {
            replayed: 2,
            restarted: 0
        });
        assert_eq!(count(&db, &handle).await, Some(5));
        assert_eq!(mark(&db, &s).await.map(|m| m.idx), Some(4));

        // Nothing new, nothing replayed.
        assert_eq!(s.reproject(&db).await.unwrap().replayed, 0);
    }

    #[rstest]
    #[tokio::test]
    async fn cleared_watermark_replays_everything() {
        let store = SqliteStore::in_memory(test_local_timeout()).await.unwrap();
        let [a, b] = <[_; 2]>::try_from(writers(&store, 2)).ok().unwrap();
        let (one, two) = (session_named("one"), session_named("two"));
        push_range(&a, &one, 0..3).await;
        push_range(&b, &two, 0..2).await;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sessions.db");
        let db = AiSessionDatabase::open(&path).await.unwrap();
        assert_eq!(a.reproject(&db).await.unwrap().replayed, 5);

        // The contract for a migration needing a backfill: empty the table, get a full replay.
        let raw = atuin_common::db::sqlite::Sqlite::builder(path.as_os_str()).open().await.unwrap();
        atuin_common::db::query("DELETE FROM reproject_watermark;")
            .execute(raw.pool())
            .await
            .unwrap();
        assert_eq!(a.reproject(&db).await.unwrap().replayed, 5);
        // The replay is idempotent.
        assert_eq!(count(&db, &one).await, Some(3));
        assert_eq!(count(&db, &two).await, Some(2));

        // As does an explicit full build.
        a.build(&db).await.unwrap();
        assert_eq!(count(&db, &one).await, Some(3));
        assert_eq!(a.reproject(&db).await.unwrap().replayed, 0);
    }

    #[rstest]
    #[tokio::test]
    async fn invalidating_a_sidecar_on_disk_forces_a_full_replay() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sessions.db");
        // Nothing to invalidate yet: must neither fail nor create the file.
        AiSessionDatabase::invalidate_projection(&path).await.unwrap();
        assert!(!path.exists());

        let store = SqliteStore::in_memory(test_local_timeout()).await.unwrap();
        let [s] = <[_; 1]>::try_from(writers(&store, 1)).ok().unwrap();
        push_range(&s, &sample_handle(), 0..3).await;
        let db = AiSessionDatabase::open(&path).await.unwrap();
        assert_eq!(s.reproject(&db).await.unwrap().replayed, 3);

        AiSessionDatabase::invalidate_projection(&path).await.unwrap();
        assert_eq!(mark(&db, &s).await, None);
        assert_eq!(s.reproject(&db).await.unwrap().replayed, 3);
        assert_eq!(count(&db, &sample_handle()).await, Some(3));
    }

    #[rstest]
    #[tokio::test]
    async fn a_series_rewritten_under_the_watermark_is_replayed_from_the_start() {
        let store = SqliteStore::in_memory(test_local_timeout()).await.unwrap();
        let [a, b] = <[_; 2]>::try_from(writers(&store, 2)).ok().unwrap();
        let (old, new, other) = (session_named("old"), session_named("new"), session_named("b"));
        push_range(&a, &old, 0..5).await;
        push_range(&b, &other, 0..2).await;
        let db = AiSessionDatabase::in_memory().await.unwrap();
        a.reproject(&db).await.unwrap();

        // Host a's store is reset and starts over: idx goes backwards, below the watermark.
        for r in store.all_tagged(&RecordTag::AiSession).await.unwrap() {
            if r.host.id == a.host_id {
                store.delete(r.id).await.unwrap();
            }
        }
        push_range(&a, &new, 0..2).await;
        let stats = a.reproject(&db).await.unwrap();
        // Only host a is replayed, and from its start.
        assert_eq!(stats, Reprojected {
            replayed: 2,
            restarted: 1
        });
        assert_eq!(count(&db, &new).await, Some(2));
        assert_eq!(count(&db, &old).await, None, "what the old series projected is gone");
        assert_eq!(count(&db, &other).await, Some(2), "other hosts' sessions stay");
        assert_eq!(mark(&db, &a).await.map(|m| m.idx), Some(1));

        // Rewritten to the same length or longer with different records: idx alone would not
        // tell, the record id at the watermark does.
        for r in store.all_tagged(&RecordTag::AiSession).await.unwrap() {
            if r.host.id == a.host_id {
                store.delete(r.id).await.unwrap();
            }
        }
        let renamed = session_named("renamed");
        push_range(&a, &renamed, 0..3).await;
        let stats = a.reproject(&db).await.unwrap();
        assert_eq!(stats, Reprojected {
            replayed: 3,
            restarted: 1
        });
        assert_eq!(count(&db, &renamed).await, Some(3));
        assert_eq!(count(&db, &new).await, None);

        // Deleted outright: the watermark goes, and so records arriving again replay in full.
        for r in store.all_tagged(&RecordTag::AiSession).await.unwrap() {
            if r.host.id == a.host_id {
                store.delete(r.id).await.unwrap();
            }
        }
        assert_eq!(a.reproject(&db).await.unwrap().restarted, 1);
        assert_eq!(mark(&db, &a).await, None);
        assert_eq!(count(&db, &renamed).await, None);
        assert_eq!(count(&db, &other).await, Some(2));
        assert!(mark(&db, &b).await.is_some(), "other hosts keep theirs");
    }

    #[rstest]
    #[tokio::test]
    async fn watermark_never_passes_a_failed_append_and_the_replay_is_idempotent() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sessions.db");
        let store = SqliteStore::in_memory(test_local_timeout()).await.unwrap();
        let [s] = <[_; 1]>::try_from(writers(&store, 1)).ok().unwrap();
        let handle = sample_handle();
        push_range(&s, &handle, 0..3).await;
        let db = AiSessionDatabase::open(&path).await.unwrap();
        s.reproject(&db).await.unwrap();
        let before = mark(&db, &s).await;

        // The sidecar dies partway through the next batch: the appends before it commit, the
        // one it hits and every later one do not.
        push_range(&s, &handle, 3..6).await;
        let fault =
            atuin_common::db::sqlite::Sqlite::builder(path.as_os_str()).open().await.unwrap();
        atuin_common::db::query(
            "CREATE TRIGGER fail_write BEFORE INSERT ON messages WHEN NEW.source_id = 'source-4' \
             BEGIN SELECT RAISE(FAIL, 'injected failure'); END",
        )
        .execute(fault.pool())
        .await
        .unwrap();
        assert!(s.reproject(&db).await.is_err());
        assert_eq!(count(&db, &handle).await, Some(4), "source-3 made it in");
        assert_eq!(mark(&db, &s).await, before, "the watermark must not pass what failed");

        // A restart replays from the old watermark, source-3 included, and counts it once.
        drop(db);
        atuin_common::db::query("DROP TRIGGER fail_write").execute(fault.pool()).await.unwrap();
        let db = AiSessionDatabase::open(&path).await.unwrap();
        assert_eq!(s.reproject(&db).await.unwrap().replayed, 3);
        assert_eq!(count(&db, &handle).await, Some(6));
        assert_eq!(mark(&db, &s).await.map(|m| m.idx), Some(5));
    }

    #[rstest]
    #[tokio::test]
    async fn watermark_changed_mid_reproject_is_left_for_the_next_one() {
        let store = SqliteStore::in_memory(test_local_timeout()).await.unwrap();
        let [s] = <[_; 1]>::try_from(writers(&store, 1)).ok().unwrap();
        let handle = sample_handle();
        push_range(&s, &handle, 0..2).await;
        let db = AiSessionDatabase::in_memory().await.unwrap();
        s.reproject(&db).await.unwrap();
        let series = RecordSeriesKey::new(s.host_id, RecordTag::AiSession);
        let stale = mark(&db, &s).await;

        // An invalidation lands between reading the watermark and advancing it.
        db.clear_reproject_watermarks().await.unwrap();
        let to = Watermark {
            idx: 7,
            record_id: atuin_domain::record::RecordId(atuin_common::utils::uuid_v7()),
        };
        assert!(!db.advance_reproject_watermark(&series, stale, to).await.unwrap());
        assert_eq!(mark(&db, &s).await, None, "the invalidation must stick");
    }

    #[rstest]
    #[tokio::test]
    async fn an_unknown_record_kind_holds_the_watermark_below_it() {
        let store = SqliteStore::in_memory(test_local_timeout()).await.unwrap();
        let [s] = <[_; 1]>::try_from(writers(&store, 1)).ok().unwrap();
        let handle = sample_handle();
        push_range(&s, &handle, 0..1).await;
        // A record a newer build wrote, of a kind this one cannot project.
        let record = Record::builder()
            .id(RecordId(atuin_common::utils::uuid_v7()))
            .host(Host::new(s.host_id))
            .version(RecordVersion::V0)
            .tag(RecordTag::AiSession)
            .idx(1)
            .data(DecryptedData(vec![99]))
            .build();
        store.push(&record.encrypt(&key())).await.unwrap();
        push_range(&s, &handle, 1..3).await;

        let db = AiSessionDatabase::in_memory().await.unwrap();
        assert_eq!(s.reproject(&db).await.unwrap().replayed, 4);
        assert_eq!(count(&db, &handle).await, Some(3), "later records still project");
        assert_eq!(mark(&db, &s).await.map(|m| m.idx), Some(0));
        // Retried until a build understands it.
        assert_eq!(s.reproject(&db).await.unwrap().replayed, 3);
    }

    #[rstest]
    #[tokio::test]
    async fn reproject_remote_leaves_this_hosts_records_to_startup() {
        let store = SqliteStore::in_memory(test_local_timeout()).await.unwrap();
        let [local, remote] = <[_; 2]>::try_from(writers(&store, 2)).ok().unwrap();
        let (mine, theirs) = (session_named("mine"), session_named("theirs"));
        push_range(&local, &mine, 0..2).await;
        push_range(&remote, &theirs, 0..3).await;

        let db = AiSessionDatabase::in_memory().await.unwrap();
        assert_eq!(local.reproject_remote(&db).await.unwrap().replayed, 3);
        assert_eq!(count(&db, &mine).await, None);
        assert_eq!(count(&db, &theirs).await, Some(3));
        assert_eq!(mark(&db, &local).await, None);

        assert_eq!(local.reproject(&db).await.unwrap().replayed, 2);
        assert_eq!(count(&db, &mine).await, Some(2));
    }

    /// Startup reprojection cost on a large store, before (a full replay into an already
    /// populated sidecar, what every startup used to do) and after (watermarks in place, a few
    /// new records). `cargo test -p atuin-client --release -- --ignored --nocapture reproject_timing`
    #[rstest]
    #[tokio::test]
    #[ignore = "benchmark"]
    async fn reproject_timing() {
        let records: i64 =
            std::env::var("REPROJECT_RECORDS").map_or(20_000, |n| n.parse().unwrap());
        let dir = tempfile::tempdir().unwrap();
        let store =
            SqliteStore::new(dir.path().join("records.db"), test_local_timeout()).await.unwrap();
        let [local, remote] = <[_; 2]>::try_from(writers(&store, 2)).ok().unwrap();
        for i in 0..records {
            let (s, n) = if i % 4 == 0 {
                (&remote, i / 4)
            } else {
                (&local, i)
            };
            let session = session_named(&format!("session-{}", n / 50));
            s.push(&message_in(&session, i, &"lorem ipsum dolor ".repeat(20))).await.unwrap();
        }
        let db = AiSessionDatabase::open(dir.path().join("sessions.db")).await.unwrap();

        let t = std::time::Instant::now();
        local.build(&db).await.unwrap();
        println!("initial full build of {records} records: {:?}", t.elapsed());

        let t = std::time::Instant::now();
        local.build(&db).await.unwrap();
        println!("startup before (full replay, populated sidecar): {:?}", t.elapsed());
        let t = std::time::Instant::now();
        let stats = local.reproject(&db).await.unwrap();
        println!("startup after, nothing new ({stats:?}): {:?}", t.elapsed());

        push_range(&local, &session_named("new"), 0..100).await;
        let t = std::time::Instant::now();
        let stats = local.reproject(&db).await.unwrap();
        println!("startup after, 100 new ({stats:?}): {:?}", t.elapsed());
    }

    /// A reproject takes each row's host from its record's envelope, which the body never
    /// carries.
    #[rstest]
    #[tokio::test]
    async fn build_records_the_host_each_record_came_from() {
        let store = SqliteStore::in_memory(test_local_timeout()).await.unwrap();
        let host = hid();
        let s = AiSessionStore::builder().store(store).host_id(host).key(key()).build();
        for m in ordered_messages(&sample_handle()) {
            s.push(&m).await.unwrap();
        }

        let db = AiSessionDatabase::in_memory().await.unwrap();
        s.build(&db).await.unwrap();

        let sess = db.get_session(&sample_handle()).await.unwrap().unwrap();
        assert_eq!(sess.host, Some(host));
        let rows: Vec<_> =
            futures::TryStreamExt::try_collect(db.messages(&sample_handle())).await.unwrap();
        assert!(rows.iter().all(|m| m.host == Some(host)));
    }
}
