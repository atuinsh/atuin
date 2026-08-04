//! Suggestion provider for the pty-proxy popup: prefix completions from
//! history (daemon index first, sqlite fallback), topped up with shell
//! completions from a [`CompletionOracleHandle`] matching the session's
//! shell. The backend lives on its own thread so a slow query can never
//! wedge the proxy's UI.

use std::path::Path;
use std::sync::mpsc;
use std::time::Duration;

use atuin_client::settings::Settings;
use atuin_common::shell::Shell;
use atuin_pty_proxy::{CompletionOracleHandle, OracleShell, SuggestionProvider, find_in_path};

/// How long the popup waits for the suggestion worker before giving up, so
/// a slow backend can never wedge the proxy's UI.
const SUGGEST_REPLY_TIMEOUT: Duration = Duration::from_millis(250);

/// Bound on in-flight queries; the worker drains to the newest anyway, so
/// this only caps how much stale garbage can sit in the channel.
const SUGGEST_QUEUE_DEPTH: usize = 8;

/// How long one completion-oracle collect may wait; the query is enqueued
/// before the history lookup runs, so this overlaps rather than adds.
const COMPLETION_TIMEOUT: Duration = Duration::from_millis(150);

/// Experimental, gated on `suggest.enabled`. `session_shell` is the shell
/// the proxy will spawn, so the completion oracle matches the session
/// rather than a possibly-stale `$SHELL`.
pub(super) fn history_suggestion_provider(
    settings: Settings,
    session_shell: Option<&Path>,
) -> Option<SuggestionProvider> {
    if !settings.suggest.enabled {
        return None;
    }

    let min_chars = settings.suggest.min_chars.max(1);
    let oracle = completion_oracle(session_shell);
    let (req_tx, req_rx) = mpsc::sync_channel::<SuggestRequest>(SUGGEST_QUEUE_DEPTH);

    std::thread::spawn(move || suggestion_worker(settings, oracle, &req_rx));

    Some(Box::new(move |line: &str| {
        // take(min_chars) keeps the length check O(min_chars), not O(line).
        if line.chars().take(min_chars).count() < min_chars {
            return Vec::new();
        }
        let (reply, reply_rx) = mpsc::channel();
        let request = SuggestRequest {
            line: line.to_string(),
            reply,
        };
        if req_tx.try_send(request).is_err() {
            return Vec::new();
        }
        reply_rx
            .recv_timeout(SUGGEST_REPLY_TIMEOUT)
            .unwrap_or_default()
    }))
}

struct SuggestRequest {
    line: String,
    reply: mpsc::Sender<Vec<String>>,
}

/// Pick the completion engine for the session's shell (so its config and
/// completion system answer), falling back to any installed engine. Only
/// the matching engine loads the user's rc files; a substitute runs
/// hermetically.
fn completion_oracle(session_shell: Option<&Path>) -> Option<CompletionOracleHandle> {
    let shell_name = session_shell
        .and_then(Path::file_name)
        .map(|name| name.to_string_lossy().into_owned())
        .or_else(|| std::env::var("SHELL").ok())
        .unwrap_or_default();
    let shell_name = shell_name
        .rsplit('/')
        .next()
        .unwrap_or_default()
        .trim_start_matches('-')
        .to_ascii_lowercase();

    let engines: &[(OracleShell, &str)] = match Shell::from_string(shell_name.clone()) {
        Shell::Zsh => &[
            (OracleShell::Zsh, "zsh"),
            (OracleShell::Fish, "fish"),
            (OracleShell::Bash, "bash"),
        ],
        Shell::Bash => &[
            (OracleShell::Bash, "bash"),
            (OracleShell::Fish, "fish"),
            (OracleShell::Zsh, "zsh"),
        ],
        _ => &[
            (OracleShell::Fish, "fish"),
            (OracleShell::Zsh, "zsh"),
            (OracleShell::Bash, "bash"),
        ],
    };
    engines.iter().find_map(|&(engine, binary)| {
        let bin = find_in_path(binary)?;
        let load_user_config = binary == shell_name;
        Some(CompletionOracleHandle::spawn(engine, bin, load_user_config))
    })
}

fn suggestion_worker(
    settings: Settings,
    oracle: Option<CompletionOracleHandle>,
    req_rx: &mpsc::Receiver<SuggestRequest>,
) {
    let Ok(runtime) = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    else {
        return;
    };

    let mut backend = SuggestionBackend::new(settings, oracle);

    while let Ok(mut request) = req_rx.recv() {
        // Only the newest queued request is worth a full fetch; earlier ones
        // have already outlived their caller's reply timeout.
        while let Ok(newer) = req_rx.try_recv() {
            request = newer;
        }

        let results = runtime.block_on(backend.fetch(&request.line));

        // Each newline typed into the pty would submit the line so far; the
        // daemon filters multiline already, this covers the other backends.
        let commands = results
            .into_iter()
            .filter(|command| !command.contains('\n'))
            .collect();
        let _ = request.reply.send(commands);
    }
}

