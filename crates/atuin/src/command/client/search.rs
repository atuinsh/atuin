use std::fs::File;
use std::io::{IsTerminal as _, Write, stderr, stdout};

use atuin_common::filter::OrFilter;
use atuin_common::{string::EscapeNonPrintablePosixExt as _, utils};
use clap::Parser;
use eyre::Result;

use atuin_client::{
    database::Database,
    database::{Context, DbSearchMode, OptFilters, current_context},
    history::{AuthorPattern, History, store::HistoryStore},
    record::sqlite_store::SqliteStore,
    settings::{FilterMode, KeymapMode, RequestedSearchMode, Settings},
    theme::Theme,
};
use atuin_common::encryption::paseto_v4;

use super::history::ListMode;

mod cursor;
mod engines;
mod history_list;
mod inspector;
mod interactive;
pub mod keybindings;
mod syntax;

use atuin_common::time::UtcOffsetSpec;

#[allow(clippy::struct_excessive_bools, clippy::struct_field_names)]
#[derive(Parser, Debug)]
pub struct Cmd {
    /// Filter search result by directory
    #[arg(long, short)]
    cwd: Option<String>,

    /// Exclude directory from results
    #[arg(long)]
    exclude_cwd: Option<String>,

    /// Filter search result by exit code
    #[arg(long, short)]
    exit: Option<i64>,

    /// Exclude results with this exit code
    #[arg(long)]
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
    #[arg(long)]
    filter_mode: Option<FilterMode>,

    /// Ordered list of filter modes to search. With `--limit`, results from each mode are
    /// appended (higher-priority modes first) until the limit is reached or the modes are
    /// exhausted. Without `--limit`, searching stops at the first mode that returns a result.
    #[arg(long = "filter-modes", value_delimiter = ',')]
    filter_modes: Option<Vec<FilterMode>>,

    /// Allow overriding search mode over config
    ///
    /// Note: for non-interactive searches, "daemon-fuzzy" behaves like "fuzzy". "skim" used to
    /// behave like "fuzzy" in non-interactive searches too; it has since been removed but is still
    /// accepted here as an alias of "fuzzy".
    #[arg(long)]
    search_mode: Option<RequestedSearchMode>,

    /// Marker argument used to inform atuin that it was invoked from a shell up-key binding (hidden from help to avoid confusion)
    #[arg(long, hide = true)]
    shell_up_key_binding: bool,

    /// Notify the keymap at the shell's side
    #[arg(long, default_value = "auto")]
    keymap_mode: KeymapMode,

    /// Use human-readable formatting for time
    #[arg(long)]
    human: bool,

    #[arg(allow_hyphen_values = true)]
    query: Vec<String>,

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
    ///
    /// - the special value "local" (or "l") which refers to the system time zone
    /// - an offset from UTC (e.g. "+9", "-2:30")
    #[arg(long, visible_alias = "tz", verbatim_doc_comment)]
    // `num_args = 0..=1` allows a user to run `atuin search --tz` with no argument to `--tz`. This
    // does the same thing as not providing the flag, but we previously allowed it (via an
    // `Option<Option<T>>` field type), so let's keep supporting it to avoid breaking existing
    // scripts.
    #[arg(allow_hyphen_values = true, num_args = 0..=1)]
    timezone: Option<UtcOffsetSpec>,

    /// Available variables: {command}, {directory}, {duration}, {user}, {host}, {time}, {exit} and
    /// {relativetime}.
    ///
    /// Example: --format "{time} - [{duration}] - {directory}$\t{command}"
    #[arg(long, short)]
    format: Option<String>,

    /// Set the maximum number of lines Atuin's interface should take up.
    #[arg(long)]
    inline_height: Option<u16>,

    /// Filter by author. Supports $all-user (non-agents), $all-agent, or literal names.
    ///
    /// Can be specified multiple times.
    #[arg(long)]
    author: Vec<AuthorPattern>,

    /// Include duplicate commands in the output (non-interactive only)
    #[arg(long)]
    include_duplicates: bool,

    /// File name to write the result to (hidden from help as this is meant to be used from a script)
    #[arg(long, hide = true)]
    result_file: Option<String>,

