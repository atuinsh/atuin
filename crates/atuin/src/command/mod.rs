use atuin_common::logs::LogConfig;
use clap::Subcommand;
use clap_couture::Couture;
use eyre::Result;
#[cfg(not(windows))]
use rustix::{fs::Mode, process::umask};

use crate::i18n::fl;

#[cfg(feature = "client")]
mod client;

mod contributors;

mod gen_completions;

mod external;

#[derive(Subcommand, Couture)]
#[command(infer_subcommands = true)]
#[couture(categories = {
    "ai" = { title = fl!("cli-category-ai") },
    "sync" = {
        title = fl!("cli-category-sync"),
        description = fl!("cli-category-sync-description"),
    },
    "data" = {
        title = fl!("cli-category-data"),
        description = fl!("cli-category-data-description"),
    },
    "setup" = {
        title = fl!("cli-category-setup"),
        description = fl!("cli-category-setup-description"),
    },
    "advanced" = { title = fl!("cli-category-advanced") },
})]
#[allow(clippy::large_enum_variant)]
pub enum AtuinCmd {
    #[cfg(feature = "client")]
    #[command(flatten)]
    Client(client::Cmd),

    #[cfg(feature = "pty-proxy")]
    #[command(alias = "hex", about = fl!("cmd-pty-proxy"))]
    #[category("advanced")]
    PtyProxy(atuin_pty_proxy::PtyProxy),

    // Plumbing: every shell init calls `atuin uuid` to seed ATUIN_SESSION; not for interactive use.
    #[command(hide = true, about = fl!("cmd-uuid"))]
    Uuid,

    #[command(about = fl!("cmd-contributors"))]
    #[category("advanced")]
    Contributors,

    #[command(about = fl!("cmd-gen-completions"))]
    #[category("setup")]
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

        let _log_guard = match &self {
            #[cfg(feature = "client")]
            Self::Client(_) => None,
            _ => Some(crate::logs::LogCtx::try_enable("atuin", &LogConfig::stderr_only())?),
        };

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
    proxy.run(semantic_command_capture_config(), child_umask);

    #[cfg(not(feature = "daemon"))]
    proxy.run(None, child_umask);
}

#[cfg(all(feature = "daemon", feature = "pty-proxy", unix))]
fn semantic_command_capture_config() -> Option<atuin_pty_proxy::CaptureConfig> {
    use std::borrow::Cow;
    use std::sync::mpsc;

    if is_truthy_env("ATUIN_TERMINAL") {
        return None;
    }

    let settings = atuin_client::settings::Settings::new().ok()?;
    let max_output_bytes =
        usize::try_from(settings.output.limits()?.max_output_size.as_u64()).unwrap_or(usize::MAX);
    let (tx, rx) = mpsc::sync_channel::<(
        atuin_client::history::HistoryId,
        atuin_pty_proxy::CommandCapture,
    )>(128);

    std::thread::spawn(move || {
        let Ok(runtime) = tokio::runtime::Builder::new_current_thread().enable_all().build() else {
            return;
        };

        runtime.block_on(async move {
            let Ok(mut client) = atuin_daemon::HistoryClient::from_settings(&settings).await else {
                return;
            };

            let redactor = settings.security.redactor(settings.secrets_filter);
            while let Ok((history_id, capture)) = rx.recv() {
                // Output can carry credentials the command line never showed, e.g. `cat .env`.
                // Swap the string only when something was actually taken out, so that clean
                // output -- nearly all of it -- reaches the daemon without being copied. Output
                // that can't be redacted quickly is not kept at all.
                let redact = |output: &mut String| {
                    match redactor.redact_within(output, atuin_common::secrets::REDACT_BUDGET) {
                        Some(Cow::Borrowed(_)) => true,
                        Some(Cow::Owned(redacted)) => {
                            *output = redacted;
                            true
                        }
                        None => false,
                    }
                };

                let mut output_start = capture.output_start;
                let mut output_end = capture.output_end;
                let redacted = redact(&mut output_start)
                    && output_end.as_mut().is_none_or(redact);
                if !redacted {
                    tracing::debug!(%history_id, "output took too long to redact; dropping it");
                    continue;
                }

                if let Err(err) = client
                    .register_command_output(
                        history_id,
                        output_start,
                        output_end,
                        capture.output_observed_bytes,
                        capture.terminal_width,
                        capture.terminal_height,
                    )
                    .await
                {
                    tracing::debug!(%history_id, ?err, "could not record command output; dropping it");
                }
            }
        });
    });

    let sink: atuin_pty_proxy::CommandCaptureSink = Box::new(move |history_id, capture| {
        let _ = tx.try_send((history_id, capture));
    });

    Some(atuin_pty_proxy::CaptureConfig {
        sink,
        max_output_bytes,
    })
}

#[cfg(all(feature = "daemon", feature = "pty-proxy", unix))]
#[inline]
fn is_truthy_env(name: &str) -> bool {
    std::env::var(name)
        .ok()
        .as_ref()
        .is_some_and(|value| !value.trim().is_empty() && value.trim() != "false")
}
