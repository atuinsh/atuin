use std::ffi::{OsStr, c_void};
use std::path::Path;
use std::process::Command;
use std::time::{Duration, Instant};

use rstest::rstest;

use super::*;
use crate::os::unix::process::pid_from_u32;

/// Asks macOS which process is responsible for `pid`, through a private function that sits next
/// to `responsibility_spawnattrs_setdisclaim`.
fn responsible_for(pid: Pid) -> libc::pid_t {
    type ResponsibleFn = unsafe extern "C" fn(libc::pid_t) -> libc::pid_t;
    // SAFETY: `RTLD_DEFAULT` is a valid handle, and the name is a C string.
    let symbol = unsafe {
        libc::dlsym(libc::RTLD_DEFAULT, c"responsibility_get_pid_responsible_for_pid".as_ptr())
    };
    assert!(!symbol.is_null(), "macOS has responsibility_get_pid_responsible_for_pid");
    // SAFETY: the function takes a process id and returns one.
    let responsible = unsafe { std::mem::transmute::<*mut c_void, ResponsibleFn>(symbol) };
    // SAFETY: the function has no preconditions.
    unsafe { responsible(pid.as_raw_nonzero().get()) }
}

/// Waits for the spawned script to write its output.
fn read_when_written(path: &Path) -> String {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if let Ok(contents) = std::fs::read_to_string(path) {
            return contents;
        }
        assert!(Instant::now() < deadline, "the child never wrote {}", path.display());
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// Kills and reaps the process `pid` when dropped.
struct Reap(Pid);

impl Drop for Reap {
    fn drop(&mut self) {
        let pid = self.0.as_raw_nonzero().get();
        // SAFETY: the test spawned `pid`, and nothing else reaps it.
        unsafe { libc::kill(pid, libc::SIGKILL) };
        // SAFETY: as above.
        unsafe { libc::waitpid(pid, std::ptr::null_mut(), 0) };
    }
}

#[rstest]
fn the_child_is_responsible_for_itself() {
    let child = DisclaimedCommand::new("sleep").arg("30").stdio(ChildStdio::Null).spawn().unwrap();
    let _reap = Reap(child);

    assert_eq!(responsible_for(child), child.as_raw_nonzero().get());
}

#[rstest]
fn a_plain_child_shares_the_responsible_process() {
    let mut child = Command::new("sleep").arg("30").spawn().unwrap();
    let pid = pid_from_u32(child.id()).unwrap();

    let responsible = responsible_for(pid);
    let _ = child.kill();
    let _ = child.wait();

    assert_eq!(responsible, responsible_for(pid_from_u32(std::process::id()).unwrap()));
    assert_ne!(responsible, pid.as_raw_nonzero().get());
}

#[rstest]
#[case::inherit(ChildStdio::Inherit, "inherited")]
#[case::null(ChildStdio::Null, "null")]
fn the_child_gets_its_arguments_environment_and_stdio(
    #[case] stdio: ChildStdio,
    #[case] expected_stdio: &str,
) {
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("out");
    // The script writes through fd 3 so that the checks see the shell's own stdio, and renames
    // the file into place so that the test never reads it half-written.
    let script = r#"
        exec 3> "$0.tmp"
        for fd in 0 1 2; do
            [ /dev/fd/$fd -ef /dev/null ] && echo null >&3 || echo inherited >&3
        done
        printf '%s\n%s\n%s\n' "$1" "$ATUIN_TEST_ADDED" "$PATH" >&3
        mv "$0.tmp" "$0"
    "#;

    let child = DisclaimedCommand::new("/bin/sh")
        .args([OsStr::new("-c"), OsStr::new(script), out.as_os_str(), OsStr::new("an arg")])
        .env("ATUIN_TEST_ADDED", "added")
        .stdio(stdio)
        .spawn()
        .unwrap();
    let _reap = Reap(child);

    let stdio_lines = format!("{expected_stdio}\n").repeat(3);
    let path = crate::env::var_os("PATH").unwrap();
    assert_eq!(
        read_when_written(&out),
        format!("{stdio_lines}an arg\nadded\n{}\n", path.display())
    );
}

#[rstest]
fn an_override_hides_the_inherited_variable() {
    let entries = inherited_environment(&[c_string("PATH=/replaced").unwrap()]).unwrap();

    assert!(entries.iter().all(|entry| key_of(entry) != b"PATH"), "{entries:?}");
}

#[rstest]
fn a_later_value_replaces_an_earlier_one() {
    let mut command = DisclaimedCommand::new("true");
    command.env("A", "1").env("AB", "2").env("A", "3");

    assert_eq!(command.env.unwrap(), [c"A=3", c"AB=2"]);
}

#[rstest]
fn the_child_gets_the_default_action_for_sigpipe() {
    let child =
        DisclaimedCommand::new("/bin/sh").args(["-c", "kill -PIPE $$; exit 0"]).spawn().unwrap();

    let mut status = 0;
    // SAFETY: the test spawned the child, and nothing else reaps it.
    unsafe { libc::waitpid(child.as_raw_nonzero().get(), &raw mut status, 0) };
    assert!(libc::WIFSIGNALED(status) && libc::WTERMSIG(status) == libc::SIGPIPE, "{status:#x}");
}

#[rstest]
fn a_missing_program_fails() {
    let error = DisclaimedCommand::new("/nonexistent/atuin").spawn().unwrap_err();

    assert!(
        matches!(&error, SpawnDisclaimedError::Failed(e) if e.kind() == std::io::ErrorKind::NotFound),
        "{error:?}"
    );
}

#[rstest]
fn a_nul_byte_fails() {
    let error = DisclaimedCommand::new("/bin/sh").arg("a\0b").spawn().unwrap_err();

    assert!(matches!(error, SpawnDisclaimedError::Nul(_)), "{error:?}");
}
