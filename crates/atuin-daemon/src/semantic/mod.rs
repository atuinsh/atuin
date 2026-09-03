//! Semantic command capture gRPC service types.

// The prost/tonic code generator emits `#[derive(PartialEq)]` without `Eq`; we cannot annotate
// the generated types individually, so the lint is silenced for the whole generated module.
#![allow(clippy::derive_partial_eq_without_eq)]

tonic::include_proto!("semantic");
