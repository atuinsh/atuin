//! Atuin Hub in `atuin setup`: turned on once sign-in has said the account has it, by the
//! user's own `atuin.lua`.

use std::fmt;
use std::path::PathBuf;
use std::sync::Arc;

use atuin_client::record::sqlite_store::SqliteStore;
use atuin_client::record::sync::{ClientSource, SyncSession};
use atuin_client::settings::Settings;
use atuin_daemon::service::pidfile::Pidfile;
use atuin_daemon::service::{ActiveService, ExternalManager, RestartError};
use atuin_domain::caps::OctavoCap;
use atuin_domain::record::{HostId, RecordStatus, RecordTag};
use atuin_octavo::hub::{HubCallError, HubClient, UserId};
use atuin_octavo::queue::UploadQueue;
use eyre::{Result, WrapErr as _};
use itertools::Itertools as _;

use super::plan::ThreadUploads;
use super::upload::Upload;
use super::users_file::{UsersFile, WriteError};
use super::{Status, Step, Updates, count, send};

/// Whether the user's own config file turns Atuin Hub on, and for which threads. One that
/// can't be read or run leaves it off.
pub(super) async fn found() -> (ThreadUploads, bool) {
    match users_file().await {
        Ok(file) => file.uploads(),
        Err(err) => {
            tracing::debug!(?err, "can't read your own atuin.lua; taking Atuin Hub as off");
            (ThreadUploads::All, false)
        }
    }
}

/// The user's own config file, in `$ATUIN_HOME`.
async fn users_file() -> Result<UsersFile> {
    let home: PathBuf = atuin_common::dirs::try_atuin_home()?.into();
    let read = tokio::task::spawn_blocking(move || UsersFile::read(&home));
    Ok(read.await??)
}

/// Turn Atuin Hub off, as far as the user's own config file lets setup.
pub(super) async fn disable(tx: &Updates) {
    send(tx, Step::Hub, Status::Running("Turning off Atuin Hub".into()));
    let failed =
        |err: eyre::Report| Status::Failed(format!("Couldn't turn off Atuin Hub ({err:#})"));
    let status = match users_file().await.map(|file| file.write(ThreadUploads::Off)) {
        Ok(Ok(())) => Status::Done("Atuin Hub is off".into()),
        Ok(Err(err @ WriteError::Theirs { .. })) => Status::Failed(err.to_string()),
        Ok(Err(err @ WriteError::Write { .. })) => failed(err.into()),
        Err(err) => failed(err),
    };
    send(tx, Step::Hub, status);
}

/// Turn Atuin Hub on for `uploads`, if the account has it, and upload the threads Atuin
/// already has if `backfill`. Returns the upload to watch, if it started one.
pub(super) async fn enable(
    settings: &Arc<Settings>,
    store: &SqliteStore,
    backfill: bool,
    uploads: ThreadUploads,
    tx: &Updates,
) -> Option<Upload> {
    send(tx, Step::Hub, Status::Running("Turning on Atuin Hub".into()));
    let (status, upload) = match turn_on(settings, store, backfill, uploads).await {
        Ok(on) => on,
        Err(e) => {
            let failed =
                format!("Couldn't turn on Atuin Hub ({e:#}). Run `atuin setup --hub` to try again");
            (Status::Failed(failed), None)
        }
    };
    send(tx, Step::Hub, status);
    upload
}

