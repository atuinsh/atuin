use atuin_common::range::PyStyleIdxRange;
use atuin_domain::record::RecordId as DomainRecordId;

mod codegen {
    #![allow(clippy::must_use_candidate, reason = "prost-generated code")]
    tonic::include_proto!("common");
}

pub use codegen::*;

impl From<DomainRecordId> for RecordId {
    fn from(value: DomainRecordId) -> Self {
        Self {
            uuid: Some(Uuid {
                value: value.0.into_bytes().to_vec(),
            }),
        }
    }
}

impl From<SignedIdxRange> for PyStyleIdxRange {
    fn from(value: SignedIdxRange) -> Self {
        Self::new(value.start, value.end)
    }
}

impl From<PyStyleIdxRange> for SignedIdxRange {
    fn from(value: PyStyleIdxRange) -> Self {
        Self {
            start: value.start(),
            end: value.end(),
        }
    }
}
