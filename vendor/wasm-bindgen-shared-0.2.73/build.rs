use std::process::Command;

fn main() {
    let rev = Command::new("git")
        .arg("rev-parse")
        .arg("HEAD")
        .output()
        .ok()
        .map(|s| s.stdout)
        .and_then(|s| String::from_utf8(s).ok());
    if let Some(rev) = rev {
        if rev.len() >= 9 {
            println!("cargo:rustc-env=WBG_VERSION={}", &rev[..9]);
        }
    }
}
