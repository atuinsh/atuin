#![cfg(all(unix, feature = "sync"))]

mod common;

use std::os::unix::process::ExitStatusExt;
use std::{fs, io};

use atuin_common::encryption::paseto_v4::Key;
use atuin_common::path::PathBufExt as _;
use common::FreshEnv;
use rstest::{fixture, rstest};

#[fixture]
fn env() -> FreshEnv {
    let env = FreshEnv::new();
    fs::create_dir_all(env.data_dir()).unwrap();
    Key::from([0; 32]).try_write_path(&env.data_dir().joined("key")).unwrap();
    env
}

#[rstest]
#[case(
    false,
    "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon \
     abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon \
     abandon art\n"
)]
#[case(true, "3AAgAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=\n")]
fn key_output(env: FreshEnv, #[case] base64: bool, #[case] expected: &str) {
    let mut command = env.atuin(&["key"]);
    if base64 {
        command.arg("--base64");
    }
    let output = command.output().unwrap();
    assert!(output.status.success(), "{output:?}");
    assert_eq!(output.stdout, expected.as_bytes());
    assert!(output.stderr.is_empty(), "{output:?}");
}

#[rstest]
fn key_closed_pipe(env: FreshEnv, #[values(false, true)] base64: bool) {
    let (reader, writer) = io::pipe().unwrap();
    // Close the reader before spawning Atuin so the write must fail, without a timing race.
    drop(reader);
    let mut command = env.atuin(&["key"]);
    if base64 {
        command.arg("--base64");
    }
    let output = command.stdout(writer).output().unwrap();
    assert_eq!(output.status.signal(), Some(signal_hook::consts::SIGPIPE), "{output:?}");
    assert!(output.stderr.is_empty(), "{output:?}");
}

#[cfg(target_os = "linux")]
#[rstest]
fn key_other_output_error(env: FreshEnv, #[values(false, true)] base64: bool) {
    let mut command = env.atuin(&["key"]);
    if base64 {
        command.arg("--base64");
    }
    let output = command
        .stdout(fs::File::options().write(true).open("/dev/full").unwrap())
        .output()
        .unwrap();
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("No space left on device"), "{stderr}");
    assert!(!stderr.contains("panicked"), "{stderr}");
}
