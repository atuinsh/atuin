use std::{
    fmt::{self, Display},
    io::{self, IsTerminal, Write},
    path::PathBuf,
    time::Duration,
};

use atuin_common::utils::{self, Escapable as _};
use clap::Subcommand;
use eyre::{Context, Result, bail};
use runtime_format::{FormatKey, FormatKeyError, ParseSegment, ParsedFmt};

#[cfg(feature = "daemon")]
use super::daemon as daemon_cmd;
#[cfg(feature = "daemon")]
use colored::Colorize;
#[cfg(feature = "daemon")]
use serde::Serialize;

#[cfg(feature = "daemon")]
use atuin_daemon::history::{HistoryEventKind, TailHistoryReply};

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

    /// Stream history events from the daemon as they are received
    Tail,

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

fn normalize_command_for_storage<'a>(command: &'a str, settings: &Settings) -> &'a str {
    if !settings.strip_trailing_whitespace {
        return command;
    }

    let trimmed = command.trim_end_matches([' ', '\t']);
    if trimmed.len() == command.len() {
        return command;
    }

    let trailing_backslashes = trimmed
        .as_bytes()
        .iter()
        .rev()
        .take_while(|&&byte| byte == b'\\')
        .count();

    if trailing_backslashes % 2 == 1 {
        command
    } else {
        trimmed
    }
}

async fn handle_start(
    db: &impl Database,
    settings: &Settings,
    command: &str,
    author: Option<&str>,
    intent: Option<&str>,
) -> Result<Option<String>> {
    // It's better for atuin to silently fail here and attempt to
    // store whatever is ran, than to throw an error to the terminal
    let cwd = utils::get_current_dir();
    let command = normalize_command_for_storage(command, settings);

    let mut h: History = History::capture()
        .timestamp(OffsetDateTime::now_utc())
        .command(command)
        .cwd(cwd)
        .build()
        .into();
    apply_start_metadata(&mut h, author, intent);

    if !h.should_save(settings) {
        return Ok(None);
    }

    let id = h.id.0.clone();

    // Silently ignore database errors to avoid breaking the shell
    // This is important when disk is full or database is locked
    if let Err(e) = db.save(&h).await {
        debug!("failed to save history: {e}");
    }

    Ok(Some(id))
}

#[cfg(feature = "daemon")]
async fn handle_daemon_start(
    settings: &Settings,
    command: &str,
    author: Option<&str>,
    intent: Option<&str>,
) -> Result<Option<String>> {
    // It's better for atuin to silently fail here and attempt to
    // store whatever is ran, than to throw an error to the terminal
    let cwd = utils::get_current_dir();
    let command = normalize_command_for_storage(command, settings);

    let mut h: History = History::capture()
        .timestamp(OffsetDateTime::now_utc())
        .command(command)
        .cwd(cwd)
        .build()
        .into();
    apply_start_metadata(&mut h, author, intent);

    if !h.should_save(settings) {
        return Ok(None);
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

    Ok(Some(resp))
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
                let (_, downloaded) =
                    record::sync::sync(settings, &store, &history_store.encryption_key).await?;
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

pub(super) async fn start_history_entry(
    settings: &Settings,
    command: &str,
    author: Option<&str>,
    intent: Option<&str>,
) -> Result<Option<String>> {
    #[cfg(feature = "daemon")]
    if settings.daemon.enabled {
        return handle_daemon_start(settings, command, author, intent).await;
    }

    let db_path = PathBuf::from(settings.db_path.as_str());
    let db = Sqlite::new(db_path, settings.local_timeout).await?;
    handle_start(&db, settings, command, author, intent).await
}

pub(super) async fn end_history_entry(
    settings: &Settings,
    id: &str,
    exit: i64,
    duration: Option<u64>,
) -> Result<()> {
    #[cfg(feature = "daemon")]
    if settings.daemon.enabled {
        return handle_daemon_end(settings, id, exit, duration).await;
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

    handle_end(&db, store, history_store, settings, id, exit, duration).await
}

#[cfg(feature = "daemon")]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum TailKind {
    Started,
    Ended,
}

#[cfg(feature = "daemon")]
#[derive(Clone, Debug, Eq, PartialEq)]
struct TailEvent {
    kind: TailKind,
    history: History,
}

#[cfg(feature = "daemon")]
#[derive(Serialize)]
struct TailJsonEvent<'a> {
    event: &'static str,
    history: TailJsonHistory<'a>,
}

#[cfg(feature = "daemon")]
#[derive(Serialize)]
struct TailJsonHistory<'a> {
    id: &'a str,
    timestamp: String,
    timestamp_unix_ns: u64,
    command: &'a str,
    cwd: &'a str,
    session: &'a str,
    hostname: &'a str,
    host: &'a str,
    user: &'a str,
    author: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    intent: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    exit: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    duration_ns: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    duration: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    success: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    finished_at: Option<String>,
}

