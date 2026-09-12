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
