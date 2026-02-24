use std::fs::File;
use std::io::{IsTerminal as _, Write, stderr};

use atuin_common::utils::{self, Escapable as _};
use clap::Parser;
use eyre::Result;

use atuin_client::{
    database::Database,
    database::{OptFilters, current_context},
    encryption,
    history::{History, store::HistoryStore},
    record::sqlite_store::SqliteStore,
    settings::{FilterMode, KeymapMode, SearchMode, Settings, Timezone},
    theme::Theme,
};

use super::history::ListMode;

mod cursor;
mod duration;
mod engines;
mod history_list;
mod inspector;
mod interactive;
pub mod keybindings;

pub use duration::format_duration_into;

#[allow(clippy::struct_excessive_bools, clippy::struct_field_names)]
#[derive(Parser, Debug)]
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

    /// Notify the keymap at the shell's side
    #[arg(long = "keymap-mode", default_value = "auto")]
    keymap_mode: KeymapMode,

    /// Use human-readable formatting for time
    #[arg(long)]
    human: bool,

    #[arg(allow_hyphen_values = true)]
    query: Option<Vec<String>>,

    /// Show only the text of the command
    #[arg(long)]
    cmd_only: bool,

    /// Terminate the output with a null, for better multiline handling
    #[arg(long)]
    print0: bool,

    /// Delete anything matching this query. Will not print out the match
    #[arg(long)]
    delete: bool,

    /// Delete EVERYTHING!
    #[arg(long)]
    delete_it_all: bool,

    /// Reverse the order of results, oldest first
    #[arg(long, short)]
    reverse: bool,

    /// Display the command time in another timezone other than the configured default.
    ///
    /// This option takes one of the following kinds of values:
    /// - the special value "local" (or "l") which refers to the system time zone
    /// - an offset from UTC (e.g. "+9", "-2:30")
    #[arg(long, visible_alias = "tz")]
    #[arg(allow_hyphen_values = true)]
    // Clippy warns about `Option<Option<T>>`, but we suppress it because we need
    // this distinction for proper argument handling.
    #[allow(clippy::option_option)]
    timezone: Option<Option<Timezone>>,

    /// Available variables: {command}, {directory}, {duration}, {user}, {host}, {time}, {exit} and
    /// {relativetime}.
    /// Example: --format "{time} - [{duration}] - {directory}$\t{command}"
    #[arg(long, short)]
    format: Option<String>,

    /// Set the maximum number of lines Atuin's interface should take up.
    #[arg(long = "inline-height")]
    inline_height: Option<u16>,

    /// Include duplicate commands in the output (non-interactive only)
    #[arg(long)]
    include_duplicates: bool,

    /// File name to write the result to (hidden from help as this is meant to be used from a script)
    #[arg(long = "result-file", hide = true)]
    result_file: Option<String>,
}

impl Cmd {
    /// Returns true if this search command will run in interactive (TUI) mode
    pub fn is_interactive(&self) -> bool {
        self.interactive
    }

