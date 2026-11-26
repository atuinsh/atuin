use std::env;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
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

    if features.contains(&"ai-harness") {
        embed_ai_harness();
    }
}

/// The variable that names a prebuilt `dist` of the plugin, for a build that can't run pnpm.
const DIST_VAR: &str = "ATUIN_AI_HARNESS_DIST";

/// The harnesses the plugin is built for, as `integrations/ai-harness/package.json` targets them.
const HARNESSES: [&str; 4] = ["claude-code", "codex", "opencode", "pi"];

/// What a build of the plugin is made from, under `integrations/ai-harness`.
const INPUTS: [&str; 6] =
    ["mod.tsx", "src", "package.json", "pnpm-lock.yaml", "pnpm-workspace.yaml", "tsconfig.json"];

/// Copy each harness's build of the plugin into `OUT_DIR`, and write `ai_harness.rs` there: a
/// `Plugin` of every file, for `atuin setup` to `include!`.
#[expect(
    clippy::disallowed_methods,
    reason = "a build script reads the environment that Cargo gives it, which no test mocks"
)]
fn embed_ai_harness() {
    println!("cargo:rerun-if-env-changed={DIST_VAR}");
    let dist = match env::var_os(DIST_VAR) {
        Some(dist) => {
            println!("cargo:rerun-if-changed={}", Path::new(&dist).display());
            PathBuf::from(dist)
        }
        None => build_ai_harness(),
    };
    let out = PathBuf::from(env::var_os("OUT_DIR").expect("Cargo sets OUT_DIR"));
    let embedded = out.join("ai-harness");
    if embedded.exists() {
        std::fs::remove_dir_all(&embedded).expect("the last build's copy can be removed");
    }

    let mut plugin = String::from("::atuin_common::harnesstools::plugin::Plugin {\n");
    for harness in HARNESSES {
        let (from, to) = (dist.join(harness), embedded.join(harness));
        let mut files = Vec::new();
        if let Err(err) = copy_tree(&from, &to, "", &mut files) {
            let from = from.display();
            fail(&format!("could not copy the plugin's {harness} build from {from}: {err}"));
        }
        if files.is_empty() {
            fail(&format!("the plugin's {harness} build at {} has no files", from.display()));
        }
        files.sort();
        let field = harness.replace('-', "_");
        plugin += &format!("    {field}: ::atuin_common::harnesstools::plugin::PluginFiles(&[\n");
        for file in files {
            let path = to.join(&file);
            let path = path.to_str().expect("OUT_DIR and the plugin's file names are UTF-8");
            writeln!(plugin, "        ({file:?}, include_bytes!({path:?}).as_slice()),")
                .expect("a String takes any write");
        }
        plugin += "    ]),\n";
    }
    plugin += "}\n";
    std::fs::write(out.join("ai_harness.rs"), plugin).expect("OUT_DIR can be written to");
}

/// Build the plugin from `integrations/ai-harness` with pnpm, and hand back its `dist`.
#[expect(
    clippy::disallowed_methods,
    reason = "a build script reads the environment that Cargo gives it, which no test mocks"
)]
fn build_ai_harness() -> PathBuf {
    let manifest_dir = env::var_os("CARGO_MANIFEST_DIR").expect("Cargo sets CARGO_MANIFEST_DIR");
    let source = Path::new(&manifest_dir).join("../../integrations/ai-harness");
    if !source.join("package.json").is_file() {
        let source = source.display();
        fail(&format!("the plugin's source is not at {source}, as in a crate from crates.io"));
    }
    for input in INPUTS {
        println!("cargo:rerun-if-changed={}", source.join(input).display());
    }
    pnpm(&source, &["install", "--frozen-lockfile"]);
    pnpm(&source, &["build"]);
    source.join("dist")
}

/// Run `pnpm` with `args` in `dir`, failing the build with what it printed if it fails.
fn pnpm(dir: &Path, args: &[&str]) {
    // npm installs pnpm on Windows as `pnpm.cmd`, which `Command` finds only by its full name.
    let programs: &[&str] = if cfg!(windows) {
        &["pnpm", "pnpm.cmd"]
    } else {
        &["pnpm"]
    };
    let ran = programs
        .iter()
        .map(|program| Command::new(program).args(args).current_dir(dir).output())
        .find(|ran| !matches!(ran, Err(err) if err.kind() == std::io::ErrorKind::NotFound));
    let command = format!("pnpm {}", args.join(" "));
    match ran {
        None => fail(&format!("`{command}` could not run: `pnpm` is not on PATH")),
        Some(Err(err)) => fail(&format!("`{command}` could not run: {err}")),
        Some(Ok(output)) if !output.status.success() => fail(&format!(
            "`{command}` failed in {} ({}):\n{}{}",
            dir.display(),
            output.status,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr),
        )),
        Some(Ok(_)) => {}
    }
}

/// Copy the files under `from` into `to`, adding each one's `/`-separated path under `from` to
/// `files`. `prefix` is the path of `from` itself, ending in `/` unless empty.
fn copy_tree(from: &Path, to: &Path, prefix: &str, files: &mut Vec<String>) -> std::io::Result<()> {
    std::fs::create_dir_all(to)?;
    for entry in std::fs::read_dir(from)? {
        let entry = entry?;
        let name = entry.file_name();
        let path = format!("{prefix}{}", name.to_str().expect("the plugin's file names are UTF-8"));
        if entry.file_type()?.is_dir() {
            copy_tree(&entry.path(), &to.join(&name), &format!("{path}/"), files)?;
        } else {
            std::fs::copy(entry.path(), to.join(&name))?;
            files.push(path);
        }
    }
    Ok(())
}

/// Fail the build over the `ai-harness` feature, saying what was `observed` and the ways out.
fn fail(observed: &str) -> ! {
    let fix = format!(
        "the `ai-harness` feature embeds atuin's agent plugin: install Node and pnpm, or set \
         {DIST_VAR} to a prebuilt `dist` of integrations/ai-harness, or build without the feature"
    );
    for line in observed.lines().chain([fix.as_str()]) {
        println!("cargo::error={line}");
    }
    std::process::exit(1);
}
