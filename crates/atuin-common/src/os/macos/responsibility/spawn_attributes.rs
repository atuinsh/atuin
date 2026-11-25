//! Owners of the `posix_spawn` configuration objects, which destroy the objects on drop.

use std::mem::MaybeUninit;

use super::check;

/// An initialized `posix_spawnattr_t`.
pub struct SpawnAttributes<'a>(&'a mut MaybeUninit<libc::posix_spawnattr_t>);

impl<'a> SpawnAttributes<'a> {
    pub fn new(storage: &'a mut MaybeUninit<libc::posix_spawnattr_t>) -> std::io::Result<Self> {
        // SAFETY: `storage` is writable memory for a `posix_spawnattr_t`.
        check(unsafe { libc::posix_spawnattr_init(storage.as_mut_ptr()) })?;
        Ok(Self(storage))
    }

    pub const fn as_ptr(&self) -> *const libc::posix_spawnattr_t {
        self.0.as_ptr()
    }

    pub const fn as_mut_ptr(&mut self) -> *mut libc::posix_spawnattr_t {
        self.0.as_mut_ptr()
    }
}

impl Drop for SpawnAttributes<'_> {
    fn drop(&mut self) {
        // SAFETY: `new` initialized the attributes, and nothing destroys them before this.
        unsafe { libc::posix_spawnattr_destroy(self.0.as_mut_ptr()) };
    }
}

/// An initialized `posix_spawn_file_actions_t`.
pub struct FileActions<'a>(&'a mut MaybeUninit<libc::posix_spawn_file_actions_t>);

impl<'a> FileActions<'a> {
    pub fn new(
        storage: &'a mut MaybeUninit<libc::posix_spawn_file_actions_t>,
    ) -> std::io::Result<Self> {
        // SAFETY: `storage` is writable memory for a `posix_spawn_file_actions_t`.
        check(unsafe { libc::posix_spawn_file_actions_init(storage.as_mut_ptr()) })?;
        Ok(Self(storage))
    }

    pub const fn as_ptr(&self) -> *const libc::posix_spawn_file_actions_t {
        self.0.as_ptr()
    }

    pub const fn as_mut_ptr(&mut self) -> *mut libc::posix_spawn_file_actions_t {
        self.0.as_mut_ptr()
    }
}

impl Drop for FileActions<'_> {
    fn drop(&mut self) {
        // SAFETY: `new` initialized the actions, and nothing destroys them before this.
        unsafe { libc::posix_spawn_file_actions_destroy(self.0.as_mut_ptr()) };
    }
}
