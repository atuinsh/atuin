//! Utilities related to macOS's idea of a ["responsible process"][1].
//!
//! # Background
//!
//! As of macOS 27 (Golden Gate), if a terminal command spawns a background process, macOS will
//! display a "running in background" dot next to the terminal emulator in the dock. This means that
//! if the Atuin daemon isn't running and autostart is enabled, the terminal that autostarts the
//! daemon will forever have a "running in background" dot until the daemon is killed. This is
//! undesirable as the Atuin daemon is meant to behave more like a system process that persists
//! beyond an individual terminal session.
//!
//! # Solution
//!
//! macOS associates every process with a "responsible process", and, among other uses, uses this to
//! determine whether an app is running in the background. We need to ensure that the daemon, when
//! launched, will not have the terminal emulator as its responsible process.
//!
//! To clear this association, we have to use `responsibility_spawnattrs_setdisclaim`, a private but
//! stable function that tells `posix_spawn` to mark the child process as responsible for itself.
//! This is used by [LLDB][2] and Chromium, among others, for exactly this purpose.
//!
//! This module provides [`spawn_disclaimed`], which behaves like [`std::process::Command`] but
//! marks the process as responsible for itself.
//!
//! [1]: https://www.qt.io/blog/the-curious-case-of-the-responsible-process
//! [2]: https://github.com/llvm/llvm-project/blob/llvmorg-24-init/lldb/source/Host/macosx/objcxx/PosixSpawnResponsible.h

#![allow(unsafe_code, reason = "need to call low-level C functions")]

use std::ffi::{CString, OsString, c_int, c_short, c_void};
use std::mem::MaybeUninit;
use std::os::unix::ffi::OsStringExt;

use easy_cast::Conv;

/// The type of `responsibility_spawnattrs_setdisclaim`.
///
/// LLDB [declares this function as follows][1]:
///
/// ```c
/// errno_t responsibility_spawnattrs_setdisclaim(posix_spawnattr_t *attrs, bool disclaim);
/// ```
///
/// `errno_t` is [a typedef of `int`][2], and Rust's `bool` is [guaranteed to have the same
/// representation as C's `_Bool`][3]. The `bool` in LLDB's declaration is C++'s `bool`, but this is
/// the same type as `_Bool` on macOS (and nearly every other platform).
///
/// [1]: https://github.com/llvm/llvm-project/blob/llvmorg-24-init/lldb/source/Host/macosx/objcxx/PosixSpawnResponsible.h
/// [2]: https://github.com/apple-oss-distributions/xnu/blob/xnu-12377.121.6/bsd/sys/_types/_errno_t.h
/// [3]: https://github.com/rust-lang/rust/pull/46176#issuecomment-359593446
type SetDisclaimFn = unsafe extern "C" fn(*mut libc::posix_spawnattr_t, bool) -> c_int;

