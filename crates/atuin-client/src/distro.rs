use std::process::Command;

use atuin_common::fs;

/// Parses a release file's contents into a distribution name, if it names one.
type ReleaseParser = fn(&str) -> Option<String>;

/// Detect the Linux distribution from the system,
/// using system-specific release files and falling
/// back to lsb_release.
pub async fn detect_linux_distribution() -> String {
    let release_files: [(&str, ReleaseParser); 8] = [
        ("/etc/os-release", detect_from_os_release),
        ("/etc/debian_version", |v| Some(format!("Debian {}", v.trim()))),
        ("/etc/centos-release", |v| Some(v.trim().to_string())),
        ("/etc/redhat-release", |v| Some(v.trim().to_string())),
        ("/etc/fedora-release", |v| Some(v.trim().to_string())),
        ("/etc/arch-release", |v| (!v.trim().is_empty()).then(|| "Arch Linux".to_string())),
        ("/etc/alpine-release", |v| Some(format!("Alpine {}", v.trim()))),
        ("/etc/SuSE-release", |content| content.lines().next().map(|l| l.trim().to_string())),
    ];
    for (path, parse) in release_files {
        if let Some(distro) = fs::read_to_string(path).await.ok().and_then(|v| parse(&v)) {
            return distro;
        }
    }
    detect_from_lsb_release().unwrap_or_else(|| "Unknown".to_string())
}

fn detect_from_os_release(content: &str) -> Option<String> {
    content
        .lines()
        .find(|l| l.starts_with("PRETTY_NAME="))
        .and_then(|l| l.split_once('=').map(|s| s.1))
        .map(|s| s.trim_matches('"').to_string())
}

fn detect_from_lsb_release() -> Option<String> {
    let output = Command::new("lsb_release").arg("-a").output().ok()?;

    if !output.status.success() {
        return None;
    }

    let output = String::from_utf8(output.stdout).ok()?;
    linux_distro_from_lsb_release(&output)
}

fn linux_distro_from_lsb_release(output: &str) -> Option<String> {
    output
        .lines()
        .find(|line| line.starts_with("Description:"))
        .and_then(|line| line.split_once(':').map(|s| s.1))
        .map(|s| s.trim().to_string())
}
