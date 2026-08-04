//! Suggestion provider for the pty-proxy popup: prefix completions from
//! history (daemon index first, sqlite fallback), topped up with shell
//! completions from a [`CompletionOracleHandle`] matching the session's
//! shell. The backend lives on its own thread so a slow query can never
//! wedge the proxy's UI.

use std::path::Path;
use std::sync::mpsc;
use std::time::Duration;

use atuin_client::settings::Settings;
use atuin_client::theme::Meaning;
use atuin_common::shell::Shell;
use atuin_pty_proxy::{
    CompletionOracleHandle, OracleShell, Suggestion, SuggestionProvider, SuggestionSource,
    SyntaxClass, SyntaxSpan, find_in_path,
};

use super::client::search::syntax;

/// How long the popup waits for the suggestion worker before giving up, so
/// a slow backend can never wedge the proxy's UI.
const SUGGEST_REPLY_TIMEOUT: Duration = Duration::from_millis(250);

/// Bound on in-flight queries; the worker drains to the newest anyway, so
/// this only caps how much stale garbage can sit in the channel.
const SUGGEST_QUEUE_DEPTH: usize = 8;

/// How long one completion-oracle collect may wait; the query is enqueued
/// before the history lookup runs, so this overlaps rather than adds.
const COMPLETION_TIMEOUT: Duration = Duration::from_millis(150);

/// The provider plus the proxy hook that warms the completion oracle when
/// the session's first prompt appears.
pub(super) struct SuggestHooks {
    pub(super) provider: SuggestionProvider,
    pub(super) session_ready: Option<Box<dyn FnOnce() + Send>>,
}

/// Experimental, gated on `suggest.enabled`. `session_shell` is the shell
/// the proxy will spawn, so the completion oracle matches the session
/// rather than a possibly-stale `$SHELL`.
pub(super) fn history_suggestion_provider(
    settings: Settings,
    session_shell: Option<&Path>,
) -> Option<SuggestHooks> {
    if !settings.suggest.enabled {
        return None;
    }

    let min_chars = settings.suggest.min_chars.max(1);
    let shell_name = session_shell_name(session_shell);
    let oracle = completion_oracle(&shell_name);
    let warmer = oracle.as_ref().map(CompletionOracleHandle::warmer);
    let (req_tx, req_rx) = mpsc::sync_channel::<SuggestRequest>(SUGGEST_QUEUE_DEPTH);

    std::thread::spawn(move || suggestion_worker(settings, shell_name, oracle, &req_rx));

    let provider: SuggestionProvider = Box::new(move |line: &str| {
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
    });
    Some(SuggestHooks {
        provider,
        session_ready: warmer
            .map(|warmer| Box::new(move || warmer.warm()) as Box<dyn FnOnce() + Send>),
    })
}

struct SuggestRequest {
    line: String,
    reply: mpsc::Sender<Vec<Suggestion>>,
}

/// Basename of the shell the proxy spawns, lowercased; picks both the
/// completion engine and the syntax classifier's grammar.
fn session_shell_name(session_shell: Option<&Path>) -> String {
    session_shell
        .and_then(Path::file_name)
        .map(|name| name.to_string_lossy().into_owned())
        .or_else(|| std::env::var("SHELL").ok())
        .unwrap_or_default()
        .rsplit('/')
        .next()
        .unwrap_or_default()
        .trim_start_matches('-')
        .to_ascii_lowercase()
}

/// Pick the completion engine for the session's shell (so its config and
/// completion system answer), falling back to any installed engine. Only
/// the matching engine loads the user's rc files; a substitute runs
/// hermetically.
fn completion_oracle(shell_name: &str) -> Option<CompletionOracleHandle> {
    let engines: &[(OracleShell, &str)] = match Shell::from_string(shell_name.to_string()) {
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
    shell_name: String,
    oracle: Option<CompletionOracleHandle>,
    req_rx: &mpsc::Receiver<SuggestRequest>,
) {
    let Ok(runtime) = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    else {
        return;
    };

    let mut backend = SuggestionBackend::new(settings, shell_name, oracle);

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
            .filter(|suggestion| !suggestion.text.contains('\n'))
            .collect();
        let _ = request.reply.send(commands);
    }
}

