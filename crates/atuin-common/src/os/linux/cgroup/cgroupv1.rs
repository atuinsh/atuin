use std::io;
use std::path::PathBuf;

use itertools::Itertools as _;

/// A process's cgroup in one of cgroup v1's hierarchies, such as
/// `/user.slice/user-1000.slice/user@1000.service/atuin.service` in systemd's `name=systemd` one.
#[derive(Debug, Clone, PartialEq, Eq, derive_more::Deref, derive_more::Into)]
pub struct CgroupV1(PathBuf);

impl CgroupV1 {
    /// This process's cgroup in the hierarchy `controller` is bound to, such as `cpu` or
    /// `name=systemd`, if it is mounted.
    pub fn active_process(controller: &str) -> io::Result<Option<Self>> {
        Ok(Self::listed_in(&std::fs::read_to_string(super::ACTIVE_PROCESS)?, controller))
    }

    /// The entry of a `/proc/<pid>/cgroup` list for the hierarchy `controller` is bound to,
    /// `hierarchy-ID:controller-list:cgroup-path`.
    fn listed_in(cgroups: &str, controller: &str) -> Option<Self> {
        cgroups.lines().find_map(|line| {
            // The path is last, and may itself contain colons.
            let (id, controllers, path) = line.splitn(3, ':').collect_tuple()?;
            // cgroup v2's hierarchy, ID 0, lists no controllers, which `""` would match.
            (id != "0" && controllers.split(',').contains(&controller)).then(|| Self(path.into()))
        })
    }
}

#[cfg(test)]
mod tests {
    use rstest::rstest;

    use super::*;

    #[rstest]
    #[case::named("4:cpu,cpuacct:/\n1:name=systemd:/a.scope\n", "name=systemd", Some("/a.scope"))]
    #[case::resource("4:cpu,cpuacct:/a\n1:name=systemd:/b.scope\n", "cpuacct", Some("/a"))]
    #[case::hybrid("1:name=systemd:/a.scope\n0::/b.scope\n", "name=systemd", Some("/a.scope"))]
    #[case::v2_only("0::/a.scope\n", "name=systemd", None)]
    #[case::v2_by_no_controller("0::/a.scope\n", "", None)]
    #[case::unbound("4:cpu,cpuacct:/\n", "memory", None)]
    #[case::colon_in_path("1:name=systemd:/a:b\n", "name=systemd", Some("/a:b"))]
    #[case::empty("", "name=systemd", None)]
    fn finds_the_hierarchy_entry(
        #[case] cgroups: &str,
        #[case] controller: &str,
        #[case] expected: Option<&str>,
    ) {
        assert_eq!(
            CgroupV1::listed_in(cgroups, controller),
            expected.map(|path| CgroupV1(path.into()))
        );
    }

    /// The kernel's own list for this process reads.
    #[rstest]
    fn reads_the_active_process() {
        CgroupV1::active_process("name=systemd").unwrap();
    }
}
