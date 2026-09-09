//! `atuin pty-proxy` follows the working directory of the shell it hosts.
//!
//! Terminal multiplexers report a pane's working directory by reading it off the process in the
//! foreground of the pane's terminal — tmux calls `tcgetpgrp` on the pane's pty, then reads that
//! process's working directory. Inside a proxy that process is the proxy itself, so unless the
//! proxy follows its shell, a `cd` is invisible and the pane stays pinned to wherever the proxy
//! was launched.

#![cfg(unix)]

mod common;

use std::io::{Read, Write};
use std::sync::Arc;

use atuin_common::os::unix::process;
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
        let pair = native_pty_system()
            .openpty(PtySize {
                rows: 24,
                cols: 80,
                pixel_width: 0,
                pixel_height: 0,
            })
            .expect("failed to open pty");

        let mut cmd = CommandBuilder::new(env!("CARGO_BIN_EXE_atuin"));
        cmd.args(["pty-proxy", "--shell", "/bin/sh"]);
        cmd.env_clear();
        for (key, value) in env.env_vars() {
            cmd.env(key, value);
        }
        cmd.env("PS1", format!("{PROMPT} "));
        cmd.cwd(env.home());

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
    let home = env.home().canonicalize().unwrap();
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
