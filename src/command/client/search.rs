use chrono::Utc;
use clap::Parser;
use eyre::Result;

use atuin_client::{
    database::current_context,
    database::Database,
    history::History,
    settings::{FilterMode, SearchMode, Settings},
};

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
    #[arg(long, short)]
    cwd: Option<String>,

    /// Exclude directory from results
    #[arg(long = "exclude-cwd")]
    exclude_cwd: Option<String>,

    /// Filter search result by exit code
    #[arg(long, short)]
    exit: Option<i64>,

    /// Exclude results with this exit code
    #[arg(long = "exclude-exit")]
    exclude_exit: Option<i64>,

    /// Only include results added before this date
    #[arg(long, short)]
    before: Option<String>,

    /// Only include results after this date
    #[arg(long)]
    after: Option<String>,

    /// How many entries to return at most
    #[arg(long)]
    limit: Option<i64>,

    /// Open interactive search UI
    #[arg(long, short)]
    interactive: bool,

    /// Allow overriding filter mode over config
    #[arg(long = "filter-mode")]
    filter_mode: Option<FilterMode>,

    /// Allow overriding search mode over config
    #[arg(long = "search-mode")]
    search_mode: Option<SearchMode>,

    /// Use human-readable formatting for time
    #[arg(long)]
    human: bool,

    query: Vec<String>,

    /// Show only the text of the command
    #[arg(long)]
    cmd_only: bool,
}

impl Cmd {
    pub async fn run(self, db: &mut impl Database, settings: &mut Settings) -> Result<()> {
        if self.search_mode.is_some() {
            settings.search_mode = self.search_mode.unwrap();
        }
        if self.filter_mode.is_some() {
            settings.filter_mode = self.filter_mode.unwrap();
        }
        if self.interactive {
            let item = interactive::history(&self.query, settings, db).await?;
            eprintln!("{item}");
        } else {
            let list_mode = ListMode::from_flags(self.human, self.cmd_only);
            let entries = run_non_interactive(
                settings,
                list_mode,
                self.cwd,
                self.exit,
                self.exclude_exit,
                self.exclude_cwd,
                self.before,
                self.after,
                self.limit,
                &self.query,
                db,
            )
            .await?;
            if entries == 0 {
                std::process::exit(1)
            }
        };
        Ok(())
    }
}

// This is supposed to more-or-less mirror the command line version, so ofc
// it is going to have a lot of args
#[allow(clippy::too_many_arguments)]
async fn run_non_interactive(
    settings: &Settings,
    list_mode: ListMode,
    cwd: Option<String>,
    exit: Option<i64>,
    exclude_exit: Option<i64>,
    exclude_cwd: Option<String>,
    before: Option<String>,
    after: Option<String>,
    limit: Option<i64>,
    query: &[String],
    db: &mut impl Database,
) -> Result<usize> {
    let dir = if cwd.as_deref() == Some(".") {
        let current = std::env::current_dir()?;
        let current = current.as_os_str();
        let current = current.to_str().unwrap();

        Some(current.to_owned())
    } else {
        cwd
    };

    let context = current_context();

    let results = db
        .search(
            limit,
            settings.search_mode,
            settings.filter_mode,
            &context,
            query.join(" ").as_str(),
        )
        .await?;

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

            if let Some(cwd) = &exclude_cwd {
                if h.cwd.as_str() == cwd.as_str() {
                    return false;
                }
            }

            if let Some(cwd) = &dir {
                if h.cwd.as_str() != cwd.as_str() {
                    return false;
                }
            }

            if let Some(before) = &before {
                let before =
                    interim::parse_date_string(before.as_str(), Utc::now(), interim::Dialect::Uk);

                if before.is_err() || h.timestamp.gt(&before.unwrap()) {
                    return false;
                }
            }

            if let Some(after) = &after {
                let after =
                    interim::parse_date_string(after.as_str(), Utc::now(), interim::Dialect::Uk);

                if after.is_err() || h.timestamp.lt(&after.unwrap()) {
                    return false;
                }
            }

            true
        })
        .map(std::borrow::ToOwned::to_owned)
        .collect();

    super::history::print_list(&results, list_mode);
    Ok(results.len())
}
