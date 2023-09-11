use atuin_common::utils;
use clap::Parser;
use eyre::Result;

use atuin_client::{
    database::Database,
    database::{current_context, OptFilters},
    history::History,
    settings::{FilterMode, SearchMode, Settings},
};

use super::history::ListMode;

mod cursor;
mod duration;
mod engines;
mod history_list;
mod interactive;
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

    /// Offset from the start of the results
    #[arg(long)]
    offset: Option<i64>,

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

    /// Delete anything matching this query. Will not print out the match
    #[arg(long)]
    delete: bool,

    /// Delete EVERYTHING!
    #[arg(long)]
    delete_it_all: bool,

    /// Reverse the order of results, oldest first
    #[arg(long, short)]
    reverse: bool,

    /// Available variables: {command}, {directory}, {duration}, {user}, {host}, {time}, {exit} and
    /// {relativetime}.
    /// Example: --format "{time} - [{duration}] - {directory}$\t{command}"
    #[arg(long, short)]
    format: Option<String>,

    /// Set the maximum number of lines Atuin's interface should take up.
    #[arg(long = "inline-height")]
    inline_height: Option<u16>,
}

impl Cmd {
    pub async fn run(self, mut db: impl Database, settings: &mut Settings) -> Result<()> {
        if self.delete && self.query.is_empty() {
            println!("Please specify a query to match the items you wish to delete. If you wish to delete all history, pass --delete-it-all");
            return Ok(());
        }

        if self.delete_it_all && !self.query.is_empty() {
            println!(
                "--delete-it-all will delete ALL of your history! It does not require a query."
            );
            return Ok(());
        }

        if self.search_mode.is_some() {
            settings.search_mode = self.search_mode.unwrap();
        }
        if self.filter_mode.is_some() {
            settings.filter_mode = self.filter_mode.unwrap();
        }
        if self.inline_height.is_some() {
            settings.inline_height = self.inline_height.unwrap();
        }

        settings.shell_up_key_binding = self.shell_up_key_binding;

        if self.interactive {
            let item = interactive::history(&self.query, settings, db).await?;
            eprintln!("{item}");
        } else {
            let list_mode = ListMode::from_flags(self.human, self.cmd_only);

            let opt_filter = OptFilters {
                exit: self.exit,
                exclude_exit: self.exclude_exit,
                cwd: self.cwd,
                exclude_cwd: self.exclude_cwd,
                before: self.before,
                after: self.after,
                limit: self.limit,
                offset: self.offset,
                reverse: self.reverse,
            };

            let mut entries =
                run_non_interactive(settings, opt_filter.clone(), &self.query, &mut db).await?;

            if entries.is_empty() {
                std::process::exit(1)
            }

            // if we aren't deleting, print it all
            if self.delete || self.delete_it_all {
                // delete it
                // it only took me _years_ to add this
                // sorry
                while !entries.is_empty() {
                    for entry in &entries {
                        eprintln!("deleting {}", entry.id);
                        db.delete(entry.clone()).await?;
                    }

                    entries =
                        run_non_interactive(settings, opt_filter.clone(), &self.query, &mut db)
                            .await?;
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
#[allow(clippy::too_many_arguments, clippy::cast_possible_truncation)]
async fn run_non_interactive(
    settings: &Settings,
    filter_options: OptFilters,
    query: &[String],
    db: &mut impl Database,
) -> Result<Vec<History>> {
    let dir = if filter_options.cwd.as_deref() == Some(".") {
        Some(utils::get_current_dir())
    } else {
        filter_options.cwd
    };

    let context = current_context();

    let opt_filter = OptFilters {
        cwd: dir.clone(),
        ..filter_options
    };

    let dir = dir.unwrap_or_else(|| "/".to_string());
    let filter_mode = if settings.workspaces && utils::has_git_dir(dir.as_str()) {
        FilterMode::Workspace
    } else {
        settings.filter_mode
    };

    let results = db
        .search(
            settings.search_mode,
            filter_mode,
            &context,
            query.join(" ").as_str(),
            opt_filter,
        )
        .await?;

    Ok(results)
}
