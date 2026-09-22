use std::fs::File;
use std::io::{IsTerminal as _, Write, stderr, stdout};

use atuin_client::database::{OptFilters, Sqlite, current_context};
use atuin_client::history::store::HistoryStore;
use atuin_client::history::{AuthorPattern, History};
use atuin_client::record::sqlite_store::SqliteStore;
use atuin_client::settings::{FilterMode, KeymapMode, RequestedSearchMode, Settings};
use atuin_client::theme::Theme;
use atuin_common::encryption::paseto_v4;
use atuin_common::filter::OrFilter;
use atuin_common::string::EscapeNonPrintablePosixExt as _;
use atuin_common::utils;
use clap::Parser;
use eyre::{Context as _, Result};
use tracing::instrument;

use super::history::ListMode;

mod cursor;
mod engines;
mod history_list;
mod inspector;
mod interactive;
pub mod keybindings;
mod syntax;

use atuin_common::time::UtcOffsetSpec;

use crate::i18n::fl;

#[allow(clippy::struct_excessive_bools, clippy::struct_field_names)]
#[derive(Parser, Debug)]
pub struct Cmd {
    #[arg(long, short, help = fl!("arg-search-cwd"))]
    cwd: Option<String>,

    #[arg(long, help = fl!("arg-search-exclude-cwd"))]
    exclude_cwd: Option<String>,

    #[arg(long, short, help = fl!("arg-search-exit"))]
    exit: Vec<i64>,

    #[arg(long, help = fl!("arg-search-exclude-exit"))]
    exclude_exit: Vec<i64>,

