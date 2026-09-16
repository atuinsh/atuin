//! The client command which is responsible for querying the daemon for any stored command output.
//!
//! The code quality isn't great here, and is a little messy, but it is what it is. Can be cleaned
//! up in the future.

use std::io::{self, IsTerminal};
use std::sync::OnceLock;

use atuin_client::database::Sqlite;
use atuin_client::settings::Settings;
use atuin_client::theme::Theme;
use atuin_daemon::client::SearchClient;
use clap::{Parser, ValueEnum};
use futures_util::{StreamExt, TryStreamExt};
use thiserror::Error;
use time::OffsetDateTime;
use tracing::debug;

mod writers;

use writers::{HistoryMatch, JsonWriter, PlainWriter, PrettyWriter, RenderCtx, Writer};

use crate::command::client::daemon;

#[derive(Debug, Error)]
pub enum RunError {
    #[error("output capture is disabled. enable [output] in your config to search command output.")]
    Disabled,

    #[error("blank query provided. please run 'atuin output search --help'")]
    EmptyQuery,

    #[error("could not connect to the daemon")]
    Connect(#[source] eyre::Report),

    #[error("the daemon failed to search command output")]
    Search(#[source] eyre::Report),

    #[error("could not load history from the local database")]
    LoadHistory(#[source] eyre::Report),

    #[error("could not write search results")]
    Write(#[from] io::Error),
}

#[derive(Copy, Clone, Debug, ValueEnum)]
enum Style {
    Auto,
    Plain,
    Pretty,
    /// A single JSON array of match objects.
    Json,
    /// Newline-delimited JSON: one match object per line.
    Ndjson,
}

/// Full-text search over captured command output.
#[derive(Parser, Debug)]
pub struct Cmd {
    #[arg(allow_hyphen_values = true, required = true)]
    query: Vec<String>,

    /// Maximum number of matches to return.
    #[arg(long, default_value_t = 5)]
    limit: u32,

    /// How matches are rendered.
    #[arg(long, value_enum, default_value_t = Style::Auto)]
    style: Style,
}

fn to_run_error(err: eyre::Report) -> RunError {
    if daemon::should_retry_after_error(&err) {
        RunError::Connect(err)
    } else {
        RunError::Search(err)
    }
}

/// Check whether the given command string refers to an `atuin output search ...` command.
///
/// TODO(markovejnovic): This is a little bit of a massive hack, but it seems to work for now.
/// Perhaps a better solution would be to inject some sort of magic invisible codes in the output
/// so that we can later catch that.
///
/// Another option is to have the `atuin` command inject some sort of magic metadata into the
/// history on the daemon to communicate what it was, so the daemon doesn't return it.
///
/// I don't know, but I could bikeshed this for eons. Keeping this in to ship the feature rather
/// than waste time.
fn is_own_search(command: &str) -> bool {
    let Some(words) = shlex::split(command) else {
        return false;
    };

    let mut words = words.iter().map(String::as_str);
    let Some(bin) = words.next() else {
        return false;
    };

    if bin.rsplit('/').next().unwrap_or(bin) != "atuin" {
        return false;
    }

    // Match subcommands as prefixes, not exact strings, so abbreviated invocations
    // (`atuin out sea ...`, which clap's infer_subcommands accepts) are still recognized as ours.
    let mut subcommands = words.filter(|t| !t.starts_with('-'));
    let prefixes =
        |full: &str, tok: Option<&str>| tok.is_some_and(|t| !t.is_empty() && full.starts_with(t));
    prefixes("output", subcommands.next()) && prefixes("search", subcommands.next())
}

fn writer_for(style: Style) -> Writer {
    match style {
        Style::Plain => {
            colored::control::set_override(false);
            Writer::Plain(PlainWriter)
        }
        Style::Pretty => {
            colored::control::set_override(true);
            Writer::Pretty(PrettyWriter)
        }
        Style::Auto => {
            let pretty = io::stdout().is_terminal() && std::env::var_os("NO_COLOR").is_none();
            colored::control::set_override(pretty);
            if pretty {
                Writer::Pretty(PrettyWriter)
            } else {
                Writer::Plain(PlainWriter)
            }
        }
        Style::Json => {
            colored::control::set_override(false);
            Writer::Json(JsonWriter { array: true })
        }
        Style::Ndjson => {
            colored::control::set_override(false);
            Writer::Json(JsonWriter { array: false })
        }
    }
}

impl Cmd {
    pub async fn run(
        self,
        db: &Sqlite,
        settings: &Settings,
        theme: &Theme,
    ) -> Result<(), RunError> {
        if settings.output.limits().is_none() {
            return Err(RunError::Disabled);
        }

        let query = self.query.join(" ");
        if query.trim().is_empty() {
            return Err(RunError::EmptyQuery);
        }

        // TODO(markovejnovic): This should ideally be injected rather than prepared here ad-hoc.
        //                      Existing precedent.
        let open = async || {
            #[cfg(unix)]
            let mut client =
                SearchClient::new(settings.daemon.existing_socket_path().into_owned()).await?;
            #[cfg(not(unix))]
            let mut client = SearchClient::new(settings.daemon.tcp_port).await?;
            client.search_command_output(query.clone(), None).await
        };

        let matches = match open().await {
            Ok(stream) => stream,
            Err(err) if settings.daemon.autostart && daemon::should_retry_after_error(&err) => {
                debug!("daemon unavailable for output search; attempting auto-start");
                daemon::ensure_daemon_running(settings).await.map_err(|start_err| {
                    RunError::Connect(
                        err.wrap_err(format!("failed to auto-start daemon: {start_err:#}")),
                    )
                })?;
                open().await.map_err(to_run_error)?
            }
            Err(err) => return Err(to_run_error(err)),
        };
        let mut matches = std::pin::pin!(matches);

        let writer = writer_for(self.style);
        let ctx = RenderCtx {
            now: OffsetDateTime::now_utc(),
            width: crossterm::terminal::size().map_or(80, |(cols, _)| cols as usize),
            theme,
        };

        // A load/search failure can't ride out through a stream item, so stash it and surface it
        // after rendering. A write-once `OnceLock` (not a `Cell`) keeps the stream `Send`, since
        // `async_stream::stream!` captures the slot by shared reference.
        let load_err: OnceLock<RunError> = OnceLock::new();
        let write_result = {
            let err_slot = &load_err;

            let rows = async_stream::stream! {
                loop {
                    let m = match matches.try_next().await {
                        Ok(Some(m)) => m,
                        Ok(None) => break,
                        Err(err) => {
                            let _ = err_slot.set(RunError::Search(err));
                            break;
                        }
                    };

                    // TODO(markovejnovic): This is a little bit of a hack -- we should really be
                    //                      querying the daemon. Future improvement.
                    //
                    //                      Another option is to have the daemon return the full
                    //                      history struct.
                    let history = match db.load(m.history_id).await {
                        Ok(Some(history)) => history,
                        Ok(None) => continue,
                        Err(err) => {
                            let _ = err_slot.set(RunError::LoadHistory(err.into()));
                            break;
                        }
                    };

                    if is_own_search(&history.command) {
                        continue;
                    }

                    yield HistoryMatch { history, output_match: m };
                }
            };

            let mut out = io::stdout();
            writer.write_stream(&mut out, &ctx, rows.take(self.limit as usize)).await
        };

        match write_result {
            Ok(()) => {}
            Err(err) if err.kind() == io::ErrorKind::BrokenPipe => {}
            Err(err) => return Err(err.into()),
        }

        if let Some(err) = load_err.into_inner() {
            return Err(err);
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use rstest::rstest;

    use super::is_own_search;

    #[rstest]
    #[case::bare("atuin output search hello", true)]
    #[case::abbreviated_output("atuin out search hello", true)]
    #[case::abbreviated_both("atuin out sea hello", true)]
    #[case::relative_path("./target/debug/atuin output search cargo", true)]
    #[case::absolute_path("/usr/bin/atuin output search x", true)]
    #[case::quoted_path_with_spaces("\"/opt/my tools/atuin\" output search x", true)]
    #[case::global_flag_before_subcommand("atuin --foo output search x", true)]
    #[case::flag_between_subcommands("atuin output --foo search x", true)]
    #[case::output_other_subcommand("atuin output stats", false)]
    #[case::other_subcommand("atuin search foo", false)]
    #[case::not_atuin("echo atuin output search", false)]
    #[case::query_only_contains_it("atuin search \"output search\"", false)]
    #[case::empty("", false)]
    fn detects_own_search(#[case] command: &str, #[case] expected: bool) {
        assert_eq!(is_own_search(command), expected);
    }
}
