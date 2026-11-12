//! Atuin for Teams in `atuin setup`: turned on once sign-in has said the account has it.

use std::sync::Arc;

use atuin_client::history::HistoryId;
use atuin_client::record::sqlite_store::SqliteStore;
use atuin_client::record::sync::{ClientSource, SyncSession};
use atuin_client::settings::Settings;
use atuin_daemon::service::pidfile::Pidfile;
use atuin_daemon::service::{ActiveService, ExternalManager, RestartError};
use atuin_domain::caps::OctavoCap;
use atuin_domain::record::{HostId, RecordIdx, RecordStatus, RecordTag};
use atuin_octavo::hub::{HubCallError, HubClient, UserId};
use atuin_octavo::pb;
use atuin_octavo::queue::UploadQueue;
use eyre::{Result, WrapErr as _};
use itertools::Itertools as _;
use toml_edit::Value;

use super::{Status, Step, Updates, count, names, send};

/// Turn Atuin for Teams on, if the account has it, and upload what Atuin already has here if
/// `backfill`.
pub(super) async fn enable(
    settings: &Arc<Settings>,
    store: &SqliteStore,
    backfill: bool,
    tx: &Updates,
) {
    send(tx, Step::Teams, Status::Running("Turning on Atuin for Teams".into()));
    let status = match turn_on(settings, store, backfill).await {
        Ok(status) => status,
        Err(e) => Status::Failed(format!(
            "Couldn't turn on Atuin for Teams ({e:#}). Run `atuin setup --teams` to try again"
        )),
    };
    send(tx, Step::Teams, status);
}

/// How turning it on went: `Err` for a failure that trying again may fix.
async fn turn_on(settings: &Arc<Settings>, store: &SqliteStore, backfill: bool) -> Result<Status> {
    let login = match HubClient::new(settings)?.login(settings).await {
        Ok(login) => login,
        Err(HubCallError::NotLoggedIn) => {
            return Ok(Status::Failed(
                "Atuin for Teams needs you signed in: run `atuin login`, then `atuin setup \
                 --teams`"
                    .into(),
            ));
        }
        Err(err) => return Err(err).wrap_err("couldn't ask the hub who you're signed in as"),
    };
    let user = login.user_id();

    let caps = atuin_client::api_client::caps_client(settings)?;
    caps.refresh().await?;
    if caps.get_server::<OctavoCap>().await?.is_none() {
        return Ok(Status::Failed(format!(
            "Atuin for Teams isn't enabled for your account on {}",
            settings.sync_address
        )));
    }

    let backfill = if backfill {
        Backfill::read(settings, store).await?
    } else {
        Backfill::default()
    };

    // Opened before the config says it's on, so a queue that can't be opened leaves it off.
    let queue = UploadQueue::open(settings.octavo_queue_path())
        .await
        .wrap_err("couldn't open the upload queue")?;
    // The server says the account has it now, so a daemon that found it off queues again, and
    // sends what it paused as it reloads the config.
    queue.clear_off(user).await.wrap_err("couldn't note that your account has it")?;
    // An unfixed backfill from an earlier setup would be fixed at the tail on the daemon's next
    // pass with it on, taking threads captured while it was off. Dropped before the config turns
    // it on, so that pass can't come first.
    if backfill.threads.is_none() {
        queue
            .drop_unfixed_thread_backfills(user)
            .await
            .wrap_err("couldn't cancel an earlier upload of your threads")?;
    }
    // As setup found it: `write_config` changes it only to turn it off.
    if !settings.teams.enabled {
        super::update_config_file(vec![("teams.enabled", Value::from(true))]).await?;
    }

    // Queued before the restart, so the daemon starts on it.
    let queued = backfill.queue(&queue, user).await;
    // Even with it on already, or the queue refusing: a daemon started before the upload queue
    // existed uploads nothing until it restarts, and the config can't say whether one did.
    let restarted = restart_daemon(settings).await;
    let (uploading, old) = queued?;
    if let Err(instruction) = restarted {
        return Ok(Status::Failed(format!(
            "Atuin for Teams is on once the daemon restarts: {instruction}"
        )));
    }

    let uploading =
        uploading.map_or_else(String::new, |what| format!(", uploading {what} in the background"));
    let old = if old > 0 {
        format!(". {} commands from before Atuin v14 can't go up", count(old))
    } else {
        String::new()
    };
    Ok(Status::Done(format!("Atuin for Teams is on{uploading}{old}")))
}

