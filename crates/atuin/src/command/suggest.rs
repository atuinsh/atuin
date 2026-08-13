//! Suggestion provider for the pty-proxy popup: prefix completions from the
//! daemon's history index, topped up with shell completions from a
//! [`CompletionOracleHandle`] matching the session's shell. The backend
//! lives on its own thread so a slow query can never wedge the proxy's UI.
//!
//! History is ranked by where the session is (see [`SessionCwd`]) before
//! anything else, and the two sources are merged so that a history command
//! the shell's own completions still vouch for leads the list.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use atuin_client::settings::Settings;
use atuin_client::theme::Meaning;
use atuin_common::shell::Shell;
use atuin_pty_proxy::{
    CompletionOracleHandle, OracleShell, Suggestion, SuggestionProvider, SuggestionSource,
    SyntaxClass, SyntaxSpan, find_in_path,
};

use super::client::syntax;

/// How long the popup waits for the suggestion worker before giving up, so
/// a slow backend can never wedge the proxy's UI.
const SUGGEST_REPLY_TIMEOUT: Duration = Duration::from_millis(250);

/// Bound on in-flight queries; the worker drains to the newest anyway, so
/// this only caps how much stale garbage can sit in the channel.
const SUGGEST_QUEUE_DEPTH: usize = 8;

/// How long one completion-oracle collect may wait; the query is enqueued
/// before the history lookup runs, so this overlaps rather than adds.
const COMPLETION_TIMEOUT: Duration = Duration::from_millis(150);

/// How long the daemon's in-memory index gets to answer. It normally
/// replies in well under a millisecond, so this is not a budget but a
/// backstop: past it the daemon is rebuilding or wedged, and the popup is
/// better off empty than late.
const DAEMON_SUGGEST_TIMEOUT: Duration = Duration::from_millis(100);

/// How long a reading of the session shell's working directory is reused.
/// A `cd` is picked up on the next keystroke either way; this only bounds
/// how often a burst of typing re-reads the process.
const CWD_TTL: Duration = Duration::from_millis(100);

/// The provider plus the proxy hooks it needs from the session: the warmer
/// for the completion oracle, fired when the first prompt appears, and the
/// slot the runtime publishes the shell's pid into.
pub(super) struct SuggestHooks {
    pub(super) provider: SuggestionProvider,
    pub(super) session_ready: Option<Box<dyn FnOnce() + Send>>,
    pub(super) shell_pid: Arc<AtomicU32>,
}