/// Error returned by [`spawn_disclaimed`].
#[derive(Debug, thiserror::Error)]
pub enum SpawnDisclaimedError {
    #[error("responsibility_spawnattrs_setdisclaim is unsupported on this system")]
    Unsupported,
    #[error(transparent)]
    Failed(#[from] std::io::Error),
}

/// Spawn a program as its own responsible process.
///
/// The process will be marked as being responsible for itself; see the [module documentation](self)
/// for more information.
///
/// This function is designed to behave like [`std::process::Command`] -- `program` can be relative
/// or absolute, and the child process inherits the parent's environment and file descriptors
/// (except those marked close-on-exec).
///
/// For simplicity, this function does not offer the same flexibility as [`Command`], but is rather
/// tailored towards spawning daemon processes: file descriptors 0, 1, and 2 will be connected to
/// `/dev/null`, and this function returns immediately without providing a handle that can be used
/// to wait on the child. This matches the way Atuin has historically spawned the daemon with
/// [`Command`] (still used on non-macOS systems); see
/// `atuin::command::client::daemon::spawn_daemon_process`.
///
/// [`Command`]: std::process::Command
pub fn spawn_disclaimed<P, A>(program: P, args: A) -> Result<(), SpawnDisclaimedError>
where
    P: Into<OsString>,
    A: IntoIterator<Item: Into<OsString>>,
{
    let Some(set_disclaim) = get_setdisclaim() else {
        return Err(SpawnDisclaimedError::Unsupported);
    };

    let program = os_to_cstring(program)?;
    let args = std::iter::once(Ok(program.clone()))
        .chain(args.into_iter().map(os_to_cstring))
        .collect::<std::io::Result<Vec<_>>>()?;

    let env_pairs = std::env::vars_os()
        .map(|(key, value)| {
            let mut pair = key;
            pair.push("=");
            pair.push(value);
            os_to_cstring(pair)
        })
        .collect::<std::io::Result<Vec<_>>>()?;

    let argv = null_terminated_string_array(&args);
    let envp = null_terminated_string_array(&env_pairs);

    let mut attr_storage = MaybeUninit::uninit();
    let mut attr = SpawnAttr::new(&mut attr_storage)?;

    let flags = c_short::conv(libc::POSIX_SPAWN_SETSIGMASK);
    // SAFETY: `attr` is initialized.
    posix_spawn_result(unsafe { libc::posix_spawnattr_setflags(attr.as_mut_ptr(), flags) })?;

    let mut empty_mask: MaybeUninit<libc::sigset_t> = MaybeUninit::uninit();
    // SAFETY: `empty_mask` is valid memory for a `sigset_t` out-pointer.
    if unsafe { libc::sigemptyset(empty_mask.as_mut_ptr()) } != 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    // SAFETY: `attr` is initialized and `empty_mask` was just initialized.
    posix_spawn_result(unsafe {
        libc::posix_spawnattr_setsigmask(attr.as_mut_ptr(), empty_mask.as_ptr())
    })?;
    // SAFETY: `attr` is initialized.
    posix_spawn_result(unsafe { set_disclaim(attr.as_mut_ptr(), true) })?;

    let mut actions_storage = MaybeUninit::uninit();
    let mut actions = FileActions::new(&mut actions_storage)?;
    for (fd, flags) in [
        (libc::STDIN_FILENO, libc::O_RDONLY),
        (libc::STDOUT_FILENO, libc::O_WRONLY),
        (libc::STDERR_FILENO, libc::O_WRONLY),
    ] {
        // SAFETY: `actions` is initialized, and the path is a valid null-terminated C string.
        posix_spawn_result(unsafe {
            libc::posix_spawn_file_actions_addopen(
                actions.as_mut_ptr(),
                fd,
                c"/dev/null".as_ptr(),
                flags,
                0,
            )
        })?;
    }

    let mut pid: libc::pid_t = 0;
    // SAFETY: every pointer is valid for the duration of the call:
    //
    // - `pid` points to a valid `pid_t` (although it is not required to be initialized here).
    // - `program` is a null-terminated C string.
    // - `actions` (RAII wrapper) was initialized when it was created.
    // - `attr` (RAII wrapper) was initialized when it was created.
    // - `argv` is a null-terminated array of C strings.
    // - `envp` is a null-terminated array of C strings.
    //
    // None of these are destroyed until the call finishes, and they are not required to be valid
    // past that.
    posix_spawn_result(unsafe {
        libc::posix_spawn(
            &raw mut pid,
            program.as_ptr(),
            actions.as_ptr(),
            attr.as_ptr(),
            argv.as_ref().as_ptr(),
            envp.as_ref().as_ptr(),
        )
    })?;

    Ok(())
}

#[cfg(test)]
thread_local! {
    /// In tests only: if this variable is `Some(optional_fn)`, [`get_setdisclaim`] will return
    /// `optional_fn` instead of the real `responsibility_spawnattrs_setdisclaim`.
    static MOCK_SETDISCLAIM: std::cell::Cell<Option<Option<SetDisclaimFn>>> =
        const { std::cell::Cell::new(None) };
}

/// Get the `responsibility_spawnattrs_setdisclaim` function, or return [`None`] if it isn't present
/// on this system.
fn get_setdisclaim() -> Option<SetDisclaimFn> {
    #[cfg(test)]
    if let Some(result) = MOCK_SETDISCLAIM.get() {
        return result;
    }

    // SAFETY: `RTLD_DEFAULT` is a valid handle and the symbol name is a null-terminated C string.
    let sym = unsafe {
        libc::dlsym(libc::RTLD_DEFAULT, c"responsibility_spawnattrs_setdisclaim".as_ptr())
    };
    if sym.is_null() {
        return None;
    }

    // SAFETY: `SetDisclaimFn` is the true type of `responsibility_spawnattrs_setdisclaim`; see the
    // documentation of `SetDisclaimFn` for more information.
    Some(unsafe { std::mem::transmute::<*mut c_void, SetDisclaimFn>(sym) })
}

/// Convert an [`OsString`] into a [`CString`].
fn os_to_cstring<T>(s: T) -> std::io::Result<CString>
where
    T: Into<OsString>,
{
    // This matches `std::process::Command`, which returns `InvalidInput` if there are any null
    // bytes in the command or args.
    CString::new(s.into().into_vec())
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidInput, e))
}

