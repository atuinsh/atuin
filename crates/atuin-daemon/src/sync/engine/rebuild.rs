//! Rebuilds the kv and scripts dbs after a sync.
//!
//! History is built from the ids a sync downloaded ([`HistoryStore::incremental_build`]). The kv
//! and scripts dbs have no such path: [`KvStore::build`] and [`ScriptStore::build`] replay every
//! record of their tag. Downloaded ids carry no tag, so instead each db remembers how many records
//! of its tag its last build replayed, and is rebuilt once the store holds a different number.
//!
//! [`HistoryStore::incremental_build`]: atuin_client::history::store::HistoryStore::incremental_build

use atuin_client::record::sqlite_store::SqliteStore;
use atuin_common::encryption::paseto_v4;
use atuin_domain::record::{HostId, RecordTag};
use atuin_kv::database::Database as KvDatabase;
use atuin_kv::store::KvStore;
use atuin_scripts::database::Database as ScriptsDatabase;
use atuin_scripts::store::ScriptStore;
use parking_lot::Mutex;

/// Records that land while a db is rebuilt are missed by that rebuild, so the counts are checked
/// again after one. This bounds how many times that repeats within one sync; the next sync picks
/// up the rest.
const MAX_PASSES: usize = 3;

/// The stores whose dbs are built by replaying every record of their tag.
pub(super) struct ReplayedStores {
    store: SqliteStore,
    kv: KvStore,
    scripts: ScriptStore,
    scripts_db: ScriptsDatabase,
    /// Per tag, how many records the last successful build of its db replayed. `None` before one
    /// in this process, so the first sync after startup rebuilds, repairing a db left stale.
    built: Mutex<TagCounts>,
}

/// How many records the store held per tag when taken. `None` where the count could not be read,
/// which [`changed`] treats as a change, so a failed read never leaves a db stale.
#[derive(Debug, Clone, Copy, Default)]
struct TagCounts {
    kv: Option<u64>,
    scripts: Option<u64>,
}

impl ReplayedStores {
    pub fn new(
        store: SqliteStore,
        kv_db: KvDatabase,
        scripts_db: ScriptsDatabase,
        host_id: HostId,
        key: paseto_v4::Key,
    ) -> Self {
        Self {
            kv: KvStore::new(store.clone(), kv_db, host_id, key.clone()),
            scripts: ScriptStore::new(store.clone(), host_id, key),
            scripts_db,
            store,
            built: Mutex::new(TagCounts::default()),
        }
    }

    /// Rebuild each db whose tag holds a different number of records than its last successful
    /// build replayed. A failed build leaves its count as it was, so the next sync tries again.
    pub async fn rebuild_changed(&self) {
        for _ in 0..MAX_PASSES {
            if !self.rebuild_pass().await {
                break;
            }
        }
    }

    /// Rebuild what changed since the last successful builds; `true` if a build succeeded, as
    /// records may have landed during it.
    async fn rebuild_pass(&self) -> bool {
        let now = self.counts().await;
        let built = *self.built.lock();
        let mut rebuilt = false;

        if changed(built.kv, now.kv) {
            tracing::debug!("kv records changed since the last build, rebuilding the kv db");
            match self.kv.build().await {
                Ok(()) => {
                    self.built.lock().kv = now.kv;
                    rebuilt = true;
                }
                Err(e) => tracing::error!("failed to build kv from synced records: {e}"),
            }
        }

        if changed(built.scripts, now.scripts) {
            tracing::debug!(
                "script records changed since the last build, rebuilding the scripts db"
            );
            match self.scripts.build(&self.scripts_db).await {
                Ok(()) => {
                    self.built.lock().scripts = now.scripts;
                    rebuilt = true;
                }
                Err(e) => tracing::error!("failed to build scripts from synced records: {e}"),
            }
        }

        rebuilt
    }

    async fn counts(&self) -> TagCounts {
        TagCounts {
            kv: self.count(RecordTag::Kv).await,
            scripts: self.count(RecordTag::Script).await,
        }
    }

