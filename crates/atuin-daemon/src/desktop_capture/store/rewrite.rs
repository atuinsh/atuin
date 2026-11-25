//! Rewrites the store into a new generation of its database.
//!
//! fjall keeps every write in its journal until the journal grows past 64 MB, and fjall 3.1 doesn't
//! offer a way to make the journal turn over sooner. A removed value therefore stays readable in
//! the journal for as long as that takes. A rewrite copies the entries that the store holds into a
//! new database, and then removes the old database with its journals and tables.

use atuin_common::units::ByteSize;
use tracing::{error, warn};

use super::directory::StoreDir;
use super::keyspaces::{Carried, Keyspaces};
use super::model::StoreError;

/// What a rewrite left in place of the keyspaces that it started from.
pub enum Rewritten {
    /// The new generation is open. `removal` tells whether the old generation is gone. After a
    /// failure, [`StoreDir::remove_replaced`] tries the removal again.
    Done {
        keyspaces: Keyspaces,
        removal: Result<(), StoreError>,
    },
    /// The rewrite failed before its commit, so the old keyspaces are still open and current.
    Kept {
        keyspaces: Keyspaces,
        error: StoreError,
    },
    /// The new generation is current, but it failed to open. [`Keyspaces::reopen`] with
    /// `carried` tries again.
    Unopened {
        carried: Carried,
        error: StoreError,
    },
}

/// Copies `keyspaces` into the next generation of `dir`, makes that generation current, and
/// removes the old one, together with every earlier generation that a failed removal left.
///
/// The rewrite fails before the copy when the disk doesn't have room for a second copy of the
/// store. The caller guarantees that nothing else uses `keyspaces` during the rewrite.
pub fn rewrite(keyspaces: Keyspaces, dir: &mut StoreDir) -> Rewritten {
    if let Err(error) = check_space(&keyspaces, dir) {
        return Rewritten::Kept { keyspaces, error };
    }
    let partial = match dir.begin() {
        Ok(partial) => partial,
        Err(err) => {
            return Rewritten::Kept {
                keyspaces,
                error: StoreError::Files(err),
            };
        }
    };
    if let Err(error) = keyspaces.copy_to(partial.path()) {
        partial.abandon();
        return Rewritten::Kept { keyspaces, error };
    }
    let replaced = match dir.commit(&partial) {
        Ok(replaced) => replaced,
        Err(err) => {
            partial.abandon();
            return Rewritten::Kept {
                keyspaces,
                error: StoreError::Files(err),
            };
        }
    };
    let carried = keyspaces.close();
    let removal = dir.remove(replaced).map_err(|err| {
        warn!(
            %err,
            "could not remove the old generation of the desktop capture store, so the store \
             tries again later"
        );
        StoreError::Files(err)
    });
    match Keyspaces::reopen(dir.database(), &carried) {
        Ok(keyspaces) => Rewritten::Done { keyspaces, removal },
        Err(error) => {
            error!(%error, "could not open the rewritten desktop capture store");
            Rewritten::Unopened { carried, error }
        }
    }
}

/// Fails when the disk has less space free than the store takes, which a copy can need.
///
/// A disk whose free space can't be read lets the rewrite go ahead.
fn check_space(keyspaces: &Keyspaces, dir: &StoreDir) -> Result<(), StoreError> {
    let needed = ByteSize::b(keyspaces.disk_space()?);
    match dir.available_space() {
        Ok(available) if available < needed => Err(StoreError::NoSpace { needed, available }),
        Ok(_) => Ok(()),
        Err(err) => {
            warn!(%err, "could not read the free space for a rewrite of the desktop capture store");
            Ok(())
        }
    }
}
