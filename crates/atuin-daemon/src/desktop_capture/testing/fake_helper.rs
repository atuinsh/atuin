//! A fake helper that speaks the protocol over in-memory pipes.

use std::collections::VecDeque;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::process::ExitStatus;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, SystemTime};

use atuin_common::rmp::frame::{read_frame, write_frame};
use atuin_desktop_capture::Record;
use atuin_desktop_capture::link::process::CaptureExit;
use atuin_desktop_capture::link::{DaemonFrame, HelperFrame, HelperStatus, LinkConfig, Mode};
use futures::future::{self, BoxFuture};
use parking_lot::Mutex;
use tokio::io::{AsyncWriteExt as _, DuplexStream};
use tokio::time::Instant;
use tokio_util::task::AbortOnDropHandle;

use super::records::{background, ended, notification, snapshot, started, window};
use crate::desktop_capture::Spawn;
use crate::desktop_capture::signing::SigningError;
use crate::desktop_capture::supervisor::HelperProcess;
use crate::grpc::versioning::BUILD_STR;

/// How a fake helper behaves.
#[derive(Debug, Clone, Copy)]
pub enum Script {
    /// Answers every command, as the real helper does.
    Serve,
    /// Says hello with another build, and then serves.
    OtherBuild,
    /// Reports the missing permission and exits with status 3.
    NoPermission,
    /// Sends one segment with one snapshot after the first `Configure`, and exits with
    /// status 1.
    CrashAfterSnapshot,
    /// Does not start, as when the executable is missing.
    Missing,
    /// Fails before the start, as when the signing of the helper fails.
    SigningFails,
    /// Exits with the status of `CaptureExit::NoDesktop` before its hello, as when no
    /// desktop session is running.
    NoDesktop,
    /// Exits as `NoDesktop` does, 5 s after its start, as a helper does that waits for the
    /// desktop's answer.
    SlowNoDesktop,
    /// Serves, and reports `Connected` 5 s after the first `Configure`, as a helper does that
    /// waits for the desktop's answer after its hello.
    SlowConnect,
    /// Serves until the first `Configure`, and then exits with the status of
    /// `CaptureExit::NoDesktop`, as when the desktop session ends.
    DesktopEnds,
    /// Serves, and sends a segment with one snapshot and a notification after the first
    /// `Configure`. It ends the segment when it pauses.
    Segment,
    /// Serves as `Segment` does, and sends a frame whose body is not a valid message before
    /// the segment.
    Garbled,
    /// Serves, and ignores `Shutdown`.
    Stubborn,
    /// Starts a segment after the first `Configure`, and then hangs: it answers neither `Ping`
    /// nor `Shutdown`.
    Hung,
    /// Serves, and reads one window in the background after the first `Configure` and after
    /// each `Resume`. Each read is of another window, titled `window <n>`, whose id is `n`,
    /// since the helper's ids stay unique for as long as it runs.
    Background,
    /// Stops reading its input after the first `Configure`, and its input holds one byte, so
    /// that the daemon's next frame cannot be sent.
    Deaf,
    /// Serves, and reads one window in the background after the first `Configure`, with a
    /// read time two hours in the past.
    StaleBackground,
    /// Serves, and reads one window in the background after the first `Configure`. It reports
    /// a locked screen 5 s later, and an unlocked screen 5 s after that.
    LocksScreen,
}

/// Hands out fake helpers that speak the protocol over in-memory pipes.
pub struct FakeSpawner {
    /// The scripts of the next spawns. The last script repeats.
    scripts: Mutex<VecDeque<Script>>,
    /// When each fake helper was spawned, in order.
    spawned: Mutex<Vec<Instant>>,
    /// Every frame that the fake helpers received, in order.
    received: Arc<Mutex<Vec<DaemonFrame>>>,
    /// How many records the fake helpers sent.
    records_sent: Arc<AtomicUsize>,
}

impl FakeSpawner {
    pub fn new(scripts: impl IntoIterator<Item = Script>) -> Arc<Self> {
        Arc::new(Self {
            scripts: Mutex::new(scripts.into_iter().collect()),
            spawned: Mutex::default(),
            received: Arc::default(),
            records_sent: Arc::default(),
        })
    }

    /// Replaces the scripts of the next spawns.
    pub fn script_next(&self, scripts: impl IntoIterator<Item = Script>) {
        *self.scripts.lock() = scripts.into_iter().collect();
    }

    pub fn spawns(&self) -> usize {
        self.spawned.lock().len()
    }

    /// When each fake helper was spawned, in order.
    pub fn spawn_times(&self) -> Vec<Instant> {
        self.spawned.lock().clone()
    }

    pub fn received(&self) -> Vec<DaemonFrame> {
        self.received.lock().clone()
    }

    pub fn records_sent(&self) -> usize {
        self.records_sent.load(Ordering::SeqCst)
    }

