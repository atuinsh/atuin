//! The cgroups a process belongs to, as `/proc/<pid>/cgroup` lists them: one per hierarchy, under
//! cgroup v1, v2, or both at once in the hybrid layout.

use std::num::{NonZeroU32, ParseIntError};
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::{fmt, io};

use itertools::Itertools as _;

/// The cgroups a process belongs to, one per hierarchy.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cgroups(Vec<Membership>);

/// A process's cgroup in one hierarchy: a line of `/proc/<pid>/cgroup`,
/// `hierarchy-ID:controller-list:cgroup-path`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Membership {
    pub hierarchy: Hierarchy,
    /// The cgroup's path from the hierarchy's root, such as `/user.slice/user-1000.slice`.
    pub path: PathBuf,
}

/// A cgroup hierarchy.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Hierarchy {
    /// One of cgroup v1's hierarchies, with what is bound to it.
    V1 {
        id: NonZeroU32,
        controllers: Vec<Controller>,
    },
    /// cgroup v2's one, unified hierarchy, listed as ID 0 with no controllers.
    V2,
}

/// What a cgroup v1 hierarchy is bound to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Controller {
    /// A resource controller, such as `cpu` or `memory`.
    Resource(String),
    /// A name for a hierarchy without a resource controller, such as systemd's `name=systemd`.
    Named(String),
}

/// Errors from reading a process's cgroups.
#[derive(Debug, thiserror::Error)]
pub enum CgroupsError {
    #[error("could not read {}: {source}", .path.display())]
    Read {
        path: PathBuf,
        source: io::Error,
    },
    #[error("could not parse {}: {source}", .path.display())]
    Parse {
        path: PathBuf,
        source: ParseCgroupsError,
    },
}

/// Errors from parsing a process's cgroups.
#[derive(Debug, PartialEq, Eq, thiserror::Error)]
#[error("line {line}: {source}")]
pub struct ParseCgroupsError {
    /// The line that failed, counting from 1.
    pub line: usize,
    pub source: ParseMembershipError,
}

/// Errors from parsing a process's cgroup in one hierarchy.
#[derive(Debug, PartialEq, Eq, thiserror::Error)]
pub enum ParseMembershipError {
    #[error("{0:?} is not `hierarchy-ID:controller-list:cgroup-path`")]
    Format(String),
    #[error("hierarchy ID {id:?} is not a number: {source}")]
    HierarchyId {
        id: String,
        source: ParseIntError,
    },
    #[error("cgroup v2's hierarchy, ID 0, lists controllers {0:?}")]
    V2Controllers(String),
}

impl Cgroups {
    /// The cgroups this process belongs to.
    pub fn of_this_process() -> Result<Self, CgroupsError> {
        let path = PathBuf::from("/proc/self/cgroup");
        let cgroups = match std::fs::read_to_string(&path) {
            Ok(cgroups) => cgroups,
            Err(source) => return Err(CgroupsError::Read { path, source }),
        };
        cgroups.parse().map_err(|source| CgroupsError::Parse { path, source })
    }

    /// The cgroup in cgroup v2's hierarchy, if it is mounted.
    #[must_use]
    pub fn v2(&self) -> Option<&Path> {
        self.0
            .iter()
            .find(|membership| membership.hierarchy == Hierarchy::V2)
            .map(|membership| membership.path.as_path())
    }

    /// The cgroup in the cgroup v1 hierarchy called `name`, such as systemd's `name=systemd`.
    #[must_use]
    pub fn v1_named(&self, name: &str) -> Option<&Path> {
        self.0
            .iter()
            .find(|membership| match &membership.hierarchy {
                Hierarchy::V1 { controllers, .. } => controllers
                    .iter()
                    .any(|controller| *controller == Controller::Named(name.to_owned())),
                Hierarchy::V2 => false,
            })
            .map(|membership| membership.path.as_path())
    }
}

impl FromStr for Cgroups {
    type Err = ParseCgroupsError;

    fn from_str(cgroups: &str) -> Result<Self, Self::Err> {
        cgroups
            .lines()
            .enumerate()
            .map(|(index, line)| {
                line.parse().map_err(|source| ParseCgroupsError {
                    line: index + 1,
                    source,
                })
            })
            .collect::<Result<_, _>>()
            .map(Self)
    }
}

impl FromStr for Membership {
    type Err = ParseMembershipError;

    fn from_str(line: &str) -> Result<Self, Self::Err> {
        // The path is last, and may itself contain colons.
        let mut fields = line.splitn(3, ':');
        let (Some(id), Some(controllers), Some(path)) =
            (fields.next(), fields.next(), fields.next())
        else {
            return Err(ParseMembershipError::Format(line.to_owned()));
        };

        let parsed: u32 = id.parse().map_err(|source| ParseMembershipError::HierarchyId {
            id: id.to_owned(),
            source,
        })?;
        let hierarchy = match NonZeroU32::new(parsed) {
            Some(id) => Hierarchy::V1 {
                id,
                controllers: controllers
                    .split(',')
                    .filter(|controller| !controller.is_empty())
                    .map(Controller::from)
                    .collect(),
            },
            None if controllers.is_empty() => Hierarchy::V2,
            None => return Err(ParseMembershipError::V2Controllers(controllers.to_owned())),
        };
        Ok(Self {
            hierarchy,
            path: PathBuf::from(path),
        })
    }
}

