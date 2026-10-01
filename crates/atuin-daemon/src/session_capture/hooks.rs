//! Test-only hook points in the recovery protocol, where tests hold, watch or fault its steps.

use std::fmt::Debug;

use futures::future::BoxFuture;

/// Where a hook runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Point {
    /// A replay has reprojected, before reporting. A panic fault panics it.
    ReplayBeforeSettle,
}

/// The payload of a panic [`Fault::Panic`] injects.
pub const INJECTED_PANIC: &str = "injected replay panic";

/// What a hook has the step do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Fault {
    None,
    Panic,
}

pub trait Hooks: Send + Sync + Debug {
    fn at(&self, point: Point) -> BoxFuture<'_, Fault>;
}
