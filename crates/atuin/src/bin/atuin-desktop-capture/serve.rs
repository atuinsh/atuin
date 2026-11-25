//! The capture loop that the daemon drives over the helper's stdin and stdout.

use std::sync::Arc;

use atuin_common::rmp::frame::{FrameError, FrameWriter, forward_frames};
use atuin_common::secrets::Redactor;
use atuin_desktop_capture::link::{DaemonFrame, HelperFrame, HelperStatus, LinkConfig, Mode};
use atuin_desktop_capture::{
    AppFilter, Backend, Capture, Config, Limits, NextIds, Output, Parts, Record, Screen, Stopped,
    Timing, WaitingEdit,
};
use eyre::WrapErr as _;
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::sync::mpsc;
use tokio_util::task::AbortOnDropHandle;
use tracing::{debug, warn};

use crate::redact::AtuinRedactor;

/// How many commands from the daemon wait to be handled before the reader stops reading.
const QUEUED_COMMANDS: usize = 8;

/// How [`serve`] ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Served {
    /// The daemon sent `Shutdown` or closed the input.
    Finished,
    /// The desktop stopped sending events, as it does when the desktop session ends.
    DesktopEnded,
}

/// Captures from `backend` under the daemon's commands on `input`, and writes the records to
/// `output`.
///
/// Returns once the daemon sends `Shutdown` or closes `input`, or once the desktop stops.
///
/// # Errors
///
/// Fails when writing to `output` fails, and when the input fails or ends inside a frame.
pub async fn serve<B, R, W>(backend: B, input: R, output: W, build: &str) -> eyre::Result<Served>
where
    B: Backend,
    R: AsyncRead + Unpin + Send + 'static,
    W: AsyncWrite + Unpin,
{
    let mut output = FrameWriter::new(output);
    send(&mut output, &HelperFrame::Hello {
        build: build.to_owned(),
    })
    .await?;
    let mut commands = Commands::read(input);
    backend.pause();
    let mut phase = Phase::Unconfigured(Parts {
        backend,
        ids: NextIds::default(),
    });
    loop {
        let command = match phase.wait(&mut commands, &mut output).await? {
            Interrupt::Command(command) => command,
            Interrupt::DesktopEnded => return Ok(Served::DesktopEnded),
        };
        phase = match phase.handle(command, &mut output).await? {
            Flow::Continue(phase) => phase,
            Flow::Done(served) => return Ok(served),
        };
    }
}

/// What interrupted the capture loop.
enum Interrupt {
    Command(Command),
    /// The desktop stopped sending events.
    DesktopEnded,
}

/// What arrived from the daemon.
enum Command {
    Frame(DaemonFrame),
    /// A frame failed to read in a way that leaves the input at an unknown position.
    Unreadable(FrameError),
    /// The daemon closed the input.
    InputEnded,
}

/// Whether the capture loop goes on after a command.
#[expect(
    clippy::large_enum_variant,
    reason = "a flow is handled right away, so boxing the phase would only add an allocation"
)]
enum Flow<B: Backend> {
    Continue(Phase<B>),
    Done(Served),
}

/// Forwards the records of `capture` and the state of the screen to `output`, and answers pings
/// until another command arrives.
async fn next_command<B, W>(
    capture: &mut Capture<B>,
    commands: &mut Commands,
    output: &mut FrameWriter<W>,
) -> eyre::Result<Interrupt>
where
    B: Backend,
    W: AsyncWrite + Unpin,
{
    loop {
        // Dropping `capture.next()` can lose a window event, so a ping is answered while the
        // same `next()` waits. Every other command stops this capture, which makes the loss
        // acceptable.
        let mut next = std::pin::pin!(capture.next_output());
        let next = loop {
            tokio::select! {
                biased;
                command = commands.next() => match command {
                    Command::Frame(DaemonFrame::Ping) => send(output, &HelperFrame::Pong).await?,
                    command => return Ok(Interrupt::Command(command)),
                },
                next = &mut next => break next,
            }
        };
        match next {
            Some(Output::Record(record)) => send_record(output, record).await?,
            Some(Output::Screen(screen)) => {
                let status = match screen {
                    Screen::Locked => HelperStatus::ScreenLocked,
                    Screen::Unlocked => HelperStatus::ScreenUnlocked,
                };
                send(output, &HelperFrame::Status(status)).await?;
            }
            None => return Ok(Interrupt::DesktopEnded),
        }
    }
}

/// Discards the events of `backend` until a command arrives.
///
/// Reading the events keeps the platform's queue from filling up, and keeps the events of a
/// pause from being replayed once capture resumes.
async fn discard_until_command<B: Backend>(backend: &mut B, commands: &mut Commands) -> Interrupt {
    let mut discarded: u64 = 0;
    let interrupt = loop {
        tokio::select! {
            biased;
            command = commands.next() => break Interrupt::Command(command),
            event = backend.next() => match event {
                Some(_) => discarded += 1,
                None => break Interrupt::DesktopEnded,
            },
        }
    };
    if discarded > 0 {
        debug!("discarded {discarded} desktop events while capture was not running");
    }
    interrupt
}