    /// Forgets the frames that the fake helpers received so far.
    pub fn forget_received(&self) {
        self.received.lock().clear();
    }

    pub fn configures(&self) -> Vec<LinkConfig> {
        self.received
            .lock()
            .iter()
            .filter_map(|frame| match frame {
                DaemonFrame::Configure { config, .. } => Some(config.clone()),
                _ => None,
            })
            .collect()
    }
}

impl Spawn for FakeSpawner {
    /// Fails for [`Script::SigningFails`], which then gives way to the next script.
    fn prepare(&self, program: PathBuf) -> BoxFuture<'_, Result<PathBuf, SigningError>> {
        let mut scripts = self.scripts.lock();
        let fails = matches!(scripts.front(), Some(Script::SigningFails));
        if fails && scripts.len() > 1 {
            scripts.pop_front();
        }
        drop(scripts);
        let prepared = if fails {
            Err(SigningError::NoFingerprint)
        } else {
            Ok(program)
        };
        Box::pin(future::ready(prepared))
    }

    fn spawn(&self, _program: &Path) -> std::io::Result<HelperProcess> {
        self.spawned.lock().push(Instant::now());
        let script = {
            let mut scripts = self.scripts.lock();
            if scripts.len() > 1 {
                scripts.pop_front().expect("a script")
            } else {
                *scripts.front().expect("a script")
            }
        };
        match script {
            Script::Missing => return Err(ErrorKind::NotFound.into()),
            Script::SigningFails => unreachable!("`prepare` fails before the spawn"),
            _ => {}
        }
        let (stdin, input) = tokio::io::duplex(script.input_bytes());
        let (output, stdout) = tokio::io::duplex(1 << 20);
        let helper = AbortOnDropHandle::new(tokio::spawn(fake_helper(script, Pipes {
            input,
            output,
            received: Arc::clone(&self.received),
            records_sent: Arc::clone(&self.records_sent),
        })));
        Ok(HelperProcess {
            stdin: Box::new(stdin),
            stdout: Box::new(stdout),
            exit: Box::pin(async move { helper.await.map_err(std::io::Error::other) }),
        })
    }
}

/// The records that a fake helper sends before it exits, and the status that it exits with.
struct Departure {
    records: Vec<Record>,
    status: ExitStatus,
}

/// Reads another window in the background each time, as `Script::Background` describes.
#[derive(Default)]
struct BackgroundReads {
    windows: u64,
}

impl BackgroundReads {
    fn next(&mut self) -> Record {
        self.windows += 1;
        let title = format!("window {}", self.windows);
        background(self.windows, "kate", &title, SystemTime::now(), window(&title))
    }
}

impl Script {
    /// The status that the helper exits with before its hello, if it exits then.
    fn exit_before_hello(self) -> Option<ExitStatus> {
        match self {
            Self::NoDesktop | Self::SlowNoDesktop => Some(no_desktop()),
            _ => None,
        }
    }

    /// The build that the helper's hello names.
    const fn build(self) -> &'static str {
        match self {
            Self::OtherBuild => "0123456",
            _ => BUILD_STR,
        }
    }

    /// The status that the helper reports after its hello before it exits with status 3, if it
    /// refuses to capture.
    const fn refusal(self) -> Option<HelperStatus> {
        match self {
            Self::NoPermission => Some(HelperStatus::NeedsPermission),
            _ => None,
        }
    }

    /// How many bytes the helper's input holds before a write to it waits.
    const fn input_bytes(self) -> usize {
        match self {
            Self::Deaf => 1,
            _ => 1 << 16,
        }
    }

    const fn answers_ping(self) -> bool {
        !matches!(self, Self::Hung)
    }

    const fn obeys_shutdown(self) -> bool {
        !matches!(self, Self::Stubborn | Self::Hung)
    }

    /// The records that the helper sends after the first `Configure`.
    fn on_configure(self, reads: &mut BackgroundReads) -> Vec<Record> {
        match self {
            Self::Segment | Self::Garbled => vec![
                started(1, "gedit", "notes", SystemTime::now()),
                snapshot(1, SystemTime::now(), window("zebra")),
                notification("saved", SystemTime::now()),
            ],
            Self::Background | Self::LocksScreen => vec![reads.next()],
            Self::StaleBackground => {
                let at = SystemTime::now() - Duration::from_hours(2);
                vec![background(1, "kate", "stale", at, window("stale"))]
            }
            Self::Hung => vec![started(1, "gedit", "notes", SystemTime::now())],
            _ => Vec::new(),
        }
    }

    /// The records that the helper sends when it pauses.
    fn on_pause(self) -> Vec<Record> {
        match self {
            Self::Segment | Self::Garbled => vec![ended(1, SystemTime::now())],
            _ => Vec::new(),
        }
    }

    /// The records that the helper sends when it resumes.
    fn on_resume(self, reads: &mut BackgroundReads) -> Vec<Record> {
        match self {
            Self::Background => vec![reads.next()],
            _ => Vec::new(),
        }
    }

    /// How the helper exits after it answers its first frame other than `Ping`, if it exits
    /// then.
    fn exits_after_first_frame(self) -> Option<Departure> {
        match self {
            Self::DesktopEnds => Some(Departure {
                records: Vec::new(),
                status: no_desktop(),
            }),
            Self::CrashAfterSnapshot => Some(Departure {
                records: vec![
                    started(1, "gedit", "notes", SystemTime::now()),
                    snapshot(1, SystemTime::now(), window("zebra")),
                ],
                status: exit_status(1),
            }),
            _ => None,
        }
    }
}

