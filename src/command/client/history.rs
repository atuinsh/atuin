use std::{
    env,
    fmt::{self, Display},
    io::{StdoutLock, Write},
    time::Duration,
};

use clap::Subcommand;
use eyre::Result;
use runtime_format::{FormatKey, FormatKeyError, ParsedFmt};

use atuin_client::{
    database::{current_context, Database},
    history::History,
    settings::Settings,
};

#[cfg(feature = "sync")]
use atuin_client::sync;
use log::debug;

use super::search::format_duration_into;

#[derive(Subcommand)]
#[command(infer_subcommands = true)]
pub enum Cmd {
    /// Begins a new command in the history
    Start { command: Vec<String> },

    /// Finishes a new command in the history (adds time, exit code)
    End {
        id: String,
        #[arg(long, short)]
        exit: i64,
    },

    /// List all items in history
    List {
        #[arg(long, short)]
        cwd: bool,

        #[arg(long, short)]
        session: bool,

        #[arg(long)]
        human: bool,

        /// Show only the text of the command
        #[arg(long)]
        cmd_only: bool,

        /// Available variables: {command}, {directory}, {duration} and {time}.
        /// Example: --format "{time} - [{duration}] - {directory}$\t{command}"
        #[arg(long, short)]
        format: Option<String>,
    },

    /// Get the last command ran
    Last {
        #[arg(long)]
        human: bool,

        /// Show only the text of the command
        #[arg(long)]
        cmd_only: bool,

        /// Available variables: {command}, {directory}, {duration} and {time}.
        /// Example: --format "{time} - [{duration}] - {directory}$\t{command}"
        #[arg(long, short)]
        format: Option<String>,
    },
}

#[derive(Clone, Copy, Debug)]
pub enum ListMode {
    Human,
    CmdOnly,
    Regular,
}

impl ListMode {
    pub const fn from_flags(human: bool, cmd_only: bool) -> Self {
        if human {
            ListMode::Human
        } else if cmd_only {
            ListMode::CmdOnly
        } else {
            ListMode::Regular
        }
    }
}

#[allow(clippy::cast_sign_loss)]
pub fn print_list(h: &[History], list_mode: ListMode, format: Option<&str>) {
    let w = std::io::stdout();
    let mut w = w.lock();

    match list_mode {
        ListMode::Human => print_human_list(&mut w, h, format),
        ListMode::CmdOnly => print_cmd_only(&mut w, h),
        ListMode::Regular => print_regular(&mut w, h, format),
    }

    w.flush().expect("failed to flush history");
}

/// type wrapper around `History` so we can implement traits
struct FmtHistory<'a>(&'a History);

/// defines how to format the history
impl FormatKey for FmtHistory<'_> {
    #[allow(clippy::cast_sign_loss)]
    fn fmt(&self, key: &str, f: &mut fmt::Formatter<'_>) -> Result<(), FormatKeyError> {
        match key {
            "command" => f.write_str(self.0.command.trim())?,
            "directory" => f.write_str(self.0.cwd.trim())?,
            "duration" => {
                let dur = Duration::from_nanos(std::cmp::max(self.0.duration, 0) as u64);
                format_duration_into(dur, f)?;
            }
            "time" => self.0.timestamp.format("%Y-%m-%d %H:%M:%S").fmt(f)?,
            _ => return Err(FormatKeyError::UnknownKey),
        }
        Ok(())
    }
}

fn print_list_with(w: &mut StdoutLock, h: &[History], format: &str) {
    let fmt = match ParsedFmt::new(format) {
        Ok(fmt) => fmt,
        Err(err) => {
            eprintln!("ERROR: History formatting failed with the following error: {err}");
            println!("If your formatting string contains curly braces (eg: {{var}}) you need to escape them this way: {{{{var}}.");
            std::process::exit(1)
        }
    };

    for h in h.iter().rev() {
        writeln!(w, "{}", fmt.with_args(&FmtHistory(h))).expect("failed to write history");
    }
}

pub fn print_human_list(w: &mut StdoutLock, h: &[History], format: Option<&str>) {
    let format = format
        .unwrap_or("{time} · {duration}\t{command}")
        .replace("\\t", "\t");
    print_list_with(w, h, &format);
}

pub fn print_regular(w: &mut StdoutLock, h: &[History], format: Option<&str>) {
    let format = format
        .unwrap_or("{time}\t{command}\t{duration}")
        .replace("\\t", "\t");
    print_list_with(w, h, &format);
}

pub fn print_cmd_only(w: &mut StdoutLock, h: &[History]) {
    for h in h.iter().rev() {
        writeln!(w, "{}", h.command.trim()).expect("failed to write history");
    }
}

impl Cmd {
    pub async fn run(&self, settings: &Settings, db: &mut impl Database) -> Result<()> {
        let context = current_context();

        match self {
            Self::Start { command: words } => {
                let command = words.join(" ");

                if command.starts_with(' ') {
                    return Ok(());
                }

                // It's better for atuin to silently fail here and attempt to
                // store whatever is ran, than to throw an error to the terminal
                let cwd = env::current_dir()
                    .map_or_else(|_| String::new(), |dir| dir.display().to_string());

                let h = History::new(chrono::Utc::now(), command, cwd, -1, -1, None, None);

                // print the ID
                // we use this as the key for calling end
                println!("{}", h.id);
                db.save(&h).await?;
                Ok(())
            }

            Self::End { id, exit } => {
                if id.trim() == "" {
                    return Ok(());
                }

                let mut h = db.load(id).await?;

                if h.duration > 0 {
                    debug!("cannot end history - already has duration");

                    // returning OK as this can occur if someone Ctrl-c a prompt
                    return Ok(());
                }

                h.exit = *exit;
                h.duration = chrono::Utc::now().timestamp_nanos() - h.timestamp.timestamp_nanos();

                db.update(&h).await?;

                if settings.should_sync()? {
                    #[cfg(feature = "sync")]
                    {
                        debug!("running periodic background sync");
                        sync::sync(settings, false, db).await?;
                    }
                    #[cfg(not(feature = "sync"))]
                    debug!("not compiled with sync support");
                } else {
                    debug!("sync disabled! not syncing");
                }

                Ok(())
            }

            Self::List {
                session,
                cwd,
                human,
                cmd_only,
                format,
            } => {
                let session = if *session {
                    Some(env::var("ATUIN_SESSION")?)
                } else {
                    None
                };
                let cwd = if *cwd {
                    Some(env::current_dir()?.display().to_string())
                } else {
                    None
                };

                let history = match (session, cwd) {
                    (None, None) => db.list(settings.filter_mode, &context, None, false).await?,
                    (None, Some(cwd)) => {
                        let query = format!("select * from history where cwd = '{cwd}';");
                        db.query_history(&query).await?
                    }
                    (Some(session), None) => {
                        let query = format!("select * from history where session = '{session}';");
                        db.query_history(&query).await?
                    }
                    (Some(session), Some(cwd)) => {
                        let query = format!(
                            "select * from history where cwd = '{}' and session = '{}';",
                            cwd, session
                        );
                        db.query_history(&query).await?
                    }
                };

                print_list(
                    &history,
                    ListMode::from_flags(*human, *cmd_only),
                    format.as_deref(),
                );

                Ok(())
            }

            Self::Last {
                human,
                cmd_only,
                format,
            } => {
                let last = db.last().await?;
                print_list(
                    &[last],
                    ListMode::from_flags(*human, *cmd_only),
                    format.as_deref(),
                );

                Ok(())
            }
        }
    }
}
