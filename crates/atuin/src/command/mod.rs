use atuin_common::logs::LogConfig;
use clap::Subcommand;
use eyre::Result;

#[cfg(not(windows))]
use rustix::{fs::Mode, process::umask};

#[cfg(feature = "client")]
mod client;

mod contributors;

mod gen_completions;

mod external;

#[derive(Subcommand)]
#[command(infer_subcommands = true)]
#[allow(clippy::large_enum_variant)]
pub enum AtuinCmd {
    #[cfg(feature = "client")]
    #[command(flatten)]
    Client(client::Cmd),

    /// PTY proxy for atuin
    #[cfg(feature = "pty-proxy")]
    #[command(alias = "hex")]
    PtyProxy(atuin_pty_proxy::PtyProxy),

    /// Generate a UUID
    Uuid,

    Contributors,

    /// Generate shell completions
    GenCompletions(gen_completions::Cmd),

    #[command(external_subcommand)]
    External(Vec<String>),
}

impl AtuinCmd {
    pub fn run(self) -> Result<()> {
        // set umask before we potentially open/create files
        // or in other words, 077. Do not allow any access to any other user.
        // Keep the previous umask so pty-proxy can restore it in the shell it
        // spawns — the shell must not inherit ours (#3695).
        #[cfg(not(windows))]
        let prev_umask = umask(Mode::RWXG | Mode::RWXO);

        match self {
            // Client commands initialize their own logging
            #[cfg(feature = "client")]
            Self::Client(_) => {}
            _ => crate::logs::init_logging(&LogConfig::stderr_only()),
        }

        match self {
            #[cfg(feature = "client")]
            Self::Client(client) => client.run(),

            #[cfg(all(feature = "pty-proxy", unix))]
            Self::PtyProxy(proxy) => {
                run_pty_proxy(proxy, prev_umask);
                Ok(())
            }

            #[cfg(all(feature = "pty-proxy", not(unix)))]
            Self::PtyProxy(_) => {
                eprintln!("atuin pty-proxy currently supports unix platforms");
                std::process::exit(1);
            }

            Self::Contributors => {
                contributors::run();
                Ok(())
            }
            Self::Uuid => {
                println!("{}", atuin_common::utils::uuid_v7().as_simple());
                Ok(())
            }
            Self::GenCompletions(gen_completions) => gen_completions.run(),
            Self::External(args) => external::run(&args),
        }
    }
}

#[cfg(all(feature = "pty-proxy", unix))]
fn run_pty_proxy(proxy: atuin_pty_proxy::PtyProxy, prev_umask: Mode) {
    // `Mode::bits()` returns u16 on macOS/BSD but u32 on Linux, where this
    // conversion is a no-op.
    #[allow(clippy::useless_conversion)]
    let child_umask = Some(u32::from(prev_umask.bits()));

    #[cfg(feature = "daemon")]
    let command_capture_sink = semantic_command_capture_sink();
    #[cfg(not(feature = "daemon"))]
    let command_capture_sink = None;

    #[cfg(feature = "client")]
    let suggestion_provider = history_suggestion_provider();
    #[cfg(not(feature = "client"))]
    let suggestion_provider = None;

    proxy.run(atuin_pty_proxy::RunOptions {
        command_capture_sink,
        suggestion_provider,
        child_umask,
    });
}

/// How long the popup waits for the suggestion worker before giving up, so
/// a slow backend can never wedge the proxy's UI.
#[cfg(all(feature = "client", feature = "pty-proxy", unix))]
const SUGGEST_REPLY_TIMEOUT: std::time::Duration = std::time::Duration::from_millis(250);

/// Queued queries beyond this are dropped rather than backing up behind a
/// slow backend.
#[cfg(all(feature = "client", feature = "pty-proxy", unix))]
const SUGGEST_QUEUE_DEPTH: usize = 8;

/// Prefix completions from history for the pty-proxy popup. Experimental:
/// gated on `suggest.enabled`. Daemon index when enabled, sqlite prefix
/// search otherwise; the backend lives on its own thread like
/// [`semantic_command_capture_sink`].
#[cfg(all(feature = "client", feature = "pty-proxy", unix))]
fn history_suggestion_provider() -> Option<atuin_pty_proxy::SuggestionProvider> {
    use std::sync::mpsc;

    let settings = atuin_client::settings::Settings::new().ok()?;
    if !settings.suggest.enabled {
        return None;
    }

    let min_chars = settings.suggest.min_chars.max(1);
    let (req_tx, req_rx) =
        mpsc::sync_channel::<(String, mpsc::Sender<Vec<String>>)>(SUGGEST_QUEUE_DEPTH);

    std::thread::spawn(move || suggestion_worker(settings, req_rx));

    Some(Box::new(move |line: &str| {
        // take(min_chars) keeps the length check O(min_chars), not O(line).
        if line.chars().take(min_chars).count() < min_chars {
            return Vec::new();
        }
        let (reply_tx, reply_rx) = mpsc::channel();
        if req_tx.try_send((line.to_string(), reply_tx)).is_err() {
            return Vec::new();
        }
        reply_rx
            .recv_timeout(SUGGEST_REPLY_TIMEOUT)
            .unwrap_or_default()
    }))
}

