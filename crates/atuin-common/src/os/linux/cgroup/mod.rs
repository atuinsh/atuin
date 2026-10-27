//! Control groups, as `/proc/<pid>/cgroup` lists them: one per hierarchy, under cgroup v2, v1, or
//! both at once in the hybrid layout.

mod cgroupv1;
mod cgroupv2;

pub use cgroupv1::CgroupV1;
pub use cgroupv2::CgroupV2;

/// The active process's list of cgroups.
const ACTIVE_PROCESS: &str = "/proc/self/cgroup";
