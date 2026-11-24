use std::env;
use std::process::Command;

use toml_edit::DocumentMut;

fn main() {
    let output = Command::new("git").args(["rev-parse", "HEAD"]).output();

    let sha = match output {
        Ok(sha) if sha.status.success() => String::from_utf8(sha.stdout).unwrap(),
        _ => String::from("NO_GIT"),
    };

    println!("cargo:rustc-env=GIT_HASH={sha}");

    // Cargo names each enabled feature as `CARGO_FEATURE_<NAME>`, uppercased and with `-` as `_`,
    // and also sets one for each optional dependency. The manifest supplies the real names.
    let manifest: DocumentMut =
        include_str!("Cargo.toml").parse().expect("the crate manifest is valid TOML");
    #[expect(
        clippy::disallowed_methods,
        reason = "a build script reads the environment that Cargo gives it, which no test mocks"
    )]
    let features: Vec<&str> = manifest["features"]
        .as_table()
        .expect("the crate manifest has a [features] table")
        .iter()
        .map(|(name, _)| name)
        .filter(|&name| name != "default")
        .filter(|name| {
            env::var_os(format!("CARGO_FEATURE_{}", name.to_uppercase().replace('-', "_")))
                .is_some()
        })
        .collect();

    println!("cargo:rustc-env=ATUIN_FEATURES={}", features.join(","));
}