#[cfg(all(feature = "client", feature = "pty-proxy", unix))]
fn suggestion_worker(
    settings: atuin_client::settings::Settings,
    req_rx: std::sync::mpsc::Receiver<(String, std::sync::mpsc::Sender<Vec<String>>)>,
) {
    let Ok(runtime) = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    else {
        return;
    };

    let mut backend = SuggestionBackend::new(settings);

    for (query, reply_tx) in req_rx {
        let results = runtime.block_on(backend.fetch(&query));

        // Each newline typed into the pty would submit the line so far; the
        // daemon filters multiline already, this covers the sqlite fallback.
        let commands = results
            .into_iter()
            .filter(|command| !command.contains('\n'))
            .collect();
        let _ = reply_tx.send(commands);
    }
}

/// How long one completion-oracle call may take before it's abandoned;
/// must fit inside [`SUGGEST_REPLY_TIMEOUT`] alongside the history lookup.
#[cfg(all(feature = "client", feature = "pty-proxy", unix))]
const COMPLETION_TIMEOUT: std::time::Duration = std::time::Duration::from_millis(150);

/// A wedged zsh oracle is killed and respawned this many times before
/// completions are given up on for the session.
#[cfg(all(feature = "client", feature = "pty-proxy", unix))]
const ORACLE_RESPAWN_LIMIT: u32 = 3;

/// The engine answering shell-completion queries. Both run headless: zsh as
/// a captive interactive process under a pty, fish via `complete -C`.
#[cfg(all(feature = "client", feature = "pty-proxy", unix))]
enum CompletionOracle {
    Zsh {
        zsh: std::path::PathBuf,
        proc: Option<atuin_pty_proxy::ZshOracle>,
        spawns: u32,
    },
    Fish(std::path::PathBuf),
    None,
}

#[cfg(all(feature = "client", feature = "pty-proxy", unix))]
impl CompletionOracle {
    /// Match the user's shell where we can (zsh gets real compsys answers,
    /// fish its own engine); otherwise any engine beats none.
    fn detect() -> Self {
        let find = |name: &str| {
            std::env::var_os("PATH").and_then(|path| {
                std::env::split_paths(&path)
                    .map(|dir| dir.join(name))
                    .find(|candidate| candidate.is_file())
            })
        };
        let user_shell = std::env::var("SHELL").unwrap_or_default();
        let user_shell = user_shell.rsplit('/').next().unwrap_or_default();

        let zsh = || {
            find("zsh").map(|zsh| CompletionOracle::Zsh {
                zsh,
                proc: None,
                spawns: 0,
            })
        };
        let fish = || find("fish").map(CompletionOracle::Fish);

        match user_shell {
            "zsh" => zsh().or_else(fish),
            _ => fish().or_else(zsh),
        }
        .unwrap_or(CompletionOracle::None)
    }
}

/// Lazily connected suggestion backends: history (daemon index first, sqlite
/// fallback), topped up with shell completions from an oracle when available.
#[cfg(all(feature = "client", feature = "pty-proxy", unix))]
struct SuggestionBackend {
    settings: atuin_client::settings::Settings,
    #[cfg(feature = "daemon")]
    daemon: Option<atuin_daemon::client::SearchClient>,
    local: Option<(
        atuin_client::database::Sqlite,
        atuin_client::database::Context,
    )>,
    oracle: CompletionOracle,
}

#[cfg(all(feature = "client", feature = "pty-proxy", unix))]
impl SuggestionBackend {
    fn new(settings: atuin_client::settings::Settings) -> Self {
        Self {
            settings,
            #[cfg(feature = "daemon")]
            daemon: None,
            local: None,
            oracle: CompletionOracle::detect(),
        }
    }