/// What Atuin already has here, to upload.
#[derive(Default)]
struct Backfill {
    history: Vec<HistoryId>,
    /// The commands left out of `history`, recorded before Atuin v14 with ids Atuin for Teams
    /// can't take.
    old: usize,
    threads: Option<StoredThreads>,
}

impl Backfill {
    async fn read(settings: &Arc<Settings>, store: &SqliteStore) -> Result<Self> {
        let db = atuin_client::database::Sqlite::new(&settings.db_path, settings.local_timeout);
        let (history, old): (Vec<_>, Vec<_>) = db
            .await?
            .active_ids()
            .await?
            .into_iter()
            .partition(|&id| pb::UuidV7::try_from(id).is_ok());
        Ok(Self {
            history,
            old: old.len(),
            threads: StoredThreads::read(settings, store).await?,
        })
    }

    /// Queue the uploads for `user`. Returns what goes up, if anything, and how many commands
    /// stay out.
    async fn queue(self, queue: &UploadQueue, user: &UserId) -> Result<(Option<String>, usize)> {
        let mut going = Vec::new();
        if !self.history.is_empty() {
            let queued = queue
                .push_many_history(user, self.history)
                .await
                .wrap_err("couldn't queue your history for upload")?;
            going.push(format!("{} commands", count(queued)));
        }
        // The daemon fixes this machine's bound at its tail as of when it sees Atuin for Teams on.
        if let Some(StoredThreads { tails, messages }) = self.threads {
            queue
                .request_thread_backfill(user, tails)
                .await
                .wrap_err("couldn't queue your threads for upload")?;
            going.push(format!("{} thread messages", count(messages)));
        }
        let going: Vec<&str> = going.iter().map(String::as_str).collect();
        Ok(((!going.is_empty()).then(|| names(&going)), self.old))
    }
}

/// The account's thread records: every `ai-session` series, on this machine or the server.
struct StoredThreads {
    /// Each series' host, with the idx of the last record to upload. This machine's is `None`, for
    /// the daemon to fix once it sees Atuin for Teams on, as capture appends to it until then.
    tails: Vec<(HostId, Option<RecordIdx>)>,
    /// How many records the series hold together.
    messages: u64,
}

impl StoredThreads {
    /// The thread records `store` or `settings`' server holds, or `None` when neither holds any.
    ///
    /// The server is asked too, so the records that a sync in progress has yet to download go up
    /// as well. Nothing else uploads another machine's threads.
    async fn read(settings: &Arc<Settings>, store: &SqliteStore) -> Result<Option<Self>> {
        let remote = async {
            SyncSession::builder()
                .store(store.clone())
                .client_source(ClientSource::FromSettings {
                    settings,
                    caps: None,
                })
                .build()
                .connect()
                .await?
                .record_status()
                .await
        };
        let remote =
            remote.await.wrap_err("couldn't ask the server which threads your account holds")?;
        Ok(Self::of(settings.host_id().await?, &store.status().await?, &remote))
    }

    /// The `ai-session` series `local` or `remote` holds, each at the later of its tails, with
    /// `here` this machine's host.
    fn of(here: HostId, local: &RecordStatus, remote: &RecordStatus) -> Option<Self> {
        let tails = [local, remote]
            .into_iter()
            .flat_map(|status| &status.hosts)
            .filter_map(|(&host, tails)| Some((host, *tails.get(&RecordTag::AiSession)?)))
            .into_grouping_map()
            .max();
        if tails.is_empty() {
            return None;
        }

        let messages = tails.values().map(|tail| tail.saturating_add(1)).sum();
        let tails =
            tails.into_iter().map(|(host, tail)| (host, (host != here).then_some(tail))).collect();
        Some(Self { tails, messages })
    }
}

