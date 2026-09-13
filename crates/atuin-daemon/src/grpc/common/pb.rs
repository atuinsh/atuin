use std::ops::Range;

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

impl From<Range<usize>> for UnsignedIdxRange {
    fn from(range: Range<usize>) -> Self {
        Self {
            start: u64::try_from(range.start).unwrap_or(u64::MAX),
            end: u64::try_from(range.end).unwrap_or(u64::MAX),
        }
    }
}

impl TryFrom<UnsignedIdxRange> for Range<usize> {
    type Error = std::num::TryFromIntError;

    fn try_from(range: UnsignedIdxRange) -> Result<Self, Self::Error> {
        Ok(usize::try_from(range.start)?..usize::try_from(range.end)?)
    }
}
