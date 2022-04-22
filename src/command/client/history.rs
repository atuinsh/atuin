use std::env;
use std::io::Write;
use std::time::Duration;

use clap::Subcommand;
use eyre::Result;
use tabwriter::TabWriter;

use atuin_client::database::Database;
use atuin_client::history::History;
use atuin_client::settings::Settings;
use atuin_client::sync;

#[derive(Subcommand)]
#[clap(infer_subcommands = true)]
pub enum Cmd {
    /// Begins a new command in the history
    Start { command: Vec<String> },

    /// Finishes a new command in the history (adds time, exit code)
    End {
        id: String,
        #[clap(long, short)]
        exit: i64,
    },

    /// List all items in history
    List {
        #[clap(long, short)]
        cwd: bool,

        #[clap(long, short)]
        session: bool,

        #[clap(long)]
        human: bool,

        /// Show only the text of the command
        #[clap(long)]
        cmd_only: bool,
    },

    /// Get the last command ran
    Last {
        #[clap(long)]
        human: bool,

        /// Show only the text of the command
        #[clap(long)]
        cmd_only: bool,
    },
}

#[allow(clippy::cast_sign_loss)]
pub fn print_list(h: &[History], human: bool, cmd_only: bool) {
    let mut writer = TabWriter::new(std::io::stdout()).padding(2);

    let lines = h.iter().rev().map(|h| {
        if human {
            let duration = humantime::format_duration(Duration::from_nanos(std::cmp::max(
                h.duration, 0,
            ) as u64))
            .to_string();
            let duration: Vec<&str> = duration.split(' ').collect();
            let duration = duration[0];

            format!(
                "{}\t{}\t{}\n",
                h.timestamp.format("%Y-%m-%d %H:%M:%S"),
                h.command.trim(),
                duration,
            )
        } else if cmd_only {
            format!("{}\n", h.command.trim())
        } else {
            format!(
                "{}\t{}\t{}\n",
                h.timestamp.timestamp_nanos(),
                h.command.trim(),
                h.duration
            )
        }
    });

    for i in lines {
        writer
            .write_all(i.as_bytes())
            .expect("failed to write to tab writer");
    }

    writer.flush().expect("failed to flush tab writer");
}

impl Cmd {
    pub async fn run(
        &self,
        settings: &Settings,
        db: &mut (impl Database + Send + Sync),
    ) -> Result<()> {
        match self {
            Self::Start { command: words } => {
                let command = words.join(" ");

                if command.starts_with(' ') {
                    return Ok(());
                }

                // It's better for atuin to silently fail here and attempt to
                // store whatever is ran, than to throw an error to the terminal
                let cwd = match env::current_dir() {
                    Ok(dir) => dir.display().to_string(),
                    Err(_) => String::from(""),
                };

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
                    debug!("running periodic background sync");
                    sync::sync(settings, false, db).await?;
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
                    (None, None) => db.list(settings.filter_mode, None, false).await?,
                    (None, Some(cwd)) => {
                        let query = format!("select * from history where cwd = '{}';", cwd);
                        db.query_history(&query).await?
                    }
                    (Some(session), None) => {
                        let query = format!("select * from history where session = {};", session);
                        db.query_history(&query).await?
                    }
                    (Some(session), Some(cwd)) => {
                        let query = format!(
                            "select * from history where cwd = '{}' and session = {};",
                            cwd, session
                        );
                        db.query_history(&query).await?
                    }
                };

                print_list(&history, *human, *cmd_only);

                Ok(())
            }

            Self::Last { human, cmd_only } => {
                let last = db.last().await?;
                print_list(&[last], *human, *cmd_only);

                Ok(())
            }
        }
    }
}
