use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use atuin_common::encryption::paseto_v4::{EncryptedData, Key};
use atuin_domain::record::{
    DecryptedData, Host, HostId, Record, RecordId, RecordIdx, RecordSeriesKey, RecordTag,
    RecordVersion,
};
use tracing::warn;
use typed_builder::TypedBuilder;

use crate::ai_session::Message;
use crate::ai_session::database::{AiSessionDatabase, DbError, PreparedMessage, Watermark};
use crate::record::decode::decode_parallel;
use crate::record::sqlite_store::SqliteStore;

/// The zstd level of a v2 record's body: on captured messages, higher levels shrink them only a
/// little more, for several times the time.
const RECORD_ZSTD_LEVEL: i32 = 3;

/// Records read from the record store at a time while reprojecting, each page appended in one
/// transaction: fewer, larger commits flush and merge the search index far less often. A page
/// holds capture off for as long as it takes to append (a fraction of a second).
const REPROJECT_PAGE: u64 = 4096;

/// How many times one reprojection starts over after an invalidation lands in the middle of it.
/// Past that it gives up for now with [`BuildError::Incomplete`]: the invalidation cleared the
/// watermarks it concerned, so the next reprojection replays whatever this one left.
const REPROJECT_PASSES: usize = 4;

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
    #[error("unknown ai-session record version {}", .0.as_str())]
    UnknownVersion(RecordVersion),
    #[error("failed to decode ai-session record body: {0}")]
    Body(#[from] rmp_serde::decode::Error),
    #[error("failed to decompress ai-session record body: {0}")]
    Decompress(#[source] std::io::Error),
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
    /// Invalidations kept landing in the middle of it: after `REPROJECT_PASSES` passes the
    /// sidecar may still be missing records. The next reprojection replays what this one left.
    #[error("the ai-session projection kept being invalidated, and is incomplete")]
    Incomplete,
    /// Only [`AiSessionStore::reproject_beside_capture`]: the host's series was rewritten or
    /// deleted under its watermark, and forgetting what it projected would delete rows of this
    /// host, which capture dedups against. Nothing was forgotten: the caller must have it
    /// forgotten with capture held off, then replay.
    #[error("the ai-session records of host {0} were rewritten under capture")]
    ForgetHeldOff(HostId),
}

impl AiSessionRecord {
    const MESSAGE_KIND: u8 = 0;

    /// v0: named msgpack, without [`Message::atuin_id`]. v1: positional, so any change to
    /// [`Message`]'s fields needs a new version. v2: v1's body, compressed with zstd (captured
    /// tool payloads compress about 2.4 times).
    pub const VERSION: RecordVersion = RecordVersion::V2;

    /// A kind byte, then the zstd-compressed positional msgpack body.
    #[must_use]
    pub fn serialize(&self) -> Vec<u8> {
        match self {
            Self::Message(msg) => {
                let body = rmp_serde::to_vec(msg).expect("Message is always serializable");
                let mut out = vec![Self::MESSAGE_KIND];
                out.extend(
                    zstd::bulk::compress(&body, RECORD_ZSTD_LEVEL)
                        .expect("compressing to memory cannot fail"),
                );
                out
            }
        }
    }

    pub fn deserialize(bytes: &[u8], version: &RecordVersion) -> Result<Self, DecodeError> {
        let (&kind, body) = bytes.split_first().ok_or(DecodeError::Empty)?;
        if kind != Self::MESSAGE_KIND {
            return Err(DecodeError::UnknownKind(kind));
        }

        // from_slice reads a map by name (v0) or an array by position (v1, v2).
        match version {
            RecordVersion::V0 | RecordVersion::V1 => {
                Ok(Self::Message(rmp_serde::from_slice(body)?))
            }
            RecordVersion::V2 => {
                let body = zstd::stream::decode_all(body).map_err(DecodeError::Decompress)?;
                Ok(Self::Message(rmp_serde::from_slice(&body)?))
            }
            other @ RecordVersion::Other(_) => Err(DecodeError::UnknownVersion(other.clone())),
        }
    }
}

impl AiSessionStore {
    /// The host this store pushes records as: the local host.
    #[must_use]
    pub const fn host_id(&self) -> HostId {
        self.host_id
    }

    /// Whether the record [`Self::push`] writes for a message with this id is stored.
    pub async fn holds(&self, id: RecordId) -> Result<bool, PushError> {
        Ok(self.store.contains(id).await?)
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
                .version(AiSessionRecord::VERSION)
                .tag(RecordTag::AiSession)
                .idx(idx)
                .data(DecryptedData(bytes.clone()))
                .build();

            if self.store.push_unique(&record.encrypt(&self.key)).await? {
                return Ok(id);
            }
        }
    }

    /// Decrypt and decode `record` with `key`, and prepare it for the sidecar, which the caller
    /// appends it to.
    fn decode(key: &Key, record: &Record<EncryptedData>) -> Result<Projected, DbError> {
        if record.tag != RecordTag::AiSession {
            return Ok(Projected::Skipped);
        }

        let id = record.id;
        let host = record.host.id;

        // What a record that cannot be projected does to the watermark:
        //
        // - One this key cannot decrypt is held: the key may be the stale one (the daemon loads it
        //   once, and `atuin login` or a rekey can re-encrypt the store under it), and the right
        //   one projects it later. Passing it would lose it for good. A record no key will ever
        //   decrypt (lost key, corrupted ciphertext) is held too, which costs replaying its
        //   series' later records on each reprojection, but never stops them projecting.
        // - One that decrypts but whose kind or version this build does not know is held until
        //   an upgrade.
        // - One that decrypts to a known kind but fails to decode is skipped: decryption
        //   authenticates it, so it is exactly what its writer wrote, and no retry reads it.
        let decrypted = match record.decrypt(key) {
            Ok(decrypted) => decrypted,
            Err(err) => {
                warn!(?err, id = %id.0, "failed to decrypt ai-session record, holding it back");
                return Ok(Projected::Held);
            }
        };

        let decoded = AiSessionRecord::deserialize(&decrypted.data.0, &decrypted.version);
        let AiSessionRecord::Message(msg) = match decoded {
            Ok(record) => record,
            // Written by a newer build, which an upgrade will be able to project.
            Err(err @ (DecodeError::UnknownKind(_) | DecodeError::UnknownVersion(_))) => {
                warn!(?err, id = %id.0, "unknown ai-session record kind or version, holding it back");
                return Ok(Projected::Held);
            }
            Err(err) => {
                warn!(?err, id = %id.0, "failed to deserialize ai-session record, skipping");
                return Ok(Projected::Skipped);
            }
        };

        // The record body never carries the host: its envelope does.
        let msg = PreparedMessage::new(Message {
            host: Some(host),
            ..msg
        })?;
        Ok(Projected::Append(Box::new(msg)))
    }

    /// Replay every record into the sidecar, whatever it already holds.
    pub async fn build(&self, db: &AiSessionDatabase) -> Result<(), BuildError> {
        db.clear_reproject_watermarks().await?;
        self.reproject(db).await.map(|_| ())
    }

    /// Bring the sidecar up to date with the record store, replaying only the records past each
    /// series' [`Watermark`]. A series without one (a fresh sidecar, or watermarks cleared by a
    /// migration, maintenance command or key change) is replayed from its start, as is one
    /// rewritten under its watermark.
    ///
    /// This host's series is replayed under [`AiSessionDatabase::lock_local_projection`], which
    /// capture holds too. But a series found rewritten or deleted has what it projected forgotten
    /// ([`AiSessionDatabase::forget_host`]), which can delete rows of this host that capture
    /// dedups against: so this is for while capture is held off (startup recovery, a rebuild).
    /// Beside capture, see [`Self::reproject_beside_capture`].
    ///
    /// An invalidation landing meanwhile is noticed, and the reprojection starts over, up to
    /// `REPROJECT_PASSES` times, then fails with [`BuildError::Incomplete`]. Continues past a
    /// failed series, but reports it so callers do not enable capture against a projection
    /// missing already-persisted messages; that series' watermark stays below the failure, so
    /// the next reprojection retries it.
    pub async fn reproject(&self, db: &AiSessionDatabase) -> Result<Reprojected, BuildError> {
        self.reproject_with(db, &ReprojectProgress::default()).await
    }

    /// [`Self::reproject`], counting the records it replays into `progress` as it goes.
    pub async fn reproject_with(
        &self,
        db: &AiSessionDatabase,
        progress: &ReprojectProgress,
    ) -> Result<Reprojected, BuildError> {
        self.reproject_as(db, progress, Forgetting::HeldOff).await
    }

    /// [`Self::reproject`], safe beside live capture, for projecting what a sync downloaded:
    /// this host's own records can arrive from the server (a reinstall that kept its host id),
    /// and must be projected before capture, which dedups against the sidecar, pushes them again.
    ///
    /// It forgets only series whose forgetting deletes no row of this host: capture dedups a line
    /// it captured again against the row of this host it pushed for it. A series whose forgetting
    /// would (this host's own, rewritten or deleted under its watermark, or another host's that
    /// added rows to this host's sessions) is skipped, forgetting nothing, and once the other
    /// series are projected this fails with [`BuildError::ForgetHeldOff`] naming its host (the
    /// first, if several).
    pub async fn reproject_beside_capture(
        &self,
        db: &AiSessionDatabase,
    ) -> Result<Reprojected, BuildError> {
        self.reproject_as(db, &ReprojectProgress::default(), Forgetting::BesideCapture).await
    }

    async fn reproject_as(
        &self,
        db: &AiSessionDatabase,
        progress: &ReprojectProgress,
        forgetting: Forgetting,
    ) -> Result<Reprojected, BuildError> {
        let _running = db.lock_reprojection().await;
        if db.check_projection_key(&self.key.key_id().to_string()).await? {
            // Also the first time, on a fresh sidecar or one from before key tracking.
            tracing::info!("ai-session watermarks were not made with this key: replaying all");
        }

        let mut stats = Reprojected::default();
        let mut result = None;
        for _ in 0..REPROJECT_PASSES {
            match self.reproject_pass(db, &mut stats, progress, forgetting).await {
                Ok(Pass::Done) => {
                    result = Some(Ok(()));
                    break;
                }
                Ok(Pass::Invalidated) => {
                    warn!("ai-session projection invalidated under reproject, starting over");
                }
                Err(err) => {
                    result = Some(Err(err));
                    break;
                }
            }
        }
        match result {
            Some(Err(err)) => Err(err),
            Some(Ok(())) => Ok(stats),
            None => {
                warn!(
                    "ai-session projection kept being invalidated, leaving the rest for the next \
                     one"
                );
                Err(BuildError::Incomplete)
            }
        }
    }

    async fn reproject_pass(
        &self,
        db: &AiSessionDatabase,
        stats: &mut Reprojected,
        progress: &ReprojectProgress,
        forgetting: Forgetting,
    ) -> Result<Pass, BuildError> {
        let mut marks = db.reproject_watermarks().await?;
        let mut series: Vec<(RecordSeriesKey, RecordIdx)> = self
            .store
            .status()
            .await?
            .hosts
            .into_iter()
            .filter_map(|(host, tags)| {
                let last = *tags.get(&RecordTag::AiSession)?;
                Some((RecordSeriesKey::new(host, RecordTag::AiSession), last))
            })
            .collect();
        series.sort();

        // At most this many records to read; fewer when a series turns out rewritten and starts
        // over, which reads more.
        let pending = series
            .iter()
            .map(|(series, last)| {
                let start = marks.get(series).map_or(0, |mark| mark.idx + 1);
                (last + 1).saturating_sub(start)
            })
            .sum();
        progress.start(pending);

        let mut pass = Pass::Done;
        let mut failure = None;
        // A series this must not forget (beside capture) is skipped, and left to the caller once
        // the rest is projected.
        let mut held_off = None;
        for (series, last) in series {
            let mark = marks.remove(&series);
            match self.reproject_series(db, &series, last, mark, stats, progress, forgetting).await
            {
                Ok(Pass::Done) => {}
                Ok(Pass::Invalidated) => pass = Pass::Invalidated,
                Err(BuildError::ForgetHeldOff(host)) => {
                    held_off.get_or_insert(host);
                }
                Err(err) => {
                    warn!(?err, host = %series.host_id, "failed to reproject ai-session records");
                    failure = Some(err);
                }
            }
        }

        // Watermarks left over belong to series with no records at all any more: that host's
        // store was deleted. Forget them, so records arriving for it again replay from the start.
        for (series, _) in marks {
            if series.tag == RecordTag::AiSession {
                warn!(host = %series.host_id, "ai-session records vanished from the record store");
                match self.forget_host(db, series.host_id, forgetting).await {
                    Ok(true) => pass = Pass::Invalidated,
                    Ok(false) => {}
                    Err(BuildError::ForgetHeldOff(host)) => {
                        held_off.get_or_insert(host);
                        continue;
                    }
                    Err(err) => return Err(err),
                }
                stats.restarted += 1;
            }
        }

        if let Some(host) = held_off {
            return Err(BuildError::ForgetHeldOff(host));
        }
        failure.map_or(Ok(pass), Err)
    }

    /// [`AiSessionDatabase::forget_host`], under the local projection lock for this host. Beside
    /// capture, only if that deletes no row of this host (else [`BuildError::ForgetHeldOff`]).
    async fn forget_host(
        &self,
        db: &AiSessionDatabase,
        host: HostId,
        forgetting: Forgetting,
    ) -> Result<bool, BuildError> {
        match forgetting {
            Forgetting::HeldOff => {
                let _local = self.lock_if_local(db, host).await;
                Ok(db.forget_host(host).await?)
            }
            Forgetting::BesideCapture => db
                .forget_host_sparing(host, Some(self.host_id))
                .await?
                .ok_or(BuildError::ForgetHeldOff(host)),
        }
    }

    /// Capture's lock when `host` is this one: see [`AiSessionDatabase::lock_local_projection`].
    async fn lock_if_local(
        &self,
        db: &AiSessionDatabase,
        host: HostId,
    ) -> Option<tokio::sync::OwnedMutexGuard<()>> {
        if host == self.host_id {
            Some(db.lock_local_projection().await)
        } else {
            None
        }
    }

    /// Replay `series` past `mark`. [`Pass::Invalidated`] when an invalidation stopped it, or it
    /// forgot other hosts' watermarks, so that the reprojection goes round again.
    #[expect(clippy::too_many_arguments)]
    async fn reproject_series(
        &self,
        db: &AiSessionDatabase,
        series: &RecordSeriesKey,
        last: RecordIdx,
        mark: Option<Watermark>,
        stats: &mut Reprojected,
        progress: &ReprojectProgress,
        forgetting: Forgetting,
    ) -> Result<Pass, BuildError> {
        let mut pass = Pass::Done;
        let (start, mut from) = match mark {
            None => (0, None),
            Some(mark) => match self.store.idx(series, mark.idx).await? {
                // Still the record it was: everything up to it is projected.
                Some(record) if record.id == mark.record_id => (mark.idx + 1, Some(mark)),
                // Reset, truncated or rewritten under the watermark: what it covered is no longer
                // what the store holds, so start over.
                _ => {
                    warn!(
                        host = %series.host_id,
                        idx = mark.idx,
                        "ai-session records rewritten under the watermark, replaying the host"
                    );
                    // What the old series projected may no longer be in it. This forgets the
                    // series' watermark too.
                    if self.forget_host(db, series.host_id, forgetting).await? {
                        pass = Pass::Invalidated;
                    }
                    stats.restarted += 1;
                    (0, None)
                }
            },
        };
        if start > last {
            return Ok(pass);
        }

        // Read before replaying anything: an invalidation after this stops the watermark moving.
        let generation = db.projection_generation().await?;
        let mut next = start;
        // A held record keeps the watermark below it (see `decode`), and so does a
        // hole in the series: a record missing there (not downloaded yet, as sync can fetch a
        // series' records out of order) would never be replayed if the watermark passed it.
        // Records past either are still replayed, which a later replay repeats idempotently.
        let mut held = false;
        // Records are only read under the lock (see `lock_if_local`), so never one capture has
        // pushed but not yet appended. One read but not yet appended is to capture as one not yet
        // read: as every page after the one replaying is, between pages.
        let mut page = {
            let _local = self.lock_if_local(db, series.host_id).await;
            self.read_page(series, next).await?
        };
        while let Some(&(tail, _, _)) = page.last() {
            let page_len = page.len() as u64;
            let mut expected = next;
            next = tail + 1;

            let mut to = None;
            let mut msgs = Vec::with_capacity(page.len());
            for (idx, record_id, projected) in page {
                if idx != expected {
                    held = true;
                }
                expected = idx + 1;
                match projected? {
                    Projected::Append(msg) => msgs.push(*msg),
                    Projected::Held => held = true,
                    Projected::Skipped => {}
                }
                if !held {
                    to = Some(Watermark { idx, record_id });
                }
            }

            // Per page rather than per series, so capture never waits long. The page is appended
            // in one transaction (committing each row alone is most of a replay's cost), while
            // the next one is read and decoded.
            let local = self.lock_if_local(db, series.host_id).await;
            let (appended, following) =
                tokio::join!(db.append_all(msgs, generation), self.read_page(series, next));
            drop(local);
            if !appended? {
                warn!(host = %series.host_id, "ai-session sidecar invalidated under reproject");
                return Ok(Pass::Invalidated);
            }
            stats.replayed += page_len;
            progress.replayed.fetch_add(page_len, Ordering::Relaxed);

            // Every append up to `to` has committed, in the same database: a watermark that
            // survives a crash implies the appends it covers do too.
            if let Some(to) = to {
                if !db.advance_reproject_watermark(series, generation, from, to).await? {
                    warn!(host = %series.host_id, "ai-session watermark invalidated under reproject");
                    return Ok(Pass::Invalidated);
                }
                from = Some(to);
            }
            page = following?;
        }

        Ok(pass)
    }

    /// The page of `series` from `from`: each record's place in it, and what replaying it does,
    /// decoded and prepared across cores.
    async fn read_page(
        &self,
        series: &RecordSeriesKey,
        from: RecordIdx,
    ) -> Result<Vec<(RecordIdx, RecordId, Result<Projected, DbError>)>, BuildError> {
        let records = self.store.next(series, from, REPROJECT_PAGE).await?;
        let key = self.key.clone();
        Ok(decode_parallel(records, move |record| {
            (record.idx, record.id, Self::decode(&key, &record))
        })
        .await)
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

/// How far a running [`AiSessionStore::reproject_with`] has got, readable from other tasks
/// meanwhile. Cheap to clone: clones share the counts.
#[derive(Debug, Clone, Default)]
pub struct ReprojectProgress {
    replayed: Arc<AtomicU64>,
    pending: Arc<AtomicU64>,
}

impl ReprojectProgress {
    /// A pass begins, with `pending` records to read: counting starts again from none.
    fn start(&self, pending: u64) {
        self.replayed.store(0, Ordering::Relaxed);
        self.pending.store(pending, Ordering::Relaxed);
    }

    /// Count from none again, before a reprojection that has yet to start its first pass.
    pub fn clear(&self) {
        self.start(0);
    }

    /// Records replayed so far in this pass, and how many it set out to read. The first can pass
    /// the second when a series found rewritten is replayed from its start.
    #[must_use]
    pub fn get(&self) -> (u64, u64) {
        (self.replayed.load(Ordering::Relaxed), self.pending.load(Ordering::Relaxed))
    }
}

/// What replaying one record does to the sidecar.
#[derive(Debug)]
enum Projected {
    Append(Box<PreparedMessage>),
    /// Can never be projected (not ai-session, or authentic but undecodable): pass over it.
    Skipped,
    /// Cannot be projected with this key or by this build: replay it again (with another key,
    /// or after an upgrade), so hold the watermark below it.
    Held,
}

/// Whether a reprojection may delete rows of this host when it forgets a series.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Forgetting {
    /// Capture is held off (the store is recovering), so it may.
    HeldOff,
    /// Capture runs, and dedups against this host's rows: it must not.
    BesideCapture,
}

/// How a reprojection pass, or its replay of one series, ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Pass {
    Done,
    /// An invalidation cleared watermarks it had read or was moving: go round again.
    Invalidated,
}

