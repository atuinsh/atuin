//! `atuin-desktop-capture`: the helper that does every desktop read for the Atuin daemon.
//!
//! The helper runs [`serve`](serve::serve) for the daemon, and carries the [`apps`] and
//! [`doctor`] commands that list and diagnose the desktop.

#![warn(clippy::pedantic, clippy::nursery)]
#![allow(clippy::use_self, clippy::missing_const_for_fn)] // not 100% reliable

mod access;
mod apps;
mod caller;
mod doctor;
mod redact;
mod serve;
mod session;

use std::process::ExitCode;

use atuin_common::os::process::current_exe_name;
use atuin_common::rmp::frame::write_frame;
use atuin_daemon::grpc::versioning::BUILD_STR as BUILD;
use atuin_desktop_capture::link::process::{
    APPS_RECORD_FD_VARIABLE, COMMAND_VARIABLE, CaptureExit,
};
use atuin_desktop_capture::link::{HelperFrame, HelperStatus};
use atuin_desktop_capture::{Access, Desktop, Platform as _, Purpose};
use clap::{Parser, Subcommand};
use eyre::WrapErr as _;
use tracing::{debug, warn};
use tracing_subscriber::EnvFilter;
use tracing_subscriber::filter::LevelFilter;

use crate::access::{Presence, connect_if_present};
use crate::serve::{Served, serve};

#[derive(Parser)]
#[command(name = env!("CARGO_BIN_NAME"), about = "Reads the desktop for the Atuin daemon", version = BUILD)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Capture for the Atuin daemon, which sends commands on stdin and reads records on stdout.
    Capture,
    /// List the running applications that expose readable content, and print the apps setting
    /// that selects them.
    Apps,
    /// Check for the Accessibility permission, or ask the user for it.
    Access {
        #[command(subcommand)]
        action: AccessAction,
    },
    /// List the running applications and what each one exposes.
    Doctor {
        /// Print the role tree of this application's windows instead, without any text.
        #[arg(long)]
        outline: Option<String>,
    },
}

#[derive(Subcommand)]
enum AccessAction {
    /// Print nothing, and exit with a status that tells whether the permission is granted.
    Check,
    /// Ask the user for the permission, and exit once the user grants it.
    Request {
        /// Also exit once stdin closes, which happens when the program that holds the other end
        /// of stdin exits, regardless of how it exits.
        #[arg(long, hide = true)]
        until_stdin_closes: bool,
    },
}

fn main() -> eyre::Result<ExitCode> {
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    if let Some(exit) = caller::intercept(&args) {
        return Ok(exit);
    }
    let cli = Cli::parse();
    // The accessibility permission must attach to the helper instead of the program that
    // started it. On macOS, `take_responsibility` runs the rest of the helper in a new process.
    let responsibility = Desktop::take_responsibility();
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_env_filter(
            EnvFilter::builder().with_default_directive(LevelFilter::INFO.into()).from_env_lossy(),
        )
        .init();
    if let Err(error) = responsibility {
        warn!(%error, "the helper can't take responsibility for its permission");
    }
    Desktop::serve_main_thread(|| {
        let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build()?;
        let status = runtime.block_on(run(cli));
        // The read of stdin can block a thread of the runtime until the daemon closes stdin,
        // and the daemon may wait for this process to exit first.
        runtime.shutdown_background();
        status
    })
}

async fn run(cli: Cli) -> eyre::Result<ExitCode> {
    match cli.command {
        Command::Capture => capture().await,
        Command::Apps => {
            apps::run(&command_name(), apps_output()?).await?;
            Ok(ExitCode::SUCCESS)
        }
        Command::Access { action } => Ok(access::run(action).await),
        Command::Doctor { outline } => {
            doctor::run(outline.as_deref(), &command_name()).await?;
            Ok(ExitCode::SUCCESS)
        }
    }
}

/// The command that the hints of `apps` and `doctor` tell the user to run: the one that Atuin
/// names in [`COMMAND_VARIABLE`], or else this program's file name.
fn command_name() -> String {
    atuin_common::env::var_os(COMMAND_VARIABLE)
        .and_then(|name| name.into_string().ok())
        .unwrap_or_else(program_name)
}

