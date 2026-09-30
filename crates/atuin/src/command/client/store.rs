use atuin_client::database::Sqlite;
use atuin_client::record::sqlite_store::SqliteStore;
use atuin_client::settings::Settings;
use atuin_common::time::{OffsetDateTimeExt, UtcOffsetExt};
use atuin_domain::record::RecordSeriesKey;
use clap::Subcommand;
use eyre::{Result, WrapErr as _};
use itertools::Itertools;
use time::OffsetDateTime;
use tracing::instrument;

#[cfg(feature = "daemon")]
use crate::command::client::daemon;
use crate::i18n::fl;

#[cfg(feature = "sync")]
mod push;

#[cfg(feature = "sync")]
mod pull;

mod purge;
mod rebuild;
mod rekey;
mod verify;

#[derive(Subcommand, Debug)]
#[command(infer_subcommands = true)]
pub enum Cmd {
    #[command(about = fl!("cmd-store-status"))]
    Status,

    #[command(about = fl!("cmd-store-rebuild"))]
    Rebuild(rebuild::Rebuild),

    #[command(about = fl!("cmd-store-rekey"))]
    Rekey(rekey::Rekey),

    #[command(about = fl!("cmd-store-compact"))]
    Compact,

    #[command(about = fl!("cmd-store-purge"))]
    Purge(purge::Purge),

    #[command(about = fl!("cmd-store-verify"))]
    Verify(verify::Verify),

    #[command(about = fl!("cmd-store-push"))]
    #[cfg(feature = "sync")]
    Push(push::Push),

    #[command(about = fl!("cmd-store-pull"))]
    #[cfg(feature = "sync")]
    Pull(pull::Pull),
}

/// Have the daemon reproject the ai-session sidecar in full on its next start, after a command
/// re-encrypted the records under it: they say what they said, so what is projected stays and
/// the replay only has to cover it again with the new key. Best effort: the maintenance itself
/// has already happened.
pub async fn invalidate_ai_sessions() {
    if let Err(err) = atuin_client::ai_session::invalidate_sidecar().await {
        eprintln!("Failed to schedule a rebuild of the ai session index: {err}");
    }
}

/// Have the daemon delete the ai-session index and rebuild it from the record store alone, after
/// a command deleted records under it (or asked for a rebuild): a replay only adds, so what the
/// deleted records projected would otherwise stay.
///
/// Only the daemon does it, started for it as for the other AI session commands: it owns the
/// index, rebuilds at once, reports the rebuild to readers meanwhile, and holds capture while its
/// dedup gate cannot be trusted. Deleting the file from here could leave a running daemon serving
/// an empty index, and capture pushing duplicate records. So when the daemon is disabled, or
/// cannot be reached or asked, this fails and nothing is touched.
#[cfg_attr(
    not(feature = "daemon"),
    expect(clippy::unused_async, reason = "only the daemon rebuilds")
)]
pub async fn reset_ai_sessions(settings: &Settings) -> Result<()> {
    #[cfg(feature = "daemon")]
    {
        if !settings.daemon.enabled {
            eyre::bail!(
                "AI sessions need the daemon: enable it (`enabled = true` under `[daemon]` in the \
                 config) to rebuild the AI session index"
            );
        }
        daemon::rebuild_ai_sessions(settings)
            .await
            .wrap_err("the daemon could not rebuild the AI session index")
    }
    #[cfg(not(feature = "daemon"))]
    {
        let _ = settings;
        eyre::bail!("AI sessions need the daemon, which this build of atuin does not include");
    }
}

/// [`reset_ai_sessions`] for commands that have already deleted records: a failure says the
/// index is now stale, and how to fix it. Nothing to do when there is no index yet: the daemon
/// builds it from the records as they are now.
pub async fn reset_ai_sessions_after(settings: &Settings) -> Result<()> {
    if !atuin_client::ai_session::sidecar_path().exists() {
        return Ok(());
    }
    reset_ai_sessions(settings).await.wrap_err(
        "the records changed, but the AI session index was not rebuilt and still lists what they \
         held: run `atuin store rebuild ai-session` once the daemon is enabled and running",
    )
}

impl Cmd {
    #[instrument(level = "trace", skip_all, err)]
    pub async fn run(
        &self,
        settings: &Settings,
        database: &Sqlite,
        store: SqliteStore,
    ) -> Result<()> {
        match self {
            Self::Status => self.status(store).await,
            Self::Rebuild(rebuild) => rebuild.run(settings, store, database).await,
            Self::Rekey(rekey) => rekey.run(settings, store).await,
            Self::Compact => {
                // The daemon owns the store's writes when enabled; let it do the rewrite so the
                // binary that reads the new rows is the one that wrote them.
                #[cfg(feature = "daemon")]
                let rewritten = if settings.daemon.enabled {
                    daemon::compact_store(settings).await?
                } else {
                    store.compact().await?
                };
                #[cfg(not(feature = "daemon"))]
                let rewritten = store.compact().await?;

                println!("Rewrote {rewritten} records");
                Ok(())
            }
            Self::Verify(verify) => verify.run(settings, store).await,
            Self::Purge(purge) => purge.run(settings, store).await,

            #[cfg(feature = "sync")]
            Self::Push(push) => push.run(settings, store).await,

            #[cfg(feature = "sync")]
            Self::Pull(pull) => pull.run(settings, store, database).await,
        }
    }

    pub async fn status(&self, store: SqliteStore) -> Result<()> {
        let host_id = Settings::host_id().await?;
        let offset = time::UtcOffset::local_or_utc();

        let status = store.status().await?;

        // TODO: should probs build some data structure and then pretty-print it or smth
        for (host, st) in status.hosts.iter().sorted_by_key(|(h, _)| *h) {
            let host_string = if host == &host_id {
                format!("host: {} <- CURRENT HOST", host.0.as_hyphenated())
            } else {
                format!("host: {}", host.0.as_hyphenated())
            };

            println!("{host_string}");

            for (tag, idx) in st.iter().sorted_by_key(|(tag, _)| *tag) {
                println!("\tstore: {tag}");

                let series = RecordSeriesKey::new(*host, tag.clone());
                let first = store.first(&series).await?;
                let last = store.last(&series).await?;

                println!("\t\tidx: {idx}");

                if let Some(first) = first {
                    println!("\t\tfirst: {}", first.id.0.as_hyphenated());

                    let time =
                        OffsetDateTime::from_unix_nanos_u64(first.timestamp).to_offset(offset);
                    println!("\t\t\tcreated: {time}");
                }

                if let Some(last) = last {
                    println!("\t\tlast: {}", last.id.0.as_hyphenated());

                    let time =
                        OffsetDateTime::from_unix_nanos_u64(last.timestamp).to_offset(offset);
                    println!("\t\t\tcreated: {time}");
                }
            }

            println!();
        }

        Ok(())
    }
}