#[cfg(test)]
mod tests {
    use std::ops::Range;

    use atuin_common::harnesstools::session::{Content, Role};
    use atuin_domain::record::HostId;
    use futures::TryStreamExt;
    use proptest::prelude::*;
    use rstest::*;
    use time::OffsetDateTime;

    use super::{
        AiSessionRecord, AiSessionStore, BuildError, DecryptedData, Host, Key, Record, RecordId,
        RecordSeriesKey, RecordTag, RecordVersion, Reprojected, SqliteStore, Watermark,
    };
    use crate::ai_session::{
        AiSessionDatabase, HarnessKind, HarnessSession, Message, NativeSessionId, SearchTerms,
        SessionFilter, SourceId,
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
            AiSessionRecord::deserialize(&decrypted.data.0, &decrypted.version).unwrap();
        assert_eq!(got.id, msg.id, "stored record body id must match the message id");
        assert_eq!(got.session, msg.session);
    }

    #[rstest]
    fn record_body_roundtrips() {
        proptest!(|(m in arb_message())| {
            let bytes = AiSessionRecord::Message(m.clone()).serialize();
            let AiSessionRecord::Message(back) =
                AiSessionRecord::deserialize(&bytes, &AiSessionRecord::VERSION).unwrap();
            prop_assert_eq!(m, back);
        });
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

    /// `atuin store purge` deletes the records the key cannot decrypt, projected earlier under
    /// the key they were made with. Resetting the sidecar and reprojecting leaves no trace of
    /// them in listings or search, and keeps the rest.
    #[rstest]
    #[tokio::test]
    async fn purged_records_leave_the_sidecar_once_reset() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sessions.db");
        let store = SqliteStore::in_memory(test_local_timeout()).await.unwrap();
        let old_key = Key::from([7u8; 32]);
        let writer = |key: Key| {
            AiSessionStore::builder().store(store.clone()).host_id(hid()).key(key).build()
        };
        let (old, new) = (writer(old_key), writer(key()));
        let (purged, kept) = (session_named("purged"), session_named("kept"));
        for i in 0..2 {
            old.push(&message_in(&purged, i, "zebra crossing")).await.unwrap();
            new.push(&message_in(&kept, i, "zebra stripes")).await.unwrap();
        }
        // Projected under the old key, then under the new one: both sessions are in.
        let db = AiSessionDatabase::open(&path).await.unwrap();
        old.reproject(&db).await.unwrap();
        new.reproject(&db).await.unwrap();
        assert_eq!(db.list_sessions(&SessionFilter::default()).await.unwrap().len(), 2);

        store.purge(&key()).await.unwrap();
        db.reset().await.unwrap();
        assert!(db.list_sessions(&SessionFilter::default()).await.unwrap().is_empty());
        new.reproject(&db).await.unwrap();

        let listed: Vec<_> = db
            .list_sessions(&SessionFilter::default())
            .await
            .unwrap()
            .into_iter()
            .map(|s| s.handle)
            .collect();
        assert_eq!(listed, std::slice::from_ref(&kept));
        let found: Vec<_> = db
            .search("zebra", SearchTerms::All, &SessionFilter::default(), 0)
            .map_ok(|m| m.session.handle)
            .try_collect()
            .await
            .unwrap();
        assert_eq!(found, std::slice::from_ref(&kept), "the purged session left the index");
        assert_eq!(count(&db, &kept).await, Some(2));
        assert_eq!(new.reproject(&db).await.unwrap().replayed, 0, "watermarks are back");
    }

