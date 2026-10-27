use std::fmt;
use std::io::{self, Write as _};
use std::sync::Arc;

use atuin_client::database::Sqlite;
use atuin_client::record::sqlite_store::SqliteStore;
use atuin_client::settings::Settings;
use atuin_common::logs::USER;
use atuin_common::time::{OffsetDateTimeExt, UtcOffsetExt};
use atuin_domain::record::{
    EncryptedData, HostId, Record, RecordId, RecordIdx, RecordSeriesKey, RecordTag,
};
use clap::Subcommand;
use eyre::{Result, WrapErr as _};
use itertools::Itertools;
use time::{OffsetDateTime, UtcOffset};
use tracing::{info, instrument, warn};

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
pub async fn invalidate_ai_sessions(settings: &Settings) {
    if let Err(err) = atuin_client::ai_session::invalidate_sidecar(settings).await {
        warn!(target: USER, "failed to schedule a rebuild of the threads index: {err}");
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
                "Threads need the daemon: enable it (`enabled = true` under `[daemon]` in the \
                 config) to rebuild the threads index"
            );
        }
        daemon::rebuild_ai_sessions(settings)
            .await
            .wrap_err("the daemon could not rebuild the threads index")
    }
    #[cfg(not(feature = "daemon"))]
    {
        let _ = settings;
        eyre::bail!("Threads need the daemon, which this build of atuin does not include");
    }
}

/// [`reset_ai_sessions`] for commands that have already deleted records: a failure says the
/// index is now stale, and how to fix it. Nothing to do when there is no index yet: the daemon
/// builds it from the records as they are now.
pub async fn reset_ai_sessions_after(settings: &Settings) -> Result<()> {
    if !atuin_client::ai_session::sidecar_path(settings).exists() {
        return Ok(());
    }
    reset_ai_sessions(settings).await.wrap_err(
        "the records changed, but the threads index was not rebuilt and still lists what they \
         held: run `atuin store rebuild threads` once the daemon is enabled and running",
    )
}

impl Cmd {
    #[instrument(level = "trace", skip_all, err)]
    pub async fn run(
        &self,
        settings: &Arc<Settings>,
        database: &Sqlite,
        store: SqliteStore,
    ) -> Result<()> {
        match self {
            Self::Status => self.status(settings, store).await,
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

                info!(target: USER, "Rewrote {rewritten} records");
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

    pub async fn status(&self, settings: &Settings, store: SqliteStore) -> Result<()> {
        let status = StoreStatus::query(settings, &store).await?;
        // Each host's block already ends with a blank line.
        write!(io::stdout(), "{status}")?;
        Ok(())
    }
}

/// What `atuin store status` shows: each host's record series, with their first and last records.
struct StoreStatus {
    /// Sorted by host, and each host's series by tag.
    hosts: Vec<HostStatus>,
}

struct HostStatus {
    host: HostId,
    /// Whether it is the machine running the command.
    current: bool,
    series: Vec<SeriesStatus>,
}

struct SeriesStatus {
    tag: RecordTag,
    idx: RecordIdx,
    first: Option<SeriesEnd>,
    last: Option<SeriesEnd>,
}

/// The record at one end of a series.
struct SeriesEnd {
    id: RecordId,
    created: OffsetDateTime,
}

impl StoreStatus {
    async fn query(settings: &Settings, store: &SqliteStore) -> Result<Self> {
        let host_id = settings.host_id().await?;
        let offset = UtcOffset::local_or_utc();

        let mut hosts = Vec::new();
        for (host, tags) in store.status().await?.hosts.into_iter().sorted_by_key(|(h, _)| *h) {
            let mut series = Vec::new();
            for (tag, idx) in tags.into_iter().sorted_by(|(a, _), (b, _)| a.cmp(b)) {
                let key = RecordSeriesKey::new(host, tag.clone());
                series.push(SeriesStatus {
                    first: store.first(&key).await?.map(|r| SeriesEnd::new(&r, offset)),
                    last: store.last(&key).await?.map(|r| SeriesEnd::new(&r, offset)),
                    tag,
                    idx,
                });
            }
            hosts.push(HostStatus {
                current: host == host_id,
                host,
                series,
            });
        }

        Ok(Self { hosts })
    }
}

impl SeriesEnd {
    fn new(record: &Record<EncryptedData>, offset: UtcOffset) -> Self {
        Self {
            id: record.id,
            created: OffsetDateTime::from_unix_nanos_u64(record.timestamp).to_offset(offset),
        }
    }
}

impl fmt::Display for StoreStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for host in &self.hosts {
            let current = if host.current {
                " <- CURRENT HOST"
            } else {
                ""
            };
            writeln!(f, "host: {}{current}", host.host.0.as_hyphenated())?;

            for series in &host.series {
                writeln!(f, "\tstore: {}", series.tag)?;
                writeln!(f, "\t\tidx: {}", series.idx)?;
                for (end, record) in [("first", &series.first), ("last", &series.last)] {
                    let Some(record) = record else {
                        continue;
                    };
                    writeln!(f, "\t\t{end}: {}", record.id.0.as_hyphenated())?;
                    writeln!(f, "\t\t\tcreated: {}", record.created)?;
                }
            }

            writeln!(f)?;
        }

        Ok(())
    }
}
