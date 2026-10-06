//! Test-only hook points in the recovery protocol, where tests hold, watch or fault its steps.

use std::fmt::Debug;

use futures::future::BoxFuture;

/// Where a hook runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Point {
    /// Holding capture's lock, before wiping.
    WipeLocked,
    /// The wipe done, capture's lock released.
    AfterWipe,
    /// A replay's reprojection gave up, invalidated pass after pass (to count them).
    ReplayIncomplete,
    /// A replay has reprojected, before reporting. A panic fault panics it.
    ReplayBeforeSettle,
    /// Capture made its record pending, before pushing it. A panic fault panics the capture.
    CapturePushing,
    /// Capture pushed its record, before projecting it. A panic fault panics the capture.
    CapturePushed,
    /// The warm-up of a resumed session, before one of its steps. A fault fails the step.
    WarmRead(WarmStep),
}

/// A step of the warm-up of a resumed session (see [`Point::WarmRead`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WarmStep {
    /// Projecting the record a capture pushed but did not project.
    Repair,
    /// Reading the session row.
    Session,
    /// Reading its last message.
    Last,
    /// Reading its synthetic ids.
    Synthetic,
    /// Reading its title changes.
    Titles,
}

/// The payload of a panic [`Fault::Panic`] injects.
pub const INJECTED_PANIC: &str = "injected panic";

/// What a hook has the step do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Fault {
    None,
    Fail,
    Panic,
}

pub trait Hooks: Send + Sync + Debug {
    fn at(&self, point: Point) -> BoxFuture<'_, Fault>;

    /// The backoff the coordinator waits out between incomplete replays.
    fn backoff(&self) -> super::recovery::Backoff {
        super::recovery::Backoff::DEFAULT
    }

    /// The backoff a warm-up waits out between failed attempts.
    fn warm_backoff(&self) -> super::recovery::Backoff {
        super::recovery::Backoff::DEFAULT
    }
}
