pub mod pb;

// Brought into scope so the generated `ai.agent` code can resolve its
// cross-package references to the `common` package (e.g. `common.Uuid`).
use crate::grpc::common::pb as common;