    #[arg(
        long,
        short,
        help = fl!("arg-search-before"),
        long_help = fl!("arg-search-before", "long")
    )]
    before: Option<String>,

    #[arg(long, help = fl!("arg-search-after"))]
    after: Option<String>,

    #[arg(long, help = fl!("arg-search-limit"))]
    limit: Option<i64>,

    #[arg(long, help = fl!("arg-search-offset"))]
    offset: Option<i64>,

    #[arg(long, short, help = fl!("arg-search-interactive"))]
    interactive: bool,

    #[arg(long, help = fl!("arg-search-filter-mode"))]
    filter_mode: Option<FilterMode>,

    #[arg(
        long,
        help = fl!("arg-search-search-mode"),
        long_help = fl!("arg-search-search-mode", "long")
    )]
    search_mode: Option<RequestedSearchMode>,

    #[arg(long, hide = true, help = fl!("arg-search-shell-up-key-binding"))]
    shell_up_key_binding: bool,

    #[arg(long, default_value = "auto", help = fl!("arg-search-keymap-mode"))]
    keymap_mode: KeymapMode,

    #[arg(long, help = fl!("arg-search-human"))]
    human: bool,

    #[arg(allow_hyphen_values = true)]
    query: Vec<String>,

    #[arg(long, help = fl!("arg-search-cmd-only"))]
    cmd_only: bool,

    #[arg(long, help = fl!("arg-search-print0"))]
    print0: bool,

    #[arg(long, help = fl!("arg-search-delete"))]
    delete: bool,

    #[arg(long, help = fl!("arg-search-delete-it-all"))]
    delete_it_all: bool,

    #[arg(long, short, help = fl!("arg-search-reverse"))]
    reverse: bool,

    #[arg(
        long,
        visible_alias = "tz",
        help = fl!("arg-search-timezone"),
        long_help = fl!("arg-search-timezone", "long")
    )]
    // `num_args = 0..=1` allows a user to run `atuin search --tz` with no argument to `--tz`. This
    // does the same thing as not providing the flag, but we previously allowed it (via an
    // `Option<Option<T>>` field type), so let's keep supporting it to avoid breaking existing
    // scripts.
    #[arg(allow_hyphen_values = true, num_args = 0..=1)]
    timezone: Option<UtcOffsetSpec>,

    #[arg(
        long,
        short,
        help = fl!("arg-search-format"),
        long_help = fl!("arg-search-format", "long")
    )]
    format: Option<String>,

    #[arg(long, help = fl!("arg-search-inline-height"))]
    inline_height: Option<u16>,

    #[arg(long, help = fl!("arg-search-author"), long_help = fl!("arg-search-author", "long"))]
    author: Vec<AuthorPattern>,

    #[arg(long, help = fl!("arg-search-include-duplicates"))]
    include_duplicates: bool,

    #[arg(long, hide = true, help = fl!("arg-search-result-file"))]
    result_file: Option<String>,

    #[arg(long, help = fl!("arg-search-shell"), long_help = fl!("arg-search-shell", "long"))]
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
    #[instrument(level = "trace", skip_all, err)]
    pub async fn run(
        self,
        db: Sqlite,
        settings: &mut Settings,
        store: SqliteStore,
        theme: &Theme,
    ) -> Result<()> {
        let query = if self.query.is_empty() {
            std::env::var("ATUIN_QUERY").map_or_else(
                |_| vec![],
                |query| query.split(' ').map(std::string::ToString::to_string).collect(),
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
                "Please specify a query to match the items you wish to delete. If you wish to \
                 delete all history, pass --delete-it-all"
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

        let encryption_key = paseto_v4::Key::try_load_or_generate(&settings.key_path)
            .context("could not load or generate encryption key")?;

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
            let tz = self.timezone.unwrap_or(settings.timezone);

            let opt_filter = OptFilters {
                exit: &self.exit,
                exclude_exit: &self.exclude_exit,
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
                timezone: tz,
                dialect: settings.dialect,
            };

            let mut entries = run_non_interactive(settings, opt_filter, &query, &db).await?;

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

                    super::history::delete_history_entries(settings, &history_store, &db, entries)
                        .await?;

                    entries = run_non_interactive(settings, opt_filter, &query, &db).await?;
                }
            } else {
                let format = self.format.as_deref().unwrap_or(settings.history_format.as_str());

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
    query: &[String],
    db: &Sqlite,
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

    let filter_mode = settings.default_filter_mode(context.git_root.is_some());

    let results = db
        .search(
            settings.search_mode().closest_db_mode(),
            filter_mode,
            &context,
            query.join(" ").as_str(),
            opt_filter,
        )
        .await?;

    Ok(results)
}

#[instrument(level = "trace", skip_all, err)]
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
    use clap::Parser;
    use rstest::rstest;

    use super::{AuthorPattern, Cmd};

    #[rstest]
    #[case::default(vec![], vec![], vec![])]
    #[case::single(vec!["--exit", "0"], vec![0], vec![])]
    #[case::single_exclusion(vec!["--exclude-exit", "0"], vec![], vec![0])]
    #[case::repeated(vec!["--exit", "1", "--exit", "2"], vec![1, 2], vec![])]
    #[case::short(vec!["-e", "1", "-e", "2"], vec![1, 2], vec![])]
    #[case::excluded(vec!["--exclude-exit", "0", "--exclude-exit", "130"], vec![], vec![0, 130])]
    #[case::combined(vec!["--exit", "1", "--exit", "2", "--exclude-exit", "2"], vec![1, 2], vec![2])]
    #[case::duplicates(vec!["--exit", "1", "--exit", "1"], vec![1, 1], vec![])]
    #[case::signed(vec!["--exit=-1", "--exclude-exit=-2"], vec![-1], vec![-2])]
    fn parses_exit_filters(
        #[case] args: Vec<&str>,
        #[case] exit: Vec<i64>,
        #[case] exclude_exit: Vec<i64>,
        #[values(None, Some("--delete"), Some("--delete-it-all"))] delete: Option<&str>,
    ) {
        let cmd = Cmd::try_parse_from(
            std::iter::once("search").chain(args).chain(delete).chain(["cargo"]),
        )
        .unwrap();
        assert_eq!(cmd.exit, exit);
        assert_eq!(cmd.exclude_exit, exclude_exit);
        assert_eq!(cmd.query, ["cargo"]);
        assert_eq!(cmd.delete, delete == Some("--delete"));
        assert_eq!(cmd.delete_it_all, delete == Some("--delete-it-all"));
    }

    #[rstest]
    fn rejects_invalid_exit_filters(
        #[values("--exit", "--exclude-exit")] flag: &str,
        #[values("invalid", "9223372036854775808")] value: &str,
    ) {
        assert!(Cmd::try_parse_from(["search", flag, value]).is_err());
    }

    #[rstest]
    // triple_dash: Issue #3028 - searching for `---` should not be treated as a CLI flag
    #[case::triple_dash(vec!["search", "---"], vec!["---"])]
    // double_dash_value: searching for strings starting with -- should also work
    #[case::double_dash_value(vec!["search", "--", "--foo"], vec!["--foo"])]
    fn parses_query_args(#[case] args: Vec<&str>, #[case] expected: Vec<&str>) {
        let cmd = Cmd::try_parse_from(args).expect("should parse as query");
        assert_eq!(cmd.query, expected);
    }

    #[rstest]
    fn search_author_cli_flag() {
        let cmd =
            Cmd::try_parse_from(["search", "--author", "codex", "--author", "ellie"]).unwrap();
        assert_eq!(cmd.author, vec![
            AuthorPattern::Name("codex".to_owned()),
            AuthorPattern::Name("ellie".to_owned()),
        ],);
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
        assert_eq!(cmd.author, vec![
            AuthorPattern::AllUser,
            AuthorPattern::AllAgent,
            // Not a special value; a typo'd one is an author name, as it was before.
            AuthorPattern::Name("$all-users".to_owned()),
        ],);
    }
}