    /// Filter by the shell that was used to run the command
    ///
    /// If passed multiple times, commands from any of the shells will be shown.
    ///
    /// `--shell ""` will include commands for which the shell is unknown.
    #[arg(long)]
    shell: Vec<String>,
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
        let query = if self.query.is_empty() {
            std::env::var("ATUIN_QUERY").map_or_else(
                |_| vec![],
                |query| {
                    query
                        .split(' ')
                        .map(std::string::ToString::to_string)
                        .collect()
                },
            )
        } else {
            self.query
        };

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
            settings.requested_search_mode = search_mode;
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

        let encryption_key = paseto_v4::Key::try_load_from_path(&settings.key_path)?;

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
                eprintln!("{}", item.escape_non_printable());
            } else {
                eprintln!("{item}");
            }
        } else {
            // An empty `--author` / `--shell` list means no filtering on that field.
            let authors = OrFilter::from_list(self.author).unwrap_or_default();
            let shells = OrFilter::from_list(self.shell).unwrap_or_default();

            let opt_filter = OptFilters {
                exit: self.exit,
                exclude_exit: self.exclude_exit,
                only_failed: false,
                cwd: self.cwd.as_deref(),
                exclude_cwd: self.exclude_cwd.as_deref(),
                before: self.before.as_deref(),
                after: self.after.as_deref(),
                limit: self.limit,
                offset: self.offset,
                reverse: self.reverse,
                include_duplicates: self.include_duplicates,
                authors: authors.as_slice_filter(),
                shells: shells.as_slice_filter(),
            };

            let filter_modes = self.filter_modes.as_deref();

            let mut entries =
                run_non_interactive(settings, opt_filter, filter_modes, &query, &db).await?;

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
                    history_store.build_all(&db, &ids).await?;

                    entries = run_non_interactive(settings, opt_filter, filter_modes, &query, &db)
                        .await?;
                }
            } else {
                let format = self
                    .format
                    .as_deref()
                    .unwrap_or(settings.history_format.as_str());
                let tz = self.timezone.unwrap_or(settings.timezone);

                super::history::print_list(
                    &entries,
                    ListMode::from_flags(self.human, self.cmd_only),
                    Some(format),
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
async fn run_non_interactive(
    settings: &Settings,
    filter_options: OptFilters<'_>,
    filter_modes: Option<&[FilterMode]>,
    query: &[String],
    db: &impl Database,
) -> Result<Vec<History>> {
    let current_dir;
    let dir = if filter_options.cwd == Some(".") {
        current_dir = utils::get_current_dir();
        Some(current_dir.as_str())
    } else {
        filter_options.cwd
    };

    let context = current_context().await?;

    let opt_filter = OptFilters {
        cwd: dir,
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
        settings.search_mode().closest_db_mode(),
        &context,
        &modes,
        query.join(" ").as_str(),
        opt_filter,
    )
    .await
}

/// Search an ordered list of filter modes, highest priority first.
///
/// Without a limit, the results of the first mode that returns any match are used.
/// With a limit, unique commands are accumulated across modes (de-duplicated, earlier
/// mode wins) until the limit is filled or all modes are exhausted, paging deeper into a
/// mode when de-duplication leaves it short.
#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_possible_wrap,
    clippy::cast_sign_loss
)]
async fn search_filter_modes(
    db: &impl Database,
    search_mode: DbSearchMode,
    context: &Context,
    modes: &[FilterMode],
    query: &str,
    opt_filter: OptFilters<'_>,
) -> Result<Vec<History>> {
    let mut results = Vec::new();
    // Commands already contributed by a higher-priority mode, so each appears only once.
    let mut seen = std::collections::HashSet::new();

    for &filter_mode in modes {
        let Some(limit) = opt_filter.limit else {
            // Without a limit, take the first mode that returns any results.
            let mut found = db
                .search(search_mode, filter_mode, context, query, opt_filter)
                .await?;
            let had_results = !found.is_empty();
            results.append(&mut found);
            if had_results {
                break;
            }
            continue;
        };

        // With a limit, accumulate unique commands across modes, higher-priority first.
        // Page through this mode (advancing the offset) until we've filled the limit or
        // exhausted its results. Over-fetch by the number of already-seen commands - the
        // most this mode could collide with - so a single query usually suffices.
        let base_offset = opt_filter.offset.unwrap_or(0);
        let mut page_offset = 0;
        loop {
            let remaining = limit - results.len() as i64;
            if remaining <= 0 {
                break;
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
                        offset: Some(base_offset + page_offset),
                        ..opt_filter
                    },
                )
                .await?;

            let fetched = found.len() as i64;
            found.retain(|h| seen.insert(h.command.clone()));
            results.append(&mut found);
            // Over-fetching can push us past the limit; never return more than asked.
            results.truncate(limit as usize);

            // A short page means this mode has no more results to page through.
            if fetched < fetch {
                break;
            }
            page_offset += fetched;
        }

        if results.len() as i64 >= limit {
            break;
        }
    }

    Ok(results)
}

