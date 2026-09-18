mod codegen {
    #![allow(clippy::must_use_candidate)]
    #![allow(clippy::derive_partial_eq_without_eq)]
    #![allow(clippy::large_enum_variant)]
    tonic::include_proto!("ai_session");
}

pub use codegen::*;

use crate::grpc::ai_agent::pb as ai_agent;