#[cfg(feature = "daemon")]
impl TailEvent {
    fn from_proto(reply: TailHistoryReply) -> Result<Self> {
        let history = reply
            .history
            .ok_or_else(|| eyre::eyre!("daemon sent a history tail event without history"))?;
        let timestamp = OffsetDateTime::from_unix_timestamp_nanos(i128::from(history.timestamp))
            .context("invalid daemon history timestamp")?;
        let kind = match HistoryEventKind::try_from(reply.kind)
            .unwrap_or(HistoryEventKind::Unspecified)
        {
            HistoryEventKind::Started => TailKind::Started,
            HistoryEventKind::Ended => TailKind::Ended,
            HistoryEventKind::Unspecified => bail!("daemon sent an unspecified history tail event"),
        };

        Ok(Self {
            kind,
            history: History {
                id: history.id.into(),
                timestamp,
                duration: history.duration,
                exit: history.exit,
                command: history.command,
                cwd: history.cwd,
                session: history.session,
                hostname: history.hostname,
                author: history.author,
                intent: normalize_optional_field(&history.intent),
                deleted_at: None,
            },
        })
    }

    fn render(&self, tty: bool, tz: Timezone) -> Result<String> {
        if tty {
            Ok(self.render_pretty(tz))
        } else {
            let mut json = self.render_json(tz)?;
            json.push('\n');
            Ok(json)
        }
    }

    fn render_json(&self, tz: Timezone) -> Result<String> {
        let payload = TailJsonEvent {
            event: self.kind.as_str(),
            history: TailJsonHistory {
                id: &self.history.id.0,
                timestamp: format_history_time(self.history.timestamp, tz)?,
                timestamp_unix_ns: u64::try_from(self.history.timestamp.unix_timestamp_nanos())
                    .context("history timestamp predates unix epoch")?,
                command: &self.history.command,
                cwd: &self.history.cwd,
                session: &self.history.session,
                hostname: &self.history.hostname,
                host: self.host(),
                user: self.user(),
                author: &self.history.author,
                intent: self.history.intent.as_deref(),
                exit: self.exit_value(),
                duration_ns: self.duration_value(),
                duration: self.duration_value().map(format_duration_ns),
                success: self.success_value(),
                finished_at: self
                    .finished_at()
                    .map(|time| format_history_time(time, tz))
                    .transpose()?,
            },
        };

        Ok(serde_json::to_string(&payload)?)
    }

