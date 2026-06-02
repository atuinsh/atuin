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
        #[cfg(not(windows))]
        {
            // set umask before we potentially open/create files
            // or in other words, 077. Do not allow any access to any other user
            let mode = Mode::RWXG | Mode::RWXO;
            umask(mode);
        }

        match self {
            #[cfg(feature = "client")]
            Self::Client(client) => client.run(),

            #[cfg(feature = "pty-proxy")]
            Self::PtyProxy(proxy) => {
                run_pty_proxy(proxy);
                Ok(())
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
fn run_pty_proxy(proxy: atuin_pty_proxy::PtyProxy) {
    #[cfg(feature = "daemon")]
    proxy.run(semantic_command_capture_sink());

    #[cfg(not(feature = "daemon"))]
    proxy.run(None);
}

#[cfg(all(feature = "pty-proxy", not(unix)))]
fn run_pty_proxy(_proxy: atuin_pty_proxy::PtyProxy) {
    eprintln!("atuin pty-proxy currently supports unix platforms");
    std::process::exit(1);
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