/// Convert a slice of [`CString`]s to a null-terminated slice of raw C strings
/// (`*mut libc::c_char`).
// `use<'_>` keeps `strings` borrowed for as long as the return type exists, to ensure the pointers
// remain valid.
fn null_terminated_string_array(strings: &[CString]) -> impl AsRef<[*mut libc::c_char]> + use<'_> {
    let strings_with_null: Vec<*mut libc::c_char> = strings
        .iter()
        .map(|s| s.as_ptr().cast_mut())
        .chain(std::iter::once(std::ptr::null_mut()))
        .collect();
    strings_with_null
}

/// Convert the return value of a `posix_spawn*` function to an [`std::io::Result`].
///
/// `posix_spawn*` functions return an error number rather than using `errno`.
fn posix_spawn_result(ret: c_int) -> std::io::Result<()> {
    if ret == 0 {
        Ok(())
    } else {
        Err(std::io::Error::from_raw_os_error(ret))
    }
}

/// RAII wrappers around C types; declared as a separate module to enforce Rust's visibility rules.
mod wrappers {
    use std::mem::MaybeUninit;

    use super::posix_spawn_result;

    /// An initialized `posix_spawnattr_t`, destroyed on drop.
    pub struct SpawnAttr<'a>(&'a mut MaybeUninit<libc::posix_spawnattr_t>);

    impl<'a> SpawnAttr<'a> {
        pub fn new(storage: &'a mut MaybeUninit<libc::posix_spawnattr_t>) -> std::io::Result<Self> {
            // SAFETY: `storage` is valid memory for a `posix_spawnattr_t` out-pointer.
            posix_spawn_result(unsafe { libc::posix_spawnattr_init(storage.as_mut_ptr()) })?;
            Ok(Self(storage))
        }

        pub fn as_ptr(&self) -> *const libc::posix_spawnattr_t {
            self.0.as_ptr()
        }

        pub fn as_mut_ptr(&mut self) -> *mut libc::posix_spawnattr_t {
            self.0.as_mut_ptr()
        }
    }

    impl Drop for SpawnAttr<'_> {
        fn drop(&mut self) {
            // SAFETY: `self.0` was initialized in `new` and is destroyed exactly once.
            unsafe { libc::posix_spawnattr_destroy(self.0.as_mut_ptr()) };
        }
    }

    /// An initialized `posix_spawn_file_actions_t`, destroyed on drop.
    pub struct FileActions<'a>(&'a mut MaybeUninit<libc::posix_spawn_file_actions_t>);

    impl<'a> FileActions<'a> {
        pub fn new(
            storage: &'a mut MaybeUninit<libc::posix_spawn_file_actions_t>,
        ) -> std::io::Result<Self> {
            // SAFETY: `storage` is valid memory for a `posix_spawn_file_actions_t` out-pointer.
            posix_spawn_result(unsafe {
                libc::posix_spawn_file_actions_init(storage.as_mut_ptr())
            })?;
            Ok(Self(storage))
        }

        pub fn as_ptr(&self) -> *const libc::posix_spawn_file_actions_t {
            self.0.as_ptr()
        }

        pub fn as_mut_ptr(&mut self) -> *mut libc::posix_spawn_file_actions_t {
            self.0.as_mut_ptr()
        }
    }

    impl Drop for FileActions<'_> {
        fn drop(&mut self) {
            // SAFETY: `self.0` was initialized in `new` and is destroyed exactly once.
            unsafe { libc::posix_spawn_file_actions_destroy(self.0.as_mut_ptr()) };
        }
    }
}

use wrappers::{FileActions, SpawnAttr};

#[cfg(test)]
mod tests {
    use std::cell::{Cell, RefCell};
    use std::ffi::OsStr;
    use std::iter::empty;
    use std::path::Path;
    use std::time::{Duration, Instant};

    use rstest::rstest;

    use super::*;

    thread_local! {
        /// When the [mock `setdisclaim` is installed](install_mock_setdisclaim), the `disclaim`
        /// argument of every call to `setdisclaim` is pushed to this vector.
        static SETDISCLAIM_CALLS: RefCell<Vec<bool>> = const { RefCell::new(Vec::new()) };
    }

