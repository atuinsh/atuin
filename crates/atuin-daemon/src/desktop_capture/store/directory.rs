//! The store's directory, which holds one database per generation of the store.
//!
//! Each generation lives in a directory named `db-<n>`, and the newest one is current. A new
//! store starts in `db-1`. A rewrite builds the next generation in a directory with a
//! `.partial` suffix, and renames that directory to its final name once the new database is
//! complete and synced. The rename is the one step that commits the rewrite. A crash before the
//! rename leaves the current generation in use, and a crash after the rename leaves the new one
//! in use. The next open removes whatever the crash left of the other generation.

use std::ffi::OsStr;
use std::fmt::Write as _;
use std::fs::{File, TryLockError};
use std::io;
use std::path::{MAIN_SEPARATOR, Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

use atuin_common::os::disk::Disk;
use atuin_common::units::ByteSize;
use tracing::warn;

use super::model::StoreError;

/// The file that the open store holds a lock on, so that a second process can't open the store
/// or remove a generation that the first process uses.
const LOCK_FILE: &str = "store.lock";

/// The prefix of a generation's directory name, which the generation's number follows.
const GENERATION_PREFIX: &str = "db-";

/// The suffix of a generation that a rewrite has not committed yet.
const PARTIAL_SUFFIX: &str = ".partial";

/// The number of the generation that a new store starts in.
const FIRST_GENERATION: u64 = 1;

/// The store directory, locked by this process.
#[derive(Debug)]
pub struct StoreDir {
    dir: PathBuf,
    /// Holds the lock until the store closes.
    _lock: File,
    /// The number of the current generation.
    current: u64,
    /// Whether the directory can still hold a generation that the current one replaced, or a
    /// partial generation. Those files stay on disk until a removal succeeds, so the next open
    /// finds them too.
    owes_removal: AtomicBool,
}

/// The directory of a generation that a rewrite is building.
#[derive(Debug)]
pub struct Partial {
    path: PathBuf,
    generation: u64,
}

impl Partial {
    /// Where the rewrite builds the new database.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Removes a partial generation that a rewrite gave up on.
    pub fn abandon(self) {
        if let Err(err) = remove_dir_if_present(&self.path) {
            warn!(%err, path = ?self.path, "could not remove a partial desktop capture store");
        }
    }
}

/// A generation that a commit replaced, and that only waits for its removal.
#[must_use = "the replaced generation stays on disk until it is removed"]
#[derive(Debug)]
pub struct Replaced(());

impl StoreDir {
    /// Locks `dir`, and removes what an interrupted rewrite left in it.
    ///
    /// The directory must exist.
    pub fn lock(dir: PathBuf) -> Result<Self, StoreError> {
        let lock = File::create(dir.join(LOCK_FILE)).map_err(StoreError::Files)?;
        match lock.try_lock() {
            Ok(()) => {}
            Err(TryLockError::WouldBlock) => return Err(StoreError::Locked),
            Err(TryLockError::Error(err)) => return Err(StoreError::Files(err)),
        }
        let newest = newest_generation(&dir).map_err(StoreError::Files)?;
        let store = Self {
            dir,
            _lock: lock,
            current: newest.unwrap_or(FIRST_GENERATION),
            owes_removal: AtomicBool::new(true),
        };
        if let Err(err) = store.remove_replaced() {
            warn!(
                %err,
                dir = ?store.dir,
                "could not remove an earlier generation of the desktop capture store, so the \
                 store tries again later"
            );
        }
        Ok(store)
    }

    /// The directory of the current generation's database.
    pub fn database(&self) -> PathBuf {
        self.path_of(self.current, "")
    }

    /// The bytes that the disk of the store directory has free for this process.
    pub fn available_space(&self) -> io::Result<ByteSize> {
        Ok(Disk::of_path(&self.dir)?.available_space())
    }

    /// The directory of `generation`, with `suffix` after the directory's name.
    fn path_of(&self, generation: u64, suffix: &str) -> PathBuf {
        let mut path = self.dir.clone().into_os_string();
        let _ = write!(path, "{MAIN_SEPARATOR}{GENERATION_PREFIX}{generation}{suffix}");
        PathBuf::from(path)
    }

    /// Starts the next generation in an empty directory.
    pub fn begin(&self) -> io::Result<Partial> {
        let generation = self.current + 1;
        let path = self.path_of(generation, PARTIAL_SUFFIX);
        remove_dir_if_present(&path)?;
        Ok(Partial { path, generation })
    }

    /// Makes `partial` the current generation, once every file of its database is on disk.
    ///
    /// The generation that `partial` replaces stays on disk until [`remove`](Self::remove). A
    /// failed commit leaves the current generation current.
    pub fn commit(&mut self, partial: &Partial) -> io::Result<Replaced> {
        sync_tree(&partial.path)?;
        std::fs::rename(&partial.path, self.path_of(partial.generation, ""))?;
        self.owes_removal.store(true, Ordering::Release);
        self.current = partial.generation;
        Ok(Replaced(()))
    }

    /// Removes a generation that a commit replaced. Call it once the generation's database is
    /// closed.
    ///
    /// A failed removal leaves the generation on disk until
    /// [`remove_replaced`](Self::remove_replaced) succeeds.
    #[expect(
        clippy::needless_pass_by_value,
        reason = "the removal takes the generation, so that nothing can use it afterwards"
    )]
    pub fn remove(&self, replaced: Replaced) -> io::Result<()> {
        let Replaced(()) = replaced;
        self.remove_replaced()
    }

    /// Whether the directory can still hold a generation that the current one replaced.
    pub fn owes_removal(&self) -> bool {
        self.owes_removal.load(Ordering::Acquire)
    }

    /// Removes every numbered generation other than the current one, and every partial
    /// generation.
    ///
    /// The removal first syncs the directory, so that the commit of the current generation is
    /// on disk before the generation that the commit replaced goes.
    pub fn remove_replaced(&self) -> io::Result<()> {
        sync_dir(&self.dir)?;
        for entry in std::fs::read_dir(&self.dir)? {
            let entry = entry?;
            let name = entry.file_name();
            let stale = match committed_number(&name) {
                Some(number) => number != self.current,
                None => is_partial(&name),
            };
            if stale {
                remove_entry(&entry.path())?;
            }
        }
        sync_dir(&self.dir)?;
        self.owes_removal.store(false, Ordering::Release);
        Ok(())
    }
}

