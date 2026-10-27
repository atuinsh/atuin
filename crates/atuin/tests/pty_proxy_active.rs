//! Tests for `atuin __internal pty-proxy-active`.

use std::process::{Command, Output, Stdio};

use rstest::{fixture, rstest};

/// Run the check with every standard descriptor redirected, so the answer is deterministic:
/// with no terminal to derive from, this process cannot belong to a proxy.
#[fixture]
fn output() -> Output {
    Command::new(env!("CARGO_BIN_EXE_atuin"))
        .args(["__internal", "pty-proxy-active"])
        .stdin(Stdio::null())
        .output()
        .expect("failed to run atuin")
}

#[rstest]
fn a_check_that_ran_exits_zero_even_when_the_answer_is_no(output: Output) {
    // The exit status reports whether the check ran, not what it found. Shell integration relies
    // on that distinction: an atuin too old to know this command exits non-zero, and treating
    // that as "no proxy here" makes the shell exec a proxy whose shell repeats the mistake.
    assert!(
        output.status.success(),
        "exit {:?}, stderr: {}",
        output.status.code(),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[rstest]
fn the_answer_is_reported_on_stdout(output: Output) {
    let answer = String::from_utf8_lossy(&output.stdout);

    assert_eq!(answer.trim(), "0", "with no terminal there is no proxy to be attached to");
}