    /// Resetting keeps what capture has read of each native transcript: the records hold it.
    #[rstest]
    #[tokio::test]
    async fn resetting_keeps_capture_checkpoints() {
        let db = AiSessionDatabase::in_memory().await.unwrap();
        let checkpoint = atuin_common::harnesstools::session::Checkpoint {
            at: 42,
            digest: 7,
            generation: 0,
        };
        db.set_checkpoint(&sample_handle(), checkpoint).await.unwrap();
        db.reset().await.unwrap();
        assert_eq!(db.checkpoint(&sample_handle()).await.unwrap(), Some(checkpoint));
    }

    /// A watermark row that cannot be read is replaced by the one the replay it causes writes,
    /// not left standing in its way so the series is replayed on every reprojection.
    #[rstest]
    #[case::bad_record_id("0", "'not-a-uuid'")]
    #[case::negative_idx("-1", "?2")]
    #[case::text_idx("'seven'", "?2")]
    #[tokio::test]
    async fn a_malformed_watermark_is_replaced(#[case] idx: &str, #[case] record_id: &str) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sessions.db");
        let store = SqliteStore::in_memory(test_local_timeout()).await.unwrap();
        let [s] = <[_; 1]>::try_from(writers(&store, 1)).ok().unwrap();
        push_range(&s, &sample_handle(), 0..3).await;
        let db = AiSessionDatabase::open(&path).await.unwrap();
        // Replayed once, so the key is known and the next reprojection trusts the watermarks.
        assert_eq!(s.reproject(&db).await.unwrap().replayed, 3);

        let raw = atuin_common::db::sqlite::Sqlite::builder(path.as_os_str()).open().await.unwrap();
        let sql = format!(
            "UPDATE reproject_watermark SET idx = {idx}, record_id = {record_id} WHERE host = ?1 \
             AND tag = ?3"
        );
        atuin_common::db::query(sqlx::AssertSqlSafe(sql))
            .bind(s.host_id.as_hyphenated().to_string())
            .bind(atuin_common::utils::uuid_v7().as_hyphenated().to_string())
            .bind(RecordTag::AiSession.as_str())
            .execute(raw.pool())
            .await
            .unwrap();
        assert_eq!(mark(&db, &s).await, None, "a malformed watermark reads as none");

        assert_eq!(s.reproject(&db).await.unwrap().replayed, 3);
        assert_eq!(mark(&db, &s).await.map(|m| m.idx), Some(2), "the replay wrote a watermark");
        assert_eq!(s.reproject(&db).await.unwrap().replayed, 0, "and the next one trusts it");
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

        // The sidecar dies partway through the next page, which is appended in one transaction:
        // none of it commits, source-3 before the failure included.
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
        assert_eq!(count(&db, &handle).await, Some(3), "the page rolled back whole");
        assert_eq!(mark(&db, &s).await, before, "the watermark must not pass what failed");

        // A restart replays the page from the old watermark.
        drop(db);
        atuin_common::db::query("DROP TRIGGER fail_write").execute(fault.pool()).await.unwrap();
        let db = AiSessionDatabase::open(&path).await.unwrap();
        assert_eq!(s.reproject(&db).await.unwrap().replayed, 3);
        assert_eq!(count(&db, &handle).await, Some(6));
        assert_eq!(mark(&db, &s).await.map(|m| m.idx), Some(5));
    }

