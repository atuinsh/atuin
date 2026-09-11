use std::io::{self, IsTerminal, Write};
use std::ops::Range;

use atuin_client::database::Sqlite;
use atuin_client::history::HistoryId;
use atuin_client::settings::Settings;
use atuin_common::string::EscapeNonPrintablePosixExt as _;
use atuin_common::string::highlighted::HighlightedString;
use atuin_daemon::client::SearchClient;
use clap::Parser;
use eyre::{Result, WrapErr, bail};

/// Full-text search over captured command output.
///
/// Captured output lives only in the daemon (fjall + a sidecar sqlite FTS index), so this always
/// goes through the daemon's `SearchCommandOutput` RPC. Each match is printed as
/// `command<TAB>line`, most relevant first, where `line` is the line of output holding the first
/// match (with every match on it bolded when stdout is a terminal).
#[derive(Parser, Debug)]
pub struct Cmd {
    #[arg(allow_hyphen_values = true)]
    query: Vec<String>,

    /// Maximum number of matches to return.
    #[arg(long, default_value_t = 20)]
    limit: u32,
}

async fn connect(settings: &Settings) -> Result<SearchClient> {
    #[cfg(unix)]
    return SearchClient::new(settings.daemon.existing_socket_path().into_owned()).await;

    #[cfg(not(unix))]
    SearchClient::new(settings.daemon.tcp_port).await
}

impl Cmd {
    pub async fn run(self, db: &Sqlite, settings: &Settings) -> Result<()> {
        if settings.output.limits().is_none() {
            bail!(
                "output capture is disabled; enable [output] in your config to search command \
                 output"
            );
        }

        let query = self.query.join(" ");
        if query.trim().is_empty() {
            bail!("no search query provided");
        }

        // A connect failure already carries the "Is it running?" message; an old daemon without
        // the RPC surfaces a gRPC Unimplemented error from the call below. Both propagate as-is.
        let mut client = connect(settings).await?;
        let matches = client.search_command_output(query, self.limit).await?;

        // Hydrate commands from the local db before touching stdout: holding the stdout lock across
        // an await would make this future non-`Send`, which the dispatcher requires.
        let mut rows = Vec::with_capacity(matches.len());
        for m in matches {
            let Some(proto_id) = m.history_id else {
                bail!("daemon returned a match with no history id");
            };
            let id: HistoryId =
                proto_id.try_into().wrap_err("daemon returned a malformed history id")?;
            let Some(history) = db.load(id).await? else {
                // The daemon may hold captured output for a history row that no longer exists
                // locally; skip it rather than error.
                continue;
            };
            let Some(output) = m.output else {
                bail!("daemon returned a match with no output");
            };
            let (open, close) = (output.open, output.close);
            let highlighted: HighlightedString =
                output.try_into().wrap_err("daemon returned an invalid highlighted output")?;
            let plain = highlighted.display_plain().to_string();
            let open_len = char::from_u32(open).map_or(0, char::len_utf8);
            let close_len = char::from_u32(close).map_or(0, char::len_utf8);
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn picks_the_line_of_the_first_match_and_bolds_every_match_on_it() {
        let output = "first line\n  error: bad error\nlast line";
        let matches = [13..18, 24..29];
        assert_eq!(context_line(output, &matches, false), "error: bad error");
        assert_eq!(
            context_line(output, &matches, true),
            "\x1b[1merror\x1b[0m: bad \x1b[1merror\x1b[0m"
        );
    }

    #[test]
    fn falls_back_to_the_first_line_without_matches() {
        assert_eq!(context_line("only\nlines", &[], true), "only");
    }

    #[test]
    fn malformed_ranges_are_ignored_rather_than_panicking() {
        // Past the end, and splitting a multi-byte char.
        assert_eq!(context_line("héllo", &[100..200, 1..2], true), "héllo");
    }
}