/// How turning it on went, and the upload it started: `Err` for a failure that trying again may
/// fix.
async fn turn_on(
    settings: &Arc<Settings>,
    store: &SqliteStore,
    backfill: bool,
    uploads: ThreadUploads,
) -> Result<(Status, Option<Upload>)> {
    let login = match HubClient::new(settings)?.login(settings).await {
        Ok(login) => login,
        Err(HubCallError::NotLoggedIn) => {
            let failed =
                "Atuin Hub needs you signed in: run `atuin login`, then `atuin setup --hub`";
            return Ok((Status::Failed(failed.into()), None));
        }
        Err(err) => return Err(err).wrap_err("couldn't ask the hub who you're signed in as"),
    };
    let user = login.user_id();

    let caps = atuin_client::api_client::caps_client(settings)?;
    caps.refresh().await?;
    if caps.get_server::<OctavoCap>().await?.is_none() {
        let failed =
            format!("Atuin Hub isn't enabled for your account on {}", settings.sync_address);
        return Ok((Status::Failed(failed), None));
    }

    // Only with every thread going up, because threads stay local with uploads off, and setup
    // allows `--backfill` only when uploads aren't limited to workspaces (see
    // `impl Choice<Plan> for Backfill` in `plan.rs`).
    let threads = if backfill && uploads == ThreadUploads::All {
        StoredThreads::read(settings, store).await?
    } else {
        StoredThreads::default()
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
    threads.drop_earlier_unfixed(&queue, user).await?;
    match users_file().await?.write(uploads) {
        Ok(()) => {}
        Err(err @ WriteError::Theirs { .. }) => return Ok((Status::Failed(err.to_string()), None)),
        Err(err @ WriteError::Write { .. }) => return Err(err.into()),
    }

    // Queued before the restart, so the daemon starts on it.
    let queued = threads.queue(&queue, user).await;
    // Even with it on already, or the queue refusing: a daemon started before the upload queue
    // existed uploads nothing until it restarts, and the config can't say whether one did.
    let restarted = restart_daemon(settings).await;
    let messages = queued?;
    if let Err(instruction) = restarted {
        let failed = format!("Atuin Hub is on once the daemon restarts: {instruction}");
        return Ok((Status::Failed(failed), None));
    }

    let upload = messages
        .is_some()
        .then(|| Upload::new(queue, user.clone(), settings.daemon.pidfile_path.clone()));
    let uploading = messages.map_or_else(String::new, |messages| {
        format!(", uploading {} thread messages in the background", count(messages))
    });
    Ok((
        Status::Done(format!(
            "Atuin Hub is on{uploading}{}",
            OtherMachines(threads.other_machines)
        )),
        upload,
    ))
}

/// The summary clause about the machines that upload their own threads, empty for none.
struct OtherMachines(usize);

impl fmt::Display for OtherMachines {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let Self(n) = *self;
        match n {
            0 => Ok(()),
            1 => write!(
                f,
                ". Threads from {} other machine go up once you run atuin setup --hub --backfill \
                 on that machine",
                count(n)
            ),
            _ => write!(
                f,
                ". Threads from {} other machines go up once you run atuin setup --hub --backfill \
                 on those machines",
                count(n)
            ),
        }
    }
}

/// The account's thread records, as setup splits them between this machine and the others. The
/// default backfills nothing.
#[derive(Default)]
struct StoredThreads {
    /// This machine's series, if it has one.
    own: Option<OwnThreads>,
    /// How many other machines hold a thread series, which only those machines upload.
    other_machines: usize,
}

/// This machine's thread series.
struct OwnThreads {
    host: HostId,
    /// How many records the series holds, by the later of the local and remote tails.
    messages: u64,
}

impl StoredThreads {
    /// The thread records that `store` and the server of `settings` hold.
    ///
    /// The server is asked too, so that the series of other machines that exist only on the
    /// server are counted. Each machine uploads only its own threads, because only that machine
    /// can read the config files that decide whether a thread goes up.
    async fn read(settings: &Arc<Settings>, store: &SqliteStore) -> Result<Self> {
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

    /// This machine's `ai-session` series and a count of the other machines that have one.
    ///
    /// This machine's series is at the later of its tails in `local` and `remote`. `here` is this
    /// machine's host.
    fn of(here: HostId, local: &RecordStatus, remote: &RecordStatus) -> Self {
        let tails = [local, remote]
            .into_iter()
            .flat_map(|status| &status.hosts)
            .filter_map(|(&host, tails)| Some((host, *tails.get(&RecordTag::AiSession)?)))
            .into_grouping_map()
            .max();

        let mut threads = Self::default();
        for (host, tail) in tails {
            if host == here {
                threads.own = Some(OwnThreads {
                    host,
                    messages: tail.saturating_add(1),
                });
            } else {
                threads.other_machines += 1;
            }
        }
        threads
    }

    /// Drops `user`'s unfixed backfills from an earlier setup, unless this machine's threads are
    /// queued again.
    async fn drop_earlier_unfixed(&self, queue: &UploadQueue, user: &UserId) -> Result<()> {
        if self.own.is_none() {
            queue
                .drop_unfixed_thread_backfills(user)
                .await
                .wrap_err("couldn't cancel an earlier upload of your threads")?;
        }
        Ok(())
    }

    /// Queue the upload of this machine's threads for `user`. Returns how many thread messages go
    /// up, if any.
    ///
    /// The daemon fixes this machine's bound at its tail as of when it sees Atuin Hub on.
    async fn queue(&self, queue: &UploadQueue, user: &UserId) -> Result<Option<u64>> {
        let Some(OwnThreads { host, messages }) = &self.own else {
            return Ok(None);
        };
        queue
            .request_thread_backfill(user, [(*host, None)])
            .await
            .wrap_err("couldn't queue your threads for upload")?;
        Ok(Some(*messages))
    }
}

/// Restart the daemon, if one runs, as it opens its Atuin Hub client only as it starts.
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
    use atuin_octavo::queue::ThreadBackfill;
    use rstest::rstest;