    // clippy: please write this instead
    // clippy: now it has too many lines
    // me: I'll do it later OKAY
    #[allow(clippy::too_many_lines)]
    pub async fn run(
        self,
        db: impl Database,
        settings: &mut Settings,
        store: SqliteStore,
        theme: &Theme,
    ) -> Result<()> {
        let query = self.query.unwrap_or_else(|| {
            std::env::var("ATUIN_QUERY").map_or_else(
                |_| vec![],
                |query| {
                    query
                        .split(' ')
                        .map(std::string::ToString::to_string)
                        .collect()
                },
            )
        });

        if (self.delete_it_all || self.delete) && self.limit.is_some() {
            // Because of how deletion is implemented, it will always delete all matches
            // and disregard the limit option. It is also not clear what deletion with a
            // limit would even mean. Deleting the LIMIT most recent entries that match
            // the search query would make sense, but that wouldn't match what's displayed
            // when running the equivalent search, but deleting those entries that are
            // displayed with the search would leave any duplicates of those lines which may
            // or may not have been intended to be deleted.
            eprintln!("\"--limit\" is not compatible with deletion.");
            return Ok(());
        }

        if self.delete && query.is_empty() {
            eprintln!(
                "Please specify a query to match the items you wish to delete. If you wish to delete all history, pass --delete-it-all"
            );
            return Ok(());
        }

        if self.delete_it_all && !query.is_empty() {
            eprintln!(
                "--delete-it-all will delete ALL of your history! It does not require a query."
            );
            return Ok(());
        }

        if let Some(search_mode) = self.search_mode {
            settings.search_mode = search_mode;
        }
        if let Some(filter_mode) = self.filter_mode {
            settings.filter_mode = Some(filter_mode);
        }
        if let Some(inline_height) = self.inline_height {
            settings.inline_height = inline_height;
        }

        settings.shell_up_key_binding = self.shell_up_key_binding;

        // `keymap_mode` specified in config.toml overrides the `--keymap-mode`
        // option specified in the keybindings.
        settings.keymap_mode = match settings.keymap_mode {
            KeymapMode::Auto => self.keymap_mode,
            value => value,
        };
        settings.keymap_mode_shell = self.keymap_mode;

        let encryption_key: [u8; 32] = encryption::load_key(settings)?.into();

        let host_id = Settings::host_id().await?;
        let history_store = HistoryStore::new(store.clone(), host_id, encryption_key);

        if self.interactive {
            let item = interactive::history(&query, settings, db, &history_store, theme).await?;

            if let Some(result_file) = self.result_file {
                let mut file = File::create(result_file)?;
                write!(file, "{item}")?;
            } else if stderr().is_terminal() {
                eprintln!("{}", item.escape_control());
            } else {
                eprintln!("{item}");
            }
        } else {
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
                include_duplicates: self.include_duplicates,
            };

            let mut entries =
                run_non_interactive(settings, opt_filter.clone(), &query, &db).await?;

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

                        if settings.sync.records {
                            let (id, _) = history_store.delete(entry.id.clone()).await?;
                            history_store.incremental_build(&db, &[id]).await?;
                        } else {
                            db.delete(entry.clone()).await?;
                        }
                    }

                    entries =
                        run_non_interactive(settings, opt_filter.clone(), &query, &db).await?;
                }
            } else {
                let format = match self.format {
                    None => Some(settings.history_format.as_str()),
                    _ => self.format.as_deref(),
                };
                let tz = match self.timezone {
                    Some(Some(tz)) => tz,                   // User provided a value
                    Some(None) | None => settings.timezone, // No value was provided
                };

                super::history::print_list(
                    &entries,
                    ListMode::from_flags(self.human, self.cmd_only),
                    format,
                    self.print0,
                    true,
                    tz,
                );
            }
        }
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
    db: &impl Database,
) -> Result<Vec<History>> {
    let dir = if filter_options.cwd.as_deref() == Some(".") {
        Some(utils::get_current_dir())
    } else {
        filter_options.cwd
    };

    let context = current_context().await?;

    let opt_filter = OptFilters {
        cwd: dir.clone(),
        ..filter_options
    };

    let filter_mode = settings.default_filter_mode(context.git_root.is_some());

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

#[cfg(test)]
mod tests {
    use super::Cmd;
    use clap::Parser;

    #[test]
    fn search_for_triple_dash() {
        // Issue #3028: searching for `---` should not be treated as a CLI flag
        let cmd = Cmd::try_parse_from(["search", "---"]);
        assert!(cmd.is_ok(), "Failed to parse '---' as a query: {cmd:?}");
        let cmd = cmd.unwrap();
        assert_eq!(cmd.query, Some(vec!["---".to_string()]));
    }

    #[test]
    fn search_for_double_dash_value() {
        // Searching for strings starting with -- should also work
        let cmd = Cmd::try_parse_from(["search", "--", "--foo"]);
        assert!(cmd.is_ok());
        let cmd = cmd.unwrap();
        assert_eq!(cmd.query, Some(vec!["--foo".to_string()]));
    }
}
