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
/// On Windows, a file with the name plus one of the `PATHEXT` extensions also counts. For example,
/// `claude` finds `claude.cmd`, which spawning the bare name would not find. Spawning the returned
/// path runs the same file that was checked.
#[must_use]
pub fn resolve_in_path(program: &Path) -> Option<Cow<'_, Path>> {
    let bytes = program.as_os_str().as_encoded_bytes();
    if bytes.iter().any(|&byte| std::path::is_separator(byte.into())) {
        return runnable(Cow::Borrowed(program));
    }
    let paths = crate::env::var_os("PATH")?;
    std::env::split_paths(&paths).find_map(|dir| runnable(Cow::Owned(dir.joined(program))))
}

/// The file at `path`, if it can be run.
///
/// On Unix, the file needs at least one of its execute bits set. On Windows, `path` with one of the
/// `PATHEXT` extensions also counts.
fn runnable(path: Cow<'_, Path>) -> Option<Cow<'_, Path>> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;

        let meta = path.metadata().ok()?;
        (meta.is_file() && meta.permissions().mode() & 0o111 != 0).then_some(path)
    }
    #[cfg(windows)]
    {
        runnable_on_windows(path)
    }
}

/// The file at `path`, if Windows can run it. Windows has no execute bits, so any file counts.
/// `path` with one of the `PATHEXT` extensions also counts. For example, `cmd` finds `cmd.exe`.
///
/// This compiles on every platform, so that a build on Unix still checks it.
#[cfg_attr(not(windows), expect(dead_code, reason = "only Windows runs this"))]
fn runnable_on_windows(path: Cow<'_, Path>) -> Option<Cow<'_, Path>> {
    if path.is_file() {
        return Some(path);
    }
    let extensions = crate::env::var("PATHEXT").unwrap_or_else(|_| ".COM;.EXE;.BAT;.CMD".into());
    extensions.split(';').filter(|ext| !ext.is_empty()).find_map(|ext| {
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
