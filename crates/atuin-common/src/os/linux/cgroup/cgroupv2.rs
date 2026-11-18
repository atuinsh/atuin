use std::io;
use std::path::PathBuf;

/// A process's cgroup in cgroup v2's unified hierarchy, such as
/// `/user.slice/user-1000.slice/user@1000.service/app.slice/atuin.service`.
#[derive(Debug, Clone, PartialEq, Eq, derive_more::Deref, derive_more::Into)]
pub struct CgroupV2(PathBuf);

impl CgroupV2 {
    /// This process's cgroup, if cgroup v2 is mounted.
    pub fn active_process() -> io::Result<Option<Self>> {
        Ok(Self::listed_in(&std::fs::read_to_string(super::ACTIVE_PROCESS)?))
    }

    /// The cgroup v2 entry of a `/proc/<pid>/cgroup` list, `0::<path>`.
    fn listed_in(cgroups: &str) -> Option<Self> {
        cgroups.lines().find_map(|line| line.strip_prefix("0::")).map(|path| Self(path.into()))
    }
}

#[cfg(test)]
mod tests {
    use rstest::rstest;

    use super::*;

    #[rstest]
    #[case::v2("0::/user.slice/session-2.scope\n", Some("/user.slice/session-2.scope"))]
    #[case::hybrid("4:cpu,cpuacct:/\n1:name=systemd:/a.scope\n0::/a.scope\n", Some("/a.scope"))]
    #[case::v1_only("4:cpu,cpuacct:/\n1:name=systemd:/a.scope\n", None)]
    #[case::colon_in_path("0::/a:b\n", Some("/a:b"))]
    #[case::empty("", None)]
    fn finds_the_v2_entry(#[case] cgroups: &str, #[case] expected: Option<&str>) {
        assert_eq!(CgroupV2::listed_in(cgroups), expected.map(|path| CgroupV2(path.into())));
    }

    /// The kernel's own list for this process reads.
    #[rstest]
    fn reads_the_active_process() {
        CgroupV2::active_process().unwrap();
    }
}
