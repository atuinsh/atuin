//! `atuin desktop-capture apps`: lists the applications that expose readable content, and writes
//! the ones to capture into the configuration.

use std::fmt;
use std::io::{self, IsTerminal as _, Write};

use atuin_client::settings::desktop_capture::APPS_KEY;
use atuin_client::settings::{Apps, Settings};
use atuin_common::string::StrExt as _;
use atuin_common::userio::confirm::{Answer, confirm};
use clap::Args;
use eyre::Result;
use itertools::Itertools as _;
use toml_edit::Value;

use super::setup::find_apps;
use crate::command::client::config::{listed_apps_value, update_config_file};
use crate::error::Refusal;
use crate::i18n::fl;

/// The indentation of the lines that follow the helper's last step.
const INDENT: &str = "   ";

#[derive(Args, Debug)]
pub struct AppsCmd {
    #[arg(long, conflicts_with = "no_write", help = fl!("arg-desktop-capture-apps-write"))]
    write: bool,

    #[arg(long, help = fl!("arg-desktop-capture-apps-no-write"))]
    no_write: bool,
}

impl AppsCmd {
    pub async fn run(self, settings: &Settings) -> Result<()> {
        let Some(record) = find_apps(settings).await? else {
            return Ok(());
        };
        let capture = &settings.desktop_capture;
        let Some(change) = Change::new(&capture.apps, &capture.exclude, record.apps) else {
            println!("{INDENT}{}", fl!("desktop-capture-apps-nothing-new"));
            return Ok(());
        };
        let answered = self.how_answered();
        let answer = decide(&change, answered, &mut io::stdout().lock())?;
        if answer == Answer::Yes {
            update_config_file(vec![(APPS_KEY, change.value())]).await?;
        }
        write_outcome(&mut io::stdout().lock(), answer, answered)?;
        Ok(())
    }

    fn how_answered(&self) -> Answered {
        if self.write {
            Answered::ByFlag(Answer::Yes)
        } else if self.no_write {
            Answered::ByFlag(Answer::No)
        } else if io::stdin().is_terminal() {
            Answered::ByUser
        } else {
            Answered::NoTerminal
        }
    }
}

/// Shows `change` on `out`, and decides whether to write it as `answered` says.
///
/// `--write` adds to a list only. A change that would replace `apps = "all"` needs the user's
/// answer on a terminal, because the list captures fewer applications.
fn decide(change: &Change<'_>, answered: Answered, out: &mut impl Write) -> Result<Answer> {
    change.write_summary(out)?;
    Ok(match answered {
        Answered::ByFlag(Answer::Yes) if matches!(change, Change::Narrow { .. }) => {
            return Err(Refusal(fl!("desktop-capture-apps-write-refuses-all")).into());
        }
        Answered::ByFlag(answer) => answer,
        Answered::ByUser => {
            confirm(io::stdin().lock(), out, change.question(), change.default_answer())?
        }
        Answered::NoTerminal => Answer::No,
    })
}

/// Where the answer that decides whether to write the list comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Answered {
    ByFlag(Answer),
    ByUser,
    /// Nobody can answer, so the command doesn't write.
    NoTerminal,
}

/// The change to `desktop_capture.apps` that adds the helper's suggestions.
#[derive(Debug, PartialEq, Eq)]
enum Change<'a> {
    /// Adds `added` to the end of the configuration's list, `existing`.
    Extend {
        existing: &'a [String],
        added: Vec<String>,
    },
    /// Replaces `apps = "all"` with `list`, which captures fewer applications.
    Narrow {
        list: Vec<String>,
    },
}

impl<'a> Change<'a> {
    /// The change that adds the applications named `suggested` to `apps`, apart from the ones
    /// that `exclude` names. Names that differ only in case count as one name, as they do in
    /// capture.
    ///
    /// Returns nothing when `apps` selects or `exclude` names each suggested application already.
    fn new(
        apps: &'a Apps,
        exclude: &[String],
        suggested: impl IntoIterator<Item = String>,
    ) -> Option<Self> {
        let suggested = suggested.into_iter().filter(|name| !contains(exclude, name));
        match apps {
            Apps::All => {
                let list = without_repeats(&[], suggested);
                (!list.is_empty()).then_some(Self::Narrow { list })
            }
            Apps::Listed(existing) => {
                let added = without_repeats(existing, suggested);
                (!added.is_empty()).then_some(Self::Extend { existing, added })
            }
        }
    }

    /// The names of the list after the change.
    fn names(&self) -> impl Iterator<Item = &str> {
        let (existing, new): (&[String], &[String]) = match self {
            Self::Extend { existing, added } => (existing, added),
            Self::Narrow { list } => (&[], list),
        };
        existing.iter().chain(new).map(String::as_str)
    }

    /// The value of `desktop_capture.apps` after the change.
    fn value(&self) -> Value {
        listed_apps_value(self.names())
    }

    /// The line of the configuration after the change.
    fn line(&self) -> impl fmt::Display {
        fmt::from_fn(|f| write!(f, "apps = {}", self.value()))
    }

    const fn default_answer(&self) -> Answer {
        match self {
            Self::Extend { .. } => Answer::Yes,
            // Every application that the configuration doesn't list stops being captured.
            Self::Narrow { .. } => Answer::No,
        }
    }

    /// Writes the list after the change, and how the list differs from the configuration.
    fn write_summary(&self, out: &mut impl Write) -> io::Result<()> {
        let difference = match self {
            Self::Extend { added, .. } => {
                fl!("desktop-capture-apps-adds", names = added.iter().join(", "))
            }
            Self::Narrow { .. } => fl!("desktop-capture-apps-narrows"),
        };
        writeln!(out, "{INDENT}{difference}\n{INDENT}{}", self.line())
    }

