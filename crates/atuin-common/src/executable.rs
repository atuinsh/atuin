//! Helpers for finding programs in `$PATH`.

use std::borrow::Cow;
use std::path::{Path, PathBuf};

use crate::path::PathBufExt as _;

/// Whether `program` is an executable file. Searches `$PATH` when given a bare name.
///
/// Performs the same steps as [`resolve_in_path`].
#[must_use]
pub fn is_executable_with_path_search(program: impl AsRef<Path>) -> bool {
    resolve_in_path(program.as_ref()).is_some()
}

/// Resolve `program` to a path to an executable file. Searches `$PATH` when given a bare name.
///
/// If `program` contains a path separator, this function returns `program` if it's executable,
/// otherwise [`None`].
///
/// If `program` is a bare name, this function searches `$PATH` for it. If a directory in `$PATH`
/// contains an executable file named `program`, that path is returned; otherwise [`None`].
///
/// On Windows, a file counts when its extension is one of the `PATHEXT` extensions. If `program`
/// doesn't end with one of those extensions, it is also tried with each of those extensions added,
/// as cmd.exe does. For example, `claude` finds `claude.cmd` and skips the extensionless shell
/// script that npm installs next to it, and `python3.11` finds `python3.11.exe`.
#[must_use]
pub fn resolve_in_path(program: &Path) -> Option<Cow<'_, Path>> {
    let bytes = program.as_os_str().as_encoded_bytes();
    if bytes.iter().any(|&byte| std::path::is_separator(byte.into())) {
        return runnable(Cow::Borrowed(program));
    }
    let paths = crate::env::var_os("PATH")?;
    resolve_in_dirs(std::env::split_paths(&paths), program).map(Cow::Owned)
}

/// Resolve the bare name `program` to an executable file in the first of `dirs` that has one.
///
/// Programs can be found in the same ways as with [`resolve_in_path`].
#[must_use]
pub fn resolve_in_dirs(dirs: impl IntoIterator<Item = PathBuf>, program: &Path) -> Option<PathBuf> {
    resolve_all_in_dirs(dirs, program).next()
}

/// Resolve the bare name `program` to an executable file in each of `dirs` that has one, in order.
///
/// Programs can be found in the same ways as with [`resolve_in_path`].
pub fn resolve_all_in_dirs(
    dirs: impl IntoIterator<Item = PathBuf>,
    program: &Path,
) -> impl Iterator<Item = PathBuf> {
    dirs.into_iter()
        .filter_map(move |dir| runnable(Cow::Owned(dir.joined(program))).map(Cow::into_owned))
}