    #[rstest]
    #[tokio::test]
    async fn a_watermark_never_moves_across_an_invalidation() {
        let store = SqliteStore::in_memory(test_local_timeout()).await.unwrap();
        let [s] = <[_; 1]>::try_from(writers(&store, 1)).ok().unwrap();
        let handle = sample_handle();
        push_range(&s, &handle, 0..2).await;
        let db = AiSessionDatabase::in_memory().await.unwrap();
        s.reproject(&db).await.unwrap();
        let series = RecordSeriesKey::new(s.host_id, RecordTag::AiSession);
        let stale = mark(&db, &s).await;
        let to = Watermark {
            idx: 7,
            record_id: atuin_domain::record::RecordId(atuin_common::utils::uuid_v7()),
        };

        // An invalidation lands between reading the watermark and advancing it.
        let generation = db.projection_generation().await.unwrap();
        db.clear_reproject_watermarks().await.unwrap();
        assert!(!db.advance_reproject_watermark(&series, generation, stale, to).await.unwrap());
        // Even with no watermark to lose, the generation tells.
        assert!(!db.advance_reproject_watermark(&series, generation, None, to).await.unwrap());
        assert_eq!(mark(&db, &s).await, None, "the invalidation must stick");

        let generation = db.projection_generation().await.unwrap();
        assert!(db.advance_reproject_watermark(&series, generation, None, to).await.unwrap());
        assert_eq!(mark(&db, &s).await, Some(to));
    }

