use std::{
    fmt::{self, Display},
    io::{self, IsTerminal, Write},
    path::PathBuf,
    time::Duration,
};

use atuin_common::utils::{self, Escapable as _};
use clap::Subcommand;
use eyre::{Context, Result};
use runtime_format::{FormatKey, FormatKeyError, ParseSegment, ParsedFmt};

use atuin_client::{
    database::{Database, Sqlite, current_context},
    encryption,
    history::{History, store::HistoryStore},
    record::sqlite_store::SqliteStore,
    settings::{
        FilterMode::{Directory, Global, Session},
        Settings, Timezone,
    },
};

#[cfg(feature = "sync")]
use atuin_client::{record, sync};

use log::{debug, warn};
use time::{OffsetDateTime, macros::format_description};

#[cfg(feature = "daemon")]
use super::daemon;
use super::search::format_duration_into;

#[derive(Subcommand, Debug)]
#[command(infer_subcommands = true)]
pub enum Cmd {
    /// Begins a new command in the history
    Start {
        /// Collects the command from the `ATUIN_COMMAND_LINE` environment variable,
        /// which does not need escaping and is more compatible between OS and shells
        #[arg(long = "command-from-env", hide = true)]
        cmd_env: bool,

        /// Author of this command, eg `ellie`, `claude`, or `copilot`
        #[arg(long)]
        author: Option<String>,

        /// Optional intent/rationale for running this command
        #[arg(long)]
        intent: Option<String>,

        command: Vec<String>,
    },

    /// Finishes a new command in the history (adds time, exit code)
    End {
        id: String,
        #[arg(long, short)]
        exit: i64,
        #[arg(long, short)]
        duration: Option<u64>,
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

        /// Terminate the output with a null, for better multiline support
        #[arg(long)]
        print0: bool,

        #[arg(long, short, default_value = "true")]
        // accept no value
        #[arg(num_args(0..=1), default_missing_value("true"))]
        // accept a value
        #[arg(action = clap::ArgAction::Set)]
        reverse: bool,

        /// Display the command time in another timezone other than the configured default.
        ///
        /// This option takes one of the following kinds of values:
        /// - the special value "local" (or "l") which refers to the system time zone
        /// - an offset from UTC (e.g. "+9", "-2:30")
        #[arg(long, visible_alias = "tz")]
        timezone: Option<Timezone>,

        /// Available variables: {command}, {directory}, {duration}, {user}, {host}, {author}, {intent}, {exit}, {time}, {session}, and {uuid}
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

        /// Display the command time in another timezone other than the configured default.
        ///
        /// This option takes one of the following kinds of values:
        /// - the special value "local" (or "l") which refers to the system time zone
        /// - an offset from UTC (e.g. "+9", "-2:30")
        #[arg(long, visible_alias = "tz")]
        timezone: Option<Timezone>,

        /// Available variables: {command}, {directory}, {duration}, {user}, {host}, {author}, {intent}, {time}, {session}, {uuid} and {relativetime}.
        /// Example: --format "{time} - [{duration}] - {directory}$\t{command}"
        #[arg(long, short)]
        format: Option<String>,
    },

    InitStore,

    /// Delete history entries matching the configured exclusion filters
    Prune {
        /// List matching history lines without performing the actual deletion.
        #[arg(short = 'n', long)]
        dry_run: bool,
    },

    /// Delete duplicate history entries (that have the same command, cwd and hostname)
    Dedup {
        /// List matching history lines without performing the actual deletion.
        #[arg(short = 'n', long)]
        dry_run: bool,

        /// Only delete results added before this date
        #[arg(long, short)]
        before: String,

        /// How many recent duplicates to keep
        #[arg(long)]
        dupkeep: u32,
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
pub fn print_list(
    h: &[History],
    list_mode: ListMode,
    format: Option<&str>,
    print0: bool,
    reverse: bool,
    tz: Timezone,
) {
    let w = std::io::stdout();
    let mut w = w.lock();

    let fmt_str = match list_mode {
        ListMode::Human => format
            .unwrap_or("{time} · {duration}\t{command}")
            .replace("\\t", "\t"),
        ListMode::Regular => format
            .unwrap_or("{time}\t{command}\t{duration}")
            .replace("\\t", "\t"),
        // not used
        ListMode::CmdOnly => String::new(),
    };

    let parsed_fmt = match list_mode {
        ListMode::Human | ListMode::Regular => parse_fmt(&fmt_str),
        ListMode::CmdOnly => std::iter::once(ParseSegment::Key("command")).collect(),
    };

    let iterator = if reverse {
        Box::new(h.iter().rev()) as Box<dyn Iterator<Item = &History>>
    } else {
        Box::new(h.iter()) as Box<dyn Iterator<Item = &History>>
    };

    let entry_terminator = if print0 { "\0" } else { "\n" };
    let flush_each_line = print0;

    for history in iterator {
        let fh = FmtHistory {
            history,
            cmd_format: CmdFormat::for_output(&w),
            tz: &tz,
        };
        let args = parsed_fmt.with_args(&fh);

        // Check for formatting errors before attempting to write
        if let Err(err) = args.status() {
            eprintln!("ERROR: history output failed with: {err}");
            std::process::exit(1);
        }

        let write_result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            write!(w, "{args}{entry_terminator}")
        }));

