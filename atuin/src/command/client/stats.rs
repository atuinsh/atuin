use std::collections::{HashMap, HashSet};

use chrono::{prelude::*, Duration};
use clap::Parser;
use crossterm::style::{Color, ResetColor, SetAttribute, SetForegroundColor};
use eyre::{bail, Result};
use interim::parse_date_string;

use atuin_client::{
    database::{current_context, Database},
    history::History,
    settings::{FilterMode, Settings},
};

#[derive(Parser)]
#[command(infer_subcommands = true)]
pub struct Cmd {
    /// compute statistics for the specified period, leave blank for statistics since the beginning
    period: Vec<String>,

    /// How many top commands to list
    #[arg(long, short, default_value = "10")]
    count: usize,

    /// The number of consecutive commands to consider
    #[arg(long, short, default_value = "1")]
    ngram_size: usize,
}

fn split_at_pipe(command: &str) -> Vec<&str> {
    let mut result = vec![];
    let mut quoted = false;
    let mut start = 0;
    let mut chars = command.chars().enumerate().peekable();
    while let Some((i, c)) = chars.next() {
        let current = i;
        match c {
            '"' => {
                if command[start..current] != *"\"" {
                    quoted = !quoted
                }
            }
            '\'' => {
                if command[start..current] != *"'" {
                    quoted = !quoted
                }
            }
            '\\' => if let Some(_) = chars.next() {},
            '|' => {
                if !quoted {
                    if command[start..].starts_with('|') {
                        start += 1;
                    }
                    result.push(&command[start..current]);
                    start = current;
                }
            }
            _ => {}
        }
    }
    if command[start..].starts_with('|') {
        start += 1;
    }
    result.push(&command[start..]);
    result
}

fn compute_stats(history: &[History], count: usize, ngram_size: usize) -> Result<()> {
    let mut commands = HashSet::<&str>::with_capacity(history.len());
    let mut prefixes = HashMap::<Vec<&str>, usize>::with_capacity(history.len());
    for i in history {
        // just in case it somehow has a leading tab or space or something (legacy atuin didn't ignore space prefixes)
        split_at_pipe(i.command.as_str())
            .iter()
            .map(|l| {
                let command = l.trim();
                commands.insert(command);
                command
            })
            .collect::<Vec<_>>()
            .windows(ngram_size)
            .for_each(|w| {
                *prefixes
                    .entry(w.iter().map(|c| interesting_command(c)).collect())
                    .or_default() += 1;
            });
    }

    let unique = commands.len();
    let mut top = prefixes.into_iter().collect::<Vec<_>>();
    top.sort_unstable_by_key(|x| std::cmp::Reverse(x.1));
    top.truncate(count);
    if top.is_empty() {
        bail!("No commands found");
    }

    let max = top.iter().map(|x| x.1).max().unwrap();
    let num_pad = max.ilog10() as usize + 1;

    // Find the length of the longest command name for each column
    let column_widths = top
        .iter()
        .map(|(commands, _)| commands.iter().map(|c| c.len()).collect::<Vec<usize>>())
        .fold(vec![0; ngram_size], |acc, item| {
            acc.iter()
                .zip(item.iter())
                .map(|(a, i)| *std::cmp::max(a, i))
                .collect()
        });

    for (command, count) in top {
        let gray = SetForegroundColor(Color::Grey);
        let bold = SetAttribute(crossterm::style::Attribute::Bold);

        let in_ten = 10 * count / max;
        print!("[");
        print!("{}", SetForegroundColor(Color::Red));
        for i in 0..in_ten {
            if i == 2 {
                print!("{}", SetForegroundColor(Color::Yellow));
            }
            if i == 5 {
                print!("{}", SetForegroundColor(Color::Green));
            }
            print!("▮");
        }
        for _ in in_ten..10 {
            print!(" ");
        }

        let formatted_command = command
            .iter()
            .zip(column_widths.iter())
            .map(|(cmd, width)| format!("{:width$}", cmd))
            .collect::<Vec<_>>()
            .join(" | ");

        println!("{ResetColor}] {gray}{count:num_pad$}{ResetColor} {bold}{formatted_command}{ResetColor}");
    }
    println!("Total commands:   {}", history.len());
    println!("Unique commands:  {unique}");

    Ok(())
}

