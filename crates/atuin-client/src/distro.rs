use std::process::Command;

/// Detect the Linux distribution from the system,
/// using system-specific release files and falling
/// back to lsb_release.
pub fn detect_linux_distribution() -> String {
    detect_from_os_release()
        .or_else(detect_from_debian_version)
        .or_else(detect_from_centos_release)
        .or_else(detect_from_redhat_release)
        .or_else(detect_from_fedora_release)
        .or_else(detect_from_arch_release)
        .or_else(detect_from_alpine_release)
        .or_else(detect_from_suse_release)
        .or_else(detect_from_lsb_release)
        .unwrap_or_else(|| "Unknown".to_string())
}

fn detect_from_os_release() -> Option<String> {
    let content = std::fs::read_to_string("/etc/os-release").ok()?;

    content
        .lines()
        .find(|l| l.starts_with("PRETTY_NAME="))
        .and_then(|l| l.split_once('=').map(|s| s.1))
        .map(|s| s.trim_matches('"').to_string())
}

fn detect_from_debian_version() -> Option<String> {
    std::fs::read_to_string("/etc/debian_version")
        .ok()
        .map(|v| format!("Debian {}", v.trim()))
}

fn detect_from_centos_release() -> Option<String> {
    std::fs::read_to_string("/etc/centos-release")
        .ok()
        .map(|v| v.trim().to_string())
}

fn detect_from_redhat_release() -> Option<String> {
    std::fs::read_to_string("/etc/redhat-release")
        .ok()
        .map(|v| v.trim().to_string())
}

fn detect_from_fedora_release() -> Option<String> {
    std::fs::read_to_string("/etc/fedora-release")
        .ok()
        .map(|v| v.trim().to_string())
}

fn detect_from_arch_release() -> Option<String> {
    std::fs::read_to_string("/etc/arch-release")
        .ok()
        .filter(|v| !v.trim().is_empty())
        .map(|_| "Arch Linux".to_string())
}

fn detect_from_alpine_release() -> Option<String> {
    std::fs::read_to_string("/etc/alpine-release")
        .ok()
        .map(|v| format!("Alpine {}", v.trim()))
}

fn detect_from_suse_release() -> Option<String> {
    std::fs::read_to_string("/etc/SuSE-release")
        .ok()
        .and_then(|content| content.lines().next().map(|l| l.trim().to_string()))
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