    /// An invalidation landing while a series is replayed, even one with no watermark yet (which
    /// the watermark row alone cannot tell), makes the reprojection start over rather than record
    /// as projected what the invalidation removed.
    #[rstest]
    #[tokio::test]
    async fn an_invalidation_mid_reproject_starts_it_over() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sessions.db");
        let store = SqliteStore::in_memory(test_local_timeout()).await.unwrap();
        let [s] = <[_; 1]>::try_from(writers(&store, 1)).ok().unwrap();
        let handle = sample_handle();
        push_range(&s, &handle, 0..6).await;
        let db = AiSessionDatabase::open(&path).await.unwrap();

        // Once source-4 is appended, an invalidation deletes a row already replayed (as
        // forgetting a host does) and clears the watermarks. It fires once: a replay of source-4
        // conflicts, and inserts nothing.
        let raw = atuin_common::db::sqlite::Sqlite::builder(path.as_os_str()).open().await.unwrap();
        atuin_common::db::query(
            "CREATE TRIGGER invalidate AFTER INSERT ON messages WHEN NEW.source_id = 'source-4' \
             BEGIN DELETE FROM messages WHERE source_id = 'source-1'; DELETE FROM \
             reproject_watermark; UPDATE projection_state SET generation = generation + 1; END",
        )
        .execute(raw.pool())
        .await
        .unwrap();