/// Restart the daemon, if one runs, as it opens its Atuin for Teams client only as it starts.
/// `Err` says how the user can restart it instead.
async fn restart_daemon(settings: &Settings) -> Result<(), String> {
    // Connecting to a systemd socket, or to launchd's for the agent, would start a daemon that
    // isn't running, which reads the new config anyway.
    if !Pidfile::new(&settings.daemon.pidfile_path).is_live() {
        return Ok(());
    }
    let service = match ActiveService::find(settings).await {
        Ok(Some(service)) => service,
        Ok(None) => return Err("run `atuin daemon restart`".into()),
        Err(err) => {
            tracing::debug!(?err, "can't tell what runs the daemon");
            return Err("run `atuin daemon restart`".into());
        }
    };

    match service.external_manager().cloned() {
        // It doesn't say which unit the socket starts, so a restart would only be refused.
        Some(manager @ ExternalManager::SystemdSocket) => Err(manager.to_string()),
        Some(
            manager @ (ExternalManager::SystemdUnit { .. } | ExternalManager::LaunchdJob { .. }),
        ) => service.restart().await.map(drop).map_err(|e| with_instruction(&e, &manager)),
        // Atuin runs it, so `atuin daemon restart` is the instruction.
        None if service.is_launchd_agent() || settings.daemon.autostart => service
            .restart()
            .await
            .map(drop)
            .map_err(|e| format!("{e}; run `atuin daemon restart`")),
        // Started by hand, so restarted by hand.
        None => Err("run `atuin daemon restart`".into()),
    }
}

/// `err`, from restarting the daemon `manager` runs, followed by the manager's instruction for
/// restarting it.
fn with_instruction(err: &RestartError, manager: &ExternalManager) -> String {
    let (err, instruction) = (err.to_string(), manager.to_string());
    // A daemon that recorded no program to run again is refused with the instruction alone.
    // The comparison relies on the error wrappers being `#[error(transparent)]`; a
    // non-transparent one would only print the instruction twice.
    if err == instruction {
        return err;
    }
    format!("{err}; {instruction}")
}

#[cfg(test)]
mod tests {
    use atuin_domain::record::RecordSeriesKey;
    use rstest::rstest;

    use super::*;

    #[rstest]
    fn stored_threads_bound_other_machines_series_at_the_later_tail() {
        let [here, behind, remote_only] = [1, 2, 3].map(|n| HostId(uuid::Uuid::from_u128(n)));
        let threads = |host| RecordSeriesKey::new(host, RecordTag::AiSession);
        let local = RecordStatus::from_points([
            (threads(here), 5),
            (threads(behind), 3),
            (RecordSeriesKey::new(remote_only, RecordTag::History), 7),
        ]);
        let remote = RecordStatus::from_points([
            (threads(here), 4),
            (threads(behind), 9),
            (threads(remote_only), 2),
        ]);

        let StoredThreads {
            mut tails,
            messages,
        } = StoredThreads::of(here, &local, &remote).expect("both hold threads");

        tails.sort();
        assert_eq!(tails, [(here, None), (behind, Some(9)), (remote_only, Some(2))]);
        assert_eq!(messages, 6 + 10 + 3);
    }

    #[rstest]
    fn stored_threads_are_none_without_a_thread_series() {
        let host = HostId(uuid::Uuid::from_u128(1));
        let history =
            RecordStatus::from_points([(RecordSeriesKey::new(host, RecordTag::History), 7)]);

        assert!(StoredThreads::of(host, &history, &RecordStatus::new()).is_none());
    }
}
