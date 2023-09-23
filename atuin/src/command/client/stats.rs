use std::collections::{HashMap, HashSet};

use clap::Parser;
use crossterm::style::{Color, ResetColor, SetAttribute, SetForegroundColor};
use eyre::{bail, Result};
use interim::parse_date_string;

use atuin_client::{
    database::{current_context, Database},
    history::History,
    settings::{FilterMode, Settings},
};
use time::{Duration, OffsetDateTime, Time};

#[derive(Parser)]
#[command(infer_subcommands = true)]
pub struct Cmd {
    /// compute statistics for the specified period, leave blank for statistics since the beginning
    period: Vec<String>,

    /// How many top commands to list
    #[arg(long, short, default_value = "10")]
    count: usize,
}

fn compute_stats(history: &[History], count: usize) -> Result<()> {
    let mut commands = HashSet::<&str>::with_capacity(history.len());
    let mut prefixes = HashMap::<&str, usize>::with_capacity(history.len());
    for i in history {
        // just in case it somehow has a leading tab or space or something (legacy atuin didn't ignore space prefixes)
        let command = i.command.trim();
        commands.insert(command);
        *prefixes.entry(interesting_command(command)).or_default() += 1;
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

        println!("{ResetColor}] {gray}{count:num_pad$}{ResetColor} {bold}{command}{ResetColor}");
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
            let start = OffsetDateTime::now_local()?.replace_time(Time::MIDNIGHT);
            let end = start + Duration::days(1);
            db.range(start, end).await?
        } else if words.trim() == "month" {
            let end = OffsetDateTime::now_local()?.replace_time(Time::MIDNIGHT);
            let start = end - Duration::days(31);
            db.range(start, end).await?
        } else if words.trim() == "week" {
            let end = OffsetDateTime::now_local()?.replace_time(Time::MIDNIGHT);
            let start = end - Duration::days(7);
            db.range(start, end).await?
        } else if words.trim() == "year" {
            let end = OffsetDateTime::now_local()?.replace_time(Time::MIDNIGHT);
            let start = end - Duration::days(365);
            db.range(start, end).await?
        } else {
            let start = parse_date_string(
                &words,
                OffsetDateTime::now_local()?,
                settings.dialect.into(),
            )?;
            let end = start + Duration::days(1);
            db.range(start, end).await?
        };
        compute_stats(&history, self.count)?;
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
    use super::interesting_command;

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
}
