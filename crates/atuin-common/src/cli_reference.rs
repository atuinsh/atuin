//! A machine-readable description of a clap command tree, which the documentation site renders
//! into its CLI reference.

use clap::{Arg, ArgAction, Command};
use serde::Serialize;

/// A command, and every visible subcommand beneath it.
#[derive(Debug, Serialize)]
pub struct CommandReference {
    /// The words a user types to run it, from the binary's name down. For example,
    /// `["atuin", "history", "list"]`.
    pub path: Vec<String>,
    pub about: Option<String>,
    pub long_about: Option<String>,
    pub usage: String,
    pub aliases: Vec<String>,
    pub args: Vec<ArgReference>,
    pub subcommands: Vec<Self>,
}

/// One argument of a command.
#[derive(Debug, Serialize)]
pub struct ArgReference {
    pub id: String,
    pub long: Option<String>,
    pub short: Option<char>,
    /// The placeholders shown for the value, if it takes one. For example, `["SHELL"]`.
    pub value_names: Vec<String>,
    pub help: Option<String>,
    pub long_help: Option<String>,
    pub required: bool,
    pub positional: bool,
    pub global: bool,
    pub repeatable: bool,
    pub defaults: Vec<String>,
    pub possible_values: Vec<PossibleValueReference>,
}

/// One value an argument accepts from a closed set.
#[derive(Debug, Serialize)]
pub struct PossibleValueReference {
    pub name: String,
    pub help: Option<String>,
}

impl CommandReference {
    /// Describe `command` and its visible subcommands, leaving out hidden ones and clap's own
    /// `help` and `version`.
    #[must_use]
    pub fn new(mut command: Command) -> Self {
        command.build();
        Self::of(&command, Vec::new())
    }

    fn of(command: &Command, parent: Vec<String>) -> Self {
        let mut path = parent;
        path.push(command.get_name().to_owned());
        Self {
            usage: command.clone().render_usage().to_string(),
            about: command.get_about().map(ToString::to_string),
            long_about: command.get_long_about().map(ToString::to_string),
            aliases: command.get_visible_aliases().map(str::to_owned).collect(),
            args: command
                .get_arguments()
                .filter(|arg| is_documented(arg))
                .map(ArgReference::of)
                .collect(),
            subcommands: command
                .get_subcommands()
                .filter(|sub| !sub.is_hide_set() && sub.get_name() != "help")
                .map(|sub| Self::of(sub, path.clone()))
                .collect(),
            path,
        }
    }
}

impl ArgReference {
    fn of(arg: &Arg) -> Self {
        let takes_value = arg.get_num_args().is_some_and(|n| n.takes_values());
        Self {
            id: arg.get_id().to_string(),
            long: arg.get_long().map(str::to_owned),
            short: arg.get_short(),
            value_names: if takes_value {
                arg.get_value_names()
                    .map(|names| names.iter().map(ToString::to_string).collect())
                    .unwrap_or_else(|| vec![arg.get_id().to_string().to_uppercase()])
            } else {
                Vec::new()
            },
            help: arg.get_help().map(ToString::to_string),
            long_help: arg.get_long_help().map(ToString::to_string),
            required: arg.is_required_set(),
            positional: arg.is_positional(),
            global: arg.is_global_set(),
            repeatable: matches!(arg.get_action(), ArgAction::Append | ArgAction::Count),
            defaults: arg
                .get_default_values()
                .iter()
                .map(|value| value.to_string_lossy().into_owned())
                .collect(),
            possible_values: if takes_value {
                arg.get_possible_values()
                    .into_iter()
                    .filter(|value| !value.is_hide_set())
                    .map(|value| PossibleValueReference {
                        name: value.get_name().to_owned(),
                        help: value.get_help().map(ToString::to_string),
                    })
                    .collect()
            } else {
                Vec::new()
            },
        }
    }
}

fn is_documented(arg: &Arg) -> bool {
    !arg.is_hide_set() && !matches!(arg.get_id().as_str(), "help" | "version")
}
