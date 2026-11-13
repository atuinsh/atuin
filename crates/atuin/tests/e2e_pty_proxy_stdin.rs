//! `atuin pty-proxy` steps aside for a shell that isn't talking to a terminal.

#![cfg(unix)]

mod common;

use std::io::Read;
use std::sync::Arc;

use common::{FreshEnv, wait_until};
use parking_lot::Mutex;
use portable_pty::{CommandBuilder, PtySize, native_pty_system};
use rstest::rstest;

/// The command that the shell runs.
///
/// The sleep keeps the shell busy after it has read all of its input. The arithmetic only expands
/// if the shell ran the line, so an echo of the input can't pass. The brackets show the proxy's
/// socket, which is empty when the shell runs outside a proxy.
const COMMAND: &str = "sleep 0.2; echo piped-$((40+2))-[${ATUIN_PTY_PROXY_SOCKET-}]";

/// What the shell prints when it ran [`COMMAND`] outside a proxy.
const EXPECTED: &str = "piped-42-[]";

/// Which of the proxy's standard streams is a pipe instead of the terminal.
#[derive(Clone, Copy, Debug)]
enum Piped {
    /// The shell reads [`COMMAND`] from a pipe.
    Stdin,
    /// The shell runs [`COMMAND`] from `-c`, and its output goes through a pipe.
    Stdout,
}

impl Piped {
    fn pipeline(self, shell: &str) -> String {
        let atuin = env!("CARGO_BIN_EXE_atuin");
        match self {
            Self::Stdin => {
                format!("printf '%s\\n' '{COMMAND}' | '{atuin}' pty-proxy --shell {shell}")
            }
            Self::Stdout => {
                format!("'{atuin}' pty-proxy --shell {shell} --shell -c --shell '{COMMAND}' | cat")
            }
        }
    }
}

#[rstest]
fn a_shell_without_a_terminal_runs_outside_the_proxy(
    #[values("/bin/sh", "/bin/bash")] shell: &str,
    #[values(Piped::Stdin, Piped::Stdout)] piped: Piped,
) {
    let env = FreshEnv::new();
    let pair = native_pty_system()
        .openpty(PtySize {
            rows: 24,
            cols: 80,
            pixel_width: 0,
            pixel_height: 0,
        })
        .expect("failed to open pty");

    let mut cmd = CommandBuilder::new("/bin/sh");
    cmd.args(["-c", &piped.pipeline(shell)]);
    cmd.env_clear();
    for (key, value) in env.env_vars() {
        cmd.env(key, value);
    }
    cmd.cwd(env.home());
    let mut child = pair.slave.spawn_command(cmd).expect("failed to spawn pty proxy");
    drop(pair.slave);

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

    wait_until("the shell to exit once its input ends", || {
        child.try_wait().is_ok_and(|status| status.is_some())
    });
    wait_until("the shell's output", || output.lock().contains(EXPECTED));
}