/// The file at `path`, if it can be run.
///
/// On Unix, the user must have permission to execute the file. On Windows, the file's extension
/// must be one of the `PATHEXT` extensions, or `path` with one of those extensions added must be a
/// file.
fn runnable(path: Cow<'_, Path>) -> Option<Cow<'_, Path>> {
    #[cfg(unix)]
    {
        // `access` applies the same rules as `exec`. For example, it takes into account which of
        // the user, group, and other execute bits apply, and whether the file is on a `noexec`
        // mount.
        let can_execute = rustix::fs::access(&*path, rustix::fs::Access::EXEC_OK).is_ok();
        (can_execute && path.is_file()).then_some(path)
    }
    #[cfg(windows)]
    {
        runnable_on_windows(path)
    }
}

/// The file at `path`, if Windows can run it. Windows has no execute bits, so a file can run when
/// its extension is one of the `PATHEXT` extensions.
///
/// If `path` doesn't already end with one of the extensions in `PATHEXT`, this function tries
/// adding each of the `PATHEXT` extensions to `path` in turn. For example, `cmd` finds `cmd.exe`,
/// and `python3.11` finds `python3.11.exe`.
///
/// This compiles on every platform, so that a build on Unix still checks it.
#[cfg_attr(not(any(windows, test)), expect(dead_code, reason = "only Windows and tests run this"))]
fn runnable_on_windows(path: Cow<'_, Path>) -> Option<Cow<'_, Path>> {
    let extensions = crate::env::var("PATHEXT").unwrap_or_else(|_| ".COM;.EXE;.BAT;.CMD".into());
    let mut extensions = extensions.split(';').filter(|ext| !ext.is_empty());
    // Every `PATHEXT` extension is UTF-8, so an extension that isn't can't match one.
    if let Some(own) = path.extension().and_then(|ext| ext.to_str())
        && extensions
            .clone()
            .any(|ext| ext.strip_prefix('.').is_some_and(|ext| ext.eq_ignore_ascii_case(own)))
        && path.is_file()
    {
        return Some(path);
    }
    extensions.find_map(|ext| {
        let mut with_ext = path.as_os_str().to_owned();
        with_ext.push(ext);
        let with_ext = PathBuf::from(with_ext);
        with_ext.is_file().then_some(Cow::Owned(with_ext))
    })
}

#[cfg(test)]
mod tests {
    use rstest::rstest;

    use super::*;
    use crate::env::MockEnv;

    #[rstest]
    fn finds_programs_on_path() {
        assert!(!is_executable_with_path_search("definitely-not-a-program-atuin"));
    }

    #[rstest]
    #[cfg_attr(not(unix), ignore = "unix programs and permissions")]
    fn finds_programs_on_path_and_by_path_on_unix() {
        assert!(is_executable_with_path_search("sh"));
        assert!(
            resolve_in_path(Path::new("sh"))
                .is_some_and(|path| path.is_absolute() && path.ends_with("sh"))
        );
        assert!(is_executable_with_path_search("/bin/sh"));
        // A path that is itself the executable comes back as it was given, without a copy.
        assert!(matches!(resolve_in_path(Path::new("/bin/sh")), Some(Cow::Borrowed(_))));
        assert!(!is_executable_with_path_search("/nonexistent/sh"));

        // A file that isn't executable can't be run.
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("not-executable");
        std::fs::write(&file, "").unwrap();
        assert!(!is_executable_with_path_search(file));
    }

    /// A directory can't be run, even one that the user can search.
    #[rstest]
    #[cfg_attr(not(unix), ignore = "unix permissions")]
    fn a_directory_is_not_runnable() {
        let dir = tempfile::tempdir().unwrap();
        assert!(!is_executable_with_path_search(dir.path()));
    }

    /// Execute bits that apply to someone else don't let the owner run the file.
    #[cfg(unix)]
    #[rstest]
    fn execute_bits_for_others_are_not_enough() {
        use std::os::unix::fs::PermissionsExt as _;

        // `root` may execute any file with an execute bit set.
        if rustix::process::geteuid().is_root() {
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("for-others");
        std::fs::write(&file, "").unwrap();
        std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o611)).unwrap();
        assert!(!is_executable_with_path_search(file));
    }

    /// [`resolve_in_dirs`] returns the first directory's program, skipping one that can't run.
    #[cfg(unix)]
    #[rstest]
    fn resolves_in_the_first_dir_that_can_run_the_program() {
        use std::os::unix::fs::PermissionsExt as _;

        // Next to the test binary: CI's temporary directory is mounted without exec.
        let exe = std::env::current_exe().unwrap();
        let dir = tempfile::tempdir_in(exe.parent().unwrap()).unwrap();
        let dirs = ["a", "b", "c"].map(|name| dir.path().join(name));
        for (sub, mode) in dirs.iter().zip([0o644, 0o755, 0o755]) {
            std::fs::create_dir(sub).unwrap();
            let program = sub.join("prog");
            std::fs::write(&program, "").unwrap();
            std::fs::set_permissions(&program, std::fs::Permissions::from_mode(mode)).unwrap();
        }
        let found = resolve_in_dirs(dirs.clone(), Path::new("prog"));
        assert_eq!(found, Some(dirs[1].join("prog")));
    }

    /// The Windows rules, checked on every platform. `PATHEXT` is lowercase here, since Unix file
    /// names are case-sensitive.
    #[rstest]
    #[case::adds_an_extension(&["claude", "claude.cmd"], "claude", Some("claude.cmd"))]
    #[case::own_extension(&["claude.exe"], "claude.exe", Some("claude.exe"))]
    #[case::own_extension_in_another_case(&["claude.EXE"], "claude.EXE", Some("claude.EXE"))]
    #[case::extension_not_in_pathext(&["notes.txt"], "notes.txt", None)]
    #[case::no_extension(&["tool"], "tool", None)]
    #[case::dotted_name(&["python3.11.exe"], "python3.11", Some("python3.11.exe"))]
    #[case::adds_to_another_extension(&["notes.txt.exe"], "notes.txt", Some("notes.txt.exe"))]
    #[case::own_extension_first(&["atuin.exe", "atuin.exe.com"], "atuin.exe", Some("atuin.exe"))]
    #[case::adds_to_an_extension_in_pathext(&["atuin.exe.com"], "atuin.exe", Some("atuin.exe.com"))]
    #[case::missing(&[], "tool", None)]
    fn windows_rules(#[case] files: &[&str], #[case] program: &str, #[case] want: Option<&str>) {
        let dir = tempfile::tempdir().unwrap();
        for file in files {
            std::fs::write(dir.path().join(file), "").unwrap();
        }
        let env = MockEnv::install();
        env.set("PATHEXT", ".com;.exe;.bat;.cmd");

        let found = runnable_on_windows(Cow::Owned(dir.path().join(program)));
        assert_eq!(found.as_deref(), want.map(|want| dir.path().join(want)).as_deref());
    }

    #[rstest]
    #[cfg_attr(not(windows), ignore = "Windows programs and PATHEXT")]
    fn finds_programs_on_path_and_by_path_on_windows() {
        // `cmd` is found as `cmd.exe` through PATHEXT, on PATH or by its path.
        assert!(is_executable_with_path_search("cmd"));
        assert!(is_executable_with_path_search("cmd.exe"));
        // What runs is the file the check found, extension and all, which spawning `cmd` would not
        // resolve to were it a `.cmd` or `.bat` script.
        let found = resolve_in_path(Path::new("cmd")).unwrap();
        assert!(found.extension().is_some_and(|ext| ext.eq_ignore_ascii_case("exe")), "{found:?}");
        let system32 = Path::new(&crate::env::var("SystemRoot").unwrap()).join("System32");
        assert!(is_executable_with_path_search(system32.join("cmd")));
        assert!(is_executable_with_path_search(system32.join("cmd.exe")));
        assert!(!is_executable_with_path_search(r"C:\nonexistent\cmd"));
    }
}
