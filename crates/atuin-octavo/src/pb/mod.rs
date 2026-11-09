mod codegen {
    #![allow(clippy::must_use_candidate, clippy::derive_partial_eq_without_eq)]
    tonic::include_proto!("atuin.api.v1");
}

mod batch;
mod history;
mod threads;

use atuin_client::history::HistoryId;
use atuin_domain::record::{HostId as DomainHostId, RecordId};
pub use batch::{Batch, HistoryBatch, ThreadBatch};
pub use codegen::*;
pub use history::{HistoryConversionError, OutputTooLarge};
use thiserror::Error;
pub use threads::{Field, KeyError, ThreadMessageConversionError};
use uuid::{Uuid, Version};

/// An id of another version than UUIDv7, the only one Octavo stores.
#[derive(Debug, Error, PartialEq, Eq)]
#[error("id {0} is not a UUIDv7, so Octavo can't store it")]
pub struct NotUuidV7<Id>(pub Id);

impl TryFrom<Uuid> for UuidV7 {
    type Error = NotUuidV7<Uuid>;

    fn try_from(uuid: Uuid) -> Result<Self, Self::Error> {
        if uuid.get_version() != Some(Version::SortRand) {
            return Err(NotUuidV7(uuid));
        }

        Ok(Self {
            value: uuid.into_bytes().to_vec(),
        })
    }
}

impl TryFrom<HistoryId> for UuidV7 {
    type Error = NotUuidV7<HistoryId>;

    fn try_from(id: HistoryId) -> Result<Self, Self::Error> {
        Uuid::from_bytes(id.into_bytes()).try_into().map_err(|_| NotUuidV7(id))
    }
}

impl TryFrom<RecordId> for UuidV7 {
    type Error = NotUuidV7<RecordId>;

    fn try_from(id: RecordId) -> Result<Self, Self::Error> {
        id.0.try_into().map_err(|_| NotUuidV7(id))
    }
}

impl From<DomainHostId> for HostId {
    fn from(id: DomainHostId) -> Self {
        Self {
            value: id.0.into_bytes().to_vec(),
        }
    }
}
