use atuin_common::encryption::paseto_v4::{EncryptedData, Key};
use atuin_domain::record::{
    DecryptedData, Host, HostId, Record, RecordId, RecordIdx, RecordSeriesKey, RecordTag,
    RecordVersion,
};
use tracing::warn;
use typed_builder::TypedBuilder;

use crate::ai_session::Message;
use crate::ai_session::database::{AiSessionDatabase, DbError};
use crate::record::sqlite_store::SqliteStore;

/// Records a build reads from the store at a time.
const BUILD_BATCH: u64 = 1000;

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
    #[must_use]
    pub const fn host_id(&self) -> HostId {
        self.host_id
    }

    /// Append `msg` to this host's chain, returning the idx it took.
    pub async fn push(&self, msg: &Message) -> Result<RecordIdx, PushError> {
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
                return Ok(idx);
            }
        }
    }

    async fn decode_and_append(
        &self,
        record: Record<EncryptedData>,
        db: &AiSessionDatabase,
    ) -> Result<(), BuildError> {
        if record.tag != RecordTag::AiSession {
            return Ok(());
        }

        let (host, idx, id) = (record.host.id, record.idx, record.id);

        // A record that cannot be read is skipped for good, so it advances the watermark like a
        // projected one: reading it again on every build would only repeat the warning.
        match record.decrypt(&self.key) {
            Ok(decrypted) => match AiSessionRecord::deserialize(&decrypted.data.0) {
                Ok(AiSessionRecord::Message(msg)) => {
                    db.append(&msg).await?;
                }
                Err(err) => {
                    warn!(?err, id = %id.0, "failed to deserialize ai-session record, skipping");
                }
            },
            Err(err) => {
                warn!(?err, id = %id.0, "failed to decrypt ai-session record, skipping");
            }
        }

        db.advance_projected(host, idx).await?;
        Ok(())
    }

    /// Project every record `db` does not have yet: each host's chain from its watermark. An
    /// up-to-date sidecar costs one status query; one that missed an append (a crash between
    /// the record and sidecar writes, a lost db file) is repaired from where it stopped.
    pub async fn build(&self, db: &AiSessionDatabase) -> Result<(), BuildError> {
        let status = self.store.status().await?;

        let mut failure = None;
        for (host, tags) in status.hosts {
            let Some(&tail) = tags.get(&RecordTag::AiSession) else {
                continue;
            };
            let series = RecordSeriesKey::new(host, RecordTag::AiSession);

            let mut from = db.projected(host).await?;
            // A watermark past the chain means the store was reset under the sidecar.
            if from > tail + 1 {
                db.forget_projected(host).await?;
                from = 0;
            }

            loop {
                let batch = self.store.next(&series, from, BUILD_BATCH).await?;
                let Some(last) = batch.last().map(|r| r.idx) else {
                    break;
                };
                for record in batch {
                    let id = record.id;
                    // Continue repairing later rows, but report incomplete recovery so callers do
                    // not enable capture against a projection missing already-persisted messages.
                    if let Err(err) = self.decode_and_append(record, db).await {
                        warn!(
                            ?err,
                            id = %id.0,
                            "failed to append ai-session record to sidecar, skipping"
                        );
                        failure = Some(err);
                    }
                }
                from = last + 1;
            }
        }

        failure.map_or(Ok(()), Err)
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

#[cfg(test)]
mod tests {
    use atuin_common::harnesstools::session::{Content, Role};
    use atuin_domain::record::HostId;
    use proptest::prelude::*;
    use rstest::*;
    use time::OffsetDateTime;

    use super::{AiSessionRecord, AiSessionStore, Key, RecordTag, SqliteStore};
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
        let idx = s.push(&msg).await.unwrap();

        let recs = store.all_tagged(&RecordTag::AiSession).await.unwrap();
        assert_eq!(recs.len(), 1);
        assert_eq!(recs[0].idx, idx, "push must return the idx the record took");
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
            s.push(&m).await.unwrap();
            ids.push(m.id);
        }

        let db = AiSessionDatabase::in_memory().await.unwrap();
        s.incremental_build(&db, &ids).await;

        let sess = db.get_session(&sample_handle()).await.unwrap().unwrap();
        assert_eq!(usize::try_from(sess.message_count).unwrap(), ids.len());
        assert_eq!(db.projected(s.host_id()).await.unwrap(), 3, "sync advances the watermark");
    }

    #[rstest]
    #[tokio::test]
    async fn build_resumes_from_the_watermark() {
        let store = SqliteStore::in_memory(test_local_timeout()).await.unwrap();
        let s = AiSessionStore::builder().store(store).host_id(hid()).key(key()).build();
        let messages = ordered_messages(&sample_handle());
        for m in &messages[..2] {
            s.push(m).await.unwrap();
        }

        let db = AiSessionDatabase::in_memory().await.unwrap();
        s.build(&db).await.unwrap();
        assert_eq!(db.projected(s.host_id()).await.unwrap(), 2);

        // A sidecar whose watermark already covers the first two records never reads them again:
        // only the record past it is projected.
        s.push(&messages[2]).await.unwrap();
        let later = AiSessionDatabase::in_memory().await.unwrap();
        for idx in 0..2 {
            later.advance_projected(s.host_id(), idx).await.unwrap();
        }
        s.build(&later).await.unwrap();

        let sess = later.get_session(&sample_handle()).await.unwrap().unwrap();
        assert_eq!(sess.message_count, 1);
        assert_eq!(later.projected(s.host_id()).await.unwrap(), 3);
    }

    #[rstest]
    #[tokio::test]
    async fn build_starts_over_when_the_watermark_outruns_the_store() {
        let store = SqliteStore::in_memory(test_local_timeout()).await.unwrap();
        let s = AiSessionStore::builder().store(store).host_id(hid()).key(key()).build();
        for m in ordered_messages(&sample_handle()) {
            s.push(&m).await.unwrap();
        }

        let db = AiSessionDatabase::in_memory().await.unwrap();
        for idx in 0..10 {
            db.advance_projected(s.host_id(), idx).await.unwrap();
        }
        s.build(&db).await.unwrap();

        let sess = db.get_session(&sample_handle()).await.unwrap().unwrap();
        assert_eq!(sess.message_count, 3);
        assert_eq!(db.projected(s.host_id()).await.unwrap(), 3);
    }

    #[rstest]
    #[tokio::test]
    async fn watermark_only_advances_contiguously() {
        let db = AiSessionDatabase::in_memory().await.unwrap();
        let host = hid();

        db.advance_projected(host, 1).await.unwrap();
        assert_eq!(db.projected(host).await.unwrap(), 0, "nothing before idx 1 was projected");

        db.advance_projected(host, 0).await.unwrap();
        db.advance_projected(host, 2).await.unwrap();
        assert_eq!(db.projected(host).await.unwrap(), 1, "idx 1 is still missing");

        db.advance_projected(host, 1).await.unwrap();
        assert_eq!(db.projected(host).await.unwrap(), 2);
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
}
