//! The client command which is responsible for querying the daemon for any stored command output.
//!
//! The code quality isn't great here, and is a little messy, but it is what it is. Can be cleaned
//! up in the future.
use std::io::{self, IsTerminal, Write};
use std::ops::Range;

use atuin_client::database::Sqlite;
use atuin_client::settings::Settings;
use atuin_common::string::EscapeNonPrintablePosixExt as _;
use atuin_daemon::client::SearchClient;
use clap::Parser;
use futures_util::TryStreamExt;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum RunError {
    #[error("output capture is disabled. enable [output] in your config to search command output.")]
    Disabled,

    #[error("blank query provided. please run 'atuin search-output --help'")]
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

/// Full-text search over captured command output.
#[derive(Parser, Debug)]
pub struct Cmd {
    #[arg(allow_hyphen_values = true, required = true)]
    query: Vec<String>,

    /// Maximum number of matches to return.
    #[arg(long, default_value_t = 20)]
    limit: u32,
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

impl Cmd {
    pub async fn run(self, db: &Sqlite, settings: &Settings) -> Result<(), RunError> {
        if settings.output.limits().is_none() {
            return Err(RunError::Disabled);
        }

        let query = self.query.join(" ");
        if query.trim().is_empty() {
            return Err(RunError::EmptyQuery);
        }

        let mut client = connect(settings).await?;
        let matches =
            client.search_command_output(query, self.limit).await.map_err(RunError::Search)?;
        let mut matches = std::pin::pin!(matches);

        let mut rows = Vec::new();
        while let Some(m) = matches.try_next().await.map_err(RunError::Search)? {
            let Some(history) =
                db.load(m.history_id).await.map_err(|e| RunError::LoadHistory(e.into()))?
            else {
                continue;
            };
            let highlighted = m.output;
            let [open, close] = highlighted.markers();
            let plain = highlighted.display_plain().to_string();
            let open_len = open.len_utf8();
            let close_len = close.len_utf8();
            let matches: Vec<Range<usize>> = highlighted
                .ranges()
                .scan(0usize, |stripped, r| {
                    *stripped += open_len;
                    let shifted = (r.start - *stripped)..(r.end - *stripped);
                    *stripped += close_len;
                    Some(shifted)
                })
                .collect();
            rows.push((history.command, plain, matches));
        }

        let mut w = io::stdout().lock();
        let tty = w.is_terminal();

        for (command, output, matches) in rows {
            let command = command.trim();
            let context = context_line(&output, &matches, tty);

            let line = if tty {
                format!("{}\t{context}", command.escape_non_printable())
            } else {
                format!("{command}\t{context}")
            };

            if let Err(err) = writeln!(w, "{line}") {
                if err.kind() == io::ErrorKind::BrokenPipe {
                    break;
                }
                return Err(err.into());
            }
        }

        Ok(())
    }
}

/// The line of `output` holding the first match (or its first line, when there is none), trimmed.
/// With `tty`, non-printables are escaped and every match on the line is bolded; without, the line
/// is emitted verbatim so it stays greppable.
fn context_line(output: &str, matches: &[Range<usize>], tty: bool) -> String {
    let anchor = matches.first().map_or(0, |m| m.start.min(output.len()));
    let line_start = output[..anchor].rfind('\n').map_or(0, |i| i + 1);
    let line_end = output[anchor..].find('\n').map_or(output.len(), |i| anchor + i);
    let line = &output[line_start..line_end];

    if !tty {
        return line.trim().to_string();
    }

    // Trim the line, then shift the ranges into the trimmed line's coordinates.
    let trimmed = line.trim();
    let offset = line_start + (line.len() - line.trim_start().len());
    let end = offset + trimmed.len();

    let mut out = String::with_capacity(trimmed.len());
    let mut cursor = 0;
    for m in matches {
        // Clamp to the line and skip anything malformed (the ranges come from the daemon).
        let (start, stop) = (m.start.max(offset), m.end.min(end));
        if start >= stop || start < offset + cursor {
            continue;
        }
        let (start, stop) = (start - offset, stop - offset);
        let (Some(gap), Some(hit)) = (trimmed.get(cursor..start), trimmed.get(start..stop)) else {
            continue;
        };
        out.push_str(&gap.escape_non_printable());
        out.push_str("\x1b[1m");
        out.push_str(&hit.escape_non_printable());
        out.push_str("\x1b[0m");
        cursor = stop;
    }
    let tail = &trimmed[cursor..];
    out.push_str(&tail.escape_non_printable());
    out
}
