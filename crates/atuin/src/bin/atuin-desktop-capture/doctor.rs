//! The `doctor` command: shows what each running application exposes.

use std::fmt;
use std::time::{Duration, Instant};

use atuin_desktop_capture::{
    AppInfo, Desktop, Error, Inspect, Limits, Platform as _, Purpose, render,
};

use crate::access::connect;
use crate::session::{self, Woken};

/// How many elements an outline lists before it stops.
const OUTLINE_LINES: usize = 4000;

/// Prints the role tree of the windows of the application named `outline`, or else a table
/// of what each running application exposes.
///
/// The hints show commands that start with `command`. For example, `atuin desktop-capture
/// apps`.
///
/// # Errors
///
/// Fails without the permission to use the accessibility API, and when listing the
/// applications fails.
pub async fn run(outline: Option<&str>, command: &str) -> eyre::Result<()> {
    let backend = connect(Purpose::Inspect).await?;
    let woken =
        session::wake_apps(&backend, "Waiting for applications to expose their content…").await;
    if let Some(app) = outline {
        return print_outline(&backend, app).await;
    }
    report(&backend, woken, command).await?;
    session::print_hiding_variables("");
    Ok(())
}

/// Prints the role tree of the windows of the application named `wanted`, without any text.
async fn print_outline<B: Inspect>(backend: &B, wanted: &str) -> eyre::Result<()> {
    for app in &backend.apps().await? {
        if !backend.app_name(app).await.is_ok_and(|name| name.eq_ignore_ascii_case(wanted)) {
            continue;
        }
        for window in backend.windows(app).await.unwrap_or_default() {
            print!("{}", backend.outline(&window, OUTLINE_LINES).await?);
        }
    }
    Ok(())
}

/// An application whose windows hold readable content.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReadableApp {
    pub name: String,
    /// Whether the application shows nothing but the system's prompts.
    pub shows_only_prompts: bool,
}

/// Prints a table of what each running application of `backend` exposes, and returns the
/// applications whose windows hold readable content.
///
/// `woken` tells whether the applications were just asked to expose their content. The result
/// leaves out the applications that desktop capture never follows, such as terminals. The hints
/// show commands that start with `command`.
///
/// # Errors
///
/// Fails when listing the applications fails.
pub async fn report<B: Inspect>(
    backend: &B,
    woken: Woken,
    command: &str,
) -> eyre::Result<Vec<ReadableApp>> {
    let limits = Limits {
        budget: Duration::from_secs(3),
        ..Limits::default()
    };
    println!(
        "{:<16} {:<10} {:>7} {:>7} {:>8} {:>8}",
        "app", "toolkit", "windows", "nodes", "bytes", "ms"
    );
    let mut readable: Vec<ReadableApp> = Vec::new();
    for app in &backend.apps().await? {
        let exposure = Exposure::measure(backend, app, limits).await;
        println!("{exposure}");
        if let Some(name) = exposure.into_readable_name()
            && !backend.is_terminal(app, &name)
            && !backend.is_left_out(app)
            && !readable.iter().any(|known| known.name.eq_ignore_ascii_case(&name))
        {
            readable.push(ReadableApp {
                shows_only_prompts: backend.shows_only_prompts(app, &name),
                name,
            });
        }
    }
    print_hints(woken, command);
    Ok(readable)
}

/// What one application exposes in its windows, which prints as a row of the table.
struct Exposure {
    identity: Identity,
    windows: usize,
    nodes: usize,
    bytes: usize,
    elapsed: Duration,
}

/// The name of an application, or the handle that stands in for an unknown name.
enum Identity {
    Known(AppInfo),
    Unknown {
        handle: String,
        error: Error,
    },
}

impl Exposure {
    /// Reads every window of `app` within `limits`.
    async fn measure<B: Inspect>(backend: &B, app: &B::App, limits: Limits) -> Self {
        let identity = match backend.app_info(app).await {
            Ok(info) => Identity::Known(info),
            Err(error) => Identity::Unknown {
                handle: format!("{app:?}"),
                error,
            },
        };
        let windows = backend.windows(app).await.unwrap_or_default();
        let started = Instant::now();
        let mut nodes = 0;
        let mut bytes = 0;
        for window in &windows {
            if let Ok(reading) = backend.read(window, limits).await {
                nodes += reading.stats.nodes;
                bytes += render(&reading.root, usize::MAX).text.len();
            }
        }
        Self {
            identity,
            windows: windows.len(),
            nodes,
            bytes,
            elapsed: started.elapsed(),
        }
    }

    /// The name of the application, when the name is known and the windows hold readable
    /// content.
    fn into_readable_name(self) -> Option<String> {
        match self.identity {
            Identity::Known(info) if self.bytes > 0 => Some(info.name),
            _ => None,
        }
    }
}

impl fmt::Display for Exposure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let Self {
            windows,
            nodes,
            bytes,
            ..
        } = self;
        let (name, toolkit) = match &self.identity {
            Identity::Known(info) => (info.name.as_str(), info.toolkit.as_deref()),
            Identity::Unknown { handle, .. } => (handle.as_str(), None),
        };
        let toolkit = toolkit.unwrap_or("-");
        let ms = self.elapsed.as_secs_f64() * 1e3;
        write!(f, "{name:<16.16} {toolkit:<10.10} {windows:>7} {nodes:>7} {bytes:>8} {ms:>8.1}")?;
        // The error of an unknown name follows on its own line.
        if let Identity::Unknown { error, .. } = &self.identity {
            write!(f, "\n  ↳ the name is unknown: {error}")?;
        }
        Ok(())
    }
}

/// Prints how to read the table, and what to do about the applications that are missing.
fn print_hints(woken: Woken, command: &str) {
    println!("\nAn application that has windows and 0 bytes doesn't expose readable content.");
    println!("`{command} doctor --outline <app>` shows the structure that an application exposes.");
    let notes = Desktop::missing_app_notes();
    if let Some(note) = notes.started_early {
        session::print_paragraph("", note);
        println!("`{command} apps` names the ones that need a restart.");
    }
    if let Some(note) = notes.just_woken
        && woken == Woken::Asked
    {
        println!();
        session::print_paragraph("", note);
    }
}