/// Lazily connected suggestion backends: history (daemon index first,
/// sqlite fallback), plus the completion oracle.
struct SuggestionBackend {
    settings: Settings,
    #[cfg(feature = "daemon")]
    daemon: Option<atuin_daemon::client::SearchClient>,
    local: Option<(
        atuin_client::database::Sqlite,
        atuin_client::database::Context,
    )>,
    oracle: Option<CompletionOracleHandle>,
}

impl SuggestionBackend {
    fn new(settings: Settings, oracle: Option<CompletionOracleHandle>) -> Self {
        Self {
            settings,
            #[cfg(feature = "daemon")]
            daemon: None,
            local: None,
            oracle,
        }
    }

    async fn fetch(&mut self, query: &str) -> Vec<String> {
        // The oracle computes on its own thread while history runs, so the
        // two costs overlap instead of adding.
        let pending = self
            .oracle
            .as_mut()
            .and_then(|oracle| oracle.enqueue(query));

        // History first — it's ranked and personal; completions top up
        // below so the ghost stays a command you've actually run when one
        // matches.
        let mut suggestions = self.fetch_history(query).await;

        if let (Some(oracle), Some(id)) = (self.oracle.as_mut(), pending) {
            let limit = self.settings.suggest.limit as usize;
            let completions = oracle
                .collect(id, COMPLETION_TIMEOUT)
                .into_iter()
                .filter_map(|candidate| apply_completion(query, &candidate.completion))
                .take(limit);
            for completion in completions {
                if !suggestions.contains(&completion) {
                    suggestions.push(completion);
                }
            }
        }
        suggestions
    }

    async fn fetch_history(&mut self, query: &str) -> Vec<String> {
        // Daemon first: in-memory, frecency-ranked, no sqlite per keystroke.
        #[cfg(feature = "daemon")]
        if self.settings.daemon.enabled {
            if self.daemon.is_none() {
                self.daemon = atuin_daemon::client::SearchClient::new(
                    self.settings.daemon.socket_path.clone(),
                )
                .await
                .ok();
            }
            if let Some(client) = self.daemon.as_mut() {
                match client.suggest(query, self.settings.suggest.limit).await {
                    Ok(suggestions) => {
                        return suggestions
                            .into_iter()
                            .map(|suggestion| suggestion.command)
                            .collect();
                    }
                    // Drop the connection and fall through to sqlite for
                    // this query; the next one retries the daemon.
                    Err(_) => self.daemon = None,
                }
            }
        }

        self.fetch_local(query).await
    }

    async fn fetch_local(&mut self, query: &str) -> Vec<String> {
        use atuin_client::database::{Database, DbSearchMode, OptFilters, Sqlite, query_context};
        use atuin_client::settings::FilterMode;

        if self.local.is_none() {
            let Ok(db) = Sqlite::new(&self.settings.db_path, self.settings.local_timeout).await
            else {
                return Vec::new();
            };
            // The proxy runs outside the hooked shell (no ATUIN_SESSION
            // yet), so use the sessionless context and a global filter.
            let Ok(context) = query_context().await else {
                return Vec::new();
            };
            self.local = Some((db, context));
        }
        let Some((db, context)) = self.local.as_ref() else {
            return Vec::new();
        };

        db.search(
            DbSearchMode::Prefix,
            FilterMode::Global,
            context,
            query,
            OptFilters {
                limit: Some(i64::from(self.settings.suggest.limit)),
                ..OptFilters::default()
            },
        )
        .await
        .unwrap_or_default()
        .into_iter()
        .map(|entry| entry.command)
        .collect()
    }
}

/// Splice a completion token back into the command line by replacing its
/// last whitespace-separated token. Whole-line form keeps completions
/// prefix-extensions of the typed line, which is what the ghost text and
/// accept paths expect.
fn apply_completion(line: &str, token: &str) -> Option<String> {
    if token.is_empty() {
        return None;
    }
    let token_start = line
        .rfind(char::is_whitespace)
        .map_or(0, |position| position + 1);
    let completed = format!("{}{}", &line[..token_start], token);
    (completed != line).then_some(completed)
}

#[cfg(test)]
mod tests {
    use super::apply_completion;
    use rstest::rstest;

    #[rstest]
    #[case::subcommand("git ch", "checkout", Some("git checkout"))]
    #[case::flag("git status --sh", "--short", Some("git status --short"))]
    #[case::first_token("gi", "git", Some("git"))]
    #[case::after_trailing_space("git ", "checkout", Some("git checkout"))]
    #[case::noop_completion("git checkout", "checkout", None)]
    #[case::empty_token("git ch", "", None)]
    fn splices_completion_into_line(
        #[case] line: &str,
        #[case] token: &str,
        #[case] expected: Option<&str>,
    ) {
        assert_eq!(apply_completion(line, token).as_deref(), expected);
    }
}