/// Experimental, gated on `suggest.enabled`. `session_shell` is the shell
/// the proxy will spawn, so the completion oracle matches the session
/// rather than a possibly-stale `$SHELL`.
///
/// Suggestions are served from the daemon's in-memory index and nowhere
/// else: it is the only backend that can rank a whole history per keystroke
/// — by where the session is and by how often each command has been run —
/// without touching sqlite between characters.
pub(super) fn history_suggestion_provider(
    settings: Settings,
    session_shell: Option<&Path>,
) -> Option<SuggestHooks> {
    if !settings.suggest.enabled {
        return None;
    }
    if !settings.daemon.enabled {
        // Printed once per session, before raw mode: the alternative is a
        // feature the user has turned on that silently never appears.
        eprintln!("atuin: suggestions need the daemon; set `daemon.enabled = true` to use them");
        return None;
    }

    let min_chars = settings.suggest.min_chars.max(1);
    let shell_name = session_shell_name(session_shell);
    let oracle = completion_oracle(&shell_name);
    let warmer = oracle.as_ref().map(CompletionOracleHandle::warmer);
    let shell_pid = Arc::new(AtomicU32::new(0));
    let (req_tx, req_rx) = mpsc::sync_channel::<SuggestRequest>(SUGGEST_QUEUE_DEPTH);

    let worker_pid = shell_pid.clone();
    std::thread::spawn(move || {
        suggestion_worker(settings, shell_name, oracle, worker_pid, &req_rx);
    });

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
        shell_pid,
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

/// The session shell's working directory: where suggestions rank from.
///
/// The proxy's own cwd is fixed at startup and nothing in the terminal
/// stream reports a `cd`, so the shell process is asked directly —
/// `/proc/<pid>/cwd` on Linux, `proc_pidinfo` on macOS. This works whatever
/// the shell is, and needs nothing of the user's prompt.
///
/// The kernel reports the physical path, while history records `$PWD`, so a
/// directory reached through a symlink ranks as if the user were nowhere in
/// particular. That is a missing boost rather than a wrong one; closing it
/// would mean reading `$PWD` from the shell, which only OSC 7 reports.
struct SessionCwd {
    /// Published by the runtime once the shell is spawned; zero until then.
    pid: Arc<AtomicU32>,
    system: sysinfo::System,
    /// Last reading, and when it was taken.
    reading: Option<(PathBuf, Instant)>,
    /// Workspace root of the directory it was resolved for. `in_git_repo`
    /// walks up to the filesystem root, so it runs once per directory
    /// rather than once per keystroke.
    workspace: Option<(PathBuf, Option<PathBuf>)>,
}

impl SessionCwd {
    fn new(pid: Arc<AtomicU32>) -> Self {
        Self {
            pid,
            system: sysinfo::System::new(),
            reading: None,
            workspace: None,
        }
    }

    /// The shell's directory and the workspace root containing it. `None`
    /// before the shell has been spawned, or on a platform that will not
    /// report a cwd — the daemon then ranks without locality rather than
    /// against the wrong directory.
    fn resolve(&mut self) -> Option<(&Path, Option<&Path>)> {
        let pid = self.pid.load(Ordering::Relaxed);
        if pid == 0 {
            return None;
        }

        let fresh = self
            .reading
            .as_ref()
            .is_some_and(|(_, read_at)| read_at.elapsed() < CWD_TTL);
        if !fresh {
            let pid = sysinfo::Pid::from_u32(pid);
            self.system.refresh_process_specifics(
                pid,
                sysinfo::ProcessRefreshKind::new().with_cwd(sysinfo::UpdateKind::Always),
            );
            // A shell that has exited, or a platform that answers nothing,
            // leaves the last reading in place: where the session was is a
            // better guess than nowhere.
            if let Some(cwd) = self.system.process(pid).and_then(sysinfo::Process::cwd) {
                self.reading = Some((cwd.to_path_buf(), Instant::now()));
            }
        }

        let (cwd, _) = self.reading.as_ref()?;
        if self.workspace.as_ref().is_none_or(|(dir, _)| dir != cwd) {
            self.workspace = Some((
                cwd.clone(),
                atuin_common::utils::in_git_repo(&cwd.to_string_lossy()),
            ));
        }
        Some((
            cwd.as_path(),
            self.workspace
                .as_ref()
                .and_then(|(_, root)| root.as_deref()),
        ))
    }
}

fn suggestion_worker(
    settings: Settings,
    shell_name: String,
    oracle: Option<CompletionOracleHandle>,
    shell_pid: Arc<AtomicU32>,
    req_rx: &mpsc::Receiver<SuggestRequest>,
) {
    let Ok(runtime) = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    else {
        return;
    };

    let mut backend = SuggestionBackend::new(settings, shell_name, oracle, shell_pid);

    while let Ok(mut request) = req_rx.recv() {
        // Only the newest queued request is worth a full fetch; earlier ones
        // have already outlived their caller's reply timeout.
        while let Ok(newer) = req_rx.try_recv() {
            request = newer;
        }

        let results = runtime.block_on(backend.fetch(&request.line));

        // Display sanitization does not change the bytes accepted into the
        // pty, so no suggestion may contain a terminal-active control.
        let commands = results
            .into_iter()
            .filter(|suggestion| suggestion_text_is_safe(&suggestion.text))
            .collect();
        let _ = request.reply.send(commands);
    }
}

/// Lazily connected suggestion backends: the daemon's history index, plus
/// the completion oracle.
struct SuggestionBackend {
    settings: Settings,
    /// Session shell basename; selects the syntax classifier's grammar.
    shell_name: String,
    daemon: Option<atuin_daemon::client::SearchClient>,
    /// Hostname and host id, which never change, resolved on first use.
    /// The cwd and workspace this is stamped with do change, per query.
    context: Option<atuin_client::database::Context>,
    cwd: SessionCwd,
    oracle: Option<CompletionOracleHandle>,
}

impl SuggestionBackend {
    fn new(
        settings: Settings,
        shell_name: String,
        oracle: Option<CompletionOracleHandle>,
        shell_pid: Arc<AtomicU32>,
    ) -> Self {
        Self {
            settings,
            shell_name,
            daemon: None,
            context: None,
            cwd: SessionCwd::new(shell_pid),
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
        // The oracle completes relative paths against wherever it is, so it
        // has to be told where the session is — its own directory is the
        // proxy's, fixed at startup and blind to every `cd` since. Resolved
        // before the enqueue below; `query_context` re-reads it within the
        // TTL, so this costs a clone rather than a second look at the shell.
        let cwd = self.cwd.resolve().map(|(cwd, _)| cwd.to_path_buf());
        // The oracle computes on its own thread while history runs, so the
        // two costs overlap instead of adding.
        let pending = self
            .oracle
            .as_mut()
            .and_then(|oracle| oracle.enqueue(query, cwd.as_deref()));

        let history = self.fetch_history(query).await;

        // Scoping to this directory says the command belongs here; it says
        // nothing about whether the directory it names still does.
        let history = match cwd.as_deref() {
            Some(cwd) => history
                .into_iter()
                .filter(|command| cd_target_exists(command, cwd))
                .collect(),
            None => history,
        };

        // History honors `suggest.limit`; completions are shown in full —
        // the shell already scoped them to the typed word, and the popup
        // windows long lists anyway. The oracle protocol caps each batch,
        // so "in full" stays bounded.
        let collected = match (self.oracle.as_mut(), pending) {
            (Some(oracle), Some(id)) => oracle.collect(id, COMPLETION_TIMEOUT),
            _ => Vec::new(),
        };

        // A completion vouches for a history command when it offers the very
        // word that command would put at the cursor. That is the shell
        // saying the word is still good *now* — the branch still exists, the
        // file is still there, the subcommand is real — so history the shell
        // confirms leads the list, and the ghost text with it. History it
        // says nothing about (a deleted branch, a renamed script) sinks
        // below, and is still offered: the oracle is often simply silent.
        let offered: HashSet<&str> = collected
            .iter()
            .map(|candidate| candidate.completion.trim_end_matches('/'))
            .collect();
        let word_start = shell_word_start(query);
        let (confirmed, unconfirmed): (Vec<String>, Vec<String>) = history
            .into_iter()
            .partition(|text| vouched_for(text, word_start, &offered));

        let history: Vec<Suggestion> = confirmed
            .into_iter()
            .chain(unconfirmed)
            .map(|text| self.suggestion(text, SuggestionSource::History))
            .collect();
        let completions: Vec<Suggestion> = collected
            .into_iter()
            .filter_map(|candidate| apply_completion(query, &candidate.completion))
            .map(|text| self.suggestion(text, SuggestionSource::Completion))
            .collect();

        // Once there is a command in front of the cursor, the word being typed
        // is an argument, and the shell is the better authority on it: it
        // knows the branches, files and subcommands that exist *now*, while
        // history only knows what was run before. Leading with history there
        // puts the command you just ran above the one you are reaching for.
        //
        // On the command name itself the order is the other way round. History
        // completes a whole line — the flags and arguments too — where the
        // oracle can only name the binary, so the same keystrokes get you much
        // further from history.
        let (first, second) = if completing_an_argument(query) {
            (completions, history)
        } else {
            (history, completions)
        };
        let mut seen = HashSet::new();
        first
            .into_iter()
            .chain(second)
            .filter(|suggestion| seen.insert(suggestion.text.clone()))
            .collect()
    }

    /// Prefix matches from the daemon's in-memory index, ranked by where the
    /// session is and by how often each command has been run.
    async fn fetch_history(&mut self, query: &str) -> Vec<String> {
        if self.daemon.is_none() {
            self.daemon =
                atuin_daemon::client::SearchClient::new(self.settings.daemon.socket_path.clone())
                    .await
                    .ok();
        }
        let Some(context) = self.query_context().await else {
            return Vec::new();
        };
        let Some(client) = self.daemon.as_mut() else {
            return Vec::new();
        };

        // A daemon rebuilding its index answers late, not never, and an
        // unbounded await would pin this worker there, leaving the popup
        // blank for the whole stall.
        // No shell filter is sent: the daemon serves suggestions from the
        // index it already holds rather than rebuilding one to match, so a
        // filter here would only be discarded.
        let call = client.suggest(
            query,
            self.settings.suggest.limit,
            context,
            self.settings.suggest.filter_failed,
        );
        let Ok(Ok(suggestions)) = tokio::time::timeout(DAEMON_SUGGEST_TIMEOUT, call).await else {
            // Drop the connection so the next keystroke reconnects.
            self.daemon = None;
            return Vec::new();
        };
        suggestions
            .into_iter()
            .map(|suggestion| suggestion.command)
            .collect()
    }

    /// The query context, stamped with where the session shell is now. Only
    /// the cwd and workspace are read by `Suggest`, and both move with the
    /// user; the rest is resolved once.
    async fn query_context(&mut self) -> Option<atuin_client::database::Context> {
        if self.context.is_none() {
            // The proxy runs outside the hooked shell, so there is no
            // ATUIN_SESSION to read: the sessionless context is the one.
            self.context = atuin_client::database::query_context().await.ok();
        }
        let mut context = self.context.clone()?;
        if let Some((cwd, workspace)) = self.cwd.resolve() {
            context.cwd = cwd.to_string_lossy().into_owned();
            context.git_root = workspace.map(Path::to_path_buf);
        }
        Some(context)
    }
}

/// Whether the shell's own completions vouch for a history command: one of
/// them offers the word that command would leave at the cursor, meaning the
/// shell considers that word good right now.
///
/// `offered` holds completion tokens with any trailing `/` already removed,
/// and matching is by prefix in both directions of a path: `crates` vouches
/// for `cd crates` and for `cd crates/atuin-daemon`, since the shell has
/// confirmed as much of the path as it was asked about.
fn vouched_for(text: &str, word_start: usize, offered: &HashSet<&str>) -> bool {
    let word = text
        .get(word_start..)
        .unwrap_or_default()
        .split_whitespace()
        .next()
        .unwrap_or_default();

    !word.is_empty()
        && (offered.contains(word)
            || offered
                .iter()
                .any(|token| !token.is_empty() && word.starts_with(token)))
}

/// Whether the cursor sits on an argument rather than on the command name —
/// that is, whether anything but whitespace precedes the word being typed.
fn completing_an_argument(line: &str) -> bool {
    !line[..shell_word_start(line)].trim().is_empty()
}

/// Whether a `cd` still leads somewhere. History remembers where you used to
/// go, and directories get renamed, moved and deleted; `cd backend` where
/// there is no longer a backend is a suggestion that cannot run.
///
/// Only `cd` is judged, and only when it has exactly one argument. That is
/// the case where a bare relative word is certainly a directory — anywhere
/// else on a command line `foo/bar` is more often a git ref (`origin/main`)
/// than a path, and hiding a good suggestion is worse than showing a stale
/// one. Everything the shell would rewrite before opening — globs, variables,
/// quotes — is left alone for the same reason: its literal text is not the
/// path that would be used.
fn cd_target_exists(command: &str, cwd: &Path) -> bool {
    let mut words = command.split_whitespace();
    if words.next() != Some("cd") {
        return true;
    }
    // A bare `cd` goes home, and `cd -` to wherever you were: both always fine.
    let Some(target) = words.next().filter(|target| *target != "-") else {
        return true;
    };
    if words.next().is_some() {
        return true;
    }
    if target.contains(['*', '?', '[', ']', '{', '}', '$', '\\', '"', '\'']) {
        return true;
    }

    let path = match target.strip_prefix("~/") {
        Some(rest) => match std::env::var_os("HOME") {
            Some(home) => PathBuf::from(home).join(rest),
            None => return true,
        },
        None if target.starts_with('~') => return true,
        None => cwd.join(target),
    };
    path.is_dir()
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
/// current shell word. Whole-line form keeps completions
/// prefix-extensions of the typed line, which is what the ghost text and
/// accept paths expect.
fn apply_completion(line: &str, token: &str) -> Option<String> {
    if token.is_empty() {
        return None;
    }
    let token_start = shell_word_start(line);
    let completed = format!("{}{}", &line[..token_start], token);
    (completed != line).then_some(completed)
}

fn shell_word_start(line: &str) -> usize {
    let mut start = 0;
    let mut quote = None;
    let mut escaped = false;

    for (i, ch) in line.char_indices() {
        if escaped {
            escaped = false;
            continue;
        }

        match quote {
            Some('\'') => {
                if ch == '\'' {
                    quote = None;
                }
            }
            Some('"') => match ch {
                '"' => quote = None,
                '\\' => escaped = true,
                _ => {}
            },
            _ => match ch {
                '\'' | '"' => quote = Some(ch),
                '\\' => escaped = true,
                _ if ch.is_whitespace() => start = i + ch.len_utf8(),
                _ => {}
            },
        }
    }

    start
}

fn suggestion_text_is_safe(text: &str) -> bool {
    !text.chars().any(char::is_control)
}

#[cfg(test)]
mod tests {
    use super::{
        SessionCwd, apply_completion, cd_target_exists, completing_an_argument, shell_word_start,
        suggestion_text_is_safe, vouched_for,
    };
    use rstest::rstest;
    use std::collections::HashSet;
    use std::sync::Arc;
    use std::sync::atomic::AtomicU32;

    /// Reading another process's working directory is the whole basis of
    /// directory ranking, and it is platform-specific enough to be worth
    /// pinning down. This process stands in for the session shell.
    #[rstest]
    fn reads_the_working_directory_of_a_live_process() {
        let pid = std::process::id();
        let mut cwd = SessionCwd::new(Arc::new(AtomicU32::new(pid)));

        let (directory, _) = cwd.resolve().expect("a live process reports its cwd");
        assert_eq!(directory, std::env::current_dir().unwrap());
    }

    /// Nothing is claimed about the directory before the shell is spawned;
    /// pid 0 must not be read as "the proxy's own directory".
    #[rstest]
    fn reports_nothing_before_the_shell_is_spawned() {
        let mut cwd = SessionCwd::new(Arc::new(AtomicU32::new(0)));
        assert!(cwd.resolve().is_none());
    }

    /// A history command leads the popup when the shell's completions still
    /// offer the word it would put at the cursor.
    #[rstest]
    #[case::subcommand("git ch", "git checkout main", &["checkout", "cherry-pick"], true)]
    #[case::first_token("gi", "git status", &["git"], true)]
    #[case::after_trailing_space("git ", "git rebase -i", &["rebase", "status"], true)]
    #[case::flag("cargo test --", "cargo test --release", &["--release"], true)]
    // A directory completion arrives as `crates/`, trimmed to `crates` by
    // the caller; it speaks for the whole path below it.
    #[case::directory("cd cra", "cd crates/atuin-daemon", &["crates"], true)]
    #[case::directory_itself("cd cra", "cd crates", &["crates"], true)]
    // A branch that no longer exists: the shell offers every other one, so
    // this history command sinks below those it does still vouch for.
    #[case::deleted_branch("git checkout ", "git checkout old-feature", &["main", "release"], false)]
    // Nothing to say either way — the oracle is silent for this line.
    #[case::no_completions("git ch", "git checkout main", &[], false)]
    fn confirms_history_the_shell_still_offers(
        #[case] line: &str,
        #[case] history: &str,
        #[case] offered: &[&str],
        #[case] expected: bool,
    ) {
        let offered: HashSet<&str> = offered.iter().copied().collect();
        assert_eq!(
            vouched_for(history, shell_word_start(line), &offered),
            expected
        );
    }

    /// Multi-byte characters left of the cursor must not slice the history
    /// command mid-character.
    #[rstest]
    fn confirmation_survives_a_multibyte_prefix() {
        let offered = HashSet::from(["foo.txt"]);
        let word_start = shell_word_start("echo 世界 fo");
        assert!(vouched_for("echo 世界 foo.txt", word_start, &offered));
    }

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
    #[case::escaped_space("cd My\\ Do", "My\\ Documents", Some("cd My\\ Documents"))]
    #[case::double_quoted_space("cd \"My Do", "\"My Documents\"", Some("cd \"My Documents\""))]
    #[case::single_quoted_space("cd 'My Do", "'My Documents'", Some("cd 'My Documents'"))]
    #[case::noop_completion("git checkout", "checkout", None)]
    #[case::empty_token("git ch", "", None)]
    fn splices_completion_into_line(
        #[case] line: &str,
        #[case] token: &str,
        #[case] expected: Option<&str>,
    ) {
        assert_eq!(apply_completion(line, token).as_deref(), expected);
    }

    /// Which source leads the list turns on this: on an argument the shell
    /// knows what exists now, on the command name history carries the whole
    /// line. Leading and trailing whitespace must not be mistaken for a
    /// command already typed.
    #[rstest]
    #[case::empty("", false)]
    #[case::first_word("gi", false)]
    #[case::first_word_indented("   gi", false)]
    #[case::only_whitespace("   ", false)]
    #[case::after_command("git ", true)]
    #[case::second_word("git ch", true)]
    #[case::third_word("git checkout ma", true)]
    #[case::flag("cargo test --rel", true)]
    fn knows_whether_the_cursor_is_on_an_argument(#[case] line: &str, #[case] expected: bool) {
        assert_eq!(completing_an_argument(line), expected, "{line:?}");
    }

    /// The directories history remembers do not stay put. A `cd` whose target
    /// has been renamed or deleted cannot run, so it must not be offered —
    /// while every shape the check cannot resolve is kept, because hiding a
    /// working suggestion is the worse failure.
    #[rstest]
    // Dropped: nowhere to go.
    #[case::missing("cd gone-dir", false)]
    #[case::missing_nested("cd sub/gone-dir", false)]
    #[case::missing_home_relative("cd ~/definitely-not-here-xyz", false)]
    #[case::file_not_dir("cd present.txt", false)]
    // Kept: the target is right here.
    #[case::present("cd sub", true)]
    #[case::present_trailing_slash("cd sub/", true)]
    #[case::present_nested("cd sub/deeper", true)]
    #[case::absolute("cd /", true)]
    // Kept: no target to judge.
    #[case::bare("cd", true)]
    #[case::previous("cd -", true)]
    // Kept: not a `cd` at all, so a bare word may well be a git ref.
    #[case::other_command("git checkout gone-dir", true)]
    #[case::cd_substring("cdk deploy gone-dir", true)]
    // Kept: more arguments than a plain `cd`, so this is not the shape we know.
    #[case::flags("cd -P gone-dir", true)]
    // Kept: the shell would rewrite these before opening anything.
    #[case::glob("cd gone-*", true)]
    #[case::variable("cd $PROJECT", true)]
    #[case::quoted("cd 'gone dir'", true)]
    #[case::tilde_alone("cd ~", true)]
    fn drops_cd_suggestions_whose_target_is_gone(#[case] command: &str, #[case] expected: bool) {
        use std::hash::{Hash, Hasher};

        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        command.hash(&mut hasher);
        let dir = std::env::temp_dir().join(format!(
            "atuin-cd-{}-{:x}",
            std::process::id(),
            hasher.finish()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("sub/deeper")).unwrap();
        std::fs::write(dir.join("present.txt"), "").unwrap();

        assert_eq!(cd_target_exists(command, &dir), expected, "{command:?}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[rstest]
    #[case::plain("git status", true)]
    #[case::newline("git status\nrm file", false)]
    #[case::carriage_return("git status\rrm file", false)]
    #[case::tab("git\tstatus", false)]
    #[case::escape("git \x1b[31mstatus", false)]
    #[case::delete("git \x7fstatus", false)]
    fn filters_active_control_characters(#[case] text: &str, #[case] expected: bool) {
        assert_eq!(suggestion_text_is_safe(text), expected);
    }
}
