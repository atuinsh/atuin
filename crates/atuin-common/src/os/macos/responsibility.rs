//! Processes that are responsible for their own privacy permissions.
//!
//! macOS charges a privacy permission (TCC) request to the ["responsible process"][1] of the
//! process that makes the request. A process inherits the responsible process of its parent, so a
//! program that a terminal starts is charged to the terminal: the permission dialog names the
//! terminal, and the grant attaches to the terminal.
//!
//! `responsibility_spawnattrs_setdisclaim` is a private but stable function that tells
//! `posix_spawn` to make the child responsible for itself. [LLDB][2] and Chromium use it for the
//! same purpose.
//!
//! [1]: https://www.qt.io/blog/the-curious-case-of-the-responsible-process
//! [2]: https://github.com/llvm/llvm-project/blob/llvmorg-24-init/lldb/source/Host/macosx/objcxx/PosixSpawnResponsible.h

#![allow(unsafe_code, reason = "posix_spawn and the responsibility function are C functions")]

use std::ffi::{CString, NulError, OsString, c_int, c_short, c_void};
use std::mem::MaybeUninit;
use std::os::unix::ffi::{OsStrExt as _, OsStringExt as _};

use easy_cast::Conv as _;
use rustix::process::Pid;

mod respawn;
mod spawn_attributes;

pub use respawn::{RespawnError, respawn_disclaimed};

use self::spawn_attributes::{FileActions, SpawnAttributes};

/// The type of `responsibility_spawnattrs_setdisclaim`.
///
/// LLDB [declares the function][1] as
/// `errno_t responsibility_spawnattrs_setdisclaim(posix_spawnattr_t *attrs, bool disclaim)`.
/// `errno_t` is an `int`, and Rust's `bool` has the representation of C's `_Bool`, which is also
/// C++'s `bool` on macOS.
///
/// [1]: https://github.com/llvm/llvm-project/blob/llvmorg-24-init/lldb/source/Host/macosx/objcxx/PosixSpawnResponsible.h
type SetDisclaimFn = unsafe extern "C" fn(*mut libc::posix_spawnattr_t, bool) -> c_int;

/// Error returned by [`DisclaimedCommand::spawn`].
#[derive(Debug, thiserror::Error)]
pub enum SpawnDisclaimedError {
    #[error("this version of macOS lacks responsibility_spawnattrs_setdisclaim")]
    Unsupported,
    #[error("the program, an argument, or an environment variable contains a NUL byte")]
    Nul(#[from] NulError),
    #[error(transparent)]
    Failed(#[from] std::io::Error),
}

/// Where the standard streams of a [`DisclaimedCommand`] lead.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChildStdio {
    /// The child shares the standard streams of this process.
    Inherit,
    /// The child reads and writes `/dev/null`.
    Null,
}

/// A program to spawn as its own responsible process.
///
/// Like [`std::process::Command`], the program's path is the child's `argv[0]`, and the child
/// inherits the environment and the file descriptors that aren't close-on-exec. The child starts
/// with an empty signal mask, and with the default action for SIGPIPE. A program name without a
/// slash is looked up in the `PATH` of this process, even when [`Self::env`] sets `PATH`.
#[derive(Debug)]
pub struct DisclaimedCommand {
    /// The program, then its arguments.
    argv: Result<Vec<CString>, NulError>,
    /// `KEY=value` entries that replace or add to the inherited environment.
    env: Result<Vec<CString>, NulError>,
    stdio: ChildStdio,
}

impl DisclaimedCommand {
    pub fn new(program: impl Into<OsString>) -> Self {
        Self {
            argv: c_string(program).map(|program| vec![program]),
            env: Ok(Vec::new()),
            stdio: ChildStdio::Inherit,
        }
    }

    pub fn arg(&mut self, arg: impl Into<OsString>) -> &mut Self {
        push(&mut self.argv, c_string(arg));
        self
    }