pub async fn prepare_index(settings: &Settings) -> Result<()> {
    use engines::AnySearchEngine;
    #[cfg(feature = "daemon")]
    if let AnySearchEngine::Daemon(mut search) = engines::engine(settings.search_mode(), settings) {
        search.prepare_index().await?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{AuthorPattern, Cmd};
    use clap::Parser;
    use rstest::rstest;

    #[rstest]
    // triple_dash: Issue #3028 - searching for `---` should not be treated as a CLI flag
    #[case::triple_dash(vec!["search", "---"], vec!["---"])]
    // double_dash_value: searching for strings starting with -- should also work
    #[case::double_dash_value(vec!["search", "--", "--foo"], vec!["--foo"])]
    fn parses_query_args(#[case] args: Vec<&str>, #[case] expected: Vec<&str>) {
        let cmd = Cmd::try_parse_from(args).expect("should parse as query");
        assert_eq!(cmd.query, expected);
    }

    #[test]
    fn search_filter_modes_cli_flag() {
        use atuin_client::settings::FilterMode;

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

    #[rstest]
    fn search_author_cli_flag() {
        let cmd =
            Cmd::try_parse_from(["search", "--author", "codex", "--author", "ellie"]).unwrap();
        assert_eq!(
            cmd.author,
            vec![
                AuthorPattern::Name("codex".to_owned()),
                AuthorPattern::Name("ellie".to_owned()),
            ],
        );
    }

    #[rstest]
    fn search_author_cli_flag_parses_the_special_values() {
        let cmd = Cmd::try_parse_from([
            "search",
            "--author",
            "$all-user",
            "--author",
            "$all-agent",
            "--author",
            "$all-users",
        ])
        .unwrap();
        assert_eq!(
            cmd.author,
            vec![
                AuthorPattern::AllUser,
                AuthorPattern::AllAgent,
                // Not a special value; a typo'd one is an author name, as it was before.
                AuthorPattern::Name("$all-users".to_owned()),
            ],
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn filter_modes_dedup_accumulation() {
        use atuin_client::database::{Context, Database, DbSearchMode, OptFilters, Sqlite};
        use atuin_client::history::History;
        use atuin_client::settings::FilterMode;
        use std::collections::HashSet;
        use time::OffsetDateTime;

        let db = Sqlite::new("sqlite::memory:", 1.0).await.unwrap();

        // (command, session, hostname, cwd) chosen so the modes below overlap: each
        // command should appear in the final result exactly once despite matching
        // several modes.
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

        let context = Context {
            session: "S".into(),
            hostname: "H".into(),
            cwd: "/dir".into(),
            host_id: "host".into(),
            git_root: None,
        };

        let modes = [
            FilterMode::Session,
            FilterMode::Directory,
            FilterMode::Host,
            FilterMode::Global,
        ];
        let opt_filter = OptFilters {
            limit: Some(10),
            ..Default::default()
        };

        let results =
            super::search_filter_modes(&db, DbSearchMode::Prefix, &context, &modes, "", opt_filter)
                .await
                .unwrap();

        let commands: Vec<&str> = results.iter().map(|h| h.command.as_str()).collect();
        let unique: HashSet<&str> = commands.iter().copied().collect();

        // No command is repeated across modes, and every unique command is collected.
        assert_eq!(
            commands.len(),
            unique.len(),
            "results contain duplicates: {commands:?}"
        );
        assert_eq!(unique, HashSet::from(["a", "b", "c", "d"]));
    }
}