/// The commands that the daemon sends.
struct Commands {
    frames: mpsc::Receiver<Result<DaemonFrame, FrameError>>,
    /// The task that reads the frames, so that the capture loop can wait on the channel.
    _reader: AbortOnDropHandle<()>,
}

impl Commands {
    fn read<R: AsyncRead + Unpin + Send + 'static>(input: R) -> Self {
        let (sender, frames) = mpsc::channel(QUEUED_COMMANDS);
        let reader = tokio::spawn(forward_frames(input, sender));
        Self {
            frames,
            _reader: AbortOnDropHandle::new(reader),
        }
    }

    /// Waits for the next command, or for the end of the input.
    async fn next(&mut self) -> Command {
        match self.frames.recv().await {
            Some(Ok(frame)) => Command::Frame(frame),
            Some(Err(error)) => Command::Unreadable(error),
            None => Command::InputEnded,
        }
    }
}

/// Where the link stands with respect to the daemon's commands.
#[expect(
    clippy::large_enum_variant,
    reason = "one phase lives at a time, so boxing the capture would only add an allocation"
)]
enum Phase<B: Backend> {
    /// Waiting for the first `Configure`.
    Unconfigured(Parts<B>),
    Running {
        capture: Capture<B>,
        /// The redactor that `capture` uses, which the capture hands out again when it stops.
        redactor: Arc<Redactor>,
    },
    Paused {
        parts: Parts<B>,
        settings: Settings,
    },
}

impl<B: Backend> Phase<B> {
    /// Waits for a command that this phase does not answer by itself, or for the end of the
    /// desktop.
    async fn wait<W>(
        &mut self,
        commands: &mut Commands,
        output: &mut FrameWriter<W>,
    ) -> eyre::Result<Interrupt>
    where
        W: AsyncWrite + Unpin,
    {
        match self {
            Self::Running { capture, .. } => next_command(capture, commands, output).await,
            Self::Unconfigured(Parts { backend, .. })
            | Self::Paused {
                parts: Parts { backend, .. },
                ..
            } => Ok(discard_until_command(backend, commands).await),
        }
    }

    /// Carries out `command`.
    async fn handle<W>(self, command: Command, output: &mut FrameWriter<W>) -> eyre::Result<Flow<B>>
    where
        W: AsyncWrite + Unpin,
    {
        let phase = match command {
            Command::Frame(DaemonFrame::Configure { config, mode }) => {
                self.configure(config, mode, output).await?
            }
            Command::Frame(DaemonFrame::Pause) => self.pause(output).await?,
            Command::Frame(DaemonFrame::Resume) => self.resume(output).await?,
            Command::Frame(DaemonFrame::Ping) => {
                send(output, &HelperFrame::Pong).await?;
                self
            }
            Command::Frame(DaemonFrame::Shutdown) | Command::InputEnded => {
                self.finish(output).await?;
                return Ok(Flow::Done(Served::Finished));
            }
            Command::Unreadable(error) => {
                self.finish(output).await?;
                return Err(error).wrap_err("reading a command from the daemon");
            }
        };
        Ok(Flow::Continue(phase))
    }

    /// Takes on `config`, and lands in `mode`. A change of mode sends the status that `pause`
    /// or `resume` sends.
    async fn configure<W>(
        self,
        config: LinkConfig,
        mode: Mode,
        output: &mut FrameWriter<W>,
    ) -> eyre::Result<Self>
    where
        W: AsyncWrite + Unpin,
    {
        let settings = Settings::from(config);
        let parts = match self {
            Self::Unconfigured(parts) => {
                send(output, &HelperFrame::Status(HelperStatus::Connected)).await?;
                parts
            }
            Self::Running { capture, .. } => {
                // The new configuration may leave out the followed window, so the edit that waits
                // there is dropped.
                let Stopped { parts, .. } = drain(capture, WaitingEdit::Discard, output).await?;
                if mode == Mode::Paused {
                    parts.backend.pause();
                    send(output, &HelperFrame::Status(HelperStatus::Paused)).await?;
                }
                parts
            }
            Self::Paused { parts, .. } => {
                if mode == Mode::Running {
                    send(output, &HelperFrame::Status(HelperStatus::Resumed)).await?;
                }
                parts
            }
        };
        Ok(match mode {
            Mode::Running => settings.start(parts),
            Mode::Paused => {
                parts.backend.pause();
                Self::Paused { parts, settings }
            }
        })
    }

