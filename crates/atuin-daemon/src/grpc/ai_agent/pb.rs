mod codegen {
    #![allow(clippy::must_use_candidate)]
    #![allow(clippy::derive_partial_eq_without_eq)]
    tonic::include_proto!("ai.agent");
}

pub use codegen::*;
