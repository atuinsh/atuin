use std::io::{self, IsTerminal, Write};

use atuin_client::database::Sqlite;
use atuin_client::history::HistoryId;
use atuin_client::settings::Settings;
use atuin_common::string::EscapeNonPrintablePosixExt as _;
use atuin_daemon::client::SearchClient;
use clap::Parser;
use eyre::{Result, WrapErr, bail};

/// Full-text search over captured command output.
///
/// Captured output lives only in the daemon (fjall + a sidecar sqlite FTS index), so this always
/// goes through the daemon's `SearchCommandOutput` RPC. Each match is printed as
/// `command<TAB>snippet`, most relevant first.
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
                "output capture is disabled; enable [output] in your config to search command output"
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
            let bytes: [u8; 16] = m
                .history_id
                .as_slice()
                .try_into()
                .wrap_err("daemon returned a malformed history id")?;
            let id = HistoryId::from_bytes(bytes);
            let Some(history) = db.load(id).await? else {
                // The daemon may hold captured output for a history row that no longer exists
                // locally; skip it rather than error.
                continue;
            };
            rows.push((history.command, m.snippet));
        }

        let mut w = io::stdout().lock();
        let escape = w.is_terminal();

        for (command, snippet) in rows {
            let command = command.trim();
            let snippet = snippet.replace(['\n', '\r'], " ");
            let snippet = snippet.trim();

            let line = if escape {
                format!("{}\t{}", command.escape_non_printable(), snippet.escape_non_printable())
            } else {
                format!("{command}\t{snippet}")
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
