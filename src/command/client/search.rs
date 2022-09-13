use chrono::Utc;
use clap::Parser;
use eyre::Result;

use atuin_client::{database::Context, database::Database, history::History, settings::Settings};

use super::history::ListMode;

mod cursor;
mod duration;
mod event;
mod history_list;
mod interactive;
pub use duration::format_duration;

#[derive(Parser)]
pub struct Cmd {
    /// Filter search result by directory
    #[clap(long, short)]
    cwd: Option<String>,

    /// Exclude directory from results
    #[clap(long = "exclude-cwd")]
    exclude_cwd: Option<String>,

    /// Filter search result by exit code
    #[clap(long, short)]
    exit: Option<i64>,

    /// Exclude results with this exit code
    #[clap(long = "exclude-exit")]
    exclude_exit: Option<i64>,

    /// Only include results added before this date
    #[clap(long, short)]
    before: Option<String>,

    /// Only include results after this date
    #[clap(long)]
    after: Option<String>,

    /// How many entries to return at most
    #[clap(long)]
    limit: Option<i64>,

    /// Open interactive search UI
    #[clap(long, short)]
    interactive: bool,

    /// Use human-readable formatting for time
    #[clap(long)]
    human: bool,

    query: Vec<String>,

    /// Show only the text of the command
    #[clap(long)]
    cmd_only: bool,
}

impl Cmd {
    pub fn run(self, db: &mut impl Database, settings: &Settings) -> Result<()> {
        if self.interactive {
            let item = interactive::history(
                &self.query,
                settings.search_mode,
                settings.filter_mode,
                settings.style,
                db,
            )?;
            eprintln!("{}", item);
        } else {
            let list_mode = ListMode::from_flags(self.human, self.cmd_only);
            run_non_interactive(
                settings,
                list_mode,
                self.cwd,
                self.exit,
                self.exclude_exit,
                self.exclude_cwd.as_deref(),
                self.before.as_deref(),
                self.after.as_deref(),
                self.limit,
                &self.query,
                db,
            )?;
        };
        Ok(())
    }
}

// This is supposed to more-or-less mirror the command line version, so ofc
// it is going to have a lot of args
#[allow(clippy::too_many_arguments)]
fn run_non_interactive(
    settings: &Settings,
    list_mode: ListMode,
    cwd: Option<String>,
    exit: Option<i64>,
    exclude_exit: Option<i64>,
    exclude_cwd: Option<&str>,
    before: Option<&str>,
    after: Option<&str>,
    limit: Option<i64>,
    query: &[String],
    db: &mut impl Database,
) -> Result<()> {
    let dir = if cwd.as_deref() == Some(".") {
        let current = std::env::current_dir()?;
        let current = current.as_os_str();
        let current = current.to_str().unwrap();

        Some(current.to_owned())
    } else {
        cwd
    };

    let context = Context::default();

    let results = db.search(
        limit,
        settings.search_mode,
        settings.filter_mode,
        &context,
        query.join(" ").as_str(),
    )?;

    // TODO: This filtering would be better done in the SQL query, I just
    // need a nice way of building queries.
    let results: Vec<History> = results
        .iter()
        .filter(|h| {
            if let Some(exit) = exit {
                if h.exit != exit {
                    return false;
                }
            }

            if let Some(exit) = exclude_exit {
                if h.exit == exit {
                    return false;
                }
            }

            if let Some(cwd) = exclude_cwd {
                if h.cwd.as_str() == cwd {
                    return false;
                }
            }

            if let Some(cwd) = &dir {
                if h.cwd.as_str() != cwd.as_str() {
                    return false;
                }
            }

            if let Some(before) = before {
                let before = chrono_english::parse_date_string(
                    before,
                    Utc::now(),
                    chrono_english::Dialect::Uk,
                );

                if before.is_err() || h.timestamp.gt(&before.unwrap()) {
                    return false;
                }
            }

            if let Some(after) = after {
                let after = chrono_english::parse_date_string(
                    after,
                    Utc::now(),
                    chrono_english::Dialect::Uk,
                );

                if after.is_err() || h.timestamp.lt(&after.unwrap()) {
                    return false;
                }
            }

            true
        })
        .map(std::borrow::ToOwned::to_owned)
        .collect();

    super::history::print_list(&results, list_mode);
    Ok(())
}