    async fn pause<W: AsyncWrite + Unpin>(self, output: &mut FrameWriter<W>) -> eyre::Result<Self> {
        match self {
            Self::Running { capture, redactor } => {
                let Stopped { parts, config } = drain(capture, WaitingEdit::Read, output).await?;
                parts.backend.pause();
                send(output, &HelperFrame::Status(HelperStatus::Paused)).await?;
                Ok(Self::Paused {
                    parts,
                    settings: Settings { config, redactor },
                })
            }
            Self::Paused { .. } => {
                debug!("the daemon paused capture, which was paused already");
                Ok(self)
            }
            Self::Unconfigured(_) => {
                warn!("the daemon paused capture before configuring it");
                Ok(self)
            }
        }
    }

    async fn resume<W: AsyncWrite + Unpin>(
        self,
        output: &mut FrameWriter<W>,
    ) -> eyre::Result<Self> {
        match self {
            Self::Paused { parts, settings } => {
                send(output, &HelperFrame::Status(HelperStatus::Resumed)).await?;
                Ok(settings.start(parts))
            }
            Self::Running { .. } => {
                debug!("the daemon resumed capture, which was running already");
                Ok(self)
            }
            Self::Unconfigured(_) => {
                warn!("the daemon resumed capture before configuring it");
                Ok(self)
            }
        }
    }

    /// Ends capture, and sends the records that close it.
    ///
    /// The edit that waits in the followed window is dropped. The daemon shuts the helper down
    /// when capture is turned off, and a read after that would store what the user no longer
    /// wants captured.
    async fn finish<W: AsyncWrite + Unpin>(self, output: &mut FrameWriter<W>) -> eyre::Result<()> {
        if let Self::Running { capture, .. } = self {
            drain(capture, WaitingEdit::Discard, output).await?;
        }
        Ok(())
    }
}

/// Stops `capture`, sends the records that it still yields, and gives its backend, next ids
/// and configuration back.
async fn drain<B, W>(
    mut capture: Capture<B>,
    edit: WaitingEdit,
    output: &mut FrameWriter<W>,
) -> eyre::Result<Stopped<B>>
where
    B: Backend,
    W: AsyncWrite + Unpin,
{
    capture.stop(edit).await;
    while let Some(record) = capture.next().await {
        send_record(output, record).await?;
    }
    Ok(capture.into_stopped())
}

/// What one `Configure` asks for, in the form that a [`Capture`] takes.
struct Settings {
    config: Config,
    redactor: Arc<Redactor>,
}

impl From<LinkConfig> for Settings {
    fn from(config: LinkConfig) -> Self {
        let patterns = config.secret_patterns.iter().filter_map(|pattern| {
            regex::Regex::new(pattern)
                .inspect_err(|error| warn!("skipping the secret pattern {pattern:?}: {error}"))
                .ok()
        });
        Self {
            config: Config {
                timing: Timing {
                    poll_interval: config.poll_interval,
                    ..Timing::default()
                },
                limits: Limits {
                    budget: config.read_budget,
                    ..Limits::default()
                },
                filter: AppFilter {
                    scope: config.scope,
                    excluded: config.excluded,
                },
                private_windows: config.private_windows,
                ..Config::default()
            },
            redactor: Arc::new(Redactor::new(patterns.collect(), config.builtin_secrets)),
        }
    }
}

impl Settings {
    /// Resumes the backend, and starts a capture that continues the ids of `parts`, so that ids
    /// stay unique for as long as the helper runs.
    fn start<B: Backend>(self, parts: Parts<B>) -> Phase<B> {
        parts.backend.resume();
        let capture = Capture::resume(parts, self.config)
            .with_redactor(AtuinRedactor(Arc::clone(&self.redactor)));
        Phase::Running {
            capture,
            redactor: self.redactor,
        }
    }
}

/// Sends a record, and drops a record that cannot be written as a frame.
async fn send_record<W: AsyncWrite + Unpin>(
    output: &mut FrameWriter<W>,
    record: Record,
) -> eyre::Result<()> {
    match output.send(&HelperFrame::Record(record)).await {
        Err(error @ (FrameError::Encode(_) | FrameError::TooLarge { .. })) => {
            warn!("dropping a record: {error}");
            Ok(())
        }
        written => written.wrap_err("writing a record to the daemon"),
    }
}

