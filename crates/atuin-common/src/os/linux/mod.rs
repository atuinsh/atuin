//! Linux-specific utilities.

pub mod cgroup;

/// Whether systemd is the init system, by the test of `sd_booted`.
#[must_use]
pub fn systemd_is_init() -> bool {
    std::path::Path::new("/run/systemd/system").is_dir()
}