        match write_result {
            Ok(Ok(())) => {
                // Write succeeded
            }
            Ok(Err(err)) => {
                if err.kind() != io::ErrorKind::BrokenPipe {
                    eprintln!("ERROR: Failed to write history output: {err}");
                    std::process::exit(1);
                }
            }
            Err(_) => {
                eprintln!("ERROR: Format string caused a formatting error.");
                eprintln!(
                    "This may be due to an unsupported format string containing special characters."
                );
                eprintln!(
                    "Please check your format string syntax and ensure literal braces are properly escaped."
                );
                std::process::exit(1);
            }
        }
        if flush_each_line {
            check_for_write_errors(w.flush());
        }
    }

    if !flush_each_line {
        check_for_write_errors(w.flush());
    }
}

fn check_for_write_errors(write: Result<(), io::Error>) {
    if let Err(err) = write {
        // Ignore broken pipe (issue #626)
        if err.kind() != io::ErrorKind::BrokenPipe {
            eprintln!("ERROR: History output failed with the following error: {err}");
            std::process::exit(1);
        }
    }
}

/// Type wrapper around `History` with formatting settings.
#[derive(Clone, Copy, Debug)]
struct FmtHistory<'a> {
    history: &'a History,
    cmd_format: CmdFormat,
    tz: &'a Timezone,
}

#[derive(Clone, Copy, Debug)]
enum CmdFormat {
    Literal,
    Escaped,
}
impl CmdFormat {
    fn for_output<O: IsTerminal>(out: &O) -> Self {
        if out.is_terminal() {
            Self::Escaped
        } else {
            Self::Literal
        }
    }
}

static TIME_FMT: &[time::format_description::FormatItem<'static>] =
    format_description!("[year]-[month]-[day] [hour repr:24]:[minute]:[second]");

/// defines how to format the history
impl FormatKey for FmtHistory<'_> {
    #[allow(clippy::cast_sign_loss)]
    fn fmt(&self, key: &str, f: &mut fmt::Formatter<'_>) -> Result<(), FormatKeyError> {
        match key {
            "command" => match self.cmd_format {
                CmdFormat::Literal => f.write_str(self.history.command.trim()),
                CmdFormat::Escaped => f.write_str(&self.history.command.trim().escape_control()),
            }?,
            "directory" => f.write_str(self.history.cwd.trim())?,
            "exit" => f.write_str(&self.history.exit.to_string())?,
            "duration" => {
                let dur = Duration::from_nanos(std::cmp::max(self.history.duration, 0) as u64);
                format_duration_into(dur, f)?;
            }
            "time" => {
                self.history
                    .timestamp
                    .to_offset(self.tz.0)
                    .format(TIME_FMT)
                    .map_err(|_| fmt::Error)?
                    .fmt(f)?;
            }
            "relativetime" => {
                let since = OffsetDateTime::now_utc() - self.history.timestamp;
                let d = Duration::try_from(since).unwrap_or_default();
                format_duration_into(d, f)?;
            }
            "host" => f.write_str(
                self.history
                    .hostname
                    .split_once(':')
                    .map_or(&self.history.hostname, |(host, _)| host),
            )?,
            "author" => f.write_str(&self.history.author)?,
            "intent" => f.write_str(self.history.intent.as_deref().unwrap_or_default())?,
            "user" => f.write_str(
                self.history
                    .hostname
                    .split_once(':')
                    .map_or("", |(_, user)| user),
            )?,
            "session" => f.write_str(&self.history.session)?,
            "uuid" => f.write_str(&self.history.id.0)?,
            _ => return Err(FormatKeyError::UnknownKey),
        }
        Ok(())
    }
}

