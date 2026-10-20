//! `atuin pty-proxy` ends its session when its own input ends.

#![cfg(unix)]

mod common;

use std::io::Read;
use std::sync::Arc;

use common::{FreshEnv, wait_until};
use parking_lot::Mutex;
use portable_pty::{CommandBuilder, PtySize, native_pty_system};
use rstest::rstest;

#[rstest]
fn piped_input_runs_through_the_shell_and_then_ends_it() {
    // A terminal hands the end of its input to the shell as an EOF character. A proxy reading a
    // pipe has to do the same, or its shell waits for more input forever.
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
    cmd.args([
        "-c",
        &format!(
            "printf 'echo piped-$((40+2))\\n' | '{}' pty-proxy --shell /bin/sh",
            env!("CARGO_BIN_EXE_atuin")
        ),
    ]);
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

    wait_until("the proxy to exit once its input ends", || {
        child.try_wait().is_ok_and(|status| status.is_some())
    });
    // The arithmetic only expands if the shell ran the line, so an echo of the input can't pass.
    wait_until("the shell's output", || output.lock().contains("piped-42"));
}