/// The number of the newest committed generation in `dir`, if `dir` holds one.
fn newest_generation(dir: &Path) -> io::Result<Option<u64>> {
    let mut newest = None;
    for entry in std::fs::read_dir(dir)? {
        if let Some(number) = committed_number(&entry?.file_name()) {
            newest = newest.max(Some(number));
        }
    }
    Ok(newest)
}

/// The number of a committed generation's directory name.
fn committed_number(name: &OsStr) -> Option<u64> {
    let number = name.to_str()?.strip_prefix(GENERATION_PREFIX)?;
    if !number.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    number.parse().ok()
}

fn is_partial(name: &OsStr) -> bool {
    name.to_str().is_some_and(|name| {
        name.strip_suffix(PARTIAL_SUFFIX)
            .is_some_and(|generation| committed_number(OsStr::new(generation)).is_some())
    })
}

fn remove_entry(path: &Path) -> io::Result<()> {
    if path.is_dir() {
        remove_dir_if_present(path)
    } else {
        match std::fs::remove_file(path) {
            Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(()),
            result => result,
        }
    }
}

fn remove_dir_if_present(path: &Path) -> io::Result<()> {
    match std::fs::remove_dir_all(path) {
        Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(()),
        result => result,
    }
}

/// Syncs every file and directory under `dir`, and `dir` itself.
fn sync_tree(dir: &Path) -> io::Result<()> {
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        if entry.file_type()?.is_dir() {
            sync_tree(&entry.path())?;
        } else {
            File::open(entry.path())?.sync_all()?;
        }
    }
    sync_dir(dir)
}