fn parse_fmt(format: &str) -> ParsedFmt<'_> {
    match ParsedFmt::new(format) {
        Ok(fmt) => fmt,
        Err(err) => {
            eprintln!("ERROR: History formatting failed with the following error: {err}");

            if format.contains('"') && (format.contains(":{") || format.contains(",{")) {
                eprintln!("It looks like you're trying to create JSON output.");
                eprintln!("For JSON, you need to escape literal braces by doubling them:");
                eprintln!("Example: '{{\"command\":\"{{command}}\",\"time\":\"{{time}}\"}}'");
            } else {
                eprintln!(
                    "If your formatting string contains literal curly braces, you need to escape them by doubling:"
                );
                eprintln!("Use {{{{ for literal {{ and }}}} for literal }}");
            }
            std::process::exit(1)
        }
    }
}

impl Cmd {
    fn apply_start_metadata(history: &mut History, author: Option<&str>, intent: Option<&str>) {
        if let Some(author) = author.map(str::trim).filter(|author| !author.is_empty()) {
            author.clone_into(&mut history.author);
        }

        if let Some(intent) = intent.map(str::trim).filter(|intent| !intent.is_empty()) {
            history.intent = Some(intent.to_owned());
        } else if intent.is_some() {
            history.intent = None;
        }
    }

    #[allow(clippy::too_many_lines, clippy::cast_possible_truncation)]
    async fn handle_start(
        db: &impl Database,
        settings: &Settings,
        command: &str,
        author: Option<&str>,
        intent: Option<&str>,
    ) -> Result<()> {
        // It's better for atuin to silently fail here and attempt to
        // store whatever is ran, than to throw an error to the terminal
        let cwd = utils::get_current_dir();

        let mut h: History = History::capture()
            .timestamp(OffsetDateTime::now_utc())
            .command(command)
            .cwd(cwd)
            .build()
            .into();
        Self::apply_start_metadata(&mut h, author, intent);

        if !h.should_save(settings) {
            return Ok(());
        }

        // print the ID
        // we use this as the key for calling end
        println!("{}", h.id);

        // Silently ignore database errors to avoid breaking the shell
        // This is important when disk is full or database is locked
        if let Err(e) = db.save(&h).await {
            debug!("failed to save history: {e}");
        }

        Ok(())
    }

    #[cfg(feature = "daemon")]
    async fn handle_daemon_start(
        settings: &Settings,
        command: &str,
        author: Option<&str>,
        intent: Option<&str>,
    ) -> Result<()> {
        // It's better for atuin to silently fail here and attempt to
        // store whatever is ran, than to throw an error to the terminal
        let cwd = utils::get_current_dir();

        let mut h: History = History::capture()
            .timestamp(OffsetDateTime::now_utc())
            .command(command)
            .cwd(cwd)
            .build()
            .into();
        Self::apply_start_metadata(&mut h, author, intent);

        if !h.should_save(settings) {
            return Ok(());
        }

        // Attempt to start history via daemon, but silently ignore errors
        // to avoid breaking the shell when the daemon is unavailable or disk is full
        let resp = match daemon::start_history(settings, h.clone()).await {
            Ok(id) => id,
            Err(e) => {
                debug!("failed to start history via daemon: {e}");
                h.id.0.clone()
            }
        };

        // print the ID
        // we use this as the key for calling end
        println!("{resp}");

        Ok(())
    }

    #[allow(unused_variables)]
    async fn handle_end(
        db: &impl Database,
        store: SqliteStore,
        history_store: HistoryStore,
        settings: &Settings,
        id: &str,
        exit: i64,
        duration: Option<u64>,
    ) -> Result<()> {
        if id.trim() == "" {
            return Ok(());
        }

        let Some(mut h) = db.load(id).await? else {
            warn!("history entry is missing");
            return Ok(());
        };

        if h.duration > 0 {
            debug!("cannot end history - already has duration");

            // returning OK as this can occur if someone Ctrl-c a prompt
            return Ok(());
        }

        if !settings.store_failed && exit > 0 {
            debug!("history has non-zero exit code, and store_failed is false");

            // the history has already been inserted half complete. remove it
            db.delete(h).await?;

            return Ok(());
        }

        h.exit = exit;
        h.duration = match duration {
            Some(value) => i64::try_from(value).context("command took over 292 years")?,
            None => i64::try_from((OffsetDateTime::now_utc() - h.timestamp).whole_nanoseconds())
                .context("command took over 292 years")?,
        };

        db.update(&h).await?;
        history_store.push(h).await?;

        if settings.should_sync().await? {
            #[cfg(feature = "sync")]
            {
                if settings.sync.records {
                    let (_, downloaded) = record::sync::sync(settings, &store).await?;
                    Settings::save_sync_time().await?;

                    crate::sync::build(settings, &store, db, Some(&downloaded)).await?;
                } else {
                    debug!("running periodic background sync");
                    sync::sync(settings, false, db).await?;
                }
            }
            #[cfg(not(feature = "sync"))]
            debug!("not compiled with sync support");
        } else {
            debug!("sync disabled! not syncing");
        }

        Ok(())
    }

