//! CLI smoke tests with an empty home and data directory.

#![cfg(unix)]

mod common;

use common::{FreshEnv, Process, SESSION};
use rstest::{fixture, rstest};

#[fixture]
fn env() -> FreshEnv {
    FreshEnv::new()
}

fn stdout(out: &std::process::Output) -> String {
    String::from_utf8_lossy(&out.stdout).into_owned()
}

#[rstest]
fn version_runs(env: FreshEnv) {
    let out = Process::spawn(env.atuin(&["--version"])).wait();
    assert!(out.status.success());
    assert!(stdout(&out).starts_with("atuin "));
}

/// Regression for #3998: history list failed when the encryption key was missing.
#[rstest]
fn fresh_history_list_is_empty_and_bootstraps_data_dir(env: FreshEnv) {
    let mut command = env.atuin(&["history", "list"]);
    command.env("ATUIN_SESSION", SESSION);
    let out = Process::spawn(command).wait();
    assert!(out.status.success(), "stderr: {}", String::from_utf8_lossy(&out.stderr));
    assert_eq!(stdout(&out).trim(), "");

    assert!(env.data_dir().join("key").is_file(), "encryption key was not auto-generated");
    assert!(env.data_dir().join("history.db").is_file());
}

/// `atuin key` must read an existing key without creating or replacing one.
#[rstest]
fn key_loads_but_never_generates(env: FreshEnv) {
    let out = Process::spawn(env.atuin(&["key"])).wait();
    assert!(!out.status.success(), "atuin key should fail before a key exists");
    assert!(!env.data_dir().join("key").exists());

    let mut command = env.atuin(&["history", "list"]);
    command.env("ATUIN_SESSION", SESSION);
    let out = Process::spawn(command).wait();
    assert!(out.status.success());

    let key = std::fs::read(env.data_dir().join("key")).unwrap();
    let out = Process::spawn(env.atuin(&["key"])).wait();
    assert!(out.status.success());
    assert_eq!(std::fs::read(env.data_dir().join("key")).unwrap(), key);
    assert_eq!(stdout(&out).split_whitespace().count(), 24, "expected a 24-word mnemonic");
}

#[rstest]
#[case::bash("bash")]
#[case::zsh("zsh")]
#[case::fish("fish")]
#[case::nu("nu")]
fn init_emits_shell_setup(env: FreshEnv, #[case] shell: &str) {
    let out = Process::spawn(env.atuin(&["init", shell])).wait();
    assert!(out.status.success());
    assert!(stdout(&out).contains("ATUIN_SESSION"));
}

#[rstest]
fn init_zsh_registers_hooks(env: FreshEnv) {
    let out = Process::spawn(env.atuin(&["init", "zsh"])).wait();
    assert!(out.status.success());
    assert!(stdout(&out).contains("autoload -U add-zsh-hook"));
}

#[rstest]
fn doctor_runs_on_fresh_install(env: FreshEnv) {
    let out = Process::spawn(env.atuin(&["doctor"])).wait();
    assert!(out.status.success(), "stderr: {}", String::from_utf8_lossy(&out.stderr));
    assert!(stdout(&out).starts_with("Atuin Doctor"));
}
