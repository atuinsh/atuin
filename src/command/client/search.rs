use atuin_common::utils;
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

mod core;
mod cursor;
mod duration;
mod history_list;
mod skim_impl;
mod tui_shell;
pub use duration::{format_duration, format_duration_into};

#[allow(clippy::struct_excessive_bools)]
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

    /// Marker argument used to inform atuin that it was invoked from a shell up-key binding (hidden from help to avoid confusion)
    #[arg(long = "shell-up-key-binding", hide = true)]
    shell_up_key_binding: bool,

    /// Use human-readable formatting for time
    #[arg(long)]
    human: bool,

    query: Vec<String>,

    /// Show only the text of the command
    #[arg(long)]
    cmd_only: bool,

    // Delete anything matching this query. Will not print out the match
    #[arg(long)]
    delete: bool,

    /// Available variables: {command}, {directory}, {duration}, {user}, {host} and {time}.
    /// Example: --format "{time} - [{duration}] - {directory}$\t{command}"
    #[arg(long, short)]
    format: Option<String>,
}

impl Cmd {
    pub async fn run(self, mut db: impl Database, settings: &mut Settings) -> Result<()> {
        if self.search_mode.is_some() {
            settings.search_mode = self.search_mode.unwrap();
        }
        if self.filter_mode.is_some() {
            settings.filter_mode = self.filter_mode.unwrap();
        }

        settings.shell_up_key_binding = self.shell_up_key_binding;

        if self.interactive {
            let item = tui_shell::history(&self.query, settings, db).await?;
            eprintln!("{item}");
        } else {
            let list_mode = ListMode::from_flags(self.human, self.cmd_only);
            let entries = run_non_interactive(
                settings,
                self.cwd,
                self.exit,
                self.exclude_exit,
                self.exclude_cwd,
                self.before,
                self.after,
                self.limit,
                &self.query,
                &mut db,
            )
            .await?;

            if entries.is_empty() {
                std::process::exit(1)
            }

            // if we aren't deleting, print it all
            if self.delete {
                // delete it
                // it only took me _years_ to add this
                // sorry
                for entry in entries {
                    db.delete(entry).await?;
                }
            } else {
                super::history::print_list(&entries, list_mode, self.format.as_deref());
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
    cwd: Option<String>,
    exit: Option<i64>,
    exclude_exit: Option<i64>,
    exclude_cwd: Option<String>,
    before: Option<String>,
    after: Option<String>,
    limit: Option<i64>,
    query: &[String],
    db: &mut impl Database,
) -> Result<Vec<History>> {
    let dir = if cwd.as_deref() == Some(".") {
        Some(utils::get_current_dir())
    } else {
        cwd
    };

    let context = current_context();

    let before = before.and_then(|b| {
        interim::parse_date_string(b.as_str(), Utc::now(), interim::Dialect::Uk)
            .map_or(None, |d| Some(d.timestamp_nanos()))
    });

    let after = after.and_then(|a| {
        interim::parse_date_string(a.as_str(), Utc::now(), interim::Dialect::Uk)
            .map_or(None, |d| Some(d.timestamp_nanos()))
    });

    let results = db
        .search(
            settings.search_mode,
            settings.filter_mode,
            &context,
            query.join(" ").as_str(),
            limit,
            before,
            after,
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

            true
        })
        .map(std::borrow::ToOwned::to_owned)
        .collect();

    Ok(results)
}