    /// Install a mock `setdisclaim`.
    ///
    /// If `setdisclaim_result` is [`Some`], this makes [`get_setdisclaim`] return a mock function
    /// that always returns `setdisclaim_result`, and tracks its calls in [`SETDISCLAIM_CALLS`].
    ///
    /// If `setdisclaim_result` is [`None`], this makes [`get_setdisclaim`] return [`None`].
    ///
    /// This function returns a guard that, when dropped, will unininstall the mock and reset
    /// [`SETDISCLAIM_CALLS`].
    #[must_use]
    fn install_mock_setdisclaim(setdisclaim_result: Option<c_int>) -> impl Drop {
        thread_local! {
            static SETDISCLAIM_RESULT: Cell<Option<c_int>> = const { Cell::new(None) };
        }

        unsafe extern "C" fn mock_setdisclaim(
            _attr: *mut libc::posix_spawnattr_t,
            disclaim: bool,
        ) -> c_int {
            SETDISCLAIM_CALLS.with_borrow_mut(|calls| calls.push(disclaim));
            SETDISCLAIM_RESULT.get().expect("mock_setdisclaim unexpectedly called")
        }

        let fn_ptr: SetDisclaimFn = mock_setdisclaim;
        MOCK_SETDISCLAIM.set(Some(setdisclaim_result.map(|_| fn_ptr)));
        SETDISCLAIM_RESULT.set(setdisclaim_result);
        SETDISCLAIM_CALLS.set(vec![]);

        struct Guard;

        impl Drop for Guard {
            fn drop(&mut self) {
                MOCK_SETDISCLAIM.set(None);
                SETDISCLAIM_RESULT.set(None);
                SETDISCLAIM_CALLS.set(vec![]);
            }
        }

        Guard
    }

    /// Wait for the spawned script to write its output.
    fn read_when_written(path: &Path) -> String {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            if let Ok(contents) = std::fs::read_to_string(path) {
                return contents;
            }
            assert!(Instant::now() < deadline, "child never wrote {}", path.display());
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    #[rstest]
    #[case::mock(true)]
    #[cfg_attr(target_os = "macos", case::real(false))]
    fn spawn_passes_args_and_env_and_nulls_stdio(#[case] mock: bool) {
        let _guard;
        if mock {
            _guard = install_mock_setdisclaim(Some(0));
        }

        let dir = tempfile::tempdir().unwrap();
        let out = dir.path().join("out");

        // Write through fd 3 so the checks see the shell's own stdio, then rename the file into
        // place so the test never reads it half-written.
        let script = r#"
            exec 3> "$0.tmp"
            for fd in 0 1 2; do
                [ /dev/fd/$fd -ef /dev/null ] && echo null >&3 || echo "fd $fd not null" >&3
            done
            printf '%s\n%s\n' "$1" "$PATH" >&3
            mv "$0.tmp" "$0"
        "#;
        spawn_disclaimed(OsStr::new("/bin/sh"), &[
            OsStr::new("-c"),
            OsStr::new(script),
            out.as_os_str(),
            OsStr::new("an arg"),
        ])
        .unwrap();

        assert_eq!(
            read_when_written(&out),
            format!("null\nnull\nnull\nan arg\n{}\n", std::env::var("PATH").unwrap())
        );
        if mock {
            assert_eq!(SETDISCLAIM_CALLS.take(), [true]);
        }
    }

    #[rstest]
    fn spawn_reports_a_missing_program() {
        let _guard = install_mock_setdisclaim(Some(0));

        let err =
            spawn_disclaimed(OsStr::new("/nonexistent/atuin"), empty::<&OsStr>()).unwrap_err();
        assert!(
            matches!(
                &err,
                SpawnDisclaimedError::Failed(e) if e.kind() == std::io::ErrorKind::NotFound
            ),
            "{err:?}"
        );
    }

    #[rstest]
    fn spawn_falls_back_without_setdisclaim() {
        let _guard = install_mock_setdisclaim(None);

        let err =
            spawn_disclaimed(OsStr::new("/nonexistent/atuin"), empty::<&OsStr>()).unwrap_err();
        assert!(matches!(err, SpawnDisclaimedError::Unsupported), "{err:?}");
    }

    #[rstest]
    fn spawn_reports_a_setdisclaim_error() {
        let _guard = install_mock_setdisclaim(Some(libc::EINVAL));

        let err =
            spawn_disclaimed(OsStr::new("/nonexistent/atuin"), empty::<&OsStr>()).unwrap_err();
        assert!(
            matches!(
                &err,
                SpawnDisclaimedError::Failed(e) if e.raw_os_error() == Some(libc::EINVAL)
            ),
            "{err:?}"
        );
    }
}
