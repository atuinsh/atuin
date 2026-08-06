use std::io::Read;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU16, AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, SyncSender};
use std::sync::{Arc, Mutex};

use crossterm::terminal;
use eyre::WrapErr;
use portable_pty::{CommandBuilder, PtySize, native_pty_system};

use crate::capture::CommandCaptureTracker;
use crate::compositor::{CURSOR_HANDSHAKE, Compositor, OverlayFlags, lock_unpoisoned};
use crate::debug::{Osc133DebugHighlighter, RESET};
use crate::pty_proxy::RuntimeOptions;
use crate::screen::{self, Msg};

pub(crate) fn main(options: RuntimeOptions) {
    let fallback = options.shell.clone();
    let fallback_umask = options.hooks.child_umask;
    let session = match start(options) {
        Ok(session) => session,
        // The init preamble exec'd the shell away to run us: dying here
        // would close the user's tab. Hand the terminal to a plain shell
        // and keep only the proxy features unavailable.
        Err(e) => {
            let _ = terminal::disable_raw_mode();
            eprintln!("atuin pty-proxy: {e:#}; starting the shell without the proxy");
            if let Some(mask) = fallback_umask {
                // SAFETY: exec immediately follows this process-wide change.
                unsafe { libc::umask(mask as libc::mode_t) };
            }
            exec_fallback_shell(fallback.as_deref());
        }
    };

    let code = session.wait();
    let _ = terminal::disable_raw_mode();
    std::process::exit(code);
}

/// The proxied session once startup can no longer fail: the shell is
/// running and both pumps are attached.
struct Session {
    child: Box<dyn portable_pty::Child + Send + Sync>,
    stdout_thread: std::thread::JoinHandle<()>,
    proxy_dir: PathBuf,
}

impl Session {
    fn wait(mut self) -> i32 {
        let code = match self.child.wait() {
            Ok(status) => process_exit_code(status.exit_code()),
            Err(e) => {
                eprintln!("atuin pty-proxy: wait for shell: {e:#}");
                1
            }
        };
        let _ = self.stdout_thread.join();
        let _ = std::fs::remove_dir_all(&self.proxy_dir);
        code
    }
}

/// Kill the child shell if startup fails after the spawn, so a bailed
/// proxy doesn't leave an orphan on the inner pty.
struct ChildGuard(Option<Box<dyn portable_pty::Child + Send + Sync>>);

impl ChildGuard {
    fn defuse(mut self) -> Box<dyn portable_pty::Child + Send + Sync> {
        self.0.take().expect("guard defused twice")
    }
}

impl Drop for ChildGuard {
    fn drop(&mut self) {
        if let Some(child) = self.0.as_mut() {
            let _ = child.kill();
        }
    }
}

/// Replace this process with the shell the proxy would have wrapped.
/// `ATUIN_PTY_PROXY_ACTIVE` stays exported so the shell's own init
/// preamble doesn't recurse into another doomed proxy.
fn exec_fallback_shell(shell: Option<&Path>) -> ! {
    use std::os::unix::process::CommandExt;

    let shell = shell
        .map(Path::to_path_buf)
        .or_else(|| {
            std::env::var_os("SHELL")
                .filter(|s| !s.is_empty())
                .map(PathBuf::from)
        })
        .unwrap_or_else(|| PathBuf::from("/bin/sh"));
    let err = std::process::Command::new(&shell)
        .env("ATUIN_PTY_PROXY_ACTIVE", "1")
        .exec();
    eprintln!("atuin pty-proxy: exec {}: {err}", shell.display());
    std::process::exit(1);
}

/// Startup timing breadcrumbs behind `ATUIN_PTY_PROXY_TRACE=1`, to place
/// environment-specific stalls without a debugger attached.
struct Trace {
    enabled: bool,
    start: std::time::Instant,
    last: std::time::Instant,
}

impl Trace {
    fn new() -> Self {
        let now = std::time::Instant::now();
        Self {
            enabled: crate::pty_proxy::env_flag("ATUIN_PTY_PROXY_TRACE"),
            start: now,
            last: now,
        }
    }