    pub fn args<I>(&mut self, args: I) -> &mut Self
    where
        I: IntoIterator<Item: Into<OsString>>,
    {
        for arg in args {
            self.arg(arg);
        }
        self
    }

    /// Sets an environment variable for the child, in place of any earlier value.
    pub fn env(
        &mut self,
        key: impl Into<OsString>,
        value: impl AsRef<std::ffi::OsStr>,
    ) -> &mut Self {
        let mut entry = key.into();
        entry.push("=");
        entry.push(value);
        let entry = c_string(entry);
        if let (Ok(entries), Ok(entry)) = (&mut self.env, &entry)
            && let Some(earlier) =
                entries.iter_mut().find(|earlier| key_of(earlier) == key_of(entry))
        {
            *earlier = entry.clone();
            return self;
        }
        push(&mut self.env, entry);
        self
    }

    /// Sets where the child's standard streams lead. The default is [`ChildStdio::Inherit`].
    pub const fn stdio(&mut self, stdio: ChildStdio) -> &mut Self {
        self.stdio = stdio;
        self
    }

    /// Starts the child, and returns its process id.
    ///
    /// # Errors
    ///
    /// Fails with [`SpawnDisclaimedError::Unsupported`] on a macOS that can't make a child
    /// responsible for itself, which leaves the caller free to spawn it the usual way.
    pub fn spawn(&self) -> Result<Pid, SpawnDisclaimedError> {
        let set_disclaim = set_disclaim().ok_or(SpawnDisclaimedError::Unsupported)?;
        let argv = self.argv.as_ref().map_err(Clone::clone)?;
        let overrides = self.env.as_ref().map_err(Clone::clone)?;
        let inherited = inherited_environment(overrides)?;

        let mut attributes_storage = MaybeUninit::uninit();
        let mut attributes = SpawnAttributes::new(&mut attributes_storage)?;
        let flags = c_short::conv(libc::POSIX_SPAWN_SETSIGMASK | libc::POSIX_SPAWN_SETSIGDEF);
        // SAFETY: `attributes` is initialized.
        check(unsafe { libc::posix_spawnattr_setflags(attributes.as_mut_ptr(), flags) })?;
        let empty_mask = signal_set(&[])?;
        // SAFETY: `attributes` is initialized, and `empty_mask` is a signal set.
        check(unsafe {
            libc::posix_spawnattr_setsigmask(attributes.as_mut_ptr(), &raw const empty_mask)
        })?;
        // Rust programs ignore SIGPIPE, and a child would inherit that.
        let default_signals = signal_set(&[libc::SIGPIPE])?;
        // SAFETY: `attributes` is initialized, and `default_signals` is a signal set.
        check(unsafe {
            libc::posix_spawnattr_setsigdefault(attributes.as_mut_ptr(), &raw const default_signals)
        })?;
        // SAFETY: `attributes` is initialized.
        check(unsafe { set_disclaim(attributes.as_mut_ptr(), true) })?;

        let mut actions_storage = MaybeUninit::uninit();
        let mut actions = FileActions::new(&mut actions_storage)?;
        if self.stdio == ChildStdio::Null {
            for (fd, flags) in [
                (libc::STDIN_FILENO, libc::O_RDONLY),
                (libc::STDOUT_FILENO, libc::O_WRONLY),
                (libc::STDERR_FILENO, libc::O_WRONLY),
            ] {
                // SAFETY: `actions` is initialized, and the path is a C string.
                check(unsafe {
                    libc::posix_spawn_file_actions_addopen(
                        actions.as_mut_ptr(),
                        fd,
                        c"/dev/null".as_ptr(),
                        flags,
                        0,
                    )
                })?;
            }
        }

        let argv_pointers = null_terminated(argv);
        let environment_pointers = null_terminated(inherited.iter().chain(overrides));
        let mut pid = 0;
        // SAFETY: `pid` is writable, `actions` and `attributes` are initialized, and both
        // pointer arrays end with a null pointer and point to C strings that outlive the call.
        check(unsafe {
            libc::posix_spawnp(
                &raw mut pid,
                argv[0].as_ptr(),
                actions.as_ptr(),
                attributes.as_ptr(),
                argv_pointers.as_ref().as_ptr(),
                environment_pointers.as_ref().as_ptr(),
            )
        })?;
        Pid::from_raw(pid).ok_or_else(|| {
            std::io::Error::other("posix_spawn reported success without a process id").into()
        })
    }
}

/// Finds `responsibility_spawnattrs_setdisclaim`, which macOS has had since 10.14.
fn set_disclaim() -> Option<SetDisclaimFn> {
    // SAFETY: `RTLD_DEFAULT` is a valid handle, and the name is a C string.
    let symbol = unsafe {
        libc::dlsym(libc::RTLD_DEFAULT, c"responsibility_spawnattrs_setdisclaim".as_ptr())
    };
    if symbol.is_null() {
        return None;
    }
    // SAFETY: `SetDisclaimFn` is the type of the function; see its documentation.
    Some(unsafe { std::mem::transmute::<*mut c_void, SetDisclaimFn>(symbol) })
}

/// The entries of this process's environment, without the variables that `overrides` set.
fn inherited_environment(overrides: &[CString]) -> Result<Vec<CString>, NulError> {
    std::env::vars_os()
        .filter(|(key, _)| !overrides.iter().any(|entry| key_of(entry) == key.as_bytes()))
        .map(|(mut key, value)| {
            key.push("=");
            key.push(value);
            c_string(key)
        })
        .collect()
}

/// The name of the variable that a `KEY=value` entry sets.
fn key_of(entry: &CString) -> &[u8] {
    let entry = entry.as_bytes();
    entry.iter().position(|&byte| byte == b'=').map_or(entry, |equals| &entry[..equals])
}

/// A signal set that holds `signals`.
fn signal_set(signals: &[c_int]) -> std::io::Result<libc::sigset_t> {
    let mut set = MaybeUninit::uninit();
    // SAFETY: `set` is writable memory for a `sigset_t`.
    if unsafe { libc::sigemptyset(set.as_mut_ptr()) } != 0 {
        return Err(std::io::Error::last_os_error());
    }
    for &signal in signals {
        // SAFETY: `sigemptyset` initialized `set`.
        if unsafe { libc::sigaddset(set.as_mut_ptr(), signal) } != 0 {
            return Err(std::io::Error::last_os_error());
        }
    }
    // SAFETY: `sigemptyset` initialized `set`.
    Ok(unsafe { set.assume_init() })
}

fn c_string(string: impl Into<OsString>) -> Result<CString, NulError> {
    CString::new(string.into().into_vec())
}

/// Appends `item` to `list`, or keeps the first error of either.
fn push(list: &mut Result<Vec<CString>, NulError>, item: Result<CString, NulError>) {
    match (list.as_mut(), item) {
        (Ok(list), Ok(item)) => list.push(item),
        (Ok(_), Err(error)) => *list = Err(error),
        (Err(_), _) => {}
    }
}

/// Pointers to `strings`, followed by a null pointer.
///
/// The result borrows `strings`, so the pointers stay valid for as long as the result lives.
fn null_terminated<'a, I>(strings: I) -> impl AsRef<[*mut libc::c_char]> + use<'a, I>
where
    I: IntoIterator<Item = &'a CString>,
{
    let pointers: Vec<_> = strings
        .into_iter()
        .map(|string| string.as_ptr().cast_mut())
        .chain(std::iter::once(std::ptr::null_mut()))
        .collect();
    pointers
}

/// Converts the result of a `posix_spawn*` function, which returns its error number instead of
/// setting `errno`.
fn check(result: c_int) -> std::io::Result<()> {
    if result == 0 {
        Ok(())
    } else {
        Err(std::io::Error::from_raw_os_error(result))
    }
}

#[cfg(test)]
mod tests;