impl Cmd {
    pub async fn run(&self, db: &mut impl Database, settings: &Settings) -> Result<()> {
        let context = current_context();
        let words = if self.period.is_empty() {
            String::from("all")
        } else {
            self.period.join(" ")
        };
        let history = if words.as_str() == "all" {
            db.list(FilterMode::Global, &context, None, false).await?
        } else if words.trim() == "today" {
            let start = Local::now().date().and_hms(0, 0, 0);
            let end = start + Duration::days(1);
            db.range(start.into(), end.into()).await?
        } else if words.trim() == "month" {
            let end = Local::now().date().and_hms(0, 0, 0);
            let start = end - Duration::days(31);
            db.range(start.into(), end.into()).await?
        } else if words.trim() == "week" {
            let end = Local::now().date().and_hms(0, 0, 0);
            let start = end - Duration::days(7);
            db.range(start.into(), end.into()).await?
        } else if words.trim() == "year" {
            let end = Local::now().date().and_hms(0, 0, 0);
            let start = end - Duration::days(365);
            db.range(start.into(), end.into()).await?
        } else {
            let start = parse_date_string(&words, Local::now(), settings.dialect.into())?;
            let end = start + Duration::days(1);
            db.range(start.into(), end.into()).await?
        };
        compute_stats(&history, self.count, self.ngram_size)?;
        Ok(())
    }
}

// TODO: make this configurable?
static COMMON_COMMAND_PREFIX: &[&str] = &["sudo"];
static COMMON_SUBCOMMAND_PREFIX: &[&str] = &["cargo", "go", "git", "npm", "yarn", "pnpm"];

fn first_non_whitespace(s: &str) -> Option<usize> {
    s.char_indices()
        // find the first non whitespace char
        .find(|(_, c)| !c.is_ascii_whitespace())
        // return the index of that char
        .map(|(i, _)| i)
}

fn first_whitespace(s: &str) -> usize {
    s.char_indices()
        // find the first whitespace char
        .find(|(_, c)| c.is_ascii_whitespace())
        // return the index of that char, (or the max length of the string)
        .map_or(s.len(), |(i, _)| i)
}

fn interesting_command(mut command: &str) -> &str {
    // compute command prefix
    // we loop here because we might be working with a common command prefix (eg sudo) that we want to trim off
    let (i, prefix) = loop {
        let i = first_whitespace(command);
        let prefix = &command[..i];

        // is it a common prefix
        if COMMON_COMMAND_PREFIX.contains(&prefix) {
            command = command[i..].trim_start();
            if command.is_empty() {
                // no commands following, just use the prefix
                return prefix;
            }
        } else {
            break (i, prefix);
        }
    };

    // compute subcommand
    let subcommand_indices = command
        // after the end of the command prefix
        .get(i..)
        // find the first non whitespace character (start of subcommand)
        .and_then(first_non_whitespace)
        // then find the end of that subcommand
        .map(|j| i + j + first_whitespace(&command[i + j..]));

    match subcommand_indices {
        // if there is a subcommand and it's a common one, then count the full prefix + subcommand
        Some(end) if COMMON_SUBCOMMAND_PREFIX.contains(&prefix) => &command[..end],
        // otherwise just count the main command
        _ => prefix,
    }
}

#[cfg(test)]
mod tests {
    use super::{interesting_command, split_at_pipe};

    #[test]
    fn interesting_commands() {
        assert_eq!(interesting_command("cargo"), "cargo");
        assert_eq!(interesting_command("cargo build foo bar"), "cargo build");
        assert_eq!(
            interesting_command("sudo   cargo build foo bar"),
            "cargo build"
        );
        assert_eq!(interesting_command("sudo"), "sudo");
    }

    #[test]
    fn split_simple() {
        assert_eq!(split_at_pipe("fd | rg"), ["fd ", " rg"]);
    }

    #[test]
    fn split_multi() {
        assert_eq!(
            split_at_pipe("kubectl | jq | rg"),
            ["kubectl ", " jq ", " rg"]
        );
    }

    #[test]
    fn split_simple_quoted() {
        assert_eq!(
            split_at_pipe("foo | bar 'baz {} | quux' | xyzzy"),
            ["foo ", " bar 'baz {} | quux' ", " xyzzy"]
        );
    }

    #[test]
    fn split_multi_quoted() {
        assert_eq!(
            split_at_pipe("foo | bar 'baz \"{}\" | quux' | xyzzy"),
            ["foo ", " bar 'baz \"{}\" | quux' ", " xyzzy"]
        );
    }

    #[test]
    fn escaped_pipes() {
        assert_eq!(
            split_at_pipe("foo | bar baz \\| quux"),
            ["foo ", " bar baz \\| quux"]
        );
    }

    #[test]
    fn emoji() {
        assert_eq!(
            split_at_pipe("git commit -m \"🚀\""),
            ["git commit -m \"🚀\""]
        );
    }
}
