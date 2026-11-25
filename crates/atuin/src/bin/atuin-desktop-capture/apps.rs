//! The `apps` command: lists the applications that expose readable content.

use std::fmt;
use std::fs::File;
use std::io::{IsTerminal as _, Write as _};

use atuin_desktop_capture::link::process::AppsRecord;
use atuin_desktop_capture::{
    Access, AccessLifetime, Desktop, Inspect as _, Platform as _, Purpose, Remedy, UnreadyApp,
};
use eyre::WrapErr as _;

use crate::access::{Presence, connect_if_present};
use crate::doctor::ReadableApp;
use crate::session::{self, Woken};

/// Walks through the steps that list the applications whose content this desktop exposes.
///
/// The command ends by putting the applications whose content it could read where `output`
/// says. The hints show commands that start with `command`. For example, `atuin
/// desktop-capture apps`.
///
/// # Errors
///
/// Fails without the permission to use the accessibility API, when the accessibility
/// service fails in a way that the command cannot explain, and when the record can't be written.
pub async fn run(command: &str, output: Output) -> eyre::Result<()> {
    step(1, "Permission to use the accessibility API");
    check_access()?;

    step(2, "Accessibility service");
    let Some(backend) = find_service(command).await? else {
        return Ok(());
    };

    step(3, "Session environment");
    session::print_session_variables("   ");

    step(4, "Running applications");
    // The command stays connected from here on, which keeps the applications awake while the user
    // restarts one.
    let woken = ready_apps(&backend).await?;

    step(5, "Result");
    let readable = crate::doctor::report(&backend, woken, command).await?;

    step(6, "Applications to capture");
    let suggested = suggestion(readable);
    if suggested.is_empty() {
        println!("   No application exposes readable content yet.");
        return Ok(());
    }
    match output {
        Output::Terminal => print_apps_setting(suggested),
        Output::Record(file) => write_record(file, suggested)?,
    }
    Ok(())
}

/// Where `apps` puts the applications to capture.
#[derive(Debug)]
pub enum Output {
    /// Print the `apps` setting for the user to copy into the configuration.
    Terminal,
    /// Write an [`AppsRecord`] to this file, for Atuin to write into the configuration.
    Record(File),
}

/// Fails unless this process has the permission to use the accessibility API, and tells how
/// long the permission lasts.
fn check_access() -> eyre::Result<()> {
    if Desktop::access() == Access::Denied {
        let program = crate::program_name();
        eyre::bail!("{program} needs the Accessibility permission. Run `atuin setup` to grant it.");
    }
    println!("   Granted.");
    print_access_lifetime();
    // An alert that asked for the permission can stay open after the grant.
    Desktop::close_access_request();
    Ok(())
}

fn print_access_lifetime() {
    match Desktop::access_lifetime() {
        AccessLifetime::Durable => {}
        AccessLifetime::ThisBuild => {
            println!(
                "   The permission covers this exact build only. An update needs a new grant."
            );
            println!("   A signing certificate keeps the grant across updates. Run `atuin setup`");
            println!("   again to create one.");
        }
        AccessLifetime::Unknown => {
            println!("   Could not read the code signature. The permission may not last.");
        }
    }
}

/// Connects to the accessibility service. Returns `None` when no desktop session is running,
/// after the function prints the reason and how to fix it.
async fn find_service(command: &str) -> eyre::Result<Option<Desktop>> {
    match connect_if_present(Purpose::Inspect).await? {
        Presence::Present(backend) => {
            println!("   Found.");
            Ok(Some(backend))
        }
        Presence::Absent(reason) => {
            println!("   Not available: {reason}");
            if let Some(instructions) = Desktop::service_instructions() {
                println!("   {instructions} Then rerun `{command} apps`.");
            }
            Ok(None)
        }
    }
}

/// Wakes the running applications of `backend`, and walks the user through the applications
/// that stay unready.
async fn ready_apps(backend: &Desktop) -> eyre::Result<Woken> {
    let woken = session::wake_apps(
        backend,
        "   Asked the running applications to expose their content. Waiting…",
    )
    .await;
    let unready = backend.unready_apps().await?;
    if unready.is_empty() {
        println!("   Nothing to do.");
    }
    for app in &unready {
        remedy(app);
    }
    Ok(woken)
}

