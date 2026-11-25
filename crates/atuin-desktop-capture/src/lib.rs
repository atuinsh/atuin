//! Captures desktop context through accessibility APIs.
//!
//! - [`Backend`] is the platform-neutral interface to the desktop: it lists applications and
//!   windows, reads window content as a tree of [`Node`]s, and reports window events.
//!   `Desktop` is the backend of the current platform, in the default `platform` feature.
//! - [`Capture`] follows the active window of a backend and yields [`Record`]s: activity
//!   segments, text snapshots, the dialogs that appear, the notifications that applications
//!   send, and the content of every window in the background, once at the start and again
//!   when it changes.
//! - [`render()`] turns a tree into text, and [`scrub`] removes secrets from URLs and titles.
//!
//! Capture needs the main thread on macOS: run the program inside
//! [`Platform::serve_main_thread`], called from `main`.
//!
//! ```no_run
//! # #[cfg(feature = "platform")]
//! # mod example {
//! use atuin_desktop_capture::{Backend as _, Capture, Config, Desktop, Purpose, Record, SnapshotContent};
//!
//! # pub async fn example() -> Result<(), atuin_desktop_capture::Error> {
//! let desktop = Desktop::connect(Purpose::Capture).await?;
//! desktop.resume();
//! let mut capture = Capture::new(desktop, Config::default());
//! while let Some(record) = capture.next().await {
//!     if let Record::Snapshot(snapshot) = record
//!         && let SnapshotContent::Changed(content) = snapshot.content
//!         && let Ok(root) = content.tree.decode()
//!     {
//!         println!("{}", atuin_desktop_capture::render(&root, usize::MAX).text);
//!     }
//! }
//! # Ok(())
//! # }
//! # }
//! ```

pub mod backend;
pub mod capture;
mod content;
pub mod encoding;
#[cfg(all(feature = "platform", target_os = "linux"))]
mod flat;
pub mod layout;
#[cfg(feature = "link")]
pub mod link;
pub mod outline;
#[cfg(feature = "platform")]
pub mod platform;
#[cfg(feature = "platform")]
mod private_windows;
pub mod readiness;
pub mod render;
pub mod scrub;
#[cfg(any(test, feature = "test-support"))]
pub mod testing;
#[cfg(all(feature = "platform", target_os = "linux"))]
mod urls;

pub use crate::backend::{
    Access, AppIdentity, AppInfo, Backend, Cause, Completeness, Error, Event, EventCount, Extent,
    Inspect, Limits, Loading, Mark, Node, Notice, Platform, PrivateWindows, ProcessApp, Purpose,
    ReadStats, Reading, ResponsibilityError, Role, Screen, Visibility, WindowClass, WindowKind,
};
pub use crate::capture::{
    AppFilter, Background, BackgroundReads, Capture, Change, Config, Content, ContentHash, Dialog,
    EncodedTree, HashMismatch, NextIds, Notification, Output, Parts, Record, Redact, Scope,
    Segment, SegmentId, Snapshot, SnapshotContent, Stopped, Timing, Trigger, WaitingEdit, WindowId,
};
pub use crate::layout::Frame;
pub use crate::outline::{Outline, OutlineEntry, Skip};
#[cfg(feature = "platform")]
pub use crate::platform::Desktop;
pub use crate::readiness::{
    AccessLifetime, Exposure, MissingAppNotes, Remedy, SessionVariable, UnreadyApp,
};
pub use crate::render::{Rendered, is_blank, render};