    use super::*;

    #[rstest]
    #[case(0, "")]
    #[case(
        1,
        ". Threads from 1 other machine go up once you run atuin setup --hub --backfill on that \
         machine"
    )]
    #[case(
        2,
        ". Threads from 2 other machines go up once you run atuin setup --hub --backfill on those \
         machines"
    )]
    fn other_machines_clause(#[case] n: usize, #[case] text: &str) {
        assert_eq!(OtherMachines(n).to_string(), text);
    }

    const HERE: HostId = HostId(uuid::Uuid::from_u128(1));

    /// A backfill without this machine's threads drops an earlier setup's unfixed backfill.
    ///
    /// Otherwise that backfill would take the threads that were captured while Atuin Hub
    /// was off.
    #[rstest]
    #[case::no_threads_here(None, &[])]
    #[case::threads_here(
        Some(4),
        &[ThreadBackfill { host_id: HERE, next_idx: 0, until_idx: None }],
    )]
    #[tokio::test]
    async fn a_backfill_queues_this_machines_threads_or_drops_earlier_ones(
        #[case] messages: Option<u64>,
        #[case] expected: &[ThreadBackfill],
    ) {
        let dir = tempfile::tempdir().expect("a temp dir");
        let queue = UploadQueue::open(dir.path().join("octavo.db")).await.expect("a queue");
        let user = UserId::new("alice");
        queue.request_thread_backfill(&user, [(HERE, None)]).await.expect("request");
        queue.advance_thread_backfill(&user, HERE, 3).await.expect("advance");
        let threads = StoredThreads {
            own: messages.map(|messages| OwnThreads {
                host: HERE,
                messages,
            }),
            other_machines: 1,
        };

        threads.drop_earlier_unfixed(&queue, &user).await.expect("drop");
        threads.queue(&queue, &user).await.expect("queue");

        assert_eq!(queue.thread_backfills(&user).await.expect("read"), expected);
    }

    #[rstest]
    fn stored_threads_backfill_only_this_machines_series() {
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
            own,
            other_machines,
        } = StoredThreads::of(here, &local, &remote);

        let own = own.expect("this machine has threads");
        assert_eq!((own.host, own.messages), (here, 6));
        assert_eq!(other_machines, 2);
    }

    #[rstest]
    fn stored_threads_count_other_machines_without_a_series_here() {
        let [here, elsewhere] = [1, 2].map(|n| HostId(uuid::Uuid::from_u128(n)));
        let local =
            RecordStatus::from_points([(RecordSeriesKey::new(here, RecordTag::History), 7)]);
        let remote =
            RecordStatus::from_points([(RecordSeriesKey::new(elsewhere, RecordTag::AiSession), 3)]);

        let StoredThreads {
            own,
            other_machines,
        } = StoredThreads::of(here, &local, &remote);

        assert!(own.is_none());
        assert_eq!(other_machines, 1);
    }

    #[rstest]
    fn stored_threads_are_empty_without_a_thread_series() {
        let host = HostId(uuid::Uuid::from_u128(1));
        let history =
            RecordStatus::from_points([(RecordSeriesKey::new(host, RecordTag::History), 7)]);

        let StoredThreads {
            own,
            other_machines,
        } = StoredThreads::of(host, &history, &RecordStatus::new());

        assert!(own.is_none());
        assert_eq!(other_machines, 0);
    }
}