/// What a fake helper reads from and writes to.
struct Pipes {
    input: DuplexStream,
    output: DuplexStream,
    received: Arc<Mutex<Vec<DaemonFrame>>>,
    records_sent: Arc<AtomicUsize>,
}

impl Pipes {
    async fn send_status(&mut self, status: HelperStatus) {
        write_frame(&mut self.output, &HelperFrame::Status(status)).await.expect("status");
    }

    async fn send_records(&mut self, records: impl IntoIterator<Item = Record>) {
        for record in records {
            write_frame(&mut self.output, &HelperFrame::Record(record)).await.expect("record");
            self.records_sent.fetch_add(1, Ordering::SeqCst);
        }
    }
}

async fn fake_helper(script: Script, mut pipes: Pipes) -> ExitStatus {
    if matches!(script, Script::SlowNoDesktop) {
        tokio::time::sleep(Duration::from_secs(5)).await;
    }
    if let Some(status) = script.exit_before_hello() {
        return status;
    }
    let hello = HelperFrame::Hello {
        build: script.build().to_owned(),
    };
    write_frame(&mut pipes.output, &hello).await.expect("hello");
    if let Some(status) = script.refusal() {
        pipes.send_status(status).await;
        return exit_status(3);
    }
    let mut connected = false;
    let mut reads = BackgroundReads::default();
    loop {
        let frame: DaemonFrame = match read_frame(&mut pipes.input).await {
            Ok(Some(frame)) => frame,
            Ok(None) => return exit_status(0),
            Err(_) => return exit_status(1),
        };
        // Pings are left out of `received`, so that the frames that a test expects stay the
        // same however long the test runs.
        if frame == DaemonFrame::Ping {
            if script.answers_ping() {
                write_frame(&mut pipes.output, &HelperFrame::Pong).await.expect("pong");
            }
            continue;
        }
        pipes.received.lock().push(frame.clone());
        // The real helper reports `Connected` before it captures, doesn't read anything while it
        // is paused, and drains capture before it reports `Paused`.
        match frame {
            DaemonFrame::Configure { mode, .. } if !connected => {
                connected = true;
                if matches!(script, Script::SlowConnect) {
                    tokio::time::sleep(Duration::from_secs(5)).await;
                }
                pipes.send_status(HelperStatus::Connected).await;
                if mode == Mode::Running {
                    if matches!(script, Script::Garbled) {
                        // 0xc1 is a byte that MessagePack never uses.
                        pipes.output.write_all(&[0, 0, 0, 1, 0xc1]).await.expect("garbage");
                    }
                    pipes.send_records(script.on_configure(&mut reads)).await;
                }
                if matches!(script, Script::Deaf) {
                    std::future::pending::<()>().await;
                }
                if matches!(script, Script::LocksScreen) {
                    for status in [HelperStatus::ScreenLocked, HelperStatus::ScreenUnlocked] {
                        tokio::time::sleep(Duration::from_secs(5)).await;
                        pipes.send_status(status).await;
                    }
                }
            }
            DaemonFrame::Configure { .. } => {}
            DaemonFrame::Pause => {
                pipes.send_records(script.on_pause()).await;
                pipes.send_status(HelperStatus::Paused).await;
            }
            DaemonFrame::Resume => {
                pipes.send_records(script.on_resume(&mut reads)).await;
                pipes.send_status(HelperStatus::Resumed).await;
            }
            DaemonFrame::Shutdown if script.obeys_shutdown() => return exit_status(0),
            DaemonFrame::Shutdown => continue,
            DaemonFrame::Ping => unreachable!("pings are answered above"),
        }
        if let Some(departure) = script.exits_after_first_frame() {
            pipes.send_records(departure.records).await;
            return departure.status;
        }
    }
}

fn no_desktop() -> ExitStatus {
    exit_status(CaptureExit::NoDesktop.code().into())
}

#[cfg(unix)]
fn exit_status(code: i32) -> ExitStatus {
    use std::os::unix::process::ExitStatusExt as _;
    ExitStatus::from_raw(code << 8)
}

#[cfg(windows)]
fn exit_status(code: i32) -> ExitStatus {
    use std::os::windows::process::ExitStatusExt as _;
    ExitStatus::from_raw(code.cast_unsigned())
}
