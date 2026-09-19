use std::path::PathBuf;
use std::process::Command;

use rstest::rstest;

#[rstest]
fn zsh_preserves_external_osc_133_markers_when_proxy_is_inactive(
    #[values(None, Some(false))] proxy_active: Option<bool>,
) {
    if !zsh_available() {
        eprintln!("skipping zsh OSC 133 test: zsh is not installed");
        return;
    }

    let output = run_zsh_prompt_wrapper(proxy_active);

    assert!(
        output.status.success(),
        "zsh exited {:?}\nstdout: {}\nstderr: {}",
        output.status.code(),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[rstest]
fn zsh_wraps_prompt_once_when_proxy_is_active() {
    if !zsh_available() {
        eprintln!("skipping zsh OSC 133 test: zsh is not installed");
        return;
    }

    let output = run_zsh_prompt_wrapper(Some(true));

    assert!(
        output.status.success(),
        "zsh exited {:?}\nstdout: {}\nstderr: {}",
        output.status.code(),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

fn zsh_available() -> bool {
    Command::new("zsh").arg("--version").output().is_ok()
}

fn run_zsh_prompt_wrapper(proxy_active: Option<bool>) -> std::process::Output {
    let integration = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src/shell/atuin.zsh");
    let script = if proxy_active == Some(true) {
        r#"
set -u
atuin() { return 0 }
ATUIN_SESSION=test-session
ATUIN_SHLVL=$SHLVL
source "$1"
PROMPT="${__atuin_osc133_prompt_start}left${__atuin_osc133_prompt_end}"
RPROMPT="right${__atuin_osc133_prompt_end}"
__atuin_pty_proxy_owns_tty=1
__atuin_osc133_wrap_prompt
[[ $PROMPT == "${__atuin_osc133_prompt_start}left" ]] || exit 1
[[ $RPROMPT == "right${__atuin_osc133_prompt_end}" ]] || exit 1
"#
    } else {
        r#"
set -u
atuin() { return 0 }
ATUIN_SESSION=test-session
ATUIN_SHLVL=$SHLVL
source "$1"
external_start=$'%{\033]133;A;cl=line\a%}'
external_end=$'%{\033]133;B\a%}'
PROMPT="${external_start}left${external_end}"
RPROMPT="right${external_end}"
original_prompt=$PROMPT
original_rprompt=$RPROMPT
if [[ "$2" = false ]]; then
    __atuin_pty_proxy_owns_tty=0
else
    unset __atuin_pty_proxy_owns_tty
fi
__atuin_osc133_wrap_prompt
[[ $PROMPT == $original_prompt ]] || exit 1
[[ $RPROMPT == $original_rprompt ]] || exit 1
"#
    };

    Command::new("zsh")
        .args(["-f", "-c", script, "zsh"])
        .arg(&integration)
        .arg(if proxy_active == Some(false) {
            "false"
        } else {
            "unset"
        })
        .output()
        .expect("failed to run zsh")
}
