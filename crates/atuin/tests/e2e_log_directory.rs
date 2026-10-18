//! Doctor diagnoses legacy files that prevent logging without contaminating shell widget output.

#![cfg(unix)]

mod common;

use atuin_common::path::PathBufExt as _;
use common::{FreshEnv, Process};
use rstest::{fixture, rstest};

#[fixture]
fn env() -> FreshEnv {
    FreshEnv::new()
}

#[rstest]
#[case::legacy_home_file(false)]
#[case::log_directory_file(true)]
fn doctor_reports_blocked_log_directory(env: FreshEnv, #[case] log_file: bool) {
    let legacy_config = env.home().join(".config/atuin");
    let legacy_data = env.home().join(".local/share/atuin");
    std::fs::create_dir_all(&legacy_config).unwrap();
    std::fs::create_dir_all(&legacy_data).unwrap();
    std::fs::write(legacy_config.joined("config.toml"), "[logs]\nenabled = true\n").unwrap();

    let directory = env.atuin_home().joined("logs");
    let blocker = if log_file {
        std::fs::create_dir(env.atuin_home()).unwrap();
        directory.clone()
    } else {
        env.atuin_home()
    };
    std::fs::write(&blocker, "legacy shell integration").unwrap();

    let mut command = env.atuin(&["doctor"]);
    command.env("LC_ALL", "C");
    let out = Process::spawn(command).wait();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains(&blocker.display().to_string()), "{stdout}");
    assert!(stdout.contains(&directory.display().to_string()), "{stdout}");
    assert!(stdout.contains("rename or move"), "{stdout}");
    assert_eq!(std::fs::read_to_string(blocker).unwrap(), "legacy shell integration");
}
