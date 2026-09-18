mod codegen {
    #![allow(clippy::must_use_candidate)]
    #![allow(clippy::derive_partial_eq_without_eq)]
    tonic::include_proto!("ai_agent");
}

pub use codegen::*;

use crate::grpc::common::pb as common;
