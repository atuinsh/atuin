//! PowerShell integration driven through a PTY. CI doesn't install `pwsh`, so it skips when missing.

#![cfg(unix)]

mod common;

#[allow(dead_code, reason = "the pty harness is shared with the other e2e test binaries")]
#[path = "common/pty.rs"]
mod pty;
#[allow(dead_code, reason = "only `PROMPT` and `find_shell` are needed here")]
#[path = "common/shell.rs"]
mod shell;

use std::collections::BTreeMap;
use std::path::Path;

use common::{FreshEnv, SESSION, marker, output, wait_until};
use pty::PtyShell;
use rstest::rstest;
use shell::find_shell;

/// Starts pwsh with the Atuin integration, launched from `env`'s home.
fn start(env: &FreshEnv, pwsh: &Path) -> PtyShell {
    // Finish migrations before the shell hooks can open the databases.
    env.run(&["store", "status"]);
    let init = "Import-Module PSReadLine; function prompt { 'E2E_PROMPT> ' }; atuin init \
                powershell | Out-String | Invoke-Expression";
    let args = ["-NoLogo", "-NoProfile", "-NoExit", "-Command", init].map(String::from);
    // pwsh inherits PWD from whatever launched it, like a terminal's login shell.
    let vars = BTreeMap::from([("PWD".to_owned(), env.home().display().to_string())]);
    let pty = PtyShell::spawn(pwsh, &args, env, &vars);
    pty.wait_for_prompt();
    pty
}

#[rstest]
fn history_records_the_current_location() {
    let Some(pwsh) = find_shell("pwsh") else {
        eprintln!("skipping: pwsh not installed");
        return;
    };
    let env = FreshEnv::new();
    let dir = env.home().join("sub dir");
    std::fs::create_dir(&dir).unwrap();
    let pty = start(&env, &pwsh);
    // Set-Location leaves the inherited $env:PWD pointing at the launch directory.
    pty.send_line("Set-Location 'sub dir'");
    let marker = marker();
    pty.send_line(&format!("echo {marker}"));

    let expected = dir.canonicalize().unwrap();
    wait_until("entry recorded in the current location", || {
        let mut list = env.atuin(&["history", "list", "--format", "{directory}\t{command}"]);
        list.env("ATUIN_SESSION", SESSION);
        output(list).lines().any(|l| {
            l.split_once('\t').is_some_and(|(directory, command)| {
                command == format!("echo {marker}")
                    && std::fs::canonicalize(directory).is_ok_and(|d| d == expected)
            })
        })
    });
}

#[rstest]
fn search_filters_by_the_current_location() {
    let Some(pwsh) = find_shell("pwsh") else {
        eprintln!("skipping: pwsh not installed");
        return;
    };
    let env = FreshEnv::new();
    env.write_config("filter_mode = \"directory\"\nenter_accept = true\n");
    let dir = env.home().join("sub dir");
    std::fs::create_dir(&dir).unwrap();
    let (here, launch) = (marker(), marker());
    env.record(&format!("echo {here}"), SESSION, &dir.canonicalize().unwrap());
    env.record(&format!("echo {launch}"), SESSION, &env.home().canonicalize().unwrap());
    let pty = start(&env, &pwsh);
    pty.send_line("Set-Location 'sub dir'");

    pty.send_ctrl_r();
    pty.wait_for(&format!("echo {here}"));
    // Accept runs the selected, most recent visible entry: `launch` if the filter used the
    // launch directory.
    pty.send_enter();
    pty.wait_for_line(&here);
    assert!(!pty.screen().contains(&launch), "search used the launch directory");
}
