//! However the pty-proxy init is sourced, the shell it execs must keep its keyboard.
//!
//! Fish users are told to run `atuin pty-proxy init fish | source`, which puts the init script
//! itself on the sourcing shell's stdin. Fish keeps the redirections in effect across `exec`, so
//! an unqualified `exec atuin pty-proxy` hands the proxy that pipe as its stdin instead of the
//! terminal. The proxy reads EOF straight away and stops forwarding input, so the shell it hosts
//! never sees a keystroke -- nor a reply to the terminal queries it makes at startup (#4220).
//!
//! The preamble repairs that by falling back to `/dev/tty`, but only when stdin is not already a
//! terminal, so both sourcing styles are driven here: the pipe that needs the fallback, and the
//! file that must not take it.

#![cfg(all(unix, feature = "pty-proxy"))]
#![allow(clippy::disallowed_methods, reason = "tests may use std::fs for fixtures")]

mod common;

#[allow(dead_code, reason = "the pty harness is shared with the other e2e test binaries")]
#[path = "common/pty.rs"]
mod pty;
#[allow(dead_code, reason = "only `PROMPT` is needed here; `pty` reaches for it by path")]
#[path = "common/shell.rs"]
mod shell;

use std::collections::BTreeMap;
use std::path::PathBuf;

use common::FreshEnv;
use pty::PtyShell;
use rstest::rstest;
use shell::PROMPT;

/// How the pty-proxy preamble reaches the shell, and what that leaves on its stdin.
#[derive(Clone, Copy, Debug)]
enum Sourced {
    /// The setup from #4220: a pipe, so the shell that execs the proxy has no terminal on stdin.
    FromPipe,
    /// A plain file, so stdin is still the shell's own terminal and needs no repair.
    FromFile,
}

/// `config.fish` for `sourced`, minus the line that loads the preamble.
const CONFIG_FISH: &str = r#"atuin init fish | source
function fish_prompt
    echo -n "{prompt} "
end
function fish_mode_prompt
end
"#;

/// The fish binary to test against, or [`None`] when fish is not installed.
///
/// Mirrors the lookup in [`shell::Shell::start`]: `ATUIN_E2E_FISH` overrides the binary, and a
/// missing fish is a hard error only when `ATUIN_E2E_REQUIRE_SHELLS` is set.
fn fish() -> Option<PathBuf> {
    if let Some(path) = std::env::var_os("ATUIN_E2E_FISH") {
        let path = PathBuf::from(path);
        assert!(path.is_file(), "ATUIN_E2E_FISH is not a file: {}", path.display());
        return Some(path);
    }
    let found = std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default())
        .map(|dir| dir.join("fish"))
        .find(|path| path.is_file());
    if found.is_none() {
        assert!(
            std::env::var_os("ATUIN_E2E_REQUIRE_SHELLS").is_none(),
            "missing fish for e2e_pty_proxy_init"
        );
        eprintln!("skipping e2e_pty_proxy_init: missing fish");
    }
    found
}

#[rstest]
fn fish_keeps_its_keyboard_however_the_preamble_is_sourced(
    #[values(Sourced::FromPipe, Sourced::FromFile)] sourced: Sourced,
) {
    let Some(fish) = fish() else {
        return;
    };

    let env = FreshEnv::new();
    let load = match sourced {
        Sourced::FromPipe => "atuin pty-proxy init fish | source".to_string(),
        Sourced::FromFile => {
            let script = env.home().join("pty-proxy-init.fish");
            std::fs::write(&script, env.run(&["pty-proxy", "init", "fish"])).unwrap();
            format!("source {}", script.display())
        }
    };
    let config = env.home().join(".config/fish/config.fish");
    std::fs::create_dir_all(config.parent().unwrap()).unwrap();
    std::fs::write(&config, format!("{load}\n{}", CONFIG_FISH.replace("{prompt}", PROMPT)))
        .unwrap();

    let pty = PtyShell::spawn(&fish, &[], &env, &BTreeMap::new());
    pty.wait_for_prompt();

    // Typing has to reach the shell the proxy hosts, and the proxy has to have taken over -- a
    // preamble that quietly declined to exec would keep the keyboard working too.
    pty.send_line("echo proxy=$ATUIN_PTY_PROXY_ACTIVE");
    pty.wait_for_line("proxy=1");
}