    fn step(&mut self, name: &str) {
        if !self.enabled {
            return;
        }
        let now = std::time::Instant::now();
        eprintln!(
            "atuin pty-proxy: trace: {name}: +{:?} (total {:?})\r",
            now - self.last,
            now - self.start
        );
        self.last = now;
    }
}

/// Direct writes to fd 1. `std::io::stdout()` is line-buffered: it splits
/// every batched chunk at the last newline and holds the remainder for the
/// flush — two or three syscalls where one carries the compositor's
/// already-batched bytes.
pub(crate) struct RawStdout;

impl Write for RawStdout {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        // SAFETY: fd 1 outlives the process; never closed here.
        let fd = unsafe { rustix::fd::BorrowedFd::borrow_raw(1) };
        rustix::io::write(fd, buf).map_err(std::io::Error::from)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// Unbuffered stdin, for the same reason [`RawStdout`] is unbuffered on the
/// way out — but here it is a correctness requirement, not a saving.
///
/// Both the cursor handshake and the key filter decide whether more input is
/// coming by asking the kernel (`FIONREAD`), because `poll(2)` doesn't work
/// on tty fds on macOS. `std::io::Stdin` is a `BufReader`, so a read smaller
/// than its 8 KiB buffer drains the fd into userspace and leaves `FIONREAD`
/// answering zero with bytes still pending: the handshake would time out
/// without seeing its fence and then leak the terminal's reply to the shell
/// as keystrokes. Reading the fd directly keeps the two in agreement, and
/// keeps every reader of fd 0 in one queue.
pub(crate) struct RawStdin;

impl RawStdin {
    fn fd(&self) -> rustix::fd::BorrowedFd<'static> {
        // SAFETY: fd 0 outlives the process; never closed here.
        unsafe { rustix::fd::BorrowedFd::borrow_raw(0) }
    }
}

impl Read for RawStdin {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        rustix::io::read(self.fd(), buf).map_err(std::io::Error::from)
    }
}

impl std::os::fd::AsFd for RawStdin {
    fn as_fd(&self) -> rustix::fd::BorrowedFd<'_> {
        self.fd()
    }
}