    fn render_pretty(&self, tz: Timezone) -> String {
        let mut out = String::new();
        let border = match self.kind {
            TailKind::Started => "-".repeat(72).bright_blue().to_string(),
            TailKind::Ended if self.history.exit == 0 => "-".repeat(72).bright_green().to_string(),
            TailKind::Ended => "-".repeat(72).bright_red().to_string(),
        };

        out.push_str(&border);
        out.push('\n');

        let command = self.history.command.trim();
        let escaped_command = command.escape_control();
        let mut command_lines = escaped_command.lines();
        let header = format!(
            "{} {}",
            self.kind.badge(self.history.exit),
            command_lines.next().unwrap_or_default().bold()
        );
        out.push_str(&header);
        out.push('\n');

        for line in command_lines {
            out.push_str("  ");
            out.push_str(line);
            out.push('\n');
        }

        push_pretty_field(
            &mut out,
            "start",
            &format_history_time(self.history.timestamp, tz)
                .unwrap_or_else(|_| "invalid".to_owned()),
        );
        push_pretty_field(&mut out, "history", &self.history.id.0);
        push_pretty_field(&mut out, "session", &self.history.session);
        push_pretty_field(&mut out, "exit", &self.exit_display());
        push_pretty_field(&mut out, "duration", &self.duration_display());

        out.push('\n');

        push_pretty_field(&mut out, "cwd", &self.history.cwd);
        push_pretty_field(&mut out, "hostname", &self.history.hostname);
        push_pretty_field(&mut out, "host", self.host());
        push_pretty_field(&mut out, "user", self.user());
        push_pretty_field(&mut out, "author", &self.history.author);

        if let Some(intent) = self.history.intent.as_deref() {
            push_pretty_field(&mut out, "intent", intent);
        }

        if let Some(finished) = self.finished_at() {
            let finished =
                format_history_time(finished, tz).unwrap_or_else(|_| "invalid".to_owned());
            push_pretty_field(&mut out, "finished", &finished);
        }

        out.push_str(&border);
        out.push_str("\n\n");
        out
    }

    fn host(&self) -> &str {
        self.history
            .hostname
            .split_once(':')
            .map_or(self.history.hostname.as_str(), |(host, _)| host)
    }

    fn user(&self) -> &str {
        self.history
            .hostname
            .split_once(':')
            .map_or("", |(_, user)| user)
    }

    fn exit_value(&self) -> Option<i64> {
        matches!(self.kind, TailKind::Ended).then_some(self.history.exit)
    }

    fn duration_value(&self) -> Option<i64> {
        matches!(self.kind, TailKind::Ended).then_some(self.history.duration)
    }

    fn success_value(&self) -> Option<bool> {
        matches!(self.kind, TailKind::Ended).then_some(self.history.exit == 0)
    }

    fn finished_at(&self) -> Option<OffsetDateTime> {
        self.duration_value()
            .filter(|duration| *duration >= 0)
            .map(time::Duration::nanoseconds)
            .and_then(|duration| self.history.timestamp.checked_add(duration))
    }

    fn exit_display(&self) -> String {
        match self.exit_value() {
            Some(0) => "0 (success)".bright_green().to_string(),
            Some(code) => format!("{code} (failure)").bright_red().to_string(),
            None => "pending".bright_yellow().to_string(),
        }
    }

    fn duration_display(&self) -> String {
        match self.duration_value() {
            Some(duration) if duration >= 0 => format_duration_ns(duration),
            Some(_) => "unknown".bright_yellow().to_string(),
            None => "running".bright_yellow().to_string(),
        }
    }
}

#[cfg(feature = "daemon")]
impl TailKind {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Started => "started",
            Self::Ended => "ended",
        }
    }

    fn badge(self, exit: i64) -> colored::ColoredString {
        match self {
            Self::Started => "STARTED".bold().bright_blue(),
            Self::Ended if exit == 0 => "ENDED".bold().bright_green(),
            Self::Ended => "ENDED".bold().bright_red(),
        }
    }
}

#[cfg(feature = "daemon")]
fn format_history_time(timestamp: OffsetDateTime, tz: Timezone) -> Result<String> {
    Ok(timestamp.to_offset(tz.0).format(TIME_FMT)?)
}

#[cfg(feature = "daemon")]
fn format_duration_ns(duration_ns: i64) -> String {
    struct F(Duration);
    impl Display for F {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            format_duration_into(self.0, f)
        }
    }

    F(Duration::from_nanos(duration_ns.max(0).cast_unsigned())).to_string()
}