    #[cfg(feature = "daemon")]
    async fn handle_daemon_end(
        settings: &Settings,
        id: &str,
        exit: i64,
        duration: Option<u64>,
    ) -> Result<()> {
        daemon::end_history(settings, id.to_string(), duration.unwrap_or(0), exit).await?;

        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    #[allow(clippy::fn_params_excessive_bools)]
    async fn handle_list(
        db: &impl Database,
        settings: &Settings,
        context: atuin_client::database::Context,
        session: bool,
        cwd: bool,
        mode: ListMode,
        format: Option<String>,
        include_deleted: bool,
        print0: bool,
        reverse: bool,
        tz: Timezone,
    ) -> Result<()> {
        let filters = match (session, cwd) {
            (true, true) => [Session, Directory],
            (true, false) => [Session, Global],
            (false, true) => [Global, Directory],
            (false, false) => [
                settings.default_filter_mode(context.git_root.is_some()),
                Global,
            ],
        };

        let history = db
            .list(&filters, &context, None, false, include_deleted)
            .await?;

        print_list(
            &history,
            mode,
            match format {
                None => Some(settings.history_format.as_str()),
                _ => format.as_deref(),
            },
            print0,
            reverse,
            tz,
        );

        Ok(())
    }

    async fn handle_prune(
        db: &impl Database,
        settings: &Settings,
        store: SqliteStore,
        context: atuin_client::database::Context,
        dry_run: bool,
    ) -> Result<()> {
        // Grab all executed commands and filter them using History::should_save.
        // We could iterate or paginate here if memory usage becomes an issue.
        let matches: Vec<History> = db
            .list(&[Global], &context, None, false, false)
            .await?
            .into_iter()
            .filter(|h| !h.should_save(settings))
            .collect();

        match matches.len() {
            0 => {
                println!("No entries to prune.");
                return Ok(());
            }
            1 => println!("Found 1 entry to prune."),
            n => println!("Found {n} entries to prune."),
        }

        if dry_run {
            print_list(
                &matches,
                ListMode::Human,
                Some(settings.history_format.as_str()),
                false,
                false,
                settings.timezone,
            );
        } else {
            let encryption_key: [u8; 32] = encryption::load_key(settings)
                .context("could not load encryption key")?
                .into();
            let host_id = Settings::host_id().await?;
            let history_store = HistoryStore::new(store.clone(), host_id, encryption_key);

            for entry in matches {
                eprintln!("deleting {}", entry.id);
                if settings.sync.records {
                    let (id, _) = history_store.delete(entry.id.clone()).await?;
                    history_store.incremental_build(db, &[id]).await?;
                } else {
                    db.delete(entry.clone()).await?;
                }
            }
        }
        Ok(())
    }

    async fn handle_dedup(
        db: &impl Database,
        settings: &Settings,
        store: SqliteStore,
        before: i64,
        dupkeep: u32,
        dry_run: bool,
    ) -> Result<()> {
        if dupkeep == 0 {
            eprintln!(
                "\"--dupkeep 0\" would keep 0 copies of duplicate commands and thus delete all of them! Use \"atuin search --delete ...\" if you really want that."
            );
            std::process::exit(1);
        }

        let matches: Vec<History> = db.get_dups(before, dupkeep).await?;

        match matches.len() {
            0 => {
                println!("No duplicates to delete.");
                return Ok(());
            }
            1 => println!("Found 1 duplicate to delete."),
            n => println!("Found {n} duplicates to delete."),
        }

        if dry_run {
            print_list(
                &matches,
                ListMode::Human,
                Some(settings.history_format.as_str()),
                false,
                false,
                settings.timezone,
            );
        } else {
            let encryption_key: [u8; 32] = encryption::load_key(settings)
                .context("could not load encryption key")?
                .into();
            let host_id = Settings::host_id().await?;
            let history_store = HistoryStore::new(store.clone(), host_id, encryption_key);

            for entry in matches {
                eprintln!("deleting {}", entry.id);
                if settings.sync.records {
                    let (id, _) = history_store.delete(entry.id).await?;
                    history_store.incremental_build(db, &[id]).await?;
                } else {
                    db.delete(entry).await?;
                }
            }
        }
        Ok(())
    }

    pub async fn run(self, settings: &Settings) -> Result<()> {
        let context = current_context().await?;

        #[cfg(feature = "daemon")]
        // Skip initializing any databases for start/end, if the daemon is enabled
        if settings.daemon.enabled {
            match self {
                Self::Start { .. } => {
                    let command = self.get_start_command().unwrap_or_default();
                    let (author, intent) = self.get_start_metadata().unwrap_or_default();
                    return Self::handle_daemon_start(settings, &command, author, intent).await;
                }

                Self::End { id, exit, duration } => {
                    return Self::handle_daemon_end(settings, &id, exit, duration).await;
                }

                _ => {}
            }
        }

        let db_path = PathBuf::from(settings.db_path.as_str());
        let record_store_path = PathBuf::from(settings.record_store_path.as_str());

        let db = Sqlite::new(db_path, settings.local_timeout).await?;
        let store = SqliteStore::new(record_store_path, settings.local_timeout).await?;

        let encryption_key: [u8; 32] = encryption::load_key(settings)
            .context("could not load encryption key")?
            .into();

        let host_id = Settings::host_id().await?;
        let history_store = HistoryStore::new(store.clone(), host_id, encryption_key);

        match self {
            Self::Start { .. } => {
                let command = self.get_start_command().unwrap_or_default();
                let (author, intent) = self.get_start_metadata().unwrap_or_default();
                Self::handle_start(&db, settings, &command, author, intent).await
            }
            Self::End { id, exit, duration } => {
                Self::handle_end(&db, store, history_store, settings, &id, exit, duration).await
            }
            Self::List {
                session,
                cwd,
                human,
                cmd_only,
                print0,
                reverse,
                timezone,
                format,
            } => {
                let mode = ListMode::from_flags(human, cmd_only);
                let tz = timezone.unwrap_or(settings.timezone);
                Self::handle_list(
                    &db, settings, context, session, cwd, mode, format, false, print0, reverse, tz,
                )
                .await
            }

            Self::Last {
                human,
                cmd_only,
                timezone,
                format,
            } => {
                let last = db.last().await?;
                let last = last.as_slice();
                let tz = timezone.unwrap_or(settings.timezone);
                print_list(
                    last,
                    ListMode::from_flags(human, cmd_only),
                    match format {
                        None => Some(settings.history_format.as_str()),
                        _ => format.as_deref(),
                    },
                    false,
                    true,
                    tz,
                );

                Ok(())
            }

            Self::InitStore => history_store.init_store(&db).await,

            Self::Prune { dry_run } => {
                Self::handle_prune(&db, settings, store, context, dry_run).await
            }

            Self::Dedup {
                dry_run,
                before,
                dupkeep,
            } => {
                let before = i64::try_from(
                    interim::parse_date_string(
                        before.as_str(),
                        OffsetDateTime::now_utc(),
                        interim::Dialect::Uk,
                    )?
                    .unix_timestamp_nanos(),
                )?;
                Self::handle_dedup(&db, settings, store, before, dupkeep, dry_run).await
            }
        }
    }

    /// Returns the command line to use for the `Start` variant.
    /// Returns `None` for any other variant.
    fn get_start_command(&self) -> Option<String> {
        match self {
            Self::Start { cmd_env: true, .. } => {
                Some(std::env::var("ATUIN_COMMAND_LINE").unwrap_or_default())
            }
            Self::Start { command, .. } => Some(command.join(" ")),
            _ => None,
        }
    }

    /// Returns `(author, intent)` for the `Start` variant.
    /// Returns `None` for any other variant.
    fn get_start_metadata(&self) -> Option<(Option<&str>, Option<&str>)> {
        match self {
            Self::Start { author, intent, .. } => Some((author.as_deref(), intent.as_deref())),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_format_string_no_panic() {
        // Don't panic but provide helpful output (issue #2776)
        let malformed_json = r#"{"command":"{command}","key":"value"}"#;

        let result = std::panic::catch_unwind(|| parse_fmt(malformed_json));

        assert!(result.is_ok());
    }

    #[test]
    fn test_valid_formats_still_work() {
        assert!(std::panic::catch_unwind(|| parse_fmt("{command}")).is_ok());
        assert!(std::panic::catch_unwind(|| parse_fmt("{time} - {command}")).is_ok());
    }
}