    async fn count(&self, tag: RecordTag) -> Option<u64> {
        match self.store.len_tag(&tag).await {
            Ok(len) => Some(len),
            Err(e) => {
                tracing::warn!("failed to count {tag:?} records: {e}");
                None
            }
        }
    }
}

fn changed(built: Option<u64>, now: Option<u64>) -> bool {
    !matches!((built, now), (Some(built), Some(now)) if built == now)
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use atuin_client::record::sqlite_store::SqliteStore;
    use atuin_common::encryption::paseto_v4::Key;
    use atuin_common::utils::uuid_v7;
    use atuin_domain::record::HostId;
    use atuin_kv::database::Database as KvDatabase;
    use atuin_kv::store::KvStore;
    use atuin_scripts::database::Database as ScriptsDatabase;
    use atuin_scripts::store::ScriptStore;
    use atuin_scripts::store::script::Script;
    use rstest::{fixture, rstest};

    use super::ReplayedStores;

    const TIMEOUT: Duration = Duration::from_secs(5);

    /// This host's stores over a shared record store, plus a second kv and script store writing to
    /// the same record store but their own dbs: what another host's records look like once a sync
    /// lands them.
    struct Fixture {
        stores: ReplayedStores,
        kv_db: KvDatabase,
        scripts_db: ScriptsDatabase,
        remote_kv: KvStore,
        remote_scripts: ScriptStore,
    }

    #[fixture]
    async fn fixture() -> Fixture {
        let store = SqliteStore::in_memory(TIMEOUT).await.unwrap();
        let kv_db = KvDatabase::in_memory(TIMEOUT).await.unwrap();
        let scripts_db = ScriptsDatabase::in_memory(TIMEOUT).await.unwrap();
        let host_id = HostId(uuid_v7());
        let key = Key::generate();

        let stores = ReplayedStores::new(
            store.clone(),
            kv_db.clone(),
            scripts_db.clone(),
            host_id,
            key.clone(),
        );
        let remote_kv = KvStore::new(
            store.clone(),
            KvDatabase::in_memory(TIMEOUT).await.unwrap(),
            host_id,
            key.clone(),
        );
        let remote_scripts = ScriptStore::new(store, host_id, key);

        Fixture {
            stores,
            kv_db,
            scripts_db,
            remote_kv,
            remote_scripts,
        }
    }

    fn script(name: &str) -> Script {
        Script::builder().name(name.to_owned()).script("echo hi".to_owned()).build()
    }

    #[rstest]
    #[tokio::test]
    async fn records_present_before_the_first_sync_are_built(#[future(awt)] fixture: Fixture) {
        let f = fixture;
        f.remote_scripts.create(script("deploy")).await.unwrap();

        f.stores.rebuild_changed().await;

        assert!(f.scripts_db.get_by_name("deploy").await.unwrap().is_some());
    }

    #[rstest]
    #[tokio::test]
    async fn synced_script_records_are_built(#[future(awt)] fixture: Fixture) {
        let f = fixture;
        f.stores.rebuild_changed().await;

        f.remote_scripts.create(script("deploy")).await.unwrap();
        f.stores.rebuild_changed().await;

        assert!(f.scripts_db.get_by_name("deploy").await.unwrap().is_some());
    }

    #[rstest]
    #[tokio::test]
    async fn synced_kv_records_are_built(#[future(awt)] fixture: Fixture) {
        let f = fixture;
        f.stores.rebuild_changed().await;

        f.remote_kv.set("ns", "key", "value").await.unwrap();
        f.stores.rebuild_changed().await;

        let entry = f.kv_db.load("ns", "key").await.unwrap();
        assert_eq!(entry.map(|kv| kv.value), Some("value".to_owned()));
    }

    #[rstest]
    #[tokio::test]
    async fn unchanged_tags_are_not_rebuilt(#[future(awt)] fixture: Fixture) {
        let f = fixture;
        f.stores.rebuild_changed().await;
        // A row the record store never saw: a rebuild would clear it.
        f.scripts_db.save(&script("local-only")).await.unwrap();

        f.remote_kv.set("ns", "key", "value").await.unwrap();
        f.stores.rebuild_changed().await;

        assert!(f.scripts_db.get_by_name("local-only").await.unwrap().is_some());
    }
}
