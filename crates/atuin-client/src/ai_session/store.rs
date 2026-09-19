use atuin_common::encryption::paseto_v4::{EncryptedData, Key};
use atuin_domain::record::{
    DecryptedData, Host, HostId, Record, RecordId, RecordSeriesKey, RecordTag, RecordVersion,
};
use tracing::warn;
use typed_builder::TypedBuilder;

use crate::ai_session::Message;
use crate::ai_session::database::{AiSessionDatabase, DbError};
use crate::record::sqlite_store::SqliteStore;

#[derive(Debug, Clone, TypedBuilder)]
pub struct AiSessionStore {
    store: SqliteStore,
    host_id: HostId,
    key: Key,
}

#[derive(Debug, Clone, PartialEq)]
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

    pub fn serialize(&self) -> Vec<u8> {
        match self {
            Self::Message(msg) => {
                let mut out = vec![Self::MESSAGE_KIND];
                out.extend(rmp_serde::to_vec(msg).expect("Message is always serializable"));
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
    ) -> Result<(), BuildError> {
        if record.tag != RecordTag::AiSession {
            return Ok(());
        }

        let id = record.id;

        let decrypted = match record.decrypt(&self.key) {
            Ok(decrypted) => decrypted,
            Err(err) => {
                warn!(?err, id = %id.0, "failed to decrypt ai-session record, skipping");
                return Ok(());
            }
        };

        let AiSessionRecord::Message(msg) = match AiSessionRecord::deserialize(&decrypted.data.0)
        {
            Ok(record) => record,
            Err(err) => {
                warn!(?err, id = %id.0, "failed to deserialize ai-session record, skipping");
                return Ok(());
            }
        };

        db.append(&msg).await?;
        Ok(())
    }

    pub async fn build(&self, db: &AiSessionDatabase) -> Result<(), BuildError> {
        let records = self.store.all_tagged(&RecordTag::AiSession).await?;

        for record in records {
            self.decode_and_append(record, db).await?;
        }

        Ok(())
    }

    pub async fn incremental_build(
        &self,
        db: &AiSessionDatabase,
        ids: &[RecordId],
    ) -> Result<(), BuildError> {
        for id in ids {
            let record = match self.store.get(*id).await {
                Ok(record) => record,
                Err(err) => {
                    warn!(?err, id = %id.0, "failed to load ai-session record, skipping");
                    continue;
                }
            };

            self.decode_and_append(record, db).await?;
        }

        Ok(())
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
        s.incremental_build(&db, &ids).await.unwrap();

        let sess = db.get_session(&sample_handle()).await.unwrap().unwrap();
        assert_eq!(sess.message_count as usize, ids.len());
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
        assert_eq!(sess.message_count as usize, messages.len());
    }
}
