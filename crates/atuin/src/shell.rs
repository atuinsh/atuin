macro_rules! include_trimmed {
    ($path:expr) => {
        include_str!($path).trim_ascii_end()
    };
}

macro_rules! include_shell {
    ($path:expr) => {
        include_trimmed!(concat!("shell/", $path))
    };
}

pub struct Bash<'a> {
    pub include_guard: &'a str,
    pub main: &'a str,
    pub preexec: &'a str,
}

pub const BASH: Bash<'_> = Bash {
    include_guard: include_shell!("atuin.bash.d/include-guard.bash"),
    main: include_shell!("atuin.bash"),
    preexec: include_trimmed!("../vendor/bash-preexec/bash-preexec.sh"),
};

pub const FISH: &str = include_shell!("atuin.fish");
pub const NU: &str = include_shell!("atuin.nu");
pub const POWERSHELL: &str = include_shell!("atuin.ps1");
pub const XONSH: &str = include_shell!("atuin.xsh");
pub const ZSH: &str = include_shell!("atuin.zsh");

#[cfg(all(test, unix))]
mod tests {
    use std::fs;
    use std::os::unix::fs::PermissionsExt;
    use std::path::Path;
    use std::process::{Command, Output};

    use rstest::rstest;
    use tempfile::tempdir;

    use super::BASH;

    fn popup_script() -> &'static str {
        let start =
            BASH.main.find("__atuin_version_ge() {").expect("popup helpers must be present");
        let end = BASH.main[start..]
            .find("\n__atuin_history()")
            .map(|end| start + end)
            .expect("popup helpers must end before the history widget");

