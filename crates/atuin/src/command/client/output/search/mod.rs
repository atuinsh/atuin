//! The client command which is responsible for querying the daemon for any stored command output.
//!
//! The code quality isn't great here, and is a little messy, but it is what it is. Can be cleaned
//! up in the future.
use std::io::{self, IsTerminal};
use std::ops::Range;

use atuin_client::database::Sqlite;
use atuin_client::settings::Settings;
use atuin_client::theme::Theme;
use atuin_common::string::highlighted::Piece;
use atuin_daemon::client::SearchClient;
use clap::{Parser, ValueEnum};
use futures_util::TryStreamExt;
use thiserror::Error;
use time::OffsetDateTime;

mod writers;

use writers::{Hit, MatchRenderer, PlainWriter, PrettyWriter, Writer};

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

async fn connect(settings: &Settings) -> Result<SearchClient, RunError> {
    // TODO(markovejnovic): Have a better mechanism to connect to the daemon.
    #[cfg(unix)]
    return SearchClient::new(settings.daemon.existing_socket_path().into_owned())
        .await
        .map_err(RunError::Connect);

    #[cfg(not(unix))]
    SearchClient::new(settings.daemon.tcp_port).await.map_err(RunError::Connect)
}

// Fetch this many extra results beyond `--limit` so dropping our own `output search`
// runs from the results still leaves roughly `limit` to show.
const FILTER_HEADROOM: u32 = 64;

fn is_own_search(command: &str) -> bool {
    // Tokenize like a shell so a quoted/escaped binary path (spaces and all) stays one word.
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
    let mut subcommands = words.filter(|t| !t.starts_with('-'));
    subcommands.next() == Some("output") && subcommands.next() == Some("search")
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

        let mut client = connect(settings).await?;
        let matches = client
            .search_command_output(query, self.limit.saturating_add(FILTER_HEADROOM))
            .await
            .map_err(RunError::Search)?;
        let mut matches = std::pin::pin!(matches);

        let pretty = match self.style {
            Style::Plain => false,
            Style::Pretty => true,
            Style::Auto => io::stdout().is_terminal() && std::env::var_os("NO_COLOR").is_none(),
        };
        colored::control::set_override(pretty);

        let writer: Writer = if pretty {
            PrettyWriter.into()
        } else {
            PlainWriter.into()
        };
        let now = OffsetDateTime::now_utc();
        let width = crossterm::terminal::size().map_or(80, |(cols, _)| cols as usize);

        let mut rendered = 0;
        while rendered < self.limit {
            let Some(m) = matches.try_next().await.map_err(RunError::Search)? else {
                break;
            };
            let Some(history) =
                db.load(m.history_id).await.map_err(|e| RunError::LoadHistory(e.into()))?
            else {
                continue;
            };
            if is_own_search(&history.command) {
                continue;
            }

            let mut plain = String::new();
            let mut match_ranges: Vec<Range<usize>> = Vec::new();
            for piece in m.output.pieces() {
                match piece {
                    Piece::Text(text) => plain.push_str(text),
                    Piece::Match(text) => {
                        let start = plain.len();
                        plain.push_str(text);
                        match_ranges.push(start..plain.len());
                    }
                }
            }

            let hit = Hit {
                history: &history,
                output: &plain,
                ranges: &match_ranges,
                now,
                width,
                theme,
            };

            let mut out = io::stdout().lock();
            let result = if rendered > 0 {
                writer
                    .write_separator(&mut out, &hit)
                    .and_then(|()| writer.write_row(&mut out, &hit))
            } else {
                writer.write_row(&mut out, &hit)
            };
            match result {
                Ok(()) => rendered += 1,
                Err(err) if err.kind() == io::ErrorKind::BrokenPipe => break,
                Err(err) => return Err(err.into()),
            }
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
