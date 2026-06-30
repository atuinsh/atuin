use std::fs::File;
use std::io::{IsTerminal as _, Write, stderr, stdout};

use atuin_common::utils::{self, Escapable as _};
use clap::Parser;
use eyre::Result;

use atuin_client::{
    database::Database,
    database::{Context, OptFilters, current_context},
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

    /// Ordered list of filter modes to search. With `--limit`, results from each mode are
    /// appended (higher-priority modes first) until the limit is reached or the modes are
    /// exhausted. Without `--limit`, searching stops at the first mode that returns a result.
    #[arg(long = "filter-modes", value_delimiter = ',')]
    filter_modes: Option<Vec<FilterMode>>,

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

    /// Filter by author. Supports $all-user (non-agents), $all-agent, or literal names.
    /// Can be specified multiple times.
    #[arg(long)]
    author: Option<Vec<String>>,

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
            } else if !stdout().is_terminal() {
                // stdout is not a terminal - likely command substitution like VAR=$(atuin search -i)
                // Write to stdout so it gets captured. This requires some care on Windows, as the current
                // console code page or `[Console]::OutputEncoding` on PowerShell may be different from UTF-8.
                println!("{item}");
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
                authors: self.author.clone().unwrap_or_default(),
            };

            let filter_modes = self.filter_modes.as_deref();

            let mut entries =
                run_non_interactive(settings, opt_filter.clone(), filter_modes, &query, &db)
                    .await?;

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
                    }

                    let ids = history_store.delete_entries(entries).await?;
                    history_store.incremental_build(&db, &ids).await?;

                    entries = run_non_interactive(
                        settings,
                        opt_filter.clone(),
                        filter_modes,
                        &query,
                        &db,
                    )
                    .await?;
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
#[allow(
    clippy::too_many_arguments,
    clippy::cast_possible_truncation,
    clippy::cast_possible_wrap
)]
async fn run_non_interactive(
    settings: &Settings,
    filter_options: OptFilters,
    filter_modes: Option<&[FilterMode]>,
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

    // Search the requested modes in priority order, falling back to the single
    // configured default when no explicit list is given.
    let modes = match filter_modes {
        Some(modes) if !modes.is_empty() => modes.to_vec(),
        _ => vec![settings.default_filter_mode(context.git_root.is_some())],
    };

    search_filter_modes(
        db,
        settings.search_mode,
        &context,
        &modes,
        query.join(" ").as_str(),
        &opt_filter,
    )
    .await
}

/// Search an ordered list of filter modes, highest priority first.
///
/// Without a limit, the results of the first mode that returns any match are used.
/// With a limit, commands are accumulated across modes (higher-priority first) until the
/// limit is filled or all modes are exhausted, paging deeper into a mode when
/// de-duplication leaves it short. Unless `--include-duplicates` is set, a command
/// contributed by a higher-priority mode is dropped from lower-priority ones. The user's
/// `offset` then applies to the combined, ordered result stream.
#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_possible_wrap,
    clippy::cast_sign_loss
)]
async fn search_filter_modes(
    db: &impl Database,
    search_mode: SearchMode,
    context: &Context,
    modes: &[FilterMode],
    query: &str,
    opt_filter: &OptFilters,
) -> Result<Vec<History>> {
    let Some(limit) = opt_filter.limit else {
        // Without a limit, return the results of the first mode that yields a match.
        for &filter_mode in modes {
            let found = db
                .search(search_mode, filter_mode, context, query, opt_filter.clone())
                .await?;
            if !found.is_empty() {
                return Ok(found);
            }
        }
        return Ok(Vec::new());
    };

    // The offset pages the combined stream, so collect enough to cover offset + limit.
    let offset = opt_filter.offset.unwrap_or(0).max(0);
    let target = offset.saturating_add(limit);

    let mut results = Vec::new();
    // Commands already contributed by a higher-priority mode, so each appears only once.
    let mut seen = std::collections::HashSet::new();

    'modes: for &filter_mode in modes {
        // Page through this mode from its own start, advancing the offset, until we've
        // collected enough or exhausted its results. Over-fetch by the number of
        // already-seen commands - the most this mode could collide with - so a single
        // query usually suffices.
        let mut page_offset = 0;
        loop {
            let remaining = target - results.len() as i64;
            if remaining <= 0 {
                break 'modes;
            }

            let fetch = remaining + seen.len() as i64;
            let mut found = db
                .search(
                    search_mode,
                    filter_mode,
                    context,
                    query,
                    OptFilters {
                        limit: Some(fetch),
                        offset: Some(page_offset),
                        ..opt_filter.clone()
                    },
                )
                .await?;

            let fetched = found.len() as i64;
            if !opt_filter.include_duplicates {
                found.retain(|h| seen.insert(h.command.clone()));
            }
            results.append(&mut found);

            // A short page means this mode has no more results to page through.
            if fetched < fetch {
                break;
            }
            page_offset += fetched;
        }
    }

    // Apply the user's offset and limit to the combined, ordered stream.
    results.drain(..(offset as usize).min(results.len()));
    results.truncate(limit as usize);
    Ok(results)
}

