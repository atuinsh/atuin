//! Generated protobuf types for the `octavo` gRPC service.

// Required by prost-generated code to resolve imported `ai.agent` messages.
mod ai {
    pub use crate::grpc::ai::agent::pb as agent;
}

mod codegen {
    #![allow(clippy::must_use_candidate, reason = "prost-generated code")]
    #![allow(clippy::derive_partial_eq_without_eq, reason = "prost-generated code")]
    tonic::include_proto!("octavo");
}

pub use codegen::*;
