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

    #[cfg(feature = "client")]
    let suggestions = history_suggestion_provider();
    #[cfg(not(feature = "client"))]
    let suggestions = None;

    #[cfg(feature = "daemon")]
    proxy.run(semantic_command_capture_sink(), suggestions, child_umask);

    #[cfg(not(feature = "daemon"))]
    proxy.run(None, suggestions, child_umask);
}

/// Suggestion popup provider for `atuin pty-proxy`: prefix completions from
/// history, best first. Experimental: only active with
/// `pty_proxy.suggestions = true` in the config or `ATUIN_PTY_PROXY_SUGGEST=1`
/// in the environment.
///
/// Served by the daemon's in-memory search index when the daemon is enabled
/// (frecency-ranked, no database access per keystroke), falling back to a
/// prefix search of the local sqlite database otherwise.
///
/// The backend lives on its own thread with a small runtime, mirroring
/// [`semantic_command_capture_sink`]; the returned closure just does a
/// bounded request/reply so a slow query can never wedge the proxy's UI.
#[cfg(all(feature = "client", feature = "pty-proxy", unix))]
fn history_suggestion_provider() -> Option<atuin_pty_proxy::SuggestionProvider> {
    use std::sync::mpsc;
    use std::time::Duration;

    let settings = atuin_client::settings::Settings::new().ok()?;
    if !settings.pty_proxy.suggestions && !is_truthy_env("ATUIN_PTY_PROXY_SUGGEST") {
        return None;
    }

    let min_chars = usize::try_from(settings.pty_proxy.suggestions_min_chars).unwrap_or(1);
    let (req_tx, req_rx) = mpsc::sync_channel::<(String, mpsc::Sender<Vec<String>>)>(8);

    std::thread::spawn(move || suggestion_worker(&settings, &req_rx));

    Some(Box::new(move |line: &str| {
        if line.chars().count() < min_chars.max(1) {
            return Vec::new();
        }
        let (reply_tx, reply_rx) = mpsc::channel();
        if req_tx.try_send((line.to_string(), reply_tx)).is_err() {
            return Vec::new();
        }
        reply_rx
            .recv_timeout(Duration::from_millis(250))
            .unwrap_or_default()
    }))
}

#[cfg(all(feature = "client", feature = "pty-proxy", unix))]
fn suggestion_worker(
    settings: &atuin_client::settings::Settings,
    req_rx: &std::sync::mpsc::Receiver<(String, std::sync::mpsc::Sender<Vec<String>>)>,
) {
    let Ok(runtime) = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    else {
        return;
    };

    let limit = u32::try_from(settings.pty_proxy.suggestions_limit).unwrap_or(8);
    let mut backend = SuggestionBackend::default();

    while let Ok((query, reply_tx)) = req_rx.recv() {
        let results = runtime.block_on(backend.fetch(settings, &query, limit));

        // Multiline commands can't be typed into the pty safely — each
        // newline would submit the line so far. The daemon filters them
        // already; this also covers the sqlite fallback.
        let commands = results
            .into_iter()
            .filter(|command| !command.contains('\n'))
            .collect();
        let _ = reply_tx.send(commands);
    }
}

/// Lazily connected suggestion backends: the daemon's search index first,
/// the local sqlite database as fallback.
#[cfg(all(feature = "client", feature = "pty-proxy", unix))]
#[derive(Default)]
struct SuggestionBackend {
    #[cfg(feature = "daemon")]
    daemon: Option<atuin_daemon::client::SearchClient>,
    local: Option<(
        atuin_client::database::Sqlite,
        atuin_client::database::Context,
    )>,
}

#[cfg(all(feature = "client", feature = "pty-proxy", unix))]
impl SuggestionBackend {
    async fn fetch(
        &mut self,
        settings: &atuin_client::settings::Settings,
        query: &str,
        limit: u32,
    ) -> Vec<String> {
        // Prefer the daemon: its in-memory index answers prefix queries
        // without touching sqlite and ranks with the same frecency scores
        // as interactive search.
        #[cfg(feature = "daemon")]
        if settings.daemon.enabled {
            if self.daemon.is_none() {
                self.daemon =
                    atuin_daemon::client::SearchClient::new(settings.daemon.socket_path.clone())
                        .await
                        .ok();
            }
            if let Some(client) = self.daemon.as_mut() {
                match client.suggest(query, limit).await {
                    Ok(commands) => return commands,
                    // Drop the connection and fall through to sqlite for
                    // this query; the next one retries the daemon.
                    Err(_) => self.daemon = None,
                }
            }
        }

        self.fetch_local(settings, query, limit).await
    }

    async fn fetch_local(
        &mut self,
        settings: &atuin_client::settings::Settings,
        query: &str,
        limit: u32,
    ) -> Vec<String> {
        use atuin_client::database::{Database, DbSearchMode, OptFilters, Sqlite, query_context};
        use atuin_client::settings::FilterMode;

        if self.local.is_none() {
            let Ok(db) = Sqlite::new(&settings.db_path, settings.local_timeout).await else {
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
                limit: Some(i64::from(limit)),
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

#[cfg(all(any(feature = "daemon", feature = "client"), feature = "pty-proxy", unix))]
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
