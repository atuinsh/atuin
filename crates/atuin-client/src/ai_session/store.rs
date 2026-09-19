use atuin_common::encryption::paseto_v4::Key;
use atuin_domain::record::{
    DecryptedData, Host, HostId, Record, RecordId, RecordSeriesKey, RecordTag, RecordVersion,
};
use typed_builder::TypedBuilder;

use crate::ai_session::Message;
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
        let id = RecordId(atuin_common::utils::uuid_v7());

        let mut msg = msg.clone();
        msg.id = id;

        let bytes = AiSessionRecord::Message(msg).serialize();
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
}

#[cfg(test)]
mod tests {
    use atuin_common::harnesstools::session::{Content, Role};
    use atuin_domain::record::{HarnessSession, HostId};
    use proptest::prelude::*;
    use rstest::*;
    use time::OffsetDateTime;

    use super::{AiSessionRecord, AiSessionStore, Key, RecordTag, SqliteStore};
    use crate::ai_session::{HarnessKind, Message, NativeSessionId, SourceId};

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
        let store = SqliteStore::in_memory(1).await;
        let s = AiSessionStore::builder().store(store.clone()).host_id(hid()).key(key()).build();
        let msg = sample_message();
        let id = s.push(&msg).await.unwrap();

        let recs = store.all_tagged(&RecordTag::AiSession).await.unwrap();
        assert_eq!(recs.len(), 1);
        let decrypted = recs[0].decrypt(&key()).unwrap();
        let AiSessionRecord::Message(got) =
            AiSessionRecord::deserialize(&decrypted.data.0).unwrap();
        assert_eq!(got.id, id);
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
}