/// Lazily connected suggestion backends: history (daemon index first,
/// sqlite fallback), plus the completion oracle.
struct SuggestionBackend {
    settings: Settings,
    /// Session shell basename; selects the syntax classifier's grammar.
    shell_name: String,
    #[cfg(feature = "daemon")]
    daemon: Option<atuin_daemon::client::SearchClient>,
    local: Option<(
        atuin_client::database::Sqlite,
        atuin_client::database::Context,
    )>,
    oracle: Option<CompletionOracleHandle>,
}

impl SuggestionBackend {
    fn new(settings: Settings, shell_name: String, oracle: Option<CompletionOracleHandle>) -> Self {
        Self {
            settings,
            shell_name,
            #[cfg(feature = "daemon")]
            daemon: None,
            local: None,
            oracle,
        }
    }

    /// Attach provenance and syntax classification to a raw command line.
    fn suggestion(&self, text: String, source: SuggestionSource) -> Suggestion {
        let shell = (!self.shell_name.is_empty()).then_some(self.shell_name.as_str());
        let syntax = syntax_spans(&text, shell);
        Suggestion {
            text,
            source,
            syntax,
        }
    }

    async fn fetch(&mut self, query: &str) -> Vec<Suggestion> {
        // The oracle computes on its own thread while history runs, so the
        // two costs overlap instead of adding.
        let pending = self
            .oracle
            .as_mut()
            .and_then(|oracle| oracle.enqueue(query));

        // History first — it's ranked and personal; completions top up
        // below so the ghost stays a command you've actually run when one
        // matches.
        let mut suggestions: Vec<Suggestion> = self
            .fetch_history(query)
            .await
            .into_iter()
            .map(|text| self.suggestion(text, SuggestionSource::History))
            .collect();

        // History honors `suggest.limit`; completions are shown in full —
        // the shell already scoped them to the typed word, and the popup
        // windows long lists anyway. The oracle protocol caps each batch,
        // so "in full" stays bounded.
        let collected = match (self.oracle.as_mut(), pending) {
            (Some(oracle), Some(id)) => oracle.collect(id, COMPLETION_TIMEOUT),
            _ => Vec::new(),
        };
        let completions = collected
            .into_iter()
            .filter_map(|candidate| apply_completion(query, &candidate.completion));
        for completion in completions {
            if !suggestions.iter().any(|s| s.text == completion) {
                suggestions.push(self.suggestion(completion, SuggestionSource::Completion));
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

/// Classify `text` with the TUI's tree-sitter highlighter and run-length
/// encode the verdicts into the proxy's minimal span form. `classify`
/// memoizes per thread, and this runs on the worker's, so repeated
/// keystrokes over the same suggestions cost a hash lookup.
fn syntax_spans(text: &str, shell: Option<&str>) -> Vec<SyntaxSpan> {
    let mut spans: Vec<SyntaxSpan> = Vec::new();
    for meaning in syntax::classify(text, shell) {
        let class = match meaning {
            Meaning::SyntaxCommand => SyntaxClass::Command,
            Meaning::SyntaxFlag => SyntaxClass::Flag,
            Meaning::SyntaxString => SyntaxClass::String,
            Meaning::SyntaxVariable => SyntaxClass::Variable,
            Meaning::SyntaxComment => SyntaxClass::Comment,
            // Operators keep the row's foreground, like the TUI default.
            _ => SyntaxClass::Plain,
        };
        match spans.last_mut() {
            Some(span) if span.class == class => span.len += 1,
            _ => spans.push(SyntaxSpan { len: 1, class }),
        }
    }
    spans
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

    /// Mirrors the classifier's own `simple_command` case, re-encoded as runs.
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    #[rstest]
    fn syntax_spans_run_length_encode_the_classification() {
        use super::syntax_spans;
        use atuin_pty_proxy::{SyntaxClass, SyntaxSpan};

        let spans = syntax_spans("git commit -m 'hi'", Some("zsh"));
        let expected = [
            (3, SyntaxClass::Command),
            (8, SyntaxClass::Plain),
            (2, SyntaxClass::Flag),
            (1, SyntaxClass::Plain),
            (4, SyntaxClass::String),
        ]
        .map(|(len, class)| SyntaxSpan { len, class });
        assert_eq!(spans, expected);
    }

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