async fn send<W: AsyncWrite + Unpin>(
    output: &mut FrameWriter<W>,
    frame: &HelperFrame,
) -> eyre::Result<()> {
    output.send(frame).await.wrap_err("writing to the daemon")
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::Ordering;
    use std::time::Duration;

    use atuin_common::rmp::frame::{read_frame, write_frame};
    use atuin_common::secrets::{BuiltinSecrets, REDACTED};
    use atuin_desktop_capture::link::{DaemonFrame, HelperFrame, HelperStatus, LinkConfig};
    use atuin_desktop_capture::testing::{Page, Script, at};
    use atuin_desktop_capture::{
        Cause, Event, PrivateWindows, Record, Scope, Screen, Segment, SnapshotContent, Trigger,
        render,
    };
    use rstest::rstest;
    use tokio::io::{AsyncWriteExt as _, DuplexStream};
    use tokio::sync::mpsc;
    use tokio::task::JoinHandle;

    use super::*;

    /// How many bytes each in-memory pipe buffers.
    const PIPE_BYTES: usize = 64 * 1024;

    /// The daemon's side of a link to a helper that serves a [`Script`].
    struct Link {
        commands: Option<DuplexStream>,
        frames: mpsc::UnboundedReceiver<HelperFrame>,
        served: JoinHandle<eyre::Result<Served>>,
    }

    impl Link {
        fn start(script: Script) -> Self {
            let (commands, input) = tokio::io::duplex(PIPE_BYTES);
            let (output, mut from_helper) = tokio::io::duplex(PIPE_BYTES);
            let served = tokio::spawn(Box::pin(serve(script, input, output, "test-build")));
            let (sender, frames) = mpsc::unbounded_channel();
            tokio::spawn(async move {
                while let Some(frame) = read_frame(&mut from_helper).await.expect("a valid frame") {
                    if sender.send(frame).is_err() {
                        break;
                    }
                }
            });
            Self {
                commands: Some(commands),
                frames,
                served,
            }
        }

        async fn send(&mut self, frame: DaemonFrame) {
            let commands = self.commands.as_mut().expect("the input is open");
            write_frame(commands, &frame).await.expect("the helper reads its input");
        }

        async fn next(&mut self) -> Option<HelperFrame> {
            self.frames.recv().await
        }

        /// Asserts that the helper writes nothing for `quiet`.
        async fn assert_quiet(&mut self, quiet: Duration) {
            if let Ok(frame) = tokio::time::timeout(quiet, self.next()).await {
                panic!("expected no frame, got {frame:?}");
            }
        }

        /// Returns the frames that the helper writes within `period`.
        async fn frames_within(&mut self, period: Duration) -> Vec<HelperFrame> {
            let mut frames = Vec::new();
            let _ = tokio::time::timeout(period, async {
                while let Some(frame) = self.next().await {
                    frames.push(frame);
                }
            })
            .await;
            frames
        }

        /// Returns the frames up to and including the first one that `wanted` accepts.
        async fn until(&mut self, wanted: impl Fn(&HelperFrame) -> bool) -> Vec<HelperFrame> {
            let mut seen = Vec::new();
            loop {
                let frame = self.next().await.expect("the wanted frame comes before the end");
                let found = wanted(&frame);
                seen.push(frame);
                if found {
                    return seen;
                }
            }
        }

        /// Reads the hello, configures the helper, and reads its status.
        async fn configure(&mut self, config: LinkConfig) {
            assert!(matches!(self.next().await, Some(HelperFrame::Hello { .. })));
            self.send(running(config)).await;
            assert_eq!(self.next().await, Some(HelperFrame::Status(HelperStatus::Connected)));
        }
    }

    fn running(config: LinkConfig) -> DaemonFrame {
        DaemonFrame::Configure {
            config,
            mode: Mode::Running,
        }
    }

    fn is_segment_started(frame: &HelperFrame) -> bool {
        matches!(frame, HelperFrame::Record(Record::SegmentStarted(_)))
    }

    fn is_segment_ended(frame: &HelperFrame) -> bool {
        matches!(frame, HelperFrame::Record(Record::SegmentEnded { .. }))
    }

    fn only(app: &str) -> LinkConfig {
        LinkConfig {
            scope: Scope::Only(vec![app.to_owned()]),
            excluded: Vec::new(),
            secret_patterns: Vec::new(),
            builtin_secrets: BuiltinSecrets::On,
            poll_interval: Timing::default().poll_interval,
            read_budget: Limits::default().budget,
            private_windows: PrivateWindows::default(),
        }
    }

    #[rstest]
    #[case::polling(Some(Duration::from_secs(5)))]
    #[case::no_polling(None)]
    fn the_configuration_sets_the_timing(#[case] poll_interval: Option<Duration>) {
        let settings = Settings::from(LinkConfig {
            poll_interval,
            read_budget: Duration::from_millis(150),
            private_windows: PrivateWindows::Capture,
            ..only("kate")
        });

        assert_eq!(settings.config.timing.poll_interval, poll_interval);
        assert_eq!(settings.config.limits.budget, Duration::from_millis(150));
        assert_eq!(settings.config.private_windows, PrivateWindows::Capture);
    }

    /// When the scripted desktop stops, in milliseconds. A backend stops after its last
    /// event, and a test ends long before this one.
    const END_MS: u64 = 3_600_000;

    /// A desktop on which gedit is active, and on which the user moves to Kate after
    /// `kate_at_ms`.
    fn desktop(kate_at_ms: u64) -> Script {
        Script {
            active: Some("gedit/1"),
            ..Script::new(
                [
                    at(kate_at_ms, Event::Activated { window: "kate/1" }),
                    at(END_MS, Event::Deactivated { window: "kate/1" }),
                ],
                [Page::new("gedit/1", "notes in gedit"), Page::new("kate/1", "notes in Kate")],
            )
        }
    }

    #[rstest]
    #[tokio::test(start_paused = true)]
    async fn hello_comes_first() {
        let mut link = Link::start(desktop(60_000));
        assert_eq!(
            link.next().await,
            Some(HelperFrame::Hello {
                build: "test-build".to_owned(),
            })
        );
    }

    #[rstest]
    #[tokio::test(start_paused = true)]
    async fn records_flow_after_configure() {
        let mut link = Link::start(desktop(60_000));
        assert!(matches!(link.next().await, Some(HelperFrame::Hello { .. })));
        link.assert_quiet(Duration::from_secs(2)).await;

        link.send(running(only("gedit"))).await;
        assert_eq!(link.next().await, Some(HelperFrame::Status(HelperStatus::Connected)));
        assert_eq!(next_segment(&mut link).await.app, "gedit");
    }

    /// The daemon configures a helper in the mode that the user asked for, so that a stopped
    /// capture reads nothing.
    #[rstest]
    #[tokio::test(start_paused = true)]
    async fn a_paused_configure_reads_nothing_until_resume() {
        let mut link = Link::start(desktop(60_000));
        assert!(matches!(link.next().await, Some(HelperFrame::Hello { .. })));

        link.send(DaemonFrame::Configure {
            config: only("gedit"),
            mode: Mode::Paused,
        })
        .await;
        assert_eq!(link.next().await, Some(HelperFrame::Status(HelperStatus::Connected)));
        link.assert_quiet(Duration::from_secs(5)).await;

        link.send(DaemonFrame::Resume).await;
        assert_eq!(link.next().await, Some(HelperFrame::Status(HelperStatus::Resumed)));
        assert_eq!(next_segment(&mut link).await.app, "gedit");
    }

    /// How a test makes the helper pause a running capture.
    #[derive(Debug, Clone, Copy)]
    enum Pausing {
        Pause,
        ConfigurePaused,
    }

    /// A backend can read the desktop on its own, so the helper keeps it paused whenever
    /// capture isn't running.
    #[rstest]
    #[case::pause(Pausing::Pause)]
    #[case::configure_paused(Pausing::ConfigurePaused)]
    #[tokio::test(start_paused = true)]
    async fn the_backend_runs_only_while_capture_runs(#[case] pausing: Pausing) {
        let script = desktop(END_MS - 1);
        let paused = Arc::clone(&script.paused);
        // A backend that was left running is paused before the daemon configures it.
        paused.store(false, Ordering::Relaxed);
        let mut link = Link::start(script);
        assert!(matches!(link.next().await, Some(HelperFrame::Hello { .. })));
        assert!(paused.load(Ordering::Relaxed), "unconfigured");

        link.send(running(only("gedit"))).await;
        link.until(is_segment_started).await;
        assert!(!paused.load(Ordering::Relaxed), "running");

        match pausing {
            Pausing::Pause => link.send(DaemonFrame::Pause).await,
            Pausing::ConfigurePaused => {
                link.send(DaemonFrame::Configure {
                    config: only("gedit"),
                    mode: Mode::Paused,
                })
                .await;
            }
        }
        link.until(|frame| *frame == HelperFrame::Status(HelperStatus::Paused)).await;
        assert!(paused.load(Ordering::Relaxed), "paused");

        link.send(DaemonFrame::Resume).await;
        link.until(is_segment_started).await;
        assert!(!paused.load(Ordering::Relaxed), "resumed");
    }

    #[rstest]
    #[tokio::test(start_paused = true)]
    async fn a_paused_configure_keeps_the_backend_paused() {
        let script = desktop(60_000);
        let paused = Arc::clone(&script.paused);
        let mut link = Link::start(script);
        assert!(matches!(link.next().await, Some(HelperFrame::Hello { .. })));

        link.send(DaemonFrame::Configure {
            config: only("gedit"),
            mode: Mode::Paused,
        })
        .await;
        assert_eq!(link.next().await, Some(HelperFrame::Status(HelperStatus::Connected)));
        link.assert_quiet(Duration::from_secs(1)).await;
        assert!(paused.load(Ordering::Relaxed));
    }

    #[rstest]
    #[tokio::test(start_paused = true)]
    async fn configure_changes_the_mode() {
        let mut link = Link::start(desktop(60_000));
        link.configure(only("gedit")).await;
        link.until(is_segment_started).await;

        link.send(DaemonFrame::Configure {
            config: only("gedit"),
            mode: Mode::Paused,
        })
        .await;
        let frames = link.until(|frame| matches!(frame, HelperFrame::Status(_))).await;
        assert!(frames.iter().any(is_segment_ended), "{frames:?}");
        assert_eq!(frames.last(), Some(&HelperFrame::Status(HelperStatus::Paused)));
        link.assert_quiet(Duration::from_secs(5)).await;

        link.send(running(only("gedit"))).await;
        assert_eq!(link.next().await, Some(HelperFrame::Status(HelperStatus::Resumed)));
        assert_eq!(next_segment(&mut link).await.app, "gedit");
    }

    #[rstest]
    #[tokio::test(start_paused = true)]
    async fn pause_ends_the_segment_and_resume_starts_one() {
        let mut link = Link::start(desktop(1_500));
        link.configure(LinkConfig {
            scope: Scope::All,
            ..only("gedit")
        })
        .await;
        assert_eq!(next_segment(&mut link).await.app, "gedit");

        link.send(DaemonFrame::Pause).await;
        let frames = link.until(|frame| matches!(frame, HelperFrame::Status(_))).await;
        let ended = frames.iter().position(is_segment_ended).expect("the segment ends");
        assert_eq!(frames.last(), Some(&HelperFrame::Status(HelperStatus::Paused)));
        assert!(ended < frames.len() - 1);
        // The user visits Kate during this time, and is back in gedit when capture resumes.
        link.assert_quiet(Duration::from_secs(2)).await;

        link.send(DaemonFrame::Resume).await;
        assert_eq!(link.next().await, Some(HelperFrame::Status(HelperStatus::Resumed)));
        assert_eq!(next_segment(&mut link).await.app, "gedit");
        let later = link.frames_within(Duration::from_secs(5)).await;
        assert!(!later.iter().any(is_segment_started), "{later:?}");
    }

    #[rstest]
    #[tokio::test(start_paused = true)]
    async fn configure_while_paused_applies_on_resume() {
        let mut link = Link::start(desktop(5_000));
        link.configure(only("gedit")).await;
        assert_eq!(next_segment(&mut link).await.app, "gedit");
        link.send(DaemonFrame::Pause).await;
        link.until(|frame| matches!(frame, HelperFrame::Status(_))).await;

        link.send(DaemonFrame::Configure {
            config: only("kate"),
            mode: Mode::Paused,
        })
        .await;
        link.assert_quiet(Duration::from_secs(1)).await;

        link.send(DaemonFrame::Resume).await;
        assert_eq!(link.next().await, Some(HelperFrame::Status(HelperStatus::Resumed)));
        assert_eq!(next_segment(&mut link).await.app, "kate");
    }

    #[rstest]
    #[tokio::test(start_paused = true)]
    async fn a_malformed_command_is_skipped() {
        let mut link = Link::start(desktop(60_000));
        link.configure(only("gedit")).await;
        link.until(is_segment_started).await;

        // 0xc1 is a byte that MessagePack never uses.
        let commands = link.commands.as_mut().expect("the input is open");
        commands.write_all(&[0, 0, 0, 1, 0xc1]).await.expect("the helper reads its input");
        link.send(DaemonFrame::Ping).await;

        let frames = link.until(|frame| *frame == HelperFrame::Pong).await;
        assert!(!frames.iter().any(is_segment_ended), "{frames:?}");
        assert!(!link.served.is_finished());
    }

    #[rstest]
    #[tokio::test(start_paused = true)]
    async fn a_truncated_command_fails_serve() {
        let mut link = Link::start(desktop(60_000));
        link.configure(only("gedit")).await;
        link.until(is_segment_started).await;

        let mut commands = link.commands.take().expect("the input is open");
        commands.write_all(&[0, 0, 0, 100, 0x01]).await.expect("the helper reads its input");
        drop(commands);
        let frames = link.frames_within(Duration::from_secs(5)).await;
        assert!(frames.iter().any(is_segment_ended), "{frames:?}");
        assert!(link.served.await.expect("serve does not panic").is_err());
    }

    #[rstest]
    #[tokio::test(start_paused = true)]
    async fn reconfigure_applies_the_new_scope() {
        let mut link = Link::start(desktop(5_000));
        link.configure(only("gedit")).await;
        assert_eq!(next_segment(&mut link).await.app, "gedit");

        link.send(running(only("kate"))).await;
        let frames = link.until(is_segment_started).await;
        assert!(frames.iter().any(is_segment_ended));
        assert!(!frames.iter().any(|frame| matches!(frame, HelperFrame::Status(_))));
        let Some(HelperFrame::Record(Record::SegmentStarted(segment))) = frames.last() else {
            unreachable!("`until` ends with the wanted frame");
        };
        assert_eq!(segment.app, "kate");
    }

    #[derive(Debug, Clone, Copy)]
    enum Interruption {
        /// A new configuration that leaves gedit out.
        Exclusion,
        Shutdown,
        Pause,
    }

    /// An edit that waits to be read when the daemon interrupts capture is read only for a
    /// pause, because a new configuration or a shutdown may leave the window out.
    #[rstest]
    #[case::exclusion(Interruption::Exclusion, false)]
    #[case::shutdown(Interruption::Shutdown, false)]
    #[case::pause(Interruption::Pause, true)]
    #[tokio::test(start_paused = true)]
    async fn an_interruption_reads_a_waiting_edit_only_for_a_pause(
        #[case] interruption: Interruption,
        #[case] read: bool,
    ) {
        let start = tokio::time::Instant::now();
        let edited = Event::ContentChanged {
            app: "gedit",
            window: Some("gedit/1"),
            cause: Cause::Edited,
        };
        let mut script = desktop(END_MS);
        script.events =
            [at(3_000, edited), at(END_MS, Event::Deactivated { window: "gedit/1" })].into();
        script.later_pages = vec![at(3_000, Page::new("gedit/1", "notes in gedit, edited"))];
        let mut link = Link::start(script);
        link.configure(only("gedit")).await;
        link.until(is_segment_started).await;
        // The edit waits to settle for 1.5 s.
        tokio::time::sleep_until(start + Duration::from_millis(3_500)).await;

        match interruption {
            Interruption::Exclusion => {
                link.send(running(LinkConfig {
                    excluded: vec!["gedit".to_owned()],
                    ..only("gedit")
                }))
                .await;
            }
            Interruption::Shutdown => link.send(DaemonFrame::Shutdown).await,
            Interruption::Pause => link.send(DaemonFrame::Pause).await,
        }
        let frames = link.frames_within(Duration::from_secs(5)).await;
        assert!(frames.iter().any(is_segment_ended), "{frames:?}");
        let edit_read = frames.iter().any(|frame| {
            matches!(frame, HelperFrame::Record(Record::Snapshot(snapshot))
                if snapshot.trigger == Trigger::Edited)
        });
        assert_eq!(edit_read, read, "{frames:?}");
    }

    #[derive(Debug, Clone, Copy)]
    enum Pinged {
        Unconfigured,
        Running,
        Paused,
    }

    /// The daemon pings the helper to tell whether the helper is still answering, so the helper
    /// answers in every phase, and a ping leaves a running capture as it was.
    #[rstest]
    #[case::unconfigured(Pinged::Unconfigured)]
    #[case::running(Pinged::Running)]
    #[case::paused(Pinged::Paused)]
    #[tokio::test(start_paused = true)]
    async fn ping_is_answered_in_every_phase(#[case] pinged: Pinged) {
        let mut link = Link::start(desktop(END_MS - 1));
        match pinged {
            Pinged::Unconfigured => {
                assert!(matches!(link.next().await, Some(HelperFrame::Hello { .. })));
            }
            Pinged::Running => {
                link.configure(only("gedit")).await;
                link.until(is_segment_started).await;
            }
            Pinged::Paused => {
                link.configure(only("gedit")).await;
                link.send(DaemonFrame::Pause).await;
                link.until(|frame| matches!(frame, HelperFrame::Status(_))).await;
            }
        }

        link.send(DaemonFrame::Ping).await;
        let frames = link.until(|frame| *frame == HelperFrame::Pong).await;

        let started = frames.iter().filter(|frame| is_segment_started(frame)).count();
        assert_eq!(started, 0, "a ping doesn't start a segment: {frames:?}");
        assert!(!frames.iter().any(is_segment_ended), "a ping doesn't end a segment: {frames:?}");
    }

    /// The phase that the helper is in when the desktop ends.
    #[derive(Debug, Clone, Copy)]
    enum Ended {
        Unconfigured,
        Running,
    }

    /// The desktop stops sending events when the desktop session ends, which tells the daemon
    /// to wait for the next session.
    #[rstest]
    #[case::unconfigured(Ended::Unconfigured)]
    #[case::running(Ended::Running)]
    #[tokio::test(start_paused = true)]
    async fn the_end_of_the_desktop_ends_serve(#[case] ended: Ended) {
        let mut link = Link::start(desktop(60_000));
        match ended {
            Ended::Unconfigured => {
                assert!(matches!(link.next().await, Some(HelperFrame::Hello { .. })));
            }
            Ended::Running => link.configure(only("gedit")).await,
        }

        let served = link.served.await.expect("serve does not panic").expect("serve succeeds");
        assert_eq!(served, Served::DesktopEnded);
    }

    /// How a test makes the helper start a new capture.
    #[derive(Debug, Clone, Copy)]
    enum Restart {
        PauseAndResume,
        Reconfigure,
    }

    /// Returns the next segment that the helper starts.
    async fn next_segment(link: &mut Link) -> Segment {
        match link.until(is_segment_started).await.pop() {
            Some(HelperFrame::Record(Record::SegmentStarted(segment))) => segment,
            frame => panic!("expected a segment, got {frame:?}"),
        }
    }

    /// The daemon keeps the ids of one helper for as long as the helper runs, so a new capture
    /// of the same helper must not hand out an id again.
    #[rstest]
    #[case::pause_and_resume(Restart::PauseAndResume)]
    #[case::reconfigure(Restart::Reconfigure)]
    #[tokio::test(start_paused = true)]
    async fn ids_stay_new_after_a_restart(#[case] restart: Restart) {
        let mut link = Link::start(desktop(END_MS - 1));
        link.configure(only("gedit")).await;
        let before = next_segment(&mut link).await;

        match restart {
            Restart::PauseAndResume => {
                link.send(DaemonFrame::Pause).await;
                link.until(|frame| matches!(frame, HelperFrame::Status(_))).await;
                link.send(DaemonFrame::Resume).await;
            }
            Restart::Reconfigure => {
                link.send(running(LinkConfig {
                    scope: Scope::All,
                    ..only("gedit")
                }))
                .await;
            }
        }
        let after = next_segment(&mut link).await;

        assert_eq!(after.app, "gedit");
        assert_ne!(after.id, before.id);
        assert_ne!(after.window, before.window);
    }

    #[derive(Debug, Clone, Copy)]
    enum Ending {
        Shutdown,
        EndOfInput,
    }

    #[rstest]
    #[case::shutdown(Ending::Shutdown)]
    #[case::end_of_input(Ending::EndOfInput)]
    #[tokio::test(start_paused = true)]
    async fn shutdown_drains(#[case] ending: Ending) {
        let mut link = Link::start(desktop(60_000));
        link.configure(only("gedit")).await;
        link.until(is_segment_started).await;

        match ending {
            Ending::Shutdown => link.send(DaemonFrame::Shutdown).await,
            Ending::EndOfInput => drop(link.commands.take()),
        }
        let mut frames = Vec::new();
        while let Some(frame) = link.next().await {
            frames.push(frame);
        }
        assert!(frames.iter().any(is_segment_ended), "{frames:?}");
        let served = link.served.await.expect("serve does not panic").expect("serve succeeds");
        assert_eq!(served, Served::Finished);
    }

    #[rstest]
    #[case::alone(&["hunter2"])]
    #[case::after_an_invalid_pattern(&["(", "hunter2"])]
    #[tokio::test(start_paused = true)]
    async fn secrets_are_redacted(#[case] patterns: &[&str]) {
        let mut link = Link::start(Script {
            active: Some("gedit/1"),
            ..Script::new([at(END_MS, Event::Deactivated { window: "gedit/1" })], [Page::new(
                "gedit/1",
                "the password is hunter2",
            )])
        });
        link.configure(LinkConfig {
            secret_patterns: patterns.iter().map(|&pattern| pattern.to_owned()).collect(),
            builtin_secrets: BuiltinSecrets::Off,
            ..only("gedit")
        })
        .await;

        let frames =
            link.until(|frame| matches!(frame, HelperFrame::Record(Record::Snapshot(_)))).await;
        let Some(HelperFrame::Record(Record::Snapshot(snapshot))) = frames.last() else {
            unreachable!("`until` ends with the wanted frame");
        };
        let SnapshotContent::Changed(content) = &snapshot.content else {
            panic!("the first snapshot of a segment has changed");
        };
        let root = content.tree.decode().expect("a captured tree decodes");
        let text = render(&root, usize::MAX).text;
        assert!(text.contains(REDACTED), "{text}");
        assert!(!text.contains("hunter2"), "{text}");
    }

    /// The daemon shows a locked screen in the state of capture, so the helper reports each
    /// change of the screen, after the end of the segment that the lock ended.
    #[rstest]
    #[tokio::test(start_paused = true)]
    async fn the_screen_goes_to_the_daemon() {
        let screen = |screen| Event::ScreenChanged { screen };
        let mut link = Link::start(Script {
            active: Some("gedit/1"),
            ..Script::new(
                [
                    at(5_000, screen(Screen::Locked)),
                    at(10_000, screen(Screen::Unlocked)),
                    at(END_MS, Event::Deactivated { window: "gedit/1" }),
                ],
                [Page::new("gedit/1", "notes in gedit")],
            )
        });
        link.configure(only("gedit")).await;

        let locked = HelperFrame::Status(HelperStatus::ScreenLocked);
        let frames = link.until(|frame| *frame == locked).await;
        assert!(is_segment_ended(&frames[frames.len() - 2]), "{frames:?}");
        let unlocked = HelperFrame::Status(HelperStatus::ScreenUnlocked);
        assert_eq!(link.next().await, Some(unlocked));
        next_segment(&mut link).await;
    }
}
