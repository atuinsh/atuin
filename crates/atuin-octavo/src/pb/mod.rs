mod codegen {
    #![allow(clippy::must_use_candidate, clippy::derive_partial_eq_without_eq)]
    tonic::include_proto!("atuin.api.v1");
}

mod batch;
mod threads;

use atuin_domain::record::{HostId as DomainHostId, RecordId};
pub use batch::{Batch, ThreadBatch};
pub use codegen::*;
use thiserror::Error;
pub use threads::{Field, KeyError, ThreadMessageConversionError};
use uuid::{Uuid, Version};

/// An id of another version than UUIDv7, the only one Octavo stores.
#[derive(Debug, Error, PartialEq, Eq)]
#[error("id {0} is not a UUIDv7, so Atuin Hub can't store it")]
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

impl UuidV7 {
    /// Mint a new id for something the client asks Octavo to create.
    #[must_use]
    pub fn mint() -> Self {
        Self {
            value: atuin_common::utils::uuid_v7().into_bytes().to_vec(),
        }
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