fn start(mut options: RuntimeOptions) -> eyre::Result<Session> {
    let mut trace = Trace::new();
    let (cols, rows) = terminal::size().wrap_err("query terminal size")?;
    // Terminals can report 0x0 during window setup; a zero-sized vt100
    // grid panics deep in the parser and would kill the output pump.
    let (cols, rows) = (cols.max(1), rows.max(1));
    trace.step("terminal size");

    let pty_system = native_pty_system();
    let pair = pty_system
        .openpty(PtySize {
            rows,
            cols,
            pixel_width: 0,
            pixel_height: 0,
        })
        .map_err(|e| eyre::eyre!("{e:#}"))
        .wrap_err("open pty")?;
    trace.step("open pty");

    let mut proxy_dir = screen::default_proxy_dir();
    if !screen::socket_path_fits(&screen::socket_path_in(&proxy_dir)) {
        proxy_dir = screen::fallback_proxy_dir();
    }
    let sock_path = screen::socket_path_in(&proxy_dir);
    screen::create_proxy_dir(&proxy_dir).wrap_err("create proxy directory")?;
    let token = screen::write_token(&proxy_dir).wrap_err("create proxy input token")?;
    let listener =
        std::os::unix::net::UnixListener::bind(&sock_path).wrap_err("bind proxy socket")?;
    let (screen_tx, screen_rx) = mpsc::sync_channel::<Msg>(64);
    let (input_tx, input_rx) = mpsc::sync_channel::<Vec<u8>>(64);
    screen::spawn_parser_thread(rows, cols, screen_rx);
    screen::spawn_socket_server(listener, screen_tx.clone(), token, input_tx.clone());

    // The init preamble can only pass what its shell knew about itself,
    // which is often a bare name ("zsh" via ZSH_ARGZERO). Resolve it here
    // so the spawn, the child's `$SHELL`, and everything reading it
    // downstream get a real path instead of a name they must re-resolve.
    let shell = options.shell.take().map(resolve_shell);

    let mut cmd = match shell {
        Some(ref path) => CommandBuilder::new(path),
        None => CommandBuilder::new_default_prog(),
    };
    cmd.cwd(std::env::current_dir().wrap_err("resolve current directory")?);
    // Reflect the shell we actually spawn in `$SHELL` so the child — and
    // anything it execs via `$SHELL -c` (e.g. fzf's `become`) — sees the
    // shell the user asked for instead of a stale value inherited from the
    // parent environment.
    if let Some(ref path) = shell {
        cmd.env("SHELL", path);
    }
    cmd.env("ATUIN_PTY_PROXY_SOCKET", sock_path.as_os_str());
    cmd.env("ATUIN_PTY_PROXY_ACTIVE", "1");
    // Lets the shell integration stamp its OSC 133 markers so this proxy can
    // recognise its own shell's prompt. Any other shell reaching this
    // terminal — over ssh, inside a container, under a different user — does
    // not inherit the variable and so cannot be mistaken for ours. Only
    // generated when something actually consumes the markers.
    let mark = options
        .hooks
        .suggestion_provider
        .is_some()
        .then(screen::session_mark);
    if let Some(mark) = &mark {
        cmd.env(crate::suggest::MARK_ENV, mark);
    }
    // The init preamble re-execs when TMUX doesn't match this marker (a
    // new pane deserves its own proxy). A manually launched proxy hasn't
    // set it, and the mismatch would nest a second proxy inside this one —
    // whose overlay bytes the outer proxy would then track as input.
    cmd.env(
        "ATUIN_PTY_PROXY_TMUX",
        std::env::var_os("TMUX").unwrap_or_default(),
    );
    // Atuin sets a restrictive process-wide umask on startup to protect the
    // files it creates. The shell must not inherit it (#3695) — restore the
    // umask the user launched us with. Applied in the child between fork and
    // exec, so the proxy's own umask stays restrictive.
    if let Some(mask) = options.hooks.child_umask {
        cmd.umask(Some(mask as _));
    }

    let child = ChildGuard(Some(
        pair.slave
            .spawn_command(cmd)
            .map_err(|e| eyre::eyre!("{e:#}"))
            .wrap_err("spawn shell")?,
    ));
    trace.step("spawn shell");

    drop(pair.slave);

    let mut pty_reader = pair
        .master
        .try_clone_reader()
        .map_err(|e| eyre::eyre!("{e:#}"))
        .wrap_err("open pty reader")?;
    let mut pty_writer = pair
        .master
        .take_writer()
        .map_err(|e| eyre::eyre!("{e:#}"))
        .wrap_err("open pty writer")?;

    let current_cols = Arc::new(AtomicU16::new(cols.max(1)));

    // All terminal writes go through the compositor, which also maintains
    // the socket-served screen model; splitting only matters when an
    // overlay may paint.
    let flags = Arc::new(OverlayFlags::default());
    let overlays_enabled = options.hooks.suggestion_provider.is_some();
    let compositor = Arc::new(Mutex::new(Compositor::new(
        rows,
        cols,
        RawStdout,
        flags.clone(),
        overlays_enabled,
    )));

    let activity = Arc::new(ActivityClock::new());

    spawn_resize_handler(
        pair.master,
        compositor.clone(),
        current_cols.clone(),
        activity.clone(),
        screen_tx.clone(),
        overlays_enabled,
    )
    .wrap_err("install resize handler")?;
    trace.step("socket + resize handlers");

    let input_activity = Arc::new(ActivityClock::new());
    let session_ready = options.hooks.session_ready.take();
    let (mut input_tracker, mut key_filter) = options
        .hooks
        .suggestion_provider
        .take()
        .map(|provider| {
            let handles = crate::suggest::spawn(
                provider,
                compositor.clone(),
                flags,
                current_cols.clone(),
                input_activity.clone(),
                session_ready,
                mark,
            );
            (handles.tracker, handles.keys)
        })
        .unzip();

    trace.step("suggestion hooks");
    claim_foreground_tty();
    terminal::enable_raw_mode()
        .wrap_err_with(|| format!("enable raw mode ({})", tty_diagnosis()))?;
    trace.step("raw mode");
    // Overlays are the only consumer of the seeded cursor; without a
    // suggestion provider the handshake would only delay first output.
    if input_tracker.is_some() {
        seed_cursor_from_terminal(&compositor, &mut pty_writer);
        trace.step("cursor handshake");
    }
    spawn_pty_writer_thread(pty_writer, input_rx);

    let pump_compositor = compositor;
    let stdout_thread = std::thread::spawn(move || {
        let mut highlighter = options.debug_osc133.then(Osc133DebugHighlighter::new);
        let mut capture_tracker = options
            .hooks
            .command_capture_sink
            .as_ref()
            .map(|_| CommandCaptureTracker::new(current_cols));
        let mut buf = [0u8; 8192];

        // A panic anywhere in the tracking machinery must not freeze the
        // terminal: after one, every chunk bypasses it as a raw write.
        let mut degraded = false;

        loop {
            match pty_reader.read(&mut buf) {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    trace.step("first pty output");
                    trace.enabled = false;
                    activity.touch();
                    let _ = screen_tx.send(Msg::Data(buf[..n].to_vec()));

                    if degraded {
                        let mut out = RawStdout;
                        if out.write_all(&buf[..n]).and_then(|()| out.flush()).is_err() {
                            break;
                        }
                        continue;
                    }

                    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                        if let (Some(tracker), Some(sink)) = (
                            capture_tracker.as_mut(),
                            options.hooks.command_capture_sink.as_ref(),
                        ) {
                            tracker.push(&buf[..n], sink);
                        }

                        let mut compositor = lock_unpoisoned(&pump_compositor);
                        let written = if let Some(highlighter) = highlighter.as_mut() {
                            let rendered = highlighter.render(&buf[..n]);
                            compositor.apply_pty(&rendered)
                        } else {
                            compositor.apply_pty(&buf[..n])
                        };
                        drop(compositor);
                        written?;

                        // After the chunk is applied to screen and model
                        // alike, so suggestion repaints see this chunk
                        // everywhere.
                        if let Some(tracker) = input_tracker.as_mut() {
                            tracker.push(&buf[..n]);
                        }
                        Ok::<(), std::io::Error>(())
                    }));
                    match outcome {
                        Ok(Ok(())) => {}
                        Ok(Err(_)) => break,
                        Err(_) => {
                            degraded = true;
                            // The chunk that hit the panic was never
                            // forwarded; the session continues as a plain
                            // pass-through.
                            let mut out = RawStdout;
                            let _ = out.write_all(&buf[..n]).and_then(|()| out.flush());
                            eprintln!(
                                "atuin pty-proxy: internal error; continuing without suggestions\r"
                            );
                        }
                    }
                }
            }
        }

        if !degraded {
            let mut compositor = lock_unpoisoned(&pump_compositor);
            compositor.flush_pending();
            if highlighter.is_some() {
                let _ = compositor.apply_pty(RESET);
            }
        }
        let _ = screen_tx.send(Msg::Eof);
    });

    std::thread::spawn(move || {
        let mut stdin = RawStdin;
        let mut buf = [0u8; 8192];
        loop {
            match stdin.read(&mut buf) {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    input_activity.touch();
                    // A key-filter panic must not eat the user's keyboard:
                    // forward the chunk raw and drop the filter for good.
                    let mut drop_filter = false;
                    let forwarded = match key_filter.as_mut() {
                        Some(filter) => {
                            let outcome =
                                std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                                    let bytes = filter.process(&buf[..n], &mut stdin);
                                    if bytes.is_empty() {
                                        Ok(())
                                    } else {
                                        input_tx.send(bytes.to_vec()).map_err(|_| {
                                            std::io::Error::from(std::io::ErrorKind::BrokenPipe)
                                        })
                                    }
                                }));
                            match outcome {
                                Ok(result) => result,
                                Err(_) => {
                                    drop_filter = true;
                                    input_tx.send(buf[..n].to_vec()).map_err(|_| {
                                        std::io::Error::from(std::io::ErrorKind::BrokenPipe)
                                    })
                                }
                            }
                        }
                        None => input_tx
                            .send(buf[..n].to_vec())
                            .map_err(|_| std::io::Error::from(std::io::ErrorKind::BrokenPipe)),
                    };
                    if drop_filter {
                        key_filter = None;
                    }
                    if forwarded.is_err() {
                        break;
                    }
                }
            }
        }
    });

    Ok(Session {
        child: child.defuse(),
        stdout_thread,
        proxy_dir,
    })
}