#[cfg(feature = "daemon")]
fn push_pretty_field(out: &mut String, label: &str, value: &str) {
    out.push_str("  ");
    let label = format!("{label}:");
    out.push_str(&label.bright_cyan().bold().to_string());
    if label.len() < 10 {
        out.push_str(&" ".repeat(10 - label.len()));
    }

    let mut lines = value.lines();
    if let Some(first) = lines.next() {
        out.push_str(first);
    }
    out.push('\n');

    for line in lines {
        out.push_str("             ");
        out.push_str(line);
        out.push('\n');
    }
}

#[cfg(feature = "daemon")]
fn normalize_optional_field(value: &str) -> Option<String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_owned())
    }
}

impl Cmd {
    #[cfg(feature = "daemon")]
    async fn handle_tail(settings: &Settings) -> Result<()> {
        let tty = std::io::stdout().is_terminal();
        let mut client = daemon::tail_client(settings).await?;
        let mut stream = client.tail_history().await?;
        let stdout = std::io::stdout();

        while let Some(reply) = stream.message().await? {
            let event = TailEvent::from_proto(reply)?;
            let rendered = event.render(tty, settings.timezone)?;
            let mut out = stdout.lock();

            match out.write_all(rendered.as_bytes()) {
                Ok(()) => out.flush()?,
                Err(err) if err.kind() == io::ErrorKind::BrokenPipe => break,
                Err(err) => return Err(err.into()),
            }
        }

        Ok(())
    }

    #[allow(clippy::too_many_lines, clippy::cast_possible_truncation)]
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

            #[cfg(feature = "daemon")]
            daemon_cmd::emit_event(settings, atuin_daemon::DaemonEvent::HistoryPruned).await;
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

            #[cfg(feature = "daemon")]
            let ids = matches.iter().map(|h| h.id.clone()).collect::<Vec<_>>();

            for entry in matches {
                eprintln!("deleting {}", entry.id);
                if settings.sync.records {
                    let (id, _) = history_store.delete(entry.id).await?;
                    history_store.incremental_build(db, &[id]).await?;
                } else {
                    db.delete(entry).await?;
                }
            }

