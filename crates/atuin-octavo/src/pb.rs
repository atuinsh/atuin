mod codegen {
    #![allow(clippy::must_use_candidate, clippy::derive_partial_eq_without_eq)]
    tonic::include_proto!("atuin.api.v1");
}

use std::time::{Duration, SystemTime};

use atuin_client::history::{History as DomainHistory, HistoryId};
use atuin_domain::record::{CmdOrigin as DomainCmdOrigin, RecordId};
pub use codegen::*;
use prost::Message;
use thiserror::Error;
use uuid::{Uuid, Version};
use xxhash_rust::xxh3::xxh3_128;

#[derive(Debug, Error)]
#[error("history id {0} is not a UUIDv7, so Octavo can't store it")]
pub struct NotUuidV7(pub HistoryId);

impl TryFrom<HistoryId> for UuidV7 {
    type Error = NotUuidV7;

    fn try_from(id: HistoryId) -> Result<Self, Self::Error> {
        let bytes = id.into_bytes();
        if Uuid::from_bytes(bytes).get_version() != Some(Version::SortRand) {
            return Err(NotUuidV7(id));
        }

        Ok(Self {
            value: bytes.to_vec(),
        })
    }
}

impl From<RecordId> for UuidV7 {
    fn from(id: RecordId) -> Self {
        id.0.into()
    }
}

impl From<Uuid> for UuidV7 {
    fn from(uuid: Uuid) -> Self {
        Self {
            value: uuid.into_bytes().to_vec(),
        }
    }
}

impl From<&DomainCmdOrigin> for CmdOrigin {
    fn from(origin: &DomainCmdOrigin) -> Self {
        Self {
            host: origin.host().into_inner().to_owned(),
            user: origin.user().into_inner().to_owned(),
        }
    }
}

const MAX_AUTHOR_NAME_BYTES: usize = 255;
const MAX_SESSION_BYTES: usize = 128;
const MAX_HISTORY_BYTES: usize = 1 << 20;

#[derive(Debug, Error)]
pub enum HistoryConversionError {
    #[error(transparent)]
    Id(#[from] NotUuidV7),
    #[error("exit code {0} does not fit in 32 bits")]
    ExitOutOfRange(i64),
    #[error("invalid duration: {0}")]
    InvalidDuration(#[from] prost_types::DurationError),
    #[error("the author name is {0} bytes, over the {MAX_AUTHOR_NAME_BYTES} Octavo allows")]
    AuthorTooLong(usize),
    #[error("the entry is {0} bytes, over the {MAX_HISTORY_BYTES} Octavo takes")]
    TooLarge(usize),
}

impl TryFrom<DomainHistory> for History {
    type Error = HistoryConversionError;

    fn try_from(history: DomainHistory) -> Result<Self, Self::Error> {
        let exit = i32::try_from(history.exit)
            .map_err(|_| HistoryConversionError::ExitOutOfRange(history.exit))?;
        let duration = u64::try_from(history.duration)
            .ok()
            .map(|nanos| prost_types::Duration::try_from(Duration::from_nanos(nanos)))
            .transpose()?;
        if history.author.len() > MAX_AUTHOR_NAME_BYTES {
            return Err(HistoryConversionError::AuthorTooLong(history.author.len()));
        }
        let kind = if history.is_agent() {
            AuthorKind::Agent
        } else {
            AuthorKind::User
        };

        let converted = Self {
            id: Some(history.id.try_into()?),
            record_id: None,
            cmd_origin: Some(CmdOrigin::from(&history.cmd_origin)),
            start_time: Some(SystemTime::from(history.timestamp).into()),
            duration,
            session: session_key(history.session),
            author: Some(Author {
                name: history.author,
                kind: kind.into(),
            }),
            command: history.command,
            cwd: history.cwd,
            exit,
            intent: history.intent,
            shell: history.shell,
        };

        match converted.encoded_len() {
            len if len > MAX_HISTORY_BYTES => Err(HistoryConversionError::TooLarge(len)),
            _ => Ok(converted),
        }
    }
}

/// `session` as `History.session` can carry it: as is when it fits the bytes the field allows, or
/// else as the hex of its 128-bit xxh3 hash, which keeps two long sessions apart.
fn session_key(session: String) -> String {
    if session.len() <= MAX_SESSION_BYTES {
        return session;
    }

    format!("{:032x}", xxh3_128(session.as_bytes()))
}