    async fn fetch(&mut self, query: &str) -> Vec<String> {
        let mut suggestions = self.fetch_history(query).await;
        // History first — it's ranked and personal; completions top up below
        // so the ghost stays a command you've actually run when one matches.
        for completion in self.fetch_completions(query).await {
            if !suggestions.contains(&completion) {
                suggestions.push(completion);
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

    /// Shell completions for the line's last token, returned as whole lines
    /// (`git ch` + `checkout` → `git checkout`) so the popup, ghost text,
    /// and accept treat them exactly like history suggestions.
    async fn fetch_completions(&mut self, line: &str) -> Vec<String> {
        let candidates = match &mut self.oracle {
            CompletionOracle::Zsh { zsh, proc, spawns } => {
                if proc.is_none() && *spawns < ORACLE_RESPAWN_LIMIT {
                    *spawns += 1;
                    *proc = atuin_pty_proxy::ZshOracle::spawn(zsh);
                }
                let Some(oracle) = proc.as_mut() else {
                    return Vec::new();
                };
                let Some(candidates) = oracle.complete(line, COMPLETION_TIMEOUT) else {
                    // Desynced or dead; a fresh oracle answers next query.
                    *proc = None;
                    return Vec::new();
                };
                candidates
            }
            CompletionOracle::Fish(fish) => fish_complete(fish, line).await,
            CompletionOracle::None => return Vec::new(),
        };

        candidates
            .iter()
            .filter_map(|candidate| apply_completion(line, candidate))
            .take(self.settings.suggest.limit as usize)
            .collect()
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

/// fish's engine runs headless by design: `--do-complete=$argv[1]` keeps the
/// user's line out of fish's parser — it arrives as an argument, never code.
#[cfg(all(feature = "client", feature = "pty-proxy", unix))]
async fn fish_complete(fish: &std::path::Path, line: &str) -> Vec<String> {
    let output = tokio::time::timeout(
        COMPLETION_TIMEOUT,
        tokio::process::Command::new(fish)
            .args(["--no-config", "-c", "complete --do-complete=$argv[1]"])
            .arg(line)
            .stdin(std::process::Stdio::null())
            .kill_on_drop(true)
            .output(),
    )
    .await;
    let Ok(Ok(output)) = output else {
        return Vec::new();
    };
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .map(str::to_owned)
        .collect()
}

/// Splice one oracle candidate line (`token\tdescription`) back into the
/// command line by replacing its last whitespace-separated token. Whole-line
/// form keeps completions prefix-extensions of the typed line, which is what
/// the ghost text and accept paths expect.
#[cfg(all(feature = "client", feature = "pty-proxy", unix))]
fn apply_completion(line: &str, candidate: &str) -> Option<String> {
    let token = candidate.split('\t').next().unwrap_or_default();
    if token.is_empty() {
        return None;
    }
    let token_start = line
        .rfind(char::is_whitespace)
        .map_or(0, |position| position + 1);
    let completed = format!("{}{}", &line[..token_start], token);
    (completed != line).then_some(completed)
}

#[cfg(all(feature = "client", feature = "pty-proxy", unix, test))]
mod suggestion_tests {
    use super::apply_completion;
    use rstest::rstest;

    #[rstest]
    #[case::subcommand("git ch", "checkout\tCheckout a branch", Some("git checkout"))]
    #[case::flag(
        "git status --sh",
        "--short\tGive output in short format",
        Some("git status --short")
    )]
    #[case::first_token("gi", "git\tdistributed VCS", Some("git"))]
    #[case::after_trailing_space("git ", "checkout", Some("git checkout"))]
    #[case::noop_completion("git checkout", "checkout", None)]
    #[case::empty_candidate("git ch", "", None)]
    fn splices_completion_into_line(
        #[case] line: &str,
        #[case] candidate: &str,
        #[case] expected: Option<&str>,
    ) {
        assert_eq!(apply_completion(line, candidate).as_deref(), expected);
    }
}

#[cfg(all(feature = "daemon", feature = "pty-proxy", unix))]
fn semantic_command_capture_sink() -> Option<atuin_pty_proxy::CommandCaptureSink> {
    use std::sync::mpsc;
    use std::time::Duration;

    if is_truthy_env("ATUIN_TERMINAL") {
        return None;
    }

    let settings = atuin_client::settings::Settings::new().ok()?;
    let (tx, rx) = mpsc::sync_channel::<atuin_pty_proxy::CommandCapture>(128);

    std::thread::spawn(move || {
        let Ok(runtime) = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
        else {
            return;
        };

        while let Ok(first) = rx.recv() {
            let mut batch = vec![first];

            while batch.len() < 64 {
                match rx.recv_timeout(Duration::from_millis(25)) {
                    Ok(capture) => batch.push(capture),
                    Err(mpsc::RecvTimeoutError::Timeout | mpsc::RecvTimeoutError::Disconnected) => {
                        break;
                    }
                }
            }

            runtime.block_on(send_semantic_command_captures(&settings, batch));
        }
    });

    Some(Box::new(move |capture| {
        let _ = tx.try_send(capture);
    }))
}

#[cfg(all(feature = "daemon", feature = "pty-proxy", unix))]
#[inline]
fn is_truthy_env(name: &str) -> bool {
    std::env::var(name)
        .ok()
        .as_ref()
        .is_some_and(|value| !value.trim().is_empty() && value.trim() != "false")
}

#[cfg(all(feature = "daemon", feature = "pty-proxy", unix))]
async fn send_semantic_command_captures(
    settings: &atuin_client::settings::Settings,
    batch: Vec<atuin_pty_proxy::CommandCapture>,
) {
    let captures = batch
        .into_iter()
        .map(|capture| atuin_daemon::semantic::CommandCapture {
            prompt: capture.prompt,
            command: capture.command,
            output: capture.output,
            exit_code: capture.exit_code,
            history_id: capture.history_id,
            session_id: capture.session_id,
            output_truncated: capture.output_truncated,
            output_observed_bytes: capture.output_observed_bytes,
        })
        .collect();

    if let Ok(mut client) = atuin_daemon::SemanticClient::from_settings(settings).await {
        let _ = client.record_commands(captures).await;
    }
}