        let stats = s.reproject(&db).await.unwrap();
        assert_eq!(stats.replayed, 12, "replayed twice: once invalidated, once through");
        let source_1 = crate::ai_session::SourceId::from("source-1".to_owned());
        assert!(db.contains_message(&handle, &source_1).await.unwrap(), "the replay restored it");
        assert_eq!(mark(&db, &s).await.map(|m| m.idx), Some(5));
    }

    /// A reprojection invalidated pass after pass gives up as incomplete, rather than report a
    /// sidecar that may be missing records as projected; the next one replays what it left.
    #[rstest]
    #[tokio::test]
    async fn a_reprojection_invalidated_every_pass_is_incomplete() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sessions.db");
        let store = SqliteStore::in_memory(test_local_timeout()).await.unwrap();
        let [s] = <[_; 1]>::try_from(writers(&store, 1)).ok().unwrap();
        push_range(&s, &sample_handle(), 0..3).await;
        let db = AiSessionDatabase::open(&path).await.unwrap();

        // Every watermark move fails, as when an invalidation lands in the middle of each pass.
        let raw = atuin_common::db::sqlite::Sqlite::builder(path.as_os_str()).open().await.unwrap();
        atuin_common::db::query(
            "CREATE TRIGGER invalidate BEFORE INSERT ON reproject_watermark BEGIN SELECT \
             RAISE(IGNORE); END",
        )
        .execute(raw.pool())
        .await
        .unwrap();
        assert!(matches!(s.reproject(&db).await, Err(BuildError::Incomplete)));
        assert!(matches!(s.reproject_beside_capture(&db).await, Err(BuildError::Incomplete)));
        assert_eq!(mark(&db, &s).await, None);

        atuin_common::db::query("DROP TRIGGER invalidate").execute(raw.pool()).await.unwrap();
        s.reproject(&db).await.unwrap();
        assert_eq!(mark(&db, &s).await.map(|m| m.idx), Some(2));
    }

    /// Beside capture, a reprojection never deletes a row of this host, which capture dedups
    /// against: a series whose forgetting would (this host's own, or another's that added rows
    /// to this host's sessions) is left to the caller, and nothing is forgotten. Another host's
    /// series whose sessions hold no row of this host is forgotten there and then.
    #[rstest]
    #[tokio::test]
    async fn beside_capture_a_reprojection_never_forgets_this_hosts_rows() {
        let store = SqliteStore::in_memory(test_local_timeout()).await.unwrap();
        // `a` is this host.
        let [a, b, c] = <[_; 3]>::try_from(writers(&store, 3)).ok().unwrap();
        let (mine, shared, theirs) =
            (session_named("mine"), session_named("shared"), session_named("theirs"));
        push_range(&a, &mine, 0..2).await;
        // `b` captured `shared`, and `a` added a row to it.
        push_range(&b, &shared, 10..12).await;
        push_range(&a, &shared, 12..13).await;
        push_range(&c, &theirs, 20..22).await;
        let db = AiSessionDatabase::in_memory().await.unwrap();
        a.reproject(&db).await.unwrap();
        let delete_all_of = async |host: HostId| {
            for r in store.all_tagged(&RecordTag::AiSession).await.unwrap() {
                if r.host.id == host {
                    store.delete(r.id).await.unwrap();
                }
            }
        };

        // Host c's store is reset: its sessions hold no row of `a`, so it is forgotten.
        delete_all_of(c.host_id).await;
        push_range(&c, &session_named("again"), 30..31).await;
        a.reproject_beside_capture(&db).await.unwrap();
        assert_eq!(count(&db, &theirs).await, None);
        assert_eq!(count(&db, &session_named("again")).await, Some(1));

        // Host b's: forgetting it would delete a's row in `shared`. It is skipped, and the other
        // series projected.
        delete_all_of(b.host_id).await;
        push_range(&b, &session_named("anew"), 40..41).await;
        push_range(&c, &session_named("again"), 31..32).await;
        let err = a.reproject_beside_capture(&db).await.unwrap_err();
        assert!(matches!(err, BuildError::ForgetHeldOff(host) if host == b.host_id), "{err:?}");
        assert_eq!(count(&db, &shared).await, Some(3), "nothing was forgotten");
        assert_eq!(count(&db, &session_named("anew")).await, None);
        assert_eq!(count(&db, &session_named("again")).await, Some(2));
        // With capture held off, it is.
        a.reproject(&db).await.unwrap();
        assert_eq!(count(&db, &shared).await, Some(1), "a's row alone came back");

        // This host's own series rewritten under its watermark (reset, and pushed again).
        delete_all_of(a.host_id).await;
        push_range(&a, &mine, 0..2).await;
        let err = a.reproject_beside_capture(&db).await.unwrap_err();
        assert!(matches!(err, BuildError::ForgetHeldOff(host) if host == a.host_id), "{err:?}");
        assert_eq!(count(&db, &mine).await, Some(2), "nothing was forgotten");
        assert_eq!(count(&db, &shared).await, Some(1));
        a.reproject(&db).await.unwrap();
        assert_eq!(count(&db, &mine).await, Some(2));
        assert_eq!(count(&db, &shared).await, None, "its row went with the old series");
    }

    /// Forgetting a host deletes its sessions whole, rows other hosts added included, so those
    /// hosts are replayed too and their rows come back.
    #[rstest]
    #[tokio::test]
    async fn forgetting_a_host_replays_the_hosts_that_added_to_its_sessions() {
        let store = SqliteStore::in_memory(test_local_timeout()).await.unwrap();
        let [a, b] = <[_; 2]>::try_from(writers(&store, 2)).ok().unwrap();
        let (shared, other) = (session_named("shared"), session_named("other"));
        // `a` captured `shared` (its first row), and `b` added a row to it later.
        push_range(&a, &shared, 0..3).await;
        push_range(&b, &shared, 10..11).await;
        push_range(&b, &other, 20..22).await;
        let db = AiSessionDatabase::in_memory().await.unwrap();
        a.reproject(&db).await.unwrap();
        assert_eq!(count(&db, &shared).await, Some(4));

        // Host a's store is reset and starts over elsewhere.
        for r in store.all_tagged(&RecordTag::AiSession).await.unwrap() {
            if r.host.id == a.host_id {
                store.delete(r.id).await.unwrap();
            }
        }
        push_range(&a, &session_named("new"), 0..1).await;
        a.reproject(&db).await.unwrap();

        // `shared` went with host a, and came back with b's row alone.
        assert_eq!(count(&db, &shared).await, Some(1));
        let row = crate::ai_session::SourceId::from("source-10".to_owned());
        assert!(db.contains_message(&shared, &row).await.unwrap());
        assert_eq!(count(&db, &other).await, Some(2));
        assert_eq!(mark(&db, &b).await.map(|m| m.idx), Some(2));
    }

    /// Forgetting a host takes its rows out of the sessions other hosts captured too, and what
    /// they made of those sessions: once replayed, a session `a` started and `b` continued holds
    /// `a`'s rows alone, counted, timed, titled and searchable as `a` left it.
    #[rstest]
    #[tokio::test]
    async fn forgetting_a_host_removes_its_rows_from_other_hosts_sessions() {
        let store = SqliteStore::in_memory(test_local_timeout()).await.unwrap();
        let [a, b] = <[_; 2]>::try_from(writers(&store, 2)).ok().unwrap();
        let shared = session_named("shared");
        for i in 0..2 {
            let mut m = message_in(&shared, i, &format!("alpha words {i}"));
            m.session_title = Some("alpha title".to_owned());
            a.push(&m).await.unwrap();
        }
        let mut continued = message_in(&shared, 10, "bravo words");
        continued.session_title = Some("bravo title".to_owned());
        continued.usage = Some(atuin_common::harnesstools::session::Usage {
            output: Some(7),
            ..Default::default()
        });
        b.push(&continued).await.unwrap();
        let db = AiSessionDatabase::in_memory().await.unwrap();
        a.reproject(&db).await.unwrap();
        let before = db.get_session(&shared).await.unwrap().unwrap();
        assert_eq!((before.message_count, before.title.as_deref()), (3, Some("bravo title")));

        // Host b's store is reset and starts over elsewhere.
        for r in store.all_tagged(&RecordTag::AiSession).await.unwrap() {
            if r.host.id == b.host_id {
                store.delete(r.id).await.unwrap();
            }
        }
        push_range(&b, &session_named("new"), 0..1).await;
        a.reproject(&db).await.unwrap();

        let after = db.get_session(&shared).await.unwrap().unwrap();
        assert_eq!(after.message_count, 2);
        assert_eq!(after.title.as_deref(), Some("alpha title"));
        assert_eq!(after.updated_at, OffsetDateTime::UNIX_EPOCH + time::Duration::seconds(1));
        assert_eq!(after.usage.output.unwrap_or(0), 0, "b's call went with its row");
        assert_eq!(after.host, Some(a.host_id));
        let bravo = crate::ai_session::SourceId::from("source-10".to_owned());
        assert!(!db.contains_message(&shared, &bravo).await.unwrap());
        let search = |query: &'static str| {
            let db = db.clone();
            async move {
                let matches = db.search(
                    query,
                    crate::ai_session::SearchTerms::All,
                    &crate::ai_session::SessionFilter::default(),
                    0,
                );
                futures::TryStreamExt::try_collect::<Vec<_>>(matches).await.unwrap().len()
            }
        };
        assert_eq!(search("bravo").await, 0, "b's row left the index");
        assert_eq!(search("alpha").await, 1);
        assert_eq!(mark(&db, &a).await.map(|m| m.idx), Some(1));
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

    /// A record of `s`'s series at `idx`, as sync stores one it downloaded.
    async fn push_at(store: &SqliteStore, s: &AiSessionStore, idx: u64, msg: &Message) {
        let record = Record::builder()
            .id(RecordId(atuin_common::utils::uuid_v7()))
            .host(Host::new(s.host_id))
            .version(AiSessionRecord::VERSION)
            .tag(RecordTag::AiSession)
            .idx(idx)
            .data(DecryptedData(AiSessionRecord::Message(msg.clone()).serialize()))
            .build();
        store.push(&record.encrypt(&key())).await.unwrap();
    }

    /// A hole in a series (a record not downloaded yet) holds the watermark below it, so the
    /// record is replayed once it arrives; the records past it project meanwhile.
    #[rstest]
    #[tokio::test]
    async fn a_hole_in_a_series_holds_the_watermark_below_it() {
        let store = SqliteStore::in_memory(test_local_timeout()).await.unwrap();
        let [s] = <[_; 1]>::try_from(writers(&store, 1)).ok().unwrap();
        let handle = sample_handle();
        for idx in [0, 1, 3] {
            push_at(&store, &s, idx, &message_in(&handle, idx.cast_signed(), "text")).await;
        }

        let db = AiSessionDatabase::in_memory().await.unwrap();
        s.reproject(&db).await.unwrap();
        assert_eq!(count(&db, &handle).await, Some(3), "records past the hole still project");
        assert_eq!(mark(&db, &s).await.map(|m| m.idx), Some(1));

        push_at(&store, &s, 2, &message_in(&handle, 2, "text")).await;
        assert_eq!(s.reproject(&db).await.unwrap().replayed, 2);
        assert_eq!(count(&db, &handle).await, Some(4), "the late record is replayed");
        assert_eq!(mark(&db, &s).await.map(|m| m.idx), Some(3));
        assert_eq!(s.reproject(&db).await.unwrap().replayed, 0);
    }

    /// A series whose first records are missing holds its watermark at nothing.
    #[rstest]
    #[tokio::test]
    async fn a_hole_at_the_start_of_a_series_holds_the_watermark() {
        let store = SqliteStore::in_memory(test_local_timeout()).await.unwrap();
        let [s] = <[_; 1]>::try_from(writers(&store, 1)).ok().unwrap();
        let handle = sample_handle();
        push_at(&store, &s, 1, &message_in(&handle, 1, "text")).await;

        let db = AiSessionDatabase::in_memory().await.unwrap();
        s.reproject(&db).await.unwrap();
        assert_eq!(mark(&db, &s).await, None);
        push_at(&store, &s, 0, &message_in(&handle, 0, "text")).await;
        s.reproject(&db).await.unwrap();
        assert_eq!(count(&db, &handle).await, Some(2));
        assert_eq!(mark(&db, &s).await.map(|m| m.idx), Some(1));
    }

    /// This host's series is replayed under capture's lock, other hosts' without it.
    #[rstest]
    #[tokio::test]
    async fn reproject_takes_the_capture_lock_for_this_hosts_records() {
        let store = SqliteStore::in_memory(test_local_timeout()).await.unwrap();
        let [local, remote] = <[_; 2]>::try_from(writers(&store, 2)).ok().unwrap();
        let (mine, theirs) = (session_named("mine"), session_named("theirs"));
        push_range(&remote, &theirs, 0..3).await;
        let db = AiSessionDatabase::in_memory().await.unwrap();

        let capture = db.lock_local_projection().await;
        let short = std::time::Duration::from_millis(200);
        // Only other hosts' records: nothing waits on capture.
        tokio::time::timeout(short, local.reproject(&db)).await.unwrap().unwrap();
        assert_eq!(count(&db, &theirs).await, Some(3));

        // This host's own records (say, synced back after a reinstall) wait for capture.
        push_range(&local, &mine, 0..2).await;
        assert!(tokio::time::timeout(short, local.reproject(&db)).await.is_err());
        drop(capture);
        local.reproject(&db).await.unwrap();
        assert_eq!(count(&db, &mine).await, Some(2));
        assert_eq!(mark(&db, &local).await.map(|m| m.idx), Some(1));
    }

    /// A record this key cannot decrypt (a daemon still holding the key from before `atuin
    /// login` re-encrypted the store) holds the watermark below it until the right key projects
    /// it, and a change of key replays everything.
    #[rstest]
    #[tokio::test]
    async fn a_record_the_key_cannot_decrypt_holds_the_watermark_below_it() {
        let store = SqliteStore::in_memory(test_local_timeout()).await.unwrap();
        let host = hid();
        let with = |key: Key| {
            AiSessionStore::builder().store(store.clone()).host_id(host).key(key).build()
        };
        let (stale, current) = (with(key()), with(Key::from([1u8; 32])));
        let handle = sample_handle();
        push_range(&stale, &handle, 0..2).await;
        push_range(&current, &handle, 2..4).await;
        push_range(&stale, &handle, 4..5).await;

        let db = AiSessionDatabase::in_memory().await.unwrap();
        assert_eq!(stale.reproject(&db).await.unwrap().replayed, 5);
        assert_eq!(count(&db, &handle).await, Some(3), "what it can read still projects");
        assert_eq!(mark(&db, &stale).await.map(|m| m.idx), Some(1), "held below idx 2");
        // Held, so retried.
        assert_eq!(stale.reproject(&db).await.unwrap().replayed, 3);

        // The right key (after a restart): the key changed, so everything is replayed with it.
        assert_eq!(current.reproject(&db).await.unwrap().replayed, 5);
        assert_eq!(count(&db, &handle).await, Some(5));
        assert_eq!(current.reproject(&db).await.unwrap().replayed, 5, "idx 0, 1 and 4 hold it");
    }

    /// A record that decrypts, so is exactly what its writer wrote, but does not decode is
    /// passed over: no retry would read it.
    #[rstest]
    #[tokio::test]
    async fn an_authentic_but_undecodable_record_is_skipped() {
        let store = SqliteStore::in_memory(test_local_timeout()).await.unwrap();
        let [s] = <[_; 1]>::try_from(writers(&store, 1)).ok().unwrap();
        let handle = sample_handle();
        push_range(&s, &handle, 0..1).await;
        let record = Record::builder()
            .id(RecordId(atuin_common::utils::uuid_v7()))
            .host(Host::new(s.host_id))
            .version(RecordVersion::V0)
            .tag(RecordTag::AiSession)
            .idx(1)
            // A message kind, then a msgpack byte that is never valid.
            .data(DecryptedData(vec![0, 0xc1]))
            .build();
        store.push(&record.encrypt(&key())).await.unwrap();
        push_range(&s, &handle, 1..3).await;

        let db = AiSessionDatabase::in_memory().await.unwrap();
        assert_eq!(s.reproject(&db).await.unwrap().replayed, 4);
        assert_eq!(count(&db, &handle).await, Some(3));
        assert_eq!(mark(&db, &s).await.map(|m| m.idx), Some(3));
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

    /// A session is on the host of its earliest row, however its rows arrive: live, in order,
    /// or in a full replay, which takes one host's series at a time in host-id order.
    #[rstest]
    #[case::started_on_the_lower_host(true)]
    #[case::started_on_the_higher_host(false)]
    #[tokio::test]
    async fn a_session_is_on_the_host_of_its_earliest_row(#[case] started_low: bool) {
        let store = SqliteStore::in_memory(test_local_timeout()).await.unwrap();
        let low = HostId(uuid::Uuid::from_u128(1));
        let high = HostId(uuid::Uuid::from_u128(u128::MAX));
        let (first, then) = if started_low {
            (low, high)
        } else {
            (high, low)
        };
        let writer =
            |host| AiSessionStore::builder().store(store.clone()).host_id(host).key(key()).build();
        let (first_writer, then_writer) = (writer(first), writer(then));
        let session = session_named("spans-hosts");

        let live = AiSessionDatabase::in_memory().await.unwrap();
        // Rows projected before hosts were tracked, which a replay backfills.
        let hostless = AiSessionDatabase::in_memory().await.unwrap();
        for (host, s, range) in [(first, &first_writer, 0..3), (then, &then_writer, 3..6)] {
            for i in range {
                let msg = message_in(&session, i, &format!("message {i}"));
                s.push(&msg).await.unwrap();
                hostless.append(&msg).await.unwrap();
                live.append(&Message {
                    host: Some(host),
                    ..msg
                })
                .await
                .unwrap();
            }
        }
        assert_eq!(live.get_session(&session).await.unwrap().unwrap().host, Some(first));
        first_writer.reproject(&hostless).await.unwrap();
        assert_eq!(hostless.get_session(&session).await.unwrap().unwrap().host, Some(first));

        let replayed = AiSessionDatabase::in_memory().await.unwrap();
        first_writer.build(&replayed).await.unwrap();
        let sess = replayed.get_session(&session).await.unwrap().unwrap();
        assert_eq!((sess.host, sess.message_count), (Some(first), 6));

        // Forgetting the host that only continued it takes it whole, to be replayed from the
        // hosts with rows in it, and it comes back as it was.
        assert!(replayed.forget_host(then).await.unwrap(), "the first host is replayed too");
        assert!(replayed.get_session(&session).await.unwrap().is_none());
        first_writer.reproject(&replayed).await.unwrap();
        let sess = replayed.get_session(&session).await.unwrap().unwrap();
        assert_eq!((sess.host, sess.message_count), (Some(first), 6));
    }
}
