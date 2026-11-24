//! `atuin pty-proxy` follows the program in the foreground of the shell it hosts.
//!
//! Terminals and multiplexers learn what runs in them from the process in the foreground of their
//! pty. tmux calls `tcgetpgrp` on a pane's pty and reads that process's working directory, and a
//! terminal names a tab after it. Inside a proxy that process is the proxy itself, so unless the
//! proxy follows its shell, a `cd` is invisible, the pane stays pinned to wherever the proxy was
//! launched, and every tab is called `atuin`.

#![cfg(unix)]

mod common;

use std::fs::Permissions;
use std::io::{Read, Write};
use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use atuin_common::os::unix::process;
use atuin_common::path::AbsPathBuf;
use common::{FreshEnv, wait_until};
use parking_lot::Mutex;
use portable_pty::{CommandBuilder, PtySize, native_pty_system};
use rstest::rstest;
use rustix::process::Pid;

/// Prompt of the shell under test, so the test can tell when it is ready for input.
const PROMPT: &str = "pty-proxy-e2e>";

/// An `atuin pty-proxy` running on a terminal of its own.
struct Proxy {
    child: Box<dyn portable_pty::Child + Send + Sync>,
    writer: Box<dyn Write + Send>,
    output: Arc<Mutex<String>>,
    pid: Pid,
    // Keep the terminal open for as long as the proxy runs on it.
    _master: Box<dyn portable_pty::MasterPty + Send>,
}

impl Proxy {
    /// Start a proxy hosting `/bin/sh`, in `env`'s home directory.
    fn start(env: &FreshEnv) -> Self {
        Self::start_after(env, env.home(), "")
    }

    /// Start a proxy hosting `/bin/sh` once `prepare` has run in `cwd`.
    ///
    /// `prepare` runs in the shell that execs the proxy, so it can take away access to `cwd`
    /// after the shell has entered it.
    fn start_after(env: &FreshEnv, cwd: &Path, prepare: &str) -> Self {
        let pair = native_pty_system()
            .openpty(PtySize {
                rows: 24,
                cols: 80,
                pixel_width: 0,
                pixel_height: 0,
            })
            .expect("failed to open pty");

        // The script sets `PS1` itself because bash, which is `/bin/sh` on macOS, unsets `PS1` in
        // a non-interactive shell.
        let script = format!(
            "{prepare}\nexport PS1='{PROMPT} '\nexec '{}' pty-proxy --shell /bin/sh",
            env!("CARGO_BIN_EXE_atuin")
        );
        let mut cmd = CommandBuilder::new("/bin/sh");
        cmd.args(["-c", &script]);
        cmd.env_clear();
        for (key, value) in env.env_vars() {
            cmd.env(key, value);
        }
        cmd.cwd(cwd);

        let child = pair.slave.spawn_command(cmd).expect("failed to spawn pty proxy");
        drop(pair.slave);
        let pid = Pid::from_raw(child.process_id().expect("proxy has a pid").cast_signed())
            .expect("proxy pid is not zero");

        // Read continuously: a full terminal buffer would otherwise block the proxy.
        let output = Arc::new(Mutex::new(String::new()));
        let mut reader = pair.master.try_clone_reader().expect("failed to clone pty reader");
        let read_into = Arc::clone(&output);
        std::thread::spawn(move || {
            let mut buf = [0u8; 4096];
            while let Ok(n) = reader.read(&mut buf) {
                if n == 0 {
                    break;
                }
                read_into.lock().push_str(&String::from_utf8_lossy(&buf[..n]));
            }
        });

        Self {
            child,
            writer: pair.master.take_writer().expect("failed to take pty writer"),
            output,
            pid,
            _master: pair.master,
        }
    }

    fn saw(&self, needle: &str) -> bool {
        self.output.lock().contains(needle)
    }

    fn send_line(&mut self, line: &str) {
        writeln!(self.writer, "{line}").expect("failed to write to pty");
        self.writer.flush().expect("failed to flush pty");
    }

    /// Wait for the proxy's own working directory to become `expected`.
    fn wait_for_cwd(&self, expected: &std::path::Path) {
        wait_until(&format!("the proxy to follow its shell into {}", expected.display()), || {
            process::cwd(self.pid).as_deref() == Some(expected)
        });
    }
}

impl Drop for Proxy {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[rstest]
fn the_proxy_follows_its_shell_between_directories() {
    let env = FreshEnv::new();
    // A working directory is never a symlink, but a temporary directory may be (/tmp on macOS).
    let home = AbsPathBuf::canonicalize(env.home()).unwrap();
    let elsewhere = home.join("elsewhere");
    std::fs::create_dir(&elsewhere).unwrap();

    let mut proxy = Proxy::start(&env);
    wait_until("a shell prompt inside the proxy", || proxy.saw(PROMPT));
    assert_eq!(
        process::cwd(proxy.pid).as_deref(),
        Some(home.as_path()),
        "the proxy starts where it was launched"
    );

    proxy.send_line(&format!("cd '{}'", elsewhere.display()));
    proxy.wait_for_cwd(&elsewhere);

    // And back again: following is not a one-time reading.
    proxy.send_line(&format!("cd '{}'", home.display()));
    proxy.wait_for_cwd(&home);
}

#[rstest]
fn the_proxy_titles_the_terminal_after_the_program_in_the_foreground() {
    let env = FreshEnv::new();
    let mut proxy = Proxy::start(&env);
    wait_until("a shell prompt inside the proxy", || proxy.saw(PROMPT));

    proxy.send_line("sleep 30");
    wait_until("the proxy to title the terminal after sleep", || proxy.saw("\x1b]0;sleep\x07"));
}

/// A directory whose permissions are restored on drop, so that its temporary home can be removed.
struct Restore(PathBuf);

impl Drop for Restore {
    fn drop(&mut self) {
        let _ = std::fs::set_permissions(&self.0, Permissions::from_mode(0o755));
    }
}

/// Wait for the shell in `proxy` to print the `here` file in its cwd.
fn shell_reads_here(proxy: &mut Proxy, contents: &str) {
    wait_until("a shell prompt inside the proxy", || proxy.saw(PROMPT));
    proxy.send_line("cat here");
    wait_until(&format!("the shell to read {contents:?}"), || proxy.saw(contents));
}

#[rstest]
fn the_shell_starts_in_a_cwd_that_the_user_cant_reach_by_path() {
    if rustix::process::geteuid().is_root() {
        return;
    }
    let env = FreshEnv::new();
    let locked = env.home().join("locked");
    let cwd = locked.join("cwd");
    std::fs::create_dir_all(&cwd).unwrap();
    std::fs::write(cwd.join("here"), "in-the-launch-dir").unwrap();
    let _restore = Restore(locked);

    let mut proxy = Proxy::start_after(&env, &cwd, "chmod 000 ..");
    shell_reads_here(&mut proxy, "in-the-launch-dir");
}

#[rstest]
fn the_shell_starts_at_home_from_a_cwd_that_the_user_cant_search() {
    if rustix::process::geteuid().is_root() {
        return;
    }
    let env = FreshEnv::new();
    std::fs::write(env.home().join("here"), "in-the-home-dir").unwrap();
    let sealed = Restore(env.home().join("sealed"));
    std::fs::create_dir(&sealed.0).unwrap();

    let mut proxy = Proxy::start_after(&env, &sealed.0, "chmod 000 .");
    shell_reads_here(&mut proxy, "in-the-home-dir");
}
