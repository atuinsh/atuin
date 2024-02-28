use serde::Deserialize;

#[derive(Clone, Debug, Deserialize)]
pub struct Stats {
    #[serde(default = "Stats::common_prefix_default")]
    pub common_prefix: Vec<String>, // sudo, etc. commands we want to strip off
    #[serde(default = "Stats::common_subcommands_default")]
    pub common_subcommands: Vec<String>, // kubectl, commands we should consider subcommands for
    #[serde(default = "Stats::ignored_commands_default")]
    pub ignored_commands: Vec<String>, // cd, ls, etc. commands we want to completely hide from stats
}

impl Stats {
    fn common_prefix_default() -> Vec<String> {
        vec!["sudo", "doas"].into_iter().map(String::from).collect()
    }

    fn common_subcommands_default() -> Vec<String> {
        vec![
            "apt",
            "cargo",
            "composer",
            "dnf",
            "docker",
            "git",
            "go",
            "ip",
            "kubectl",
            "nix",
            "nmcli",
            "npm",
            "pecl",
            "pnpm",
            "podman",
            "port",
            "systemctl",
            "tmux",
            "yarn",
        ]
        .into_iter()
        .map(String::from)
        .collect()
    }

    fn ignored_commands_default() -> Vec<String> {
        vec![]
    }
}

impl Default for Stats {
    fn default() -> Self {
        Self {
            common_prefix: Self::common_prefix_default(),
            common_subcommands: Self::common_subcommands_default(),
            ignored_commands: Self::ignored_commands_default(),
        }
    }
}

// FIXME: Can use upstream Dialect enum if https://github.com/stevedonovan/chrono-english/pull/16 is merged
// FIXME: Above PR was merged, but dependency was changed to interim (fork of chrono-english) in the ... interim
#[derive(Clone, Debug, Deserialize, Copy)]
pub enum Dialect {
    #[serde(rename = "us")]
    Us,

    #[serde(rename = "uk")]
    Uk,
}

impl From<Dialect> for interim::Dialect {
    fn from(d: Dialect) -> interim::Dialect {
        match d {
            Dialect::Uk => interim::Dialect::Uk,
            Dialect::Us => interim::Dialect::Us,
        }
    }
}
