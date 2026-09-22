use std::process::Command;
fn main() {
    let output = Command::new("git").args(["rev-parse", "HEAD"]).output();

    let sha = match output {
        Ok(sha) if sha.status.success() => String::from_utf8(sha.stdout).unwrap(),
        _ => String::from("NO_GIT"),
    };

    println!("cargo:rustc-env=GIT_HASH={sha}");
}