#[cfg(test)]
mod tests {
    use super::{Cmd, search_filter_modes};
    use atuin_client::database::{Context, Database, OptFilters, Sqlite};
    use atuin_client::history::History;
    use atuin_client::settings::{FilterMode, SearchMode};
    use clap::Parser;
    use time::OffsetDateTime;

    /// Seed an in-memory db with commands whose (session, hostname, cwd) make them match
    /// overlapping filter modes, and return a context where session=S, host=H, cwd=/dir.
    async fn seeded_db() -> Sqlite {
        let db = Sqlite::new("sqlite::memory:", 1.0).await.unwrap();
        // (command, session, hostname, cwd); inserted oldest-first.
        let rows = [
            ("a", "S", "H", "/dir"),   // session, host, directory, global
            ("b", "S", "H", "/other"), // session, host, global
            ("c", "X", "H", "/dir"),   // host, directory, global
            ("d", "X", "O", "/other"), // global only
        ];
        for (cmd, session, hostname, cwd) in rows {
            let mut h: History = History::capture()
                .timestamp(OffsetDateTime::now_utc())
                .command(cmd)
                .cwd(cwd)
                .build()
                .into();
            h.session = session.to_string();
            h.hostname = hostname.to_string();
            db.save(&h).await.unwrap();
        }
        db
    }

    fn test_context() -> Context {
        Context {
            session: "S".into(),
            hostname: "H".into(),
            cwd: "/dir".into(),
            host_id: "host".into(),
            git_root: None,
        }
    }

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

    #[test]
    fn search_filter_modes_cli_flag() {
        let cmd =
            Cmd::try_parse_from(["search", "--filter-modes", "session,directory,global"]).unwrap();
        assert_eq!(
            cmd.filter_modes,
            Some(vec![
                FilterMode::Session,
                FilterMode::Directory,
                FilterMode::Global
            ])
        );
    }

    #[test]
    fn search_author_cli_flag() {
        let cmd =
            Cmd::try_parse_from(["search", "--author", "codex", "--author", "ellie"]).unwrap();
        assert_eq!(
            cmd.author,
            Some(vec!["codex".to_string(), "ellie".to_string()])
        );
    }

    async fn search_commands(modes: &[FilterMode], opt_filter: &OptFilters) -> Vec<String> {
        let db = seeded_db().await;
        search_filter_modes(
            &db,
            SearchMode::Prefix,
            &test_context(),
            modes,
            "",
            opt_filter,
        )
        .await
        .unwrap()
        .into_iter()
        .map(|h| h.command)
        .collect()
    }

    // All modes; results are ordered most-recent-first within each mode (insertion order
    // was a, b, c, d), so the de-duplicated, priority-ordered stream is b, a, c, d.
    const ALL_MODES: [FilterMode; 4] = [
        FilterMode::Session,
        FilterMode::Directory,
        FilterMode::Host,
        FilterMode::Global,
    ];

    #[tokio::test(flavor = "multi_thread")]
    async fn filter_modes_dedup_accumulation() {
        let commands = search_commands(
            &ALL_MODES,
            &OptFilters {
                limit: Some(10),
                ..Default::default()
            },
        )
        .await;

        // Each command appears once despite matching several modes, higher-priority first.
        assert_eq!(commands, ["b", "a", "c", "d"]);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn filter_modes_offset_pages_combined_stream() {
        // offset/limit page the combined stream [b, a, c, d], not each mode separately.
        let commands = search_commands(
            &ALL_MODES,
            &OptFilters {
                limit: Some(2),
                offset: Some(1),
                ..Default::default()
            },
        )
        .await;

        assert_eq!(commands, ["a", "c"]);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn filter_modes_include_duplicates_keeps_cross_mode_dupes() {
        // `a` matches both session and directory; with --include-duplicates it is kept twice.
        let commands = search_commands(
            &[FilterMode::Session, FilterMode::Directory],
            &OptFilters {
                limit: Some(10),
                include_duplicates: true,
                ..Default::default()
            },
        )
        .await;

        assert_eq!(commands, ["b", "a", "c", "a"]);
    }
}
