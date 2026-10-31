//! Workspace filters keep jj history local, including colocated Git worktrees.

#![cfg(unix)]

mod common;

use std::path::Path;
use std::process::Command;

use atuin_common::path::{AbsPathBuf, PathBufExt as _};
use rstest::{fixture, rstest};

use crate::common::{FreshEnv, Process, SESSION};

#[fixture]
fn env() -> FreshEnv {
    let env = FreshEnv::new();
    env.write_config("filter_mode = 'workspace'\n");
    let jj_config = env.home().join(".config").joined("jj");
    std::fs::create_dir_all(&jj_config).unwrap();
    std::fs::write(
        jj_config.joined("config.toml"),
        "[user]\nname = 'Atuin'\nemail = 'test@example.com'\n",
    )
    .unwrap();
    env
}

fn check_history(env: &FreshEnv, cwd: &Path, expected: &str) {
    for args in [vec!["search", "--filter-mode", "workspace", "--cmd-only", "echo"], vec![
        "history",
        "list",
        "--cmd-only",
    ]] {
        let mut command = env.atuin(&args);
        command.env("ATUIN_SESSION", SESSION).current_dir(cwd);
        let out = Process::spawn(command).wait();
        assert!(out.status.success(), "atuin {args:?} in {cwd:?}: {out:?}");
        assert!(out.stderr.is_empty(), "{out:?}");
        let stdout = String::from_utf8(out.stdout).unwrap();
        let mut commands: Vec<_> = stdout.lines().collect();
        commands.sort_unstable();
        assert_eq!(commands.join("\n"), expected);
    }
}

#[rstest]
#[case::jj_workspace(true, false)]
#[case::colocated_jj_workspace(true, true)]
#[case::plain_git_worktree(false, true)]
fn workspace_filters_use_the_active_root(env: FreshEnv, #[case] jj: bool, #[case] colocated: bool) {
    let home = AbsPathBuf::canonicalize(env.home()).unwrap().into_path_buf();
    let main = home.join("main");
    let workspace = home.join("workspace");
    let deep = workspace.join("src");
    std::fs::create_dir_all(&main).unwrap();
    std::fs::create_dir_all(&deep).unwrap();
    if jj {
        std::fs::create_dir_all(main.join(".jj").joined("repo")).unwrap();
        std::fs::create_dir_all(workspace.join(".jj")).unwrap();
        std::fs::write(workspace.join(".jj").joined("repo"), "../../main/.jj/repo").unwrap();
    }
    if colocated {
        let git_dir = main.join(".git").joined("worktrees").joined("workspace");
        std::fs::create_dir_all(&git_dir).unwrap();
        std::fs::write(git_dir.join("commondir"), "../..\n").unwrap();
        std::fs::write(workspace.join(".git"), format!("gitdir: {}", git_dir.display())).unwrap();
    }
    env.record("echo main", SESSION, &main);
    env.record("echo workspace", SESSION, &deep);
    env.record("echo outside", SESSION, env.home());

    check_history(
        &env,
        &deep,
        if jj {
            "echo workspace"
        } else {
            "echo main"
        },
    );
}

#[rstest]
#[case::metacharacters("O'Brien %_ \\ 日本語")]
#[case::predicate_injection("x') OR 1=1 --")]
#[case::statement_injection("x'); DROP TABLE history; --")]
#[case::whitespace("line\n tab\t trailing ")]
#[case::star_token("\t*\t")]
#[case::alter_token(" ALTER ")]
fn workspace_paths_are_data_in_cli_queries(env: FreshEnv, #[case] name: &str) {
    let home = AbsPathBuf::canonicalize(env.home()).unwrap().into_path_buf();
    let workspace = home.join(name);
    std::fs::create_dir_all(workspace.join(".jj").joined("repo")).unwrap();
    let deep = workspace.join("src");
    std::fs::create_dir_all(&deep).unwrap();
    let sibling = home.join(format!("{name}-sibling"));
    std::fs::create_dir_all(&sibling).unwrap();
    env.record("echo root", SESSION, &workspace);
    env.record("echo workspace", SESSION, &deep);
    env.record("echo sibling", SESSION, &sibling);
    env.record("echo outside", SESSION, &home);

    check_history(&env, &deep, "echo root\necho workspace");
    let mut global = env.atuin(&["search", "--filter-mode", "global", "--cmd-only", "echo"]);
    global.env("ATUIN_SESSION", SESSION);
    let out = Process::spawn(global).wait();
    assert!(out.status.success(), "{out:?}");
    assert!(out.stderr.is_empty(), "{out:?}");
    let stdout = String::from_utf8(out.stdout).unwrap();
    let mut commands: Vec<_> = stdout.lines().collect();
    commands.sort_unstable();
    assert_eq!(commands, ["echo outside", "echo root", "echo sibling", "echo workspace"]);
}

fn jj(env: &FreshEnv, cwd: &Path, args: &[&str]) {
    let mut command = Command::new("jj");
    command.env_clear().envs(env.env_vars()).arg("--quiet").args(args).current_dir(cwd);
    let out = Process::spawn(command).wait();
    assert!(out.status.success(), "jj {args:?}: {out:?}");
    assert!(out.stderr.is_empty(), "jj {args:?}: {out:?}");
}

#[rstest]
#[ignore = "requires jj >= 0.46.0 and Git >= 2.42.0"]
#[case::default_colocation(true, &[], true)]
#[case::explicit_colocation(true, &["--colocate"], true)]
#[case::no_colocation(true, &["--no-colocate"], false)]
#[case::config_disables_colocation(true, &["--config", "git.colocate=false"], false)]
#[case::non_colocated_main(false, &[], false)]
#[case::colocate_from_non_colocated_main(false, &["--colocate"], true)]
fn real_jj_workspace_keeps_history_when_colocation_changes(
    env: FreshEnv,
    #[case] main_colocated: bool,
    #[case] flags: &[&str],
    #[case] colocated: bool,
) {
    let main = env.home().join("main");
    let workspace = env.home().join("workspace");
    let init_flag = if main_colocated {
        "--colocate"
    } else {
        "--no-colocate"
    };
    jj(&env, env.home(), &["git", "init", init_flag, main.to_str().unwrap()]);
    let mut args = vec!["workspace", "add"];
    args.extend_from_slice(flags);
    args.push(workspace.to_str().unwrap());
    jj(&env, &main, &args);
    assert_eq!(workspace.join(".git").is_file(), colocated);
    assert!(workspace.join(".jj").joined("repo").is_file());

    let deep = workspace.join("src");
    std::fs::create_dir_all(&deep).unwrap();
    env.record("echo main", SESSION, &main);
    env.record("echo workspace", SESSION, &deep);
    env.record("echo outside", SESSION, env.home());
    check_history(&env, &deep, "echo workspace");

    let operations = if colocated {
        ["disable", "enable"]
    } else {
        ["enable", "disable"]
    };
    for operation in operations {
        jj(&env, &workspace, &["git", "colocation", operation]);
        assert_eq!(workspace.join(".git").is_file(), operation == "enable");
        check_history(&env, &deep, "echo workspace");
    }
}
