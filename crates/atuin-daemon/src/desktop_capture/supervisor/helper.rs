//! One run of the helper: its process, its link, and its records.

use std::process::ExitStatus;
use std::time::Duration;

use atuin_common::rmp::frame::{FrameError, FrameWriter, forward_frames};
use atuin_desktop_capture::link::process::CaptureExit;
use atuin_desktop_capture::link::{DaemonFrame, HelperFrame, HelperStatus, LinkConfig, Mode};
use futures::future::BoxFuture;
use tokio::io::AsyncWrite;
use tokio::sync::mpsc;
use tokio::time::{Instant, timeout};
use tokio_util::task::AbortOnDropHandle;
use tracing::{debug, warn};

use super::link::{LinkState, LivenessCheck};
use super::opened::Opened;
use super::process::HelperProcess;
use super::records::RunWriter;
use crate::desktop_capture::stop_file::Requested;
use crate::grpc::versioning::BUILD_STR;

/// How long a helper has to exit after `Shutdown`, before it is killed.
pub const SHUTDOWN_GRACE: Duration = Duration::from_secs(2);

/// How long the helper has to take a frame that the supervisor sends.
///
/// A helper that stops reading its input is hung, and a send to it would wait forever once the
/// pipe is full.
pub const SEND_TIMEOUT: Duration = Duration::from_secs(10);

/// How many frames from the helper wait to be handled before the reader stops reading.
///
/// A frame can hold up to [`MAX_FRAME_BYTES`](atuin_common::rmp::frame::MAX_FRAME_BYTES), and a
/// start-up sweep sends many large ones at once, so the queue stays short. A full queue stops
/// the reader, and the helper's writes then wait for the store, which slows capture down
/// rather than growing the daemon's memory.
const QUEUED_FRAMES: usize = 4;

/// How the helper ended its run, or made the supervisor end it.
pub enum HelperEnd {
    /// The helper is from another build.
    Mismatch(String),
    NeedsPermission,
    /// No desktop session is running, or the session has ended.
    NoDesktop,
    Failed(String),
    /// The helper did not answer a ping in time.
    Unanswered,
}

/// One run of the helper.
pub struct Helper {
    stdin: FrameWriter<Box<dyn AsyncWrite + Send + Unpin>>,
    /// The frames from the helper, which a task of their own reads.
    frames: mpsc::Receiver<Result<HelperFrame, FrameError>>,
    _reader: AbortOnDropHandle<()>,
    exit: BoxFuture<'static, std::io::Result<ExitStatus>>,
    link: LinkState,
    records: RunWriter,
}

/// What a frame from the helper means for the supervisor.
pub enum Received {
    Nothing,
    /// The helper connected to the desktop and is capturing.
    Connected,
    /// The helper lacks the permission to read the desktop.
    NeedsPermission,
    /// The screen locked, and the helper doesn't read anything until the screen unlocks.
    ScreenLocked,
    /// The screen unlocked, and the helper is capturing again.
    ScreenUnlocked,
    /// The run must end.
    Ended(HelperEnd),
}

impl Helper {
    pub fn new(process: HelperProcess) -> Self {
        let HelperProcess {
            stdin,
            stdout,
            exit,
        } = process;
        let (sender, frames) = mpsc::channel(QUEUED_FRAMES);
        let reader = tokio::spawn(forward_frames(stdout, sender));
        Self {
            stdin: FrameWriter::new(stdin),
            frames,
            _reader: AbortOnDropHandle::new(reader),
            exit,
            link: LinkState::new(),
            records: RunWriter::default(),
        }
    }

    /// The next frame from the helper. `None` once the helper has closed its output.
    pub async fn frame(&mut self) -> Option<Result<HelperFrame, FrameError>> {
        self.frames.recv().await
    }

    /// Handles one frame from the helper.
    pub async fn receive(
        &mut self,
        frame: HelperFrame,
        opened: &Opened,
        config: &LinkConfig,
        requested: Requested,
    ) -> Received {
        match frame {
            HelperFrame::Hello { build } => {
                if let Some(ended) = self.hello(&build, config, requested).await {
                    return Received::Ended(ended);
                }
            }
            HelperFrame::Status(HelperStatus::Connected) => {
                self.link.connected();
                return Received::Connected;
            }
            HelperFrame::Status(HelperStatus::NeedsPermission) => {
                self.link.lacks_permission();
                return Received::NeedsPermission;
            }
            HelperFrame::Status(status @ (HelperStatus::Paused | HelperStatus::Resumed)) => {
                debug!(?status, "the helper answered");
            }
            HelperFrame::Status(HelperStatus::ScreenLocked) => return Received::ScreenLocked,
            HelperFrame::Status(HelperStatus::ScreenUnlocked) => return Received::ScreenUnlocked,
            HelperFrame::Record(record) => self.records.write(opened, requested, record).await,
            HelperFrame::Pong => {
                if self.link.pong().is_err() {
                    debug!("the helper sent a pong without a ping to answer");
                }
            }
        }
        Received::Nothing
    }

    /// Ends the background segments that the run has not written to for a while.
    pub async fn end_idle_background(&mut self, opened: &Opened) {
        self.records.end_idle_background(opened).await;
    }

    /// Ends every background segment of the run now. The next read of a window starts a new
    /// segment.
    pub async fn end_background(&mut self, opened: &Opened) {
        self.records.end_background(opened).await;
    }