/// Syncs a directory's entries, on the platforms that can open a directory as a file.
fn sync_dir(dir: &Path) -> io::Result<()> {
    if cfg!(unix) {
        File::open(dir)?.sync_all()?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use rstest::{fixture, rstest};
    use tempfile::TempDir;

    use super::*;

    #[fixture]
    fn dir() -> TempDir {
        tempfile::tempdir().expect("tempdir")
    }

    /// Creates the entries `names` in `dir`. A name that ends in `/` names a directory.
    fn create(dir: &TempDir, names: &[&str]) {
        for name in names {
            let created = match name.strip_suffix('/') {
                Some(name) => std::fs::create_dir(dir.path().join(name)),
                None => std::fs::write(dir.path().join(name), b""),
            };
            created.expect("create");
        }
    }

    /// The names in `dir`, apart from the lock.
    fn entries(dir: &TempDir) -> Vec<String> {
        let mut names: Vec<String> = std::fs::read_dir(dir.path())
            .expect("read the directory")
            .map(|entry| entry.expect("an entry").file_name().into_string().expect("utf-8"))
            .filter(|name| name != LOCK_FILE)
            .collect();
        names.sort();
        names
    }

    /// An open uses the newest committed generation, and removes what a crash at any step of a
    /// rewrite left beside it. The stop file and unknown entries stay.
    #[rstest]
    #[case::new_store(&["stopped"], "db-1", &["stopped"])]
    #[case::one_generation(&["db-1/", "stopped"], "db-1", &["db-1", "stopped"])]
    #[case::crash_while_building(
        &["db-1/", "db-2.partial/", "stopped"],
        "db-1",
        &["db-1", "stopped"],
    )]
    #[case::crash_after_the_commit(&["db-1/", "db-2/", "stopped"], "db-2", &["db-2", "stopped"])]
    #[case::crash_while_removing(&["db-2/", "db-3/", "db-4.partial/"], "db-3", &["db-3"])]
    #[case::numbers_compare_as_numbers(&["db-9/", "db-10/"], "db-10", &["db-10"])]
    #[case::only_a_partial_generation(&["db-1.partial/", "stopped"], "db-1", &["stopped"])]
    #[case::other_entries_stay(
        &["db-1/", "db-x/", "db-1.x/", "notes.txt"],
        "db-1",
        &["db-1", "db-1.x", "db-x", "notes.txt"],
    )]
    fn an_open_keeps_the_newest_committed_generation(
        dir: TempDir,
        #[case] before: &[&str],
        #[case] current: &str,
        #[case] after: &[&str],
    ) {
        create(&dir, before);

        for _ in 0..2 {
            let store = StoreDir::lock(dir.path().to_path_buf()).expect("lock");

            assert_eq!(store.database(), dir.path().join(current));
            assert_eq!(entries(&dir), after);
        }
    }

    /// Commits a new generation that holds one journal.
    fn commit_one(store: &mut StoreDir) -> Replaced {
        let partial = store.begin().expect("begin");
        std::fs::create_dir(partial.path()).expect("create");
        std::fs::write(partial.path().join("0.jnl"), b"").expect("write");
        store.commit(&partial).expect("commit")
    }

    #[rstest]
    fn a_commit_replaces_the_current_generation(dir: TempDir) {
        create(&dir, &["db-1/", "stopped"]);
        let mut store = StoreDir::lock(dir.path().to_path_buf()).expect("lock");

        let replaced = commit_one(&mut store);
        assert_eq!(entries(&dir), ["db-1", "db-2", "stopped"]);
        store.remove(replaced).expect("remove");
        assert_eq!(store.database(), dir.path().join("db-2"));
        assert_eq!(entries(&dir), ["db-2", "stopped"]);

        let replaced = commit_one(&mut store);
        store.remove(replaced).expect("remove");
        assert_eq!(store.database(), dir.path().join("db-3"));
        assert_eq!(entries(&dir), ["db-3", "stopped"]);
    }

    /// A generation that fails to go stays owed, and a later removal takes it away.
    #[rstest]
    fn a_failed_removal_stays_owed(dir: TempDir) {
        use std::os::unix::fs::PermissionsExt as _;

        create(&dir, &["db-1/"]);
        let mut store = StoreDir::lock(dir.path().to_path_buf()).expect("lock");
        let replaced = commit_one(&mut store);
        store.remove(replaced).expect("remove");
        assert!(!store.owes_removal());
        let replaced = commit_one(&mut store);
        let replaced_dir = dir.path().join("db-2");
        let mode = |mode| std::fs::Permissions::from_mode(mode);
        std::fs::set_permissions(&replaced_dir, mode(0o500)).expect("protect the generation");

        let removed = store.remove(replaced);

        std::fs::set_permissions(&replaced_dir, mode(0o700)).expect("unprotect the generation");
        assert!(removed.is_err(), "{removed:?}");
        assert!(store.owes_removal());
        store.remove_replaced().expect("remove");
        assert!(!store.owes_removal());
        assert_eq!(entries(&dir), ["db-3"]);
    }

    #[rstest]
    fn a_second_lock_fails(dir: TempDir) {
        let _first = StoreDir::lock(dir.path().to_path_buf()).expect("lock");

        let second = StoreDir::lock(dir.path().to_path_buf());

        assert!(matches!(second, Err(StoreError::Locked)), "{second:?}");
    }
}
