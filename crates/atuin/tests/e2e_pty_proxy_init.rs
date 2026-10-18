//! The pty-proxy init preserves the terminal and shell integrations across exec.
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
//!
//! Zsh tests also cover Ghostty reintegration after re-exec, including ZDOTDIR set
//! in the environment or by the user's .zshenv, and disabled integration.

#![cfg(all(unix, feature = "pty-proxy"))]

mod common;

#[allow(dead_code, reason = "the pty harness is shared with the other e2e test binaries")]
#[path = "common/pty.rs"]
mod pty;
#[allow(dead_code, reason = "the shell harness is shared with other e2e test binaries")]
#[path = "common/shell.rs"]
mod shell;

use std::collections::BTreeMap;

use atuin_common::path::PathBufExt as _;
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

#[rstest]
fn fish_keeps_its_keyboard_however_the_preamble_is_sourced(
    #[values(Sourced::FromPipe, Sourced::FromFile)] sourced: Sourced,
) {
    let Some(fish) = shell::binary("fish") else {
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
    pty.send_line("test -n \"$ATUIN_PTY_PROXY_ACTIVE\" && echo proxy=1 || echo proxy=0");
    pty.wait_for_line("proxy=1");
}

/// Model Ghostty's original injection and its documented entry point separately.
const GHOSTTY_ZSHENV: &str = r#"if [[ -n ${GHOSTTY_ZSH_ZDOTDIR+set} ]]; then
    export ZDOTDIR="$GHOSTTY_ZSH_ZDOTDIR"
    unset GHOSTTY_ZSH_ZDOTDIR
else
    unset ZDOTDIR
fi
if [[ -r "${ZDOTDIR-$HOME}/.zshenv" ]]; then
    source "${ZDOTDIR-$HOME}/.zshenv"
fi
source "$GHOSTTY_RESOURCES_DIR/shell-integration/zsh/ghostty-integration"
"#;

#[derive(Clone, Copy, Debug)]
enum Zdotdir {
    Default,
    Environment,
    ExportedInZshenv,
    LocalInZshenv,
}

#[derive(Clone, Copy, Debug)]
enum Integration {
    Enabled,
    Disabled,
}

/// Restart Zsh with integration enabled only when Ghostty enabled it for the original shell.
/// Neither exported nor shell-local ZDOTDIR values may change their startup semantics.
#[rstest]
fn zsh_preserves_ghostty_integration(
    #[values(
        Zdotdir::Default,
        Zdotdir::Environment,
        Zdotdir::ExportedInZshenv,
        Zdotdir::LocalInZshenv
    )]
    zdotdir: Zdotdir,
    #[values(Integration::Enabled, Integration::Disabled)] integration: Integration,
) {
    let Some(zsh) = shell::binary("zsh") else {
        return;
    };
    let env = FreshEnv::new();
    env.write_config("[pty_proxy]\nenabled = true\n");
    let resources = env.home().join("ghostty resources");
    let injection = resources.join("shell-integration").joined("zsh");
    std::fs::create_dir_all(&injection).unwrap();
    std::fs::write(injection.join(".zshenv"), GHOSTTY_ZSHENV).unwrap();
    std::fs::write(
        injection.join("ghostty-integration"),
        "typeset -gi _ghostty_state=0\n(( ++ghostty_loads ))\n",
    )
    .unwrap();
    let config_dir = env.home().join("zsh config");
    std::fs::create_dir_all(&config_dir).unwrap();
    let home_env = match zdotdir {
        Zdotdir::ExportedInZshenv => {
            format!("(( ++user_env_loads ))\nexport ZDOTDIR='{}'\n", config_dir.display())
        }
        Zdotdir::LocalInZshenv => {
            format!("(( ++user_env_loads ))\nZDOTDIR='{}'\n", config_dir.display())
        }
        _ => "(( ++user_env_loads ))\n".into(),
    };
    std::fs::write(env.home().join(".zshenv"), home_env).unwrap();
    std::fs::write(config_dir.join(".zshenv"), "(( ++user_env_loads ))\n").unwrap();
    let rc = format!("eval \"$(atuin init zsh)\"\nPS1='{PROMPT} '\nRPS1=''\n");
    std::fs::write(config_dir.join(".zshrc"), &rc).unwrap();
    std::fs::write(env.home().join(".zshrc"), &rc).unwrap();
    let mut vars =
        BTreeMap::from([("GHOSTTY_RESOURCES_DIR".into(), resources.display().to_string())]);
    if matches!(integration, Integration::Enabled) {
        vars.insert("ZDOTDIR".into(), injection.display().to_string());
        if matches!(zdotdir, Zdotdir::Environment) {
            vars.insert("GHOSTTY_ZSH_ZDOTDIR".into(), config_dir.display().to_string());
        }
    } else if matches!(zdotdir, Zdotdir::Environment) {
        vars.insert("ZDOTDIR".into(), config_dir.display().to_string());
    }
    let pty = PtyShell::spawn(&zsh, &[], &env, &vars);
    pty.wait_for_prompt();
    pty.send_line(r#"[[ -n $ATUIN_PTY_PROXY_ACTIVE ]] && print -r -- "integration=${ghostty_loads-0} user_env=${user_env_loads-0} proxy=1""#);
    let loads = u8::from(matches!(integration, Integration::Enabled));
    pty.wait_for_line(&format!("integration={loads} user_env=1 proxy=1"));
    pty.send_line(r#"print -r -- "zdotdir=${ZDOTDIR-unset} type=${(t)ZDOTDIR}""#);
    let expected = match zdotdir {
        Zdotdir::Default => "zdotdir=unset type=".into(),
        Zdotdir::LocalInZshenv => format!("zdotdir={} type=scalar", config_dir.display()),
        _ => format!("zdotdir={} type=scalar-export", config_dir.display()),
    };
    pty.wait_for_line(&expected);
}