    fn question(&self) -> impl fmt::Display {
        let question = match self {
            Self::Extend { .. } => fl!("desktop-capture-apps-confirm-add"),
            Self::Narrow { .. } => fl!("desktop-capture-apps-confirm-narrow"),
        };
        fmt::from_fn(move |f| write!(f, "{INDENT}{question}"))
    }
}

/// Writes the outcome of `answer`. `answered` says who gave the answer.
fn write_outcome(out: &mut impl Write, answer: Answer, answered: Answered) -> io::Result<()> {
    let outcome = match (answer, answered) {
        (Answer::Yes, _) => fl!("desktop-capture-apps-written"),
        (Answer::No, Answered::ByUser) => fl!("desktop-capture-apps-declined"),
        (Answer::No, Answered::ByFlag(_) | Answered::NoTerminal) => {
            fl!("desktop-capture-apps-unwritten")
        }
    };
    writeln!(out, "{INDENT}{outcome}")
}

/// Whether `names` contains `name`, ignoring case.
fn contains(names: &[String], name: &str) -> bool {
    names.iter().any(|known| known.eq_ignoring_case(name))
}

/// The names of `new` that neither `existing` nor an earlier name of `new` contains, ignoring
/// case.
fn without_repeats(existing: &[String], new: impl IntoIterator<Item = String>) -> Vec<String> {
    let mut kept: Vec<String> = Vec::new();
    for name in new {
        if !contains(existing, &name) && !contains(&kept, &name) {
            kept.push(name);
        }
    }
    kept
}

#[cfg(test)]
mod tests {
    use rstest::rstest;

    use super::*;

    fn names(names: &[&str]) -> Vec<String> {
        names.iter().map(|&name| name.to_owned()).collect()
    }

    fn listed(list: &[&str]) -> Apps {
        Apps::Listed(names(list))
    }

    #[rstest]
    #[case::all_narrows(Apps::All, &[], &["firefox", "kate"], Some("apps = [\"firefox\", \"kate\"]"))]
    #[case::empty_list_gets_every_name(listed(&[]), &[], &["firefox"], Some("apps = [\"firefox\"]"))]
    #[case::list_gets_the_union(
        listed(&["code", "firefox"]),
        &[],
        &["Firefox", "kate"],
        Some("apps = [\"code\", \"firefox\", \"kate\"]")
    )]
    #[case::repeats_in_the_suggestion(listed(&[]), &[], &["kate", "KATE"], Some("apps = [\"kate\"]"))]
    #[case::exclusions_are_left_out(
        listed(&["code"]),
        &["Kate"],
        &["kate", "gedit"],
        Some("apps = [\"code\", \"gedit\"]")
    )]
    #[case::nothing_new(listed(&["kate"]), &[], &["Kate"], None)]
    #[case::nothing_found(listed(&["kate"]), &[], &[], None)]
    #[case::nothing_found_with_all(Apps::All, &[], &[], None)]
    #[case::everything_excluded(Apps::All, &["kate"], &["kate"], None)]
    fn the_change_merges_the_suggestion(
        #[case] apps: Apps,
        #[case] exclude: &[&str],
        #[case] suggested: &[&str],
        #[case] expected: Option<&str>,
    ) {
        let exclude = names(exclude);

        let change = Change::new(&apps, &exclude, names(suggested));

        assert_eq!(change.map(|change| change.line().to_string()).as_deref(), expected);
    }

    #[rstest]
    #[case::extend(listed(&["code"]), "This list adds kate", Answer::Yes)]
    #[case::extend_empty(listed(&[]), "This list adds kate", Answer::Yes)]
    #[case::narrow(Apps::All, "is \"all\"", Answer::No)]
    fn the_summary_shows_the_difference(
        #[case] apps: Apps,
        #[case] difference: &str,
        #[case] default: Answer,
    ) {
        let change = Change::new(&apps, &[], names(&["kate"])).expect("a change");
        let mut out = Vec::new();

        change.write_summary(&mut out).unwrap();

        let summary = String::from_utf8(out).unwrap();
        assert!(summary.contains(difference), "{summary}");
        assert!(summary.ends_with(&format!("   {}\n", change.line())), "{summary}");
        assert_eq!(change.default_answer(), default);
    }

    #[rstest]
    #[case::write_adds_to_a_list(listed(&["code"]), Answered::ByFlag(Answer::Yes), Some(Answer::Yes))]
    #[case::write_refuses_all(Apps::All, Answered::ByFlag(Answer::Yes), None)]
    #[case::no_write_on_all(Apps::All, Answered::ByFlag(Answer::No), Some(Answer::No))]
    #[case::no_terminal_on_all(Apps::All, Answered::NoTerminal, Some(Answer::No))]
    #[case::no_terminal_on_a_list(listed(&[]), Answered::NoTerminal, Some(Answer::No))]
    fn the_flags_answer_the_question(
        #[case] apps: Apps,
        #[case] answered: Answered,
        #[case] expected: Option<Answer>,
    ) {
        let change = Change::new(&apps, &[], names(&["kate"])).expect("a change");
        let mut out = Vec::new();

        let answer = decide(&change, answered, &mut out);

        match expected {
            Some(expected) => assert_eq!(answer.unwrap(), expected),
            None => assert!(answer.unwrap_err().downcast_ref::<Refusal>().is_some()),
        }
    }

    /// The written value is the list that the summary shows.
    #[rstest]
    fn the_value_is_the_line() {
        let apps = listed(&["code"]);
        let change = Change::new(&apps, &[], names(&["kate"])).expect("a change");

        let value = change.value();

        let names: Vec<_> = value.as_array().unwrap().iter().filter_map(Value::as_str).collect();
        assert_eq!(names, ["code", "kate"]);
    }
}