impl From<&str> for Controller {
    fn from(controller: &str) -> Self {
        match controller.strip_prefix("name=") {
            Some(name) => Self::Named(name.to_owned()),
            None => Self::Resource(controller.to_owned()),
        }
    }
}

impl fmt::Display for Membership {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.hierarchy {
            Hierarchy::V1 { id, controllers } => {
                write!(f, "{id}:{}", controllers.iter().format(","))
            }
            Hierarchy::V2 => f.write_str("0:"),
        }?;
        write!(f, ":{}", self.path.display())
    }
}

impl fmt::Display for Controller {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Resource(controller) => f.write_str(controller),
            Self::Named(name) => write!(f, "name={name}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;
    use rstest::rstest;

    use super::*;

    const V2: &str = "0::/user.slice/user-1000.slice/user@1000.service/app.slice/atuin.service\n";

    const V1: &str = "12:pids:/user.slice/user-1000.slice/user@1000.service\n4:cpu,cpuacct:/user.\
                      slice\n1:name=systemd:/user.slice/user-1000.slice/session-2.scope\n";

    const HYBRID: &str = "4:cpu,cpuacct:/user.slice\n1:name=systemd:/user.slice/user-1000.slice/\
                          session-2.scope\n0::/user.slice/user-1000.slice/session-2.scope\n";

    /// Each layout says where the process is in the hierarchies it mounts, and only those.
    #[rstest]
    #[case::v2(
        V2,
        Some("/user.slice/user-1000.slice/user@1000.service/app.slice/atuin.service"),
        None
    )]
    #[case::v1(V1, None, Some("/user.slice/user-1000.slice/session-2.scope"))]
    #[case::hybrid(
        HYBRID,
        Some("/user.slice/user-1000.slice/session-2.scope"),
        Some("/user.slice/user-1000.slice/session-2.scope")
    )]
    #[case::none("", None, None)]
    fn finds_the_cgroup_in_each_mounted_hierarchy(
        #[case] cgroups: &str,
        #[case] v2: Option<&str>,
        #[case] systemd: Option<&str>,
    ) {
        let cgroups: Cgroups = cgroups.parse().unwrap();
        assert_eq!(cgroups.v2(), v2.map(Path::new));
        assert_eq!(cgroups.v1_named("systemd"), systemd.map(Path::new));
        assert_eq!(cgroups.v1_named("cpu"), None, "a resource controller is no name");
    }

    #[rstest]
    #[case::multiple_controllers(
        "4:cpu,cpuacct:/",
        Hierarchy::V1 {
            id: NonZeroU32::new(4).unwrap(),
            controllers: vec![Controller::Resource("cpu".into()), Controller::Resource("cpuacct".into())],
        },
        "/"
    )]
    #[case::colon_in_path("0::/a:b", Hierarchy::V2, "/a:b")]
    #[case::v1_without_controllers(
        "3::/",
        Hierarchy::V1 { id: NonZeroU32::new(3).unwrap(), controllers: vec![] },
        "/"
    )]
    fn parses_a_line(#[case] line: &str, #[case] hierarchy: Hierarchy, #[case] path: &str) {
        assert_eq!(
            line.parse::<Membership>(),
            Ok(Membership {
                hierarchy,
                path: PathBuf::from(path)
            })
        );
    }

    #[rstest]
    #[case::too_few_fields("0:/user.slice", ParseMembershipError::Format("0:/user.slice".into()))]
    #[case::not_a_number(
        "x::/",
        ParseMembershipError::HierarchyId { id: "x".into(), source: "x".parse::<u32>().unwrap_err() }
    )]
    #[case::v2_with_controllers("0:cpu:/", ParseMembershipError::V2Controllers("cpu".into()))]
    fn rejects_a_malformed_line(#[case] line: &str, #[case] expected: ParseMembershipError) {
        assert_eq!(line.parse::<Membership>(), Err(expected));
    }

    #[rstest]
    fn a_parse_error_says_which_line() {
        let err = "0::/\nbad\n".parse::<Cgroups>().unwrap_err();
        assert_eq!(err.line, 2);
    }

    /// The kernel's own list for this process parses.
    #[rstest]
    fn reads_this_process() {
        Cgroups::of_this_process().unwrap();
    }

    fn membership() -> impl Strategy<Value = Membership> {
        let controller = prop_oneof![
            "[a-z_]{1,8}".prop_map(Controller::Resource),
            "[a-z.-]{1,8}".prop_map(Controller::Named),
        ];
        let hierarchy = prop_oneof![
            Just(Hierarchy::V2),
            (1..=u32::MAX, prop::collection::vec(controller, 0..4)).prop_map(
                |(id, controllers)| {
                    Hierarchy::V1 {
                        id: NonZeroU32::new(id).unwrap(),
                        controllers,
                    }
                }
            ),
        ];
        (hierarchy, "/[a-z0-9./:@-]{0,24}").prop_map(|(hierarchy, path)| Membership {
            hierarchy,
            path: PathBuf::from(path),
        })
    }

    proptest! {
        #[rstest]
        fn a_list_round_trips(memberships in prop::collection::vec(membership(), 0..6)) {
            let listed = memberships.iter().map(|membership| format!("{membership}\n")).collect::<String>();
            prop_assert_eq!(listed.parse::<Cgroups>(), Ok(Cgroups(memberships)));
        }
    }
}