/// The names of the applications in `readable`, apart from the ones that show only the
/// system's prompts. Prints a line for each application that the suggestion leaves out.
///
/// The text of a system prompt says nothing about the user's work. Atuin writes the suggestion
/// into the configuration, so a name in the suggestion keeps that application captured until
/// the user removes it. `apps = "all"` still captures the prompts.
fn suggestion(readable: impl IntoIterator<Item = ReadableApp>) -> Vec<String> {
    readable
        .into_iter()
        .filter_map(|app| {
            if app.shows_only_prompts {
                println!(
                    "   The list below leaves out {}, which shows only system prompts.",
                    app.name
                );
                None
            } else {
                Some(app.name)
            }
        })
        .collect()
}

/// Prints the configuration line that selects the applications named `suggested`.
fn print_apps_setting(suggested: Vec<String>) {
    println!("   To capture the applications that exposed readable content above, put this");
    println!("   line into the [desktop_capture] section of the Atuin configuration, or");
    println!("   write apps = \"all\" to capture every application:");
    println!(
        "   apps = {}",
        toml::Value::Array(suggested.into_iter().map(toml::Value::String).collect())
    );
}

/// Writes `suggested` to `file` as one [`AppsRecord`].
fn write_record(file: File, suggested: Vec<String>) -> eyre::Result<()> {
    serde_json::to_writer(file, &AppsRecord { apps: suggested })
        .wrap_err("writing the applications to capture")
}

fn remedy(app: &UnreadyApp) {
    let UnreadyApp { name, pid, .. } = app;
    match app.remedy {
        Remedy::Restart => hand_over(format_args!(
            "{name} (pid {pid}) started before a client was connected and keeps its content to \
             itself. Restart it now."
        )),
        Remedy::RestartWithVariable => hand_over(format_args!(
            "{name} (pid {pid}) runs in a sandbox that hides whether a client is connected, and \
             keeps its content to itself. Restart it once the session has the variable above."
        )),
        // The library can name remedies that this version does not know.
        _ => println!("   {name} (pid {pid}) does not expose its content."),
    }
}

/// Tells the user to do something by hand, then waits until the user confirms it.
fn hand_over(instruction: impl fmt::Display) {
    println!("   ACTION NEEDED: {instruction}");
    if std::io::stdin().is_terminal() {
        print!("   Press Enter when that is done. ");
        wait_for_enter();
    }
}

fn wait_for_enter() {
    let mut line = String::new();
    // A failed flush or read ends the wait, as Enter does.
    let _ = std::io::stdout().flush();
    let _ = std::io::stdin().read_line(&mut line);
}

fn step(number: u8, title: &str) {
    println!("\n{number}. {title}");
}

#[cfg(test)]
mod tests {
    use std::io::Read as _;

    use rstest::rstest;

    use super::*;

    #[rstest]
    #[case::prompt(true, &["firefox"])]
    #[case::application(false, &["kate", "firefox"])]
    fn the_suggestion_leaves_out_system_prompts(
        #[case] shows_only_prompts: bool,
        #[case] expected: &[&str],
    ) {
        let readable = [
            ReadableApp {
                name: "kate".to_owned(),
                shows_only_prompts,
            },
            ReadableApp {
                name: "firefox".to_owned(),
                shows_only_prompts: false,
            },
        ];
        assert_eq!(suggestion(readable), expected);
    }

    /// The record reaches Atuin through the pipe that Atuin passes.
    #[rstest]
    fn the_record_reaches_the_reader() {
        let (mut reader, writer) = std::io::pipe().unwrap();

        write_record(std::os::fd::OwnedFd::from(writer).into(), names(&["kate", "firefox"]))
            .unwrap();

        let mut json = String::new();
        reader.read_to_string(&mut json).unwrap();
        let record: AppsRecord = serde_json::from_str(&json).unwrap();
        assert_eq!(record.apps, ["kate", "firefox"]);
    }

    fn names(names: &[&str]) -> Vec<String> {
        names.iter().map(|&name| name.to_owned()).collect()
    }
}