    /// Whether the helper has yet to say hello.
    pub fn awaits_hello(&self) -> bool {
        self.link.awaits_hello()
    }

    /// Whether the helper reported how its start went: it connected, or it lacks the
    /// permission.
    pub fn has_outcome(&self) -> bool {
        self.link.has_outcome()
    }

    /// When the supervisor next pings the helper, or gives up on it.
    pub fn liveness_due(&self) -> Option<Instant> {
        self.link.liveness_due()
    }

    /// Sends `frame`. Returns [`HelperEnd::Unanswered`] when the helper does not take the frame
    /// within [`SEND_TIMEOUT`].
    ///
    /// A helper that closed its input closes its output soon after, which ends the run, so a
    /// failed send doesn't need handling of its own.
    async fn send(&mut self, frame: &DaemonFrame<&LinkConfig>) -> Option<HelperEnd> {
        match timeout(SEND_TIMEOUT, self.stdin.send(frame)).await {
            Ok(Ok(())) => None,
            Ok(Err(err)) => {
                debug!(?err, "could not send a frame to the helper");
                None
            }
            Err(_) => {
                warn!(
                    "the helper didn't take a frame within {} s, so the helper counts as hung",
                    SEND_TIMEOUT.as_secs()
                );
                Some(HelperEnd::Unanswered)
            }
        }
    }

    /// Answers the helper's hello: configures a helper of this build with `config`, in the mode
    /// of `requested`.
    async fn hello(
        &mut self,
        build: &str,
        config: &LinkConfig,
        requested: Requested,
    ) -> Option<HelperEnd> {
        if !self.link.awaits_hello() {
            warn!("the helper said hello twice");
            return None;
        }
        if build != BUILD_STR {
            return Some(HelperEnd::Mismatch(format!(
                "helper build {build}, daemon build {BUILD_STR}: install the \
                 atuin-desktop-capture of the daemon's build, and capture starts again by itself"
            )));
        }
        self.link.configured(requested);
        self.send(&DaemonFrame::Configure {
            config,
            mode: mode(requested),
        })
        .await
    }

    /// Sends new settings to a helper that is configured already, in the mode that the helper
    /// was last told. Returns how the run ends when the helper doesn't take the settings.
    pub async fn reconfigure(&mut self, config: &LinkConfig) -> Option<HelperEnd> {
        let told = self.link.told()?;
        self.send(&DaemonFrame::Configure {
            config,
            mode: mode(told),
        })
        .await
    }

    /// Sends `Pause` or `Resume` when the helper was told otherwise. Returns how the run ends
    /// when the helper does not take the frame.
    pub async fn tell(&mut self, requested: Requested) -> Option<HelperEnd> {
        let frame = self.link.tell(requested)?;
        self.send(&frame).await
    }

    /// Pings the helper, or gives up on the helper if the helper did not answer the last ping.
    pub async fn check_liveness(&mut self) -> Option<HelperEnd> {
        match self.link.check_liveness() {
            LivenessCheck::Unanswered => Some(HelperEnd::Unanswered),
            LivenessCheck::Ping => self.send(&DaemonFrame::Ping).await,
        }
    }

    /// Asks the helper to exit, stores the records that the helper sends meanwhile, and kills the
    /// helper if the helper doesn't exit in time.
    pub async fn stop(&mut self, opened: &Opened, requested: Requested) {
        let graceful = async {
            let _ = self.send(&DaemonFrame::Shutdown).await;
            while let Some(frame) = self.frames.recv().await {
                if let Ok(HelperFrame::Record(record)) = frame {
                    self.records.write(opened, requested, record).await;
                }
            }
            (&mut self.exit).await
        };
        match timeout(SHUTDOWN_GRACE, graceful).await {
            Ok(Ok(status)) => debug!(%status, "the helper exited"),
            Ok(Err(err)) => warn!(?err, "could not wait for the helper"),
            Err(_) => warn!(
                "the helper did not exit within {} s of shutdown, so it is killed",
                SHUTDOWN_GRACE.as_secs()
            ),
        }
    }

    /// Waits for the helper to exit after the helper has closed its output, and tells how the run
    /// ended.
    pub async fn exited(&mut self) -> HelperEnd {
        let status = timeout(SHUTDOWN_GRACE, &mut self.exit).await;
        if self.link.needs_permission() {
            return HelperEnd::NeedsPermission;
        }
        match status {
            Ok(Ok(status)) => match status.code().and_then(CaptureExit::from_code) {
                Some(CaptureExit::NeedsPermission) => HelperEnd::NeedsPermission,
                Some(CaptureExit::NoDesktop) => HelperEnd::NoDesktop,
                None => HelperEnd::Failed(format!("the helper exited with {status}")),
            },
            Ok(Err(err)) => HelperEnd::Failed(format!("could not wait for the helper: {err}")),
            Err(_) => HelperEnd::Failed("the helper closed its output without exiting".to_owned()),
        }
    }

    /// Whether the helper stayed connected for at least `steady`.
    pub fn ran_for(&self, steady: Duration) -> bool {
        self.link.connected_at().is_some_and(|at| at.elapsed() >= steady)
    }
}

/// The mode that a helper runs in when the user asked for `requested`.
const fn mode(requested: Requested) -> Mode {
    match requested {
        Requested::Running => Mode::Running,
        Requested::Stopped => Mode::Paused,
    }
}