fn spawn_pty_writer_thread(mut writer: Box<dyn Write + Send>, input_rx: Receiver<Vec<u8>>) {
    std::thread::spawn(move || {
        for chunk in input_rx {
            if writer.write_all(&chunk).is_err() {
                break;
            }
            let _ = writer.flush();
        }
    });
}

/// How long the de-orphaning helpers stay alive: long enough for the
/// retried `tcsetpgrp` to land, short enough to never be noticed.
const DEORPHAN_HELPER_LIFETIME_MS: u32 = 300;

/// Make this process group the terminal's foreground group if it isn't.
///
/// Some terminals leave the shell's group in the background at the moment
/// the init preamble execs the proxy — ghostty hands the foreground to a
/// group that exits without giving it back. A background proxy is broken
/// outright: `tcsetattr` fails with EIO on macOS ("enable raw mode:
/// Input/output error"), and stdin reads die under SIGTTIN elsewhere.
///
/// `tcsetpgrp` is the fix, and it needs two preconditions this arranges:
/// SIGTTOU must be ignored (set explicitly — the disposition inherited
/// from the exec'ing shell is unknown), and the group must not be
/// orphaned — a session leader spawned directly by the terminal has its
/// parent outside the session, and macOS refuses every tty operation from
/// an orphaned background group with EIO, including this one. Both fds
/// crossterm might use are tried, since it falls back to stdin when
/// `/dev/tty` won't open.
fn claim_foreground_tty() {
    fn claim(fd: impl std::os::fd::AsFd, ours: rustix::process::Pid) -> bool {
        match rustix::termios::tcgetpgrp(&fd) {
            Ok(fg) if fg == ours => true,
            Ok(_) => rustix::termios::tcsetpgrp(&fd, ours).is_ok(),
            Err(_) => false,
        }
    }
    fn claim_any(ours: rustix::process::Pid) -> bool {
        if let Ok(tty) = std::fs::File::open("/dev/tty")
            && claim(&tty, ours)
        {
            return true;
        }
        claim(std::io::stdin(), ours)
    }

    // SAFETY: setting a signal disposition to SIG_IGN.
    unsafe { libc::signal(libc::SIGTTOU, libc::SIG_IGN) };

    let ours = rustix::process::getpgrp();
    if claim_any(ours) {
        return;
    }

    // Orphaned is the likely refusal: lend the group a member whose parent
    // is another group of this session, and retry while the loan lasts.
    deorphan();
    let deadline = std::time::Instant::now() + std::time::Duration::from_millis(150);
    loop {
        if claim_any(ours) || std::time::Instant::now() >= deadline {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
}

/// Un-orphan our process group for a moment.
///
/// A group is orphaned when no member has a parent in a different group
/// of the same session. Fork a helper into its own new group, and have it
/// fork a grandchild that rejoins ours: the grandchild's parent now sits
/// in another in-session group, so ours is no longer orphaned while they
/// live. The children only touch async-signal-safe calls (this process
/// has threads), and a detached thread reaps the helper.
fn deorphan() {
    let ours = rustix::process::getpgrp();
    // SAFETY: fork with only async-signal-safe calls (setpgid, fork,
    // usleep, _exit) in the children.
    let helper = unsafe { libc::fork() };
    match helper {
        -1 => {}
        0 => unsafe {
            let _ = libc::setpgid(0, 0);
            let grandchild = libc::fork();
            if grandchild == 0 {
                let _ = libc::setpgid(0, ours.as_raw_nonzero().get());
                libc::usleep(DEORPHAN_HELPER_LIFETIME_MS * 1000);
                libc::_exit(0);
            }
            libc::usleep(DEORPHAN_HELPER_LIFETIME_MS * 1000);
            libc::_exit(0);
        },
        pid => {
            std::thread::spawn(move || {
                let mut status = 0;
                // SAFETY: reaping our own direct child.
                unsafe { libc::waitpid(pid, &mut status, 0) };
            });
        }
    }
}

/// One-line job-control snapshot for raw-mode failures: which group we're
/// in and who owns the terminal, via both fds crossterm might use. The
/// EIO class of failures is undiagnosable without it.
fn tty_diagnosis() -> String {
    fn fg_of(fd: impl std::os::fd::AsFd) -> String {
        match rustix::termios::tcgetpgrp(&fd) {
            Ok(fg) => fg.as_raw_nonzero().to_string(),
            Err(e) => format!("err:{e}"),
        }
    }

    let pgrp = rustix::process::getpgrp().as_raw_nonzero();
    let tty = match std::fs::File::open("/dev/tty") {
        Ok(tty) => fg_of(&tty),
        Err(e) => format!("open-err:{e}"),
    };
    format!(
        "pgrp={pgrp} tty-fg={tty} stdin-fg={}",
        fg_of(std::io::stdin())
    )
}

/// Resolve a bare shell name against `PATH`; paths pass through, and an
/// unresolvable name is kept for the spawn to report.
fn resolve_shell(shell: PathBuf) -> PathBuf {
    if shell.as_os_str().to_string_lossy().contains('/') {
        return shell;
    }
    crate::oracle::find_in_path(&shell.to_string_lossy()).unwrap_or(shell)
}

/// How long to wait for the terminal's cursor-position report — the one
/// startup step that delays first output, so it errs small. Real terminals
/// answer within a few milliseconds; on a miss the proxy runs with an
/// unseeded model, as before.
const CPR_TIMEOUT: std::time::Duration = std::time::Duration::from_millis(150);
const CPR_POLL_INTERVAL: std::time::Duration = std::time::Duration::from_millis(1);

/// Ask the terminal where its cursor is (`ESC[6n`) and seed the screen
/// model with the answer, so overlays land on the prompt the user sees
/// even when the proxy starts on a non-empty screen.
///
/// The shell this proxy replaced may have had its own queries in flight
/// (powerlevel10k and iTerm2 send `ESC[6n` around every prompt), so the
/// first cursor report to arrive is not necessarily the answer to ours.
/// A DA1 query (`ESC[c`) sent after the CPR acts as a fence: the terminal
/// answers in order, so the last cursor report before the DA1 reply is
/// ours, and every stale report is swallowed rather than forwarded to the
/// child as junk keystrokes. Keys the user types ahead of the replies are
/// forwarded untouched.
fn seed_cursor_from_terminal(
    compositor: &Arc<Mutex<Compositor<RawStdout>>>,
    pty_writer: &mut (impl Write + ?Sized),
) {
    let mut stdout = std::io::stdout();
    if stdout
        .write_all(CURSOR_HANDSHAKE)
        .and_then(|()| stdout.flush())
        .is_err()
    {
        return;
    }

    let mut stdin = RawStdin;
    let deadline = std::time::Instant::now() + CPR_TIMEOUT;
    let mut pending = Vec::new();
    let mut buf = [0u8; 256];
    let mut cursor = None;
    'read: loop {
        if rustix::io::ioctl_fionread(&stdin).unwrap_or(0) == 0 {
            if std::time::Instant::now() >= deadline {
                break;
            }
            std::thread::sleep(CPR_POLL_INTERVAL);
            continue;
        }
        let Ok(n) = stdin.read(&mut buf) else { break };
        if n == 0 {
            break;
        }
        pending.extend_from_slice(&buf[..n]);
        while let Some(report) = take_report(&mut pending) {
            match report {
                Report::Cursor { row, col } => cursor = Some((row, col)),
                Report::Attributes => break 'read,
            }
        }
    }

    if let Some((row, col)) = cursor {
        lock_unpoisoned(compositor).seed_cursor(row, col);
    }
    if !pending.is_empty() {
        let _ = pty_writer.write_all(&pending);
    }
}

/// A terminal-to-host report the handshake consumes: these answer queries,
/// possibly ones sent by the shell this proxy replaced, and must never
/// reach the child as input.
enum Report {
    /// `ESC[row;colR` — 1-based cursor position.
    Cursor { row: u16, col: u16 },
    /// `ESC[?...c` — DA1 attributes, the handshake's fence.
    Attributes,
}

/// Find and remove the first complete report in `bytes`; everything else
/// (keystrokes racing the replies) is left in place.
fn take_report(bytes: &mut Vec<u8>) -> Option<Report> {
    for start in 0..bytes.len() {
        if bytes[start] != 0x1b {
            continue;
        }
        if let Some((len, report)) = parse_report(&bytes[start..]) {
            bytes.drain(start..start + len);
            return Some(report);
        }
    }
    None
}

fn parse_report(bytes: &[u8]) -> Option<(usize, Report)> {
    let rest = bytes.strip_prefix(b"\x1b[")?;
    if let Some(params) = rest.strip_prefix(b"?") {
        let end = params
            .iter()
            .position(|&b| !b.is_ascii_digit() && b != b';')?;
        return (params[end] == b'c').then_some((2 + 1 + end + 1, Report::Attributes));
    }
    let end = rest
        .iter()
        .position(|&b| !b.is_ascii_digit() && b != b';')?;
    if rest[end] != b'R' {
        return None;
    }
    let (row, col) = std::str::from_utf8(&rest[..end]).ok()?.split_once(';')?;
    let report = Report::Cursor {
        row: row.parse().ok()?,
        col: col.parse().ok()?,
    };
    Some((2 + end + 1, report))
}

/// Millisecond activity clock. One instance tracks pty output (the resize
/// thread's cursor resync waits for the post-SIGWINCH repaint to settle);
/// another tracks user keystrokes (the input tracker only queries when the
/// echo follows one, so program output can't conjure the popup).
pub(crate) struct ActivityClock {
    epoch: std::time::Instant,
    elapsed_ms: AtomicU64,
}

/// Sentinel for "never touched": maximally idle, so gates that require
/// recent activity stay closed until the first real event.
const NEVER: u64 = u64::MAX;

impl ActivityClock {
    pub(crate) fn new() -> Self {
        Self {
            epoch: std::time::Instant::now(),
            elapsed_ms: AtomicU64::new(NEVER),
        }
    }

    /// Relaxed: only tens-of-milliseconds freshness matters.
    pub(crate) fn touch(&self) {
        self.elapsed_ms
            .store(self.epoch.elapsed().as_millis() as u64, Ordering::Relaxed);
    }

    /// Time since the last touch; `Duration::MAX` if never touched.
    pub(crate) fn idle(&self) -> std::time::Duration {
        match self.elapsed_ms.load(Ordering::Relaxed) {
            NEVER => std::time::Duration::MAX,
            last => self
                .epoch
                .elapsed()
                .saturating_sub(std::time::Duration::from_millis(last)),
        }
    }
}

/// Querying while the shell is still repainting its prompt would race the
/// reply against output that moves the cursor.
const RESYNC_QUIET: std::time::Duration = std::time::Duration::from_millis(50);
const RESYNC_POLL: std::time::Duration = std::time::Duration::from_millis(25);
/// A chatty background job must not defer the resync forever — a slightly
/// racy seed beats none at all.
const RESYNC_MAX_WAIT: std::time::Duration = std::time::Duration::from_millis(500);

fn await_pty_quiet(activity: &ActivityClock) {
    let deadline = std::time::Instant::now() + RESYNC_MAX_WAIT;
    while activity.idle() < RESYNC_QUIET && std::time::Instant::now() < deadline {
        std::thread::sleep(RESYNC_POLL);
    }
}

fn spawn_resize_handler(
    master: Box<dyn portable_pty::MasterPty + Send>,
    compositor: Arc<Mutex<Compositor<RawStdout>>>,
    current_cols: Arc<AtomicU16>,
    activity: Arc<ActivityClock>,
    screen_tx: SyncSender<Msg>,
    overlays_enabled: bool,
) -> eyre::Result<()> {
    use signal_hook::consts::SIGWINCH;
    use signal_hook::iterator::Signals;

    let mut signals = Signals::new([SIGWINCH])?;
    // Real terminals reflow wrapped lines on resize; the vt100 model doesn't,
    // so the model cursor has drifted and overlays would paint at stale rows.
    // Re-asking the terminal has to wait for the repaint to settle, which is
    // up to half a second — far too long to hold the signal loop, since
    // dragging a window delivers SIGWINCH continuously and the inner pty's
    // size must track it. So the waiting happens here, and only when an
    // overlay is around to use the answer.
    let resync_tx = overlays_enabled.then(|| {
        let (tx, rx) = mpsc::sync_channel::<()>(1);
        let compositor = compositor.clone();
        let activity = activity.clone();
        std::thread::spawn(move || {
            while rx.recv().is_ok() {
                // One resync answers however many resizes piled up.
                while rx.try_recv().is_ok() {}
                await_pty_quiet(&activity);
                lock_unpoisoned(&compositor).begin_cursor_resync();
            }
        });
        tx
    });

    std::thread::spawn(move || {
        for _ in signals.forever() {
            if let Ok((cols, rows)) = terminal::size() {
                // Same 0x0 guard as startup: a zero-sized grid panics vt100.
                let (cols, rows) = (cols.max(1), rows.max(1));
                current_cols.store(cols, Ordering::Relaxed);
                let _ = master.resize(PtySize {
                    rows,
                    cols,
                    pixel_width: 0,
                    pixel_height: 0,
                });
                let _ = screen_tx.send(Msg::Resize { rows, cols });
                // The resize counts as activity: quiet is then measured
                // from it, so a repaint that hasn't started yet after a
                // long-idle pty can't look like quiet already.
                activity.touch();
                lock_unpoisoned(&compositor).resize(rows, cols);
                if let Some(tx) = &resync_tx {
                    // Never blocks: a resync is already pending, and it will
                    // pick up this size too.
                    let _ = tx.try_send(());
                }
            }
        }
    });

    Ok(())
}

fn process_exit_code(code: u32) -> i32 {
    i32::try_from(code).unwrap_or(1)
}

#[cfg(test)]
mod tests {
    use super::{Report, process_exit_code, take_report};
    use rstest::rstest;

    fn cursor_of(report: Option<Report>) -> Option<(u16, u16)> {
        match report {
            Some(Report::Cursor { row, col }) => Some((row, col)),
            _ => None,
        }
    }

    #[rstest]
    #[case::clean(b"\x1b[24;1R".to_vec(), Some((24, 1)), b"".to_vec())]
    #[case::typed_ahead(b"ls\x1b[3;80R".to_vec(), Some((3, 80)), b"ls".to_vec())]
    #[case::arrow_key_first(
        b"\x1b[A\x1b[12;5R".to_vec(),
        Some((12, 5)),
        b"\x1b[A".to_vec()
    )]
    #[case::incomplete(b"\x1b[24;1".to_vec(), None, b"\x1b[24;1".to_vec())]
    #[case::no_report(b"plain keys".to_vec(), None, b"plain keys".to_vec())]
    fn extracts_cursor_report(
        #[case] mut bytes: Vec<u8>,
        #[case] expected: Option<(u16, u16)>,
        #[case] remainder: Vec<u8>,
    ) {
        assert_eq!(cursor_of(take_report(&mut bytes)), expected);
        assert_eq!(bytes, remainder);
    }

    /// Stale reports from queries the replaced shell sent are all consumed
    /// in order; the terminal answers in order, so the last cursor report
    /// before the DA1 fence is the proxy's own.
    #[rstest]
    fn stale_reports_are_swallowed_and_ordered() {
        let mut bytes = b"\x1b[3;42Rls\x1b[24;1R\x1b[?64;1;9cx".to_vec();
        assert_eq!(cursor_of(take_report(&mut bytes)), Some((3, 42)));
        assert_eq!(cursor_of(take_report(&mut bytes)), Some((24, 1)));
        assert!(matches!(take_report(&mut bytes), Some(Report::Attributes)));
        assert!(take_report(&mut bytes).is_none());
        assert_eq!(bytes, b"lsx".to_vec());
    }

    #[rstest]
    #[case::zero(0, 0)]
    #[case::mid_range(127, 127)]
    #[case::max_i32(i32::MAX as u32, i32::MAX)]
    #[case::overflow_defaults_to_one(i32::MAX as u32 + 1, 1)]
    fn maps_exit_code(#[case] input: u32, #[case] expected: i32) {
        assert_eq!(process_exit_code(input), expected);
    }
}
