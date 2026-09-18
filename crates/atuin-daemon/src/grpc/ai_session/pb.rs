mod codegen {
    #![allow(clippy::must_use_candidate)]
    #![allow(clippy::derive_partial_eq_without_eq)]
    #![allow(clippy::large_enum_variant)]
    tonic::include_proto!("ai.session");
}

pub use codegen::*;

// The generated `ai.session` code refers to the `ai.agent` package by its final
// path component (`agent`); alias it here so those references resolve.
use crate::grpc::ai_agent::pb as agent;