/// Where `apps` puts the applications to capture: the file descriptor that Atuin names in
/// [`APPS_RECORD_FD_VARIABLE`], or else the terminal.
///
/// The helper takes ownership of the descriptor, and marks it close-on-exec so that only the
/// helper holds it. Atuin reads until every copy of the descriptor is closed, so a child of the
/// helper that kept a copy would keep Atuin waiting.
#[cfg(unix)]
fn apps_output() -> eyre::Result<apps::Output> {
    use std::os::fd::{BorrowedFd, FromRawFd as _, OwnedFd, RawFd};

    let Some(fd) = atuin_common::env::var_os(APPS_RECORD_FD_VARIABLE) else {
        return Ok(apps::Output::Terminal);
    };
    let fd: RawFd = fd.to_str().and_then(|fd| fd.parse().ok()).ok_or_else(|| {
        eyre::eyre!("{APPS_RECORD_FD_VARIABLE} must name a file descriptor, but it is {fd:?}")
    })?;
    // SAFETY: Atuin opens the descriptor for this process, and nothing else in this process
    // uses it. A descriptor that isn't open makes the call fail with EBADF.
    let borrowed = unsafe { BorrowedFd::borrow_raw(fd) };
    rustix::io::fcntl_setfd(borrowed, rustix::io::FdFlags::CLOEXEC)
        .wrap_err_with(|| format!("{APPS_RECORD_FD_VARIABLE} names {fd}, which isn't open"))?;
    // SAFETY: the descriptor is open, as the call above showed, and only this value owns it.
    let owned = unsafe { OwnedFd::from_raw_fd(fd) };
    Ok(apps::Output::Record(owned.into()))
}

/// Puts the applications to capture on the terminal, because Atuin passes a descriptor only on
/// Unix.
#[cfg(not(unix))]
#[expect(clippy::unnecessary_wraps, reason = "the Unix version can fail")]
fn apps_output() -> eyre::Result<apps::Output> {
    Ok(apps::Output::Terminal)
}

async fn capture() -> eyre::Result<ExitCode> {
    let mut stdout = tokio::io::stdout();
    if Desktop::access() == Access::Denied {
        warn!("the Accessibility permission is missing. Run `atuin setup`.");
        // The daemon expects the hello as the first frame.
        let frames = [
            HelperFrame::Hello {
                build: BUILD.to_owned(),
            },
            HelperFrame::Status(HelperStatus::NeedsPermission),
        ];
        for frame in &frames {
            write_frame(&mut stdout, frame).await.wrap_err("writing to the daemon")?;
        }
        return Ok(ExitCode::from(CaptureExit::NeedsPermission.code()));
    }
    // The daemon starts the helper again soon after a `NoDesktop` exit, so the helper logs the
    // reason at the debug level to keep the daemon's log quiet when no desktop session is running.
    let backend = match connect_if_present(Purpose::Capture).await? {
        Presence::Present(backend) => backend,
        Presence::Absent(reason) => {
            debug!("{reason}");
            return Ok(ExitCode::from(CaptureExit::NoDesktop.code()));
        }
    };
    match Box::pin(serve(backend, tokio::io::stdin(), stdout, BUILD)).await? {
        Served::Finished => Ok(ExitCode::SUCCESS),
        // On macOS, the events end when the user revokes the permission.
        Served::DesktopEnded if Desktop::access() == Access::Denied => {
            warn!("the Accessibility permission was revoked. Run `atuin setup`.");
            Ok(ExitCode::from(CaptureExit::NeedsPermission.code()))
        }
        Served::DesktopEnded => {
            debug!("the desktop stopped sending events");
            Ok(ExitCode::from(CaptureExit::NoDesktop.code()))
        }
    }
}

/// The file name of the running program, by which the user calls it.
fn program_name() -> String {
    current_exe_name().unwrap_or_else(|| env!("CARGO_BIN_NAME").to_owned())
}

#[cfg(test)]
mod tests {
    use rstest::rstest;

    use super::*;

    #[rstest]
    #[case::apps(&["helper", "apps"], true)]
    #[case::access_check(&["helper", "access", "check"], true)]
    #[case::access_request(&["helper", "access", "request"], true)]
    #[case::access_request_until_closed(
        &["helper", "access", "request", "--until-stdin-closes"],
        true
    )]
    #[case::setup_is_gone(&["helper", "setup"], false)]
    #[case::access_needs_an_action(&["helper", "access"], false)]
    fn commands_parse(#[case] args: &[&str], #[case] parses: bool) {
        assert_eq!(Cli::try_parse_from(args).is_ok(), parses);
    }
}