            #[cfg(feature = "daemon")]
            daemon_cmd::emit_event(settings, atuin_daemon::DaemonEvent::HistoryDeleted { ids })
                .await;
        }
        Ok(())
    }

    #[allow(clippy::too_many_lines)]
    pub async fn run(self, settings: &Settings) -> Result<()> {
        match self {
            Self::Start {
                cmd_env,
                author,
                intent,
                command,
            } => {
                let command = if cmd_env {
                    std::env::var("ATUIN_COMMAND_LINE").unwrap_or_default()
                } else {
                    command.join(" ")
                };

                if let Some(id) =
                    start_history_entry(settings, &command, author.as_deref(), intent.as_deref())
                        .await?
                {
                    println!("{id}");
                }

                Ok(())
            }
            Self::End { id, exit, duration } => {
                end_history_entry(settings, &id, exit, duration).await
            }
            Self::Tail => {
                #[cfg(feature = "daemon")]
                {
                    return Self::handle_tail(settings).await;
                }

                #[cfg(not(feature = "daemon"))]
                bail!("`atuin history tail` requires Atuin to be built with the `daemon` feature");
            }
            cmd => {
                let context = current_context().await?;

                let db_path = PathBuf::from(settings.db_path.as_str());
                let record_store_path = PathBuf::from(settings.record_store_path.as_str());

                let db = Sqlite::new(db_path, settings.local_timeout).await?;
                let store = SqliteStore::new(record_store_path, settings.local_timeout).await?;

                let encryption_key: [u8; 32] = encryption::load_key(settings)
                    .context("could not load encryption key")?
                    .into();

                let host_id = Settings::host_id().await?;
                let history_store = HistoryStore::new(store.clone(), host_id, encryption_key);

                match cmd {
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
                            &db, settings, context, session, cwd, mode, format, false, print0,
                            reverse, tz,
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

                    Self::Start { .. } | Self::End { .. } | Self::Tail => unreachable!(),
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    #[cfg(feature = "daemon")]
    use time::macros::datetime;

    use super::*;

    #[test]
    fn normalize_command_strips_trailing_spaces_and_tabs() {
        let settings = Settings::utc();

        assert!(settings.strip_trailing_whitespace);
        assert_eq!(normalize_command_for_storage("ls   \t", &settings), "ls");
    }

    #[test]
    fn normalize_command_preserves_escaped_trailing_space() {
        let settings = Settings::utc();

        assert_eq!(
            normalize_command_for_storage("printf foo\\ ", &settings),
            "printf foo\\ "
        );
        assert_eq!(
            normalize_command_for_storage("printf foo\\\\ ", &settings),
            "printf foo\\\\"
        );
    }

    #[tokio::test]
    async fn handle_start_saves_trimmed_command() {
        let db = Sqlite::new("sqlite::memory:", 2.0).await.unwrap();
        let settings = Settings::utc();

        handle_start(&db, &settings, "ls   \t", None, None)
            .await
            .unwrap();

        let history = db
            .before(OffsetDateTime::now_utc() + time::Duration::SECOND, 1)
            .await
            .unwrap()
            .pop()
            .unwrap();
        assert_eq!(history.command, "ls");
    }

    #[tokio::test]
    async fn handle_start_can_keep_trailing_whitespace() {
        let db = Sqlite::new("sqlite::memory:", 2.0).await.unwrap();
        let settings = Settings {
            strip_trailing_whitespace: false,
            ..Settings::utc()
        };

        handle_start(&db, &settings, "ls   \t", None, None)
            .await
            .unwrap();

        let history = db
            .before(OffsetDateTime::now_utc() + time::Duration::SECOND, 1)
            .await
            .unwrap()
            .pop()
            .unwrap();
        assert_eq!(history.command, "ls   \t");
    }

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

    #[cfg(feature = "daemon")]
    fn sample_tail_event(kind: TailKind) -> TailEvent {
        TailEvent {
            kind,
            history: History {
                id: "history-id".to_owned().into(),
                timestamp: datetime!(2026-04-09 17:18:19 UTC),
                duration: 12_345_678,
                exit: 0,
                command: "git status".to_owned(),
                cwd: "/tmp/repo".to_owned(),
                session: "session-id".to_owned(),
                hostname: "host:ellie".to_owned(),
                author: "claude".to_owned(),
                intent: Some("inspect repository state".to_owned()),
                deleted_at: None,
            },
        }
    }

    #[cfg(feature = "daemon")]
    #[test]
    fn test_tail_json_output_contains_history_fields() {
        let json = sample_tail_event(TailKind::Ended)
            .render(false, Timezone(time::UtcOffset::UTC))
            .unwrap();
        let value: serde_json::Value = serde_json::from_str(&json).unwrap();

        assert_eq!(value["event"], "ended");
        assert_eq!(value["history"]["id"], "history-id");
        assert_eq!(value["history"]["duration_ns"], 12_345_678);
        assert_eq!(value["history"]["success"], true);
        assert!(value.get("record").is_none());
    }

    #[cfg(feature = "daemon")]
    #[test]
    fn test_tail_pretty_output_shows_pending_fields_for_started_events() {
        let rendered = sample_tail_event(TailKind::Started)
            .render(true, Timezone(time::UtcOffset::UTC))
            .unwrap();
        let plain = regex::Regex::new(r"\x1b\[[0-9;]*m")
            .unwrap()
            .replace_all(&rendered, "");

        assert!(plain.contains("STARTED git status"));
        assert!(plain.contains("exit:"));
        assert!(plain.contains("pending"));
        assert!(plain.contains("duration:"));
        assert!(plain.contains("running"));
    }
}
