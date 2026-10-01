//! Test-only hook points in the recovery protocol, for tests to delay, fail or panic its steps
//! and to watch which run at once (see the chaos test).

use std::fmt::Debug;

use futures::future::BoxFuture;

/// Where a hook runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Point {
    /// A rebuild's wipe, before taking capture's lock.
    BeforeWipe,
    /// Holding capture's lock, before wiping. A fault fails the wipe (deleting nothing).
    WipeLocked,
    /// The wipe done, capture's lock released.
    AfterWipe,
    /// A replay's reprojection gave up, invalidated pass after pass (to count them).
    ReplayIncomplete,
    /// A replay has reprojected, before reporting. A fault fails, gives up or panics it.
    ReplayBeforeSettle,
    /// Capture found the store ready, before taking its lock.
    CaptureWaited,
    /// Capture found a rebuild had begun by the time it had its lock, and waits again.
    CaptureOvertaken,
    /// Capture found the store ready under its lock, before its dedup check and push.
    CaptureChecked,
    /// Capture made its record pending, before pushing it. A panic fault panics the capture.
    CapturePushing,
    /// Capture pushed its record, before projecting it. A panic fault panics the capture.
    CapturePushed,
    /// The sync worker, before reprojecting downloaded records.
    SyncReproject,
    /// The sync worker's reprojection found a series it must not forget beside capture, before
    /// telling the coordinator.
    SyncForgetHeldOff,
    /// The sync worker's reprojection gave up, invalidated pass after pass.
    SyncIncomplete,
    /// The coordinator began waiting out a backoff before replaying again, after an incomplete
    /// replay.
    BackoffStarted,
    /// A wipe cut that backoff short: the coordinator replays at once.
    BackoffCutShort,
    /// Capture's warm-up of a resumed session found the store ready under capture's lock,
    /// before reading the sidecar.
    WarmChecked,
}

/// The payload of a panic [`Fault::Panic`] injects.
pub const INJECTED_PANIC: &str = "injected panic";

/// What a hook has the step do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Fault {
    None,
    Fail,
    /// For a replay: give up as invalidated too often.
    Incomplete,
    Panic,
}

/// A stretch of the protocol that must not overlap some others.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Section {
    /// A replay task, from start to end (panics included).
    Replay,
    /// A wipe, under capture's lock.
    Wipe,
    /// A capture from finding the store ready under its lock to its push.
    Capture,
}

pub trait Hooks: Send + Sync + Debug {
    fn at(&self, point: Point) -> BoxFuture<'_, Fault>;

    /// Entered `section`, until the guard returned is dropped.
    fn enter(&self, section: Section) -> Box<dyn Send>;

    /// The backoff the coordinator waits out between incomplete replays.
    fn backoff(&self) -> super::recovery::Backoff {
        super::recovery::Backoff::DEFAULT
    }
}