        &BASH.main[start..end]
    }

    fn run_bash(body: &str, env: &[(&str, &str)]) -> Output {
        let popup = popup_script();
        let script = format!("{popup}\n{body}");
        let mut command = Command::new("bash");
        command.args(["--noprofile", "--norc", "-c"]).arg(script);
        for &(key, value) in env {
            command.env(key, value);
        }

        command.output().expect("bash must run the popup test")
    }

    fn write_fake_atuin(bin_dir: &Path) {
        fs::create_dir(bin_dir).unwrap();
        let atuin = bin_dir.join("atuin");
        fs::write(
            &atuin,
            r#"#!/bin/sh
{
    printf 'call\n'
    printf 'shell=%s\n' "$ATUIN_SHELL"
    printf 'query=%s\n' "$ATUIN_QUERY"
    printf 'arg=%s\n' "$@"
} >> "$ATUIN_CALLS_FILE"

result_file=
while [ "$#" -gt 0 ]; do
    if [ "$1" = "--result-file" ]; then
        result_file=$2
        shift 2
    else
        shift
    fi
done

if [ "$WRITE_RESULT" = true ] && [ -n "$result_file" ]; then
    printf selected > "$result_file"
fi
exit 0
"#,
        )
        .unwrap();

        let mut permissions = fs::metadata(&atuin).unwrap().permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(atuin, permissions).unwrap();
    }

    fn has_arg_pair(args: &[&str], flag: &str, value: &str) -> bool {
        args.windows(2).any(|pair| pair == [flag, value])
    }

    #[rstest]
    #[case::tmux_first("true", "1", "3.2", "1", "0.44.1", "0|tmux")]
    #[case::zellij_fallback("true", "1", "3.1", "1", "0.44.1", "0|zellij")]
    #[case::zellij_only("true", "", "3.2", "1", "0.44.1", "0|zellij")]
    #[case::zellij_too_old("true", "", "3.2", "1", "0.44.0", "1|")]
    #[case::disabled("false", "1", "3.2", "1", "0.44.1", "1|")]
    #[case::malformed_versions("true", "1", "invalid", "1", "invalid", "1|")]
    fn popup_backend_selection(
        #[case] enabled: &str,
        #[case] tmux: &str,
        #[case] tmux_version: &str,
        #[case] zellij: &str,
        #[case] zellij_version: &str,
        #[case] expected: &str,
    ) {
        let output = run_bash(
            r#"
tmux() { printf 'tmux %s\n' "$TMUX_VERSION"; }
zellij() { printf 'zellij %s\n' "$ZELLIJ_VERSION"; }
backend=$(__atuin_popup_backend)
status=$?
printf '%s|%s' "$status" "$backend"
"#,
            &[
                ("ATUIN_POPUP_ENABLED", enabled),
                ("TMUX", tmux),
                ("TMUX_VERSION", tmux_version),
                ("ZELLIJ", zellij),
                ("ZELLIJ_VERSION", zellij_version),
            ],
        );

        assert!(output.status.success());
        assert_eq!(String::from_utf8(output.stdout).unwrap(), expected);
        assert!(output.stderr.is_empty());
    }

    #[rstest]
    #[case::selection(0, "true", "0|selected", 1)]
    #[case::launcher_failure(42, "true", "42|", 0)]
    #[case::missing_result(0, "false", "1|", 1)]
    fn popup_launcher_result_contract(
        #[values("tmux", "zellij")] backend: &str,
        #[case] launch_status: i32,
        #[case] write_result: &str,
        #[case] expected: &str,
        #[case] expected_calls: usize,
    ) {
        let temp = tempdir().unwrap();
        let popup_dir = temp.path().join("popup");
        let bin_dir = temp.path().join("bin");
        let launcher_args_file = temp.path().join("launcher-args");
        let atuin_calls_file = temp.path().join("atuin-calls");
        fs::create_dir(&popup_dir).unwrap();
        write_fake_atuin(&bin_dir);

        let current_path = std::env::var("PATH").unwrap_or_default();
        let path = format!("{}:{current_path}", bin_dir.display());
        let launch_status = launch_status.to_string();
        let (tmux, zellij) = match backend {
            "tmux" => ("1", ""),
            "zellij" => ("", "1"),
            _ => unreachable!(),
        };

        let output = run_bash(
            r#"
mktemp() { printf '%s\n' "$POPUP_DIR"; }
tmux() {
    if [[ $1 == -V ]]; then
        printf 'tmux 3.2\n'
        return
    fi
    printf '%s\n' "$@" > "$LAUNCHER_ARGS_FILE"
    if ((LAUNCH_STATUS != 0)); then
        return "$LAUNCH_STATUS"
    fi
    while [[ $# -gt 0 && $1 != -- ]]; do shift; done
    [[ $1 == -- ]] || return 2
    shift
    "$@"
}
zellij() {
    if [[ $1 == --version ]]; then
        printf 'zellij 0.44.1\n'
        return
    fi
    printf '%s\n' "$@" > "$LAUNCHER_ARGS_FILE"
    if ((LAUNCH_STATUS != 0)); then
        return "$LAUNCH_STATUS"
    fi
    while [[ $# -gt 0 && $1 != -- ]]; do shift; done
    [[ $1 == -- ]] || return 2
    shift
    "$@"
}
READLINE_LINE="echo 'quoted query'"
output=$(__atuin_search_cmd --shell-up-key-binding)
status=$?
printf '%s|%s' "$status" "$output"
"#,
            &[
                ("ATUIN_POPUP_ENABLED", "true"),
                ("ATUIN_POPUP_WIDTH", "70%"),
                ("ATUIN_POPUP_HEIGHT", "50%"),
                ("TMUX", tmux),
                ("ZELLIJ", zellij),
                ("ATUIN_SESSION", "session"),
                ("POPUP_DIR", popup_dir.to_str().unwrap()),
                ("LAUNCHER_ARGS_FILE", launcher_args_file.to_str().unwrap()),
                ("ATUIN_CALLS_FILE", atuin_calls_file.to_str().unwrap()),
                ("WRITE_RESULT", write_result),
                ("LAUNCH_STATUS", launch_status.as_str()),
                ("PATH", path.as_str()),
            ],
        );

        assert!(output.status.success());
        assert_eq!(String::from_utf8(output.stdout).unwrap(), expected);
        assert!(output.stderr.is_empty());

        let launcher_args = fs::read_to_string(launcher_args_file).unwrap();
        let args = launcher_args.lines().collect::<Vec<_>>();
        let current_dir = std::env::current_dir().unwrap();
        let current_dir = current_dir.to_str().unwrap();
        assert!(launcher_args.contains("--result-file"));
        assert!(!launcher_args.contains("2>"));

        if backend == "zellij" {
            assert!(args.starts_with(&["action", "new-pane"]));
            assert!(args.contains(&"--floating"));
            assert!(args.contains(&"--name="));
            assert!(args.contains(&"--block-until-exit"));
            assert!(args.contains(&"--close-on-exit"));
            assert!(has_arg_pair(&args, "--cwd", current_dir));
            assert!(has_arg_pair(&args, "--width", "70%"));
            assert!(has_arg_pair(&args, "--height", "50%"));
        } else {
            assert!(args.starts_with(&["display-popup"]));
            assert_eq!(args.iter().filter(|arg| **arg == "-E").count(), 2);
            assert!(has_arg_pair(&args, "-d", current_dir));
            assert!(has_arg_pair(&args, "-w", "70%"));
            assert!(has_arg_pair(&args, "-h", "50%"));
        }

        let atuin_calls = fs::read_to_string(atuin_calls_file).unwrap_or_default();
        assert_eq!(atuin_calls.lines().filter(|line| *line == "call").count(), expected_calls);
        if expected_calls != 0 {
            assert!(atuin_calls.contains("shell=bash"));
            assert!(atuin_calls.contains("query=echo 'quoted query'"));
            assert!(atuin_calls.contains("arg=--result-file"));
            assert!(atuin_calls.contains("arg=--shell-up-key-binding"));
        }
    }
}
