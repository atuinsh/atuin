use std::collections::{HashMap, HashSet};

use atuin_common::utils::Escapable as _;
use clap::Parser;
use crossterm::style::{Color, ResetColor, SetAttribute, SetForegroundColor};
use eyre::{bail, Result};
use interim::parse_date_string;

use atuin_client::{
    database::{current_context, Database},
    history::History,
    settings::Settings,
};
use time::{Duration, OffsetDateTime, Time};

#[derive(Parser, Debug)]
#[command(infer_subcommands = true)]
pub struct Cmd {
    /// compute statistics for the specified period, leave blank for statistics since the beginning
    period: Vec<String>,

    /// How many top commands to list
    #[arg(long, short, default_value = "10")]
    count: usize,
}

fn compute_stats(settings: &Settings, history: &[History], count: usize) -> Result<()> {
    let mut commands = HashSet::<&str>::with_capacity(history.len());
    let mut prefixes = HashMap::<&str, usize>::with_capacity(history.len());
    for i in history {
        // just in case it somehow has a leading tab or space or something (legacy atuin didn't ignore space prefixes)
        let command = i.command.trim();
        commands.insert(command);
        *prefixes
            .entry(interesting_command(settings, command))
            .or_default() += 1;
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

        println!(
            "{ResetColor}] {gray}{count:num_pad$}{ResetColor} {bold}{}{ResetColor}",
            command.escape_control()
        );
    }
    println!("Total commands:   {}", history.len());
    println!("Unique commands:  {unique}");

    Ok(())
}

impl Cmd {
    pub async fn run(&self, db: &impl Database, settings: &Settings) -> Result<()> {
        let context = current_context();
        let words = if self.period.is_empty() {
            String::from("all")
        } else {
            self.period.join(" ")
        };

        let now = OffsetDateTime::now_utc();
        let now = settings.local_tz.map_or(now, |local| now.to_offset(local));
        let last_night = now.replace_time(Time::MIDNIGHT);

        let history = if words.as_str() == "all" {
            db.list(&[], &context, None, false, false).await?
        } else if words.trim() == "today" {
            let start = last_night;
            let end = start + Duration::days(1);
            db.range(start, end).await?
        } else if words.trim() == "month" {
            let end = last_night;
            let start = end - Duration::days(31);
            db.range(start, end).await?
        } else if words.trim() == "week" {
            let end = last_night;
            let start = end - Duration::days(7);
            db.range(start, end).await?
        } else if words.trim() == "year" {
            let end = last_night;
            let start = end - Duration::days(365);
            db.range(start, end).await?
        } else {
            let start = parse_date_string(&words, now, settings.dialect.into())?;
            let end = start + Duration::days(1);
            db.range(start, end).await?
        };
        compute_stats(settings, &history, self.count)?;
        Ok(())
    }
}

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

fn interesting_command<'a>(settings: &Settings, mut command: &'a str) -> &'a str {
    // Sort by length so that we match the longest prefix first
    let mut common_prefix = settings.stats.common_prefix.clone();
    common_prefix.sort_by_key(|b| std::cmp::Reverse(b.len()));

    // Trim off the common prefix, if it exists
    for p in &common_prefix {
        if command.starts_with(p) {
            let i = p.len();
            let prefix = &command[..i];
            command = command[i..].trim_start();
            if command.is_empty() {
                // no commands following, just use the prefix
                return prefix;
            }
            break;
        }
    }

    // Sort the common_subcommands by length so that we match the longest subcommand first
    let mut common_subcommands = settings.stats.common_subcommands.clone();
    common_subcommands.sort_by_key(|b| std::cmp::Reverse(b.len()));

    // Check for a common subcommand
    for p in &common_subcommands {
        if command.starts_with(p) {
            // if the subcommand is the same length as the command, then we just use the subcommand
            if p.len() == command.len() {
                return command;
            }
            // otherwise we need to use the subcommand + the next word
            let non_whitespace = first_non_whitespace(&command[p.len()..]).unwrap_or(0);
            let j =
                p.len() + non_whitespace + first_whitespace(&command[p.len() + non_whitespace..]);
            return &command[..j];
        }
    }
    // Return the first word if there is no subcommand
    &command[..first_whitespace(command)]
}

#[cfg(test)]
mod tests {
    use atuin_client::settings::Settings;

    use super::interesting_command;

    #[test]
    fn interesting_commands() {
        let settings = Settings::default();

        assert_eq!(interesting_command(&settings, "cargo"), "cargo");
        assert_eq!(
            interesting_command(&settings, "cargo build foo bar"),
            "cargo build"
        );
        assert_eq!(
            interesting_command(&settings, "sudo   cargo build foo bar"),
            "cargo build"
        );
        assert_eq!(interesting_command(&settings, "sudo"), "sudo");
    }

    // Test with spaces in the common_prefix
    #[test]
    fn interesting_commands_spaces() {
        let mut settings = Settings::default();
        settings.stats.common_prefix.push("sudo test".to_string());

        assert_eq!(interesting_command(&settings, "sudo test"), "sudo test");
        assert_eq!(interesting_command(&settings, "sudo test  "), "sudo test");
        assert_eq!(interesting_command(&settings, "sudo test foo bar"), "foo");
        assert_eq!(
            interesting_command(&settings, "sudo test    foo bar"),
            "foo"
        );

        // Works with a common_subcommand as well
        assert_eq!(
            interesting_command(&settings, "sudo test cargo build foo bar"),
            "cargo build"
        );

        // We still match on just the sudo prefix
        assert_eq!(interesting_command(&settings, "sudo"), "sudo");
        assert_eq!(interesting_command(&settings, "sudo foo"), "foo");
    }

    // Test with spaces in the common_subcommand
    #[test]
    fn interesting_commands_spaces_subcommand() {
        let mut settings = Settings::default();
        settings
            .stats
            .common_subcommands
            .push("cargo build".to_string());

        assert_eq!(interesting_command(&settings, "cargo build"), "cargo build");
        assert_eq!(
            interesting_command(&settings, "cargo build   "),
            "cargo build"
        );
        assert_eq!(
            interesting_command(&settings, "cargo build foo bar"),
            "cargo build foo"
        );

        // Works with a common_prefix as well
        assert_eq!(
            interesting_command(&settings, "sudo cargo build foo bar"),
            "cargo build foo"
        );

        // We still match on just cargo as a subcommand
        assert_eq!(interesting_command(&settings, "cargo"), "cargo");
        assert_eq!(interesting_command(&settings, "cargo foo"), "cargo foo");
    }

    // Test with spaces in the common_prefix and common_subcommand
    #[test]
    fn interesting_commands_spaces_both() {
        let mut settings = Settings::default();
        settings.stats.common_prefix.push("sudo test".to_string());
        settings
            .stats
            .common_subcommands
            .push("cargo build".to_string());

        assert_eq!(
            interesting_command(&settings, "sudo test cargo build"),
            "cargo build"
        );
        assert_eq!(
            interesting_command(&settings, "sudo test   cargo build"),
            "cargo build"
        );
        assert_eq!(
            interesting_command(&settings, "sudo test cargo build   "),
            "cargo build"
        );
        assert_eq!(
            interesting_command(&settings, "sudo test cargo build foo bar"),
            "cargo build foo"
        );
    }
}
