//! CLI fresh-install tests with an empty home and data directory.

#![cfg(unix)]

mod common;

use atuin_common::path::PathBufExt as _;
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

    assert!(env.data_dir().joined("key").is_file(), "encryption key was not auto-generated");
    assert!(env.data_dir().joined("history.db").is_file());
    assert!(env.atuin_home().joined("config.toml").is_file());
    assert!(!env.home().join(".local/share/atuin").exists());
    assert!(!env.home().join(".config/atuin").exists());
}

/// The data dir and the paths in it agree on whether `data_dir` in config.toml or
/// `ATUIN_DATA_DIR` wins (the environment does).
#[rstest]
fn data_dir_and_the_paths_in_it_agree(env: FreshEnv, #[values(false, true)] env_var: bool) {
    let from_config = env.home().join("from-config");
    let from_env = env.home().join("from-env");
    env.write_config(&format!("data_dir = {:?}\n", from_config.display().to_string()));

    let resolved = |key: &str| {
        let mut command = env.atuin(&["config", "get", key, "--resolved"]);
        if env_var {
            command.env("ATUIN_DATA_DIR", &from_env);
        }
        let out = Process::spawn(command).wait();
        assert!(out.status.success(), "stderr: {}", String::from_utf8_lossy(&out.stderr));
        std::path::PathBuf::from(stdout(&out).trim())
    };

    let expected = if env_var {
        &from_env
    } else {
        &from_config
    };
    assert_eq!(&resolved("data_dir"), expected);
    assert_eq!(resolved("db_path"), expected.join("history.db"));
    assert_eq!(resolved("meta.db_path"), expected.join("meta.db"));
}

/// A relative path in config.toml lands beside config.toml, not in the directory Atuin runs in.
#[rstest]
fn relative_paths_resolve_next_to_config_toml(env: FreshEnv) {
    env.write_config("data_dir = \"relative-data\"\ndb_path = \"relative-history.db\"\n");
    let cwd = env.home().join("tmp");

    let mut command = env.atuin(&["history", "list"]);
    command.env("ATUIN_SESSION", SESSION).current_dir(&cwd);
    let out = Process::spawn(command).wait();
    assert!(out.status.success(), "stderr: {}", String::from_utf8_lossy(&out.stderr));

    assert!(env.atuin_home().joined("relative-data").joined("key").is_file());
    assert!(env.atuin_home().joined("relative-history.db").is_file());
    assert!(!cwd.join("relative-data").exists());
    assert!(!cwd.join("relative-history.db").exists());
}

/// `config get --resolved` prints a path as Atuin opens it, not as config.toml or Atuin's escaped
/// defaults write it.
#[rstest]
#[case::relative("db_path = \"relative.db\"", "db_path", "{atuin_home}/relative.db")]
#[case::variable("[meta]\ndb_path = \"$NAME.db\"", "meta.db_path", "{atuin_home}/atuin-test.db")]
#[case::log_file("[logs.search]\nfile = \"$NAME.log\"", "logs.search.file", "atuin-test.log")]
#[case::socket(
    "[daemon]\nsocket_path = \"atuin.sock\"",
    "daemon.socket_path",
    "{atuin_home}/atuin.sock"
)]
#[case::dollar_data_dir(
    "data_dir = \"{atuin_home}/price$$usd\"",
    "data_dir",
    "{atuin_home}/price$usd"
)]
#[case::dollar_default(
    "data_dir = \"{atuin_home}/price$$usd\"",
    "db_path",
    "{atuin_home}/price$usd/history.db"
)]
fn resolved_paths_print_as_opened(
    env: FreshEnv,
    #[case] config: &str,
    #[case] key: &str,
    #[case] expected: &str,
) {
    let atuin_home = env.atuin_home().display().to_string();
    env.write_config(&config.replace("{atuin_home}", &atuin_home));

    let mut command = env.atuin(&["config", "get", key, "--resolved"]);
    // The harness's private socket would win over config.toml's.
    command.env("NAME", "atuin-test").env_remove("ATUIN_DAEMON__SOCKET_PATH");
    let out = Process::spawn(command).wait();
    assert!(out.status.success(), "stderr: {}", String::from_utf8_lossy(&out.stderr));
    assert_eq!(stdout(&out).trim(), expected.replace("{atuin_home}", &atuin_home));
}

/// An install from before `$ATUIN_HOME` keeps its XDG data and config where they are.
#[rstest]
fn existing_xdg_install_is_used_in_place(env: FreshEnv) {
    let legacy_data = env.home().join(".local/share/atuin");
    let legacy_config = env.home().join(".config/atuin");
    std::fs::create_dir_all(&legacy_data).unwrap();
    std::fs::create_dir_all(&legacy_config).unwrap();
    let db_path = env.home().join("legacy-config-was-read.db");
    std::fs::write(
        legacy_config.joined("config.toml"),
        format!("db_path = {:?}\n", db_path.display().to_string()),
    )
    .unwrap();

    let mut command = env.atuin(&["history", "list"]);
    command.env("ATUIN_SESSION", SESSION);
    let out = Process::spawn(command).wait();
    assert!(out.status.success(), "stderr: {}", String::from_utf8_lossy(&out.stderr));

    assert!(legacy_data.joined("key").is_file(), "key was not generated in the legacy data dir");
    assert!(db_path.is_file(), "the legacy config.toml was not read");
    assert!(!env.data_dir().exists(), "a new data dir was created alongside the legacy one");
    assert!(!env.atuin_home().joined("config.toml").exists());
}

/// `$ATUIN_HOME` holds a whole, separate installation, even alongside an install from before
/// `$ATUIN_HOME` that it would otherwise fall back to.
#[rstest]
fn atuin_home_holds_a_separate_install(env: FreshEnv, #[values(false, true)] xdg_install: bool) {
    let legacy_data = env.home().join(".local/share/atuin");
    let legacy_config = env.home().join(".config/atuin");
    if xdg_install {
        std::fs::create_dir_all(&legacy_data).unwrap();
        std::fs::create_dir_all(&legacy_config).unwrap();
        std::fs::write(legacy_config.joined("config.toml"), "").unwrap();
    }

    let profile = env.home().join("profile");
    let mut command = env.atuin(&["history", "list"]);
    command.env("ATUIN_SESSION", SESSION).env("ATUIN_HOME", &profile);
    let out = Process::spawn(command).wait();
    assert!(out.status.success(), "stderr: {}", String::from_utf8_lossy(&out.stderr));

    assert!(profile.join("data/key").is_file());
    assert!(profile.join("data/history.db").is_file());
    assert!(profile.join("config.toml").is_file());
    assert!(!env.data_dir().exists());
    assert!(!env.atuin_home().joined("config.toml").exists());
    assert!(!legacy_data.joined("key").exists(), "the legacy data dir was used");
}

#[rstest]
fn key_loads_but_never_generates(env: FreshEnv) {
    let out = Process::spawn(env.atuin(&["key"])).wait();
    assert!(!out.status.success(), "atuin key should fail before a key exists");
    assert!(!env.data_dir().joined("key").exists());

    let mut command = env.atuin(&["history", "list"]);
    command.env("ATUIN_SESSION", SESSION);
    let out = Process::spawn(command).wait();
    assert!(out.status.success());

    let key = std::fs::read(env.data_dir().joined("key")).unwrap();
    let out = Process::spawn(env.atuin(&["key"])).wait();
    assert!(out.status.success());
    assert_eq!(std::fs::read(env.data_dir().joined("key")).unwrap(), key);
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
