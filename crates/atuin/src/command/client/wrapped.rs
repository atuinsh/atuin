use clap::Parser;
use crossterm::style::{Color, ResetColor, SetAttribute, SetForegroundColor};
use eyre::Result;
use std::collections::{HashMap, HashSet};
use time::{Duration, OffsetDateTime, Time};

use atuin_client::{
    database::{current_context, Database},
    settings::Settings,
    theme::{Meaning, Theme},
};

use atuin_history::stats::{compute, Stats};

#[derive(Debug)]
struct WrappedStats {
    nav_commands: usize,
    pkg_commands: usize,
    longest_pipeline: usize,
    most_complex_git: String,
    error_rate: f64,
    most_error_prone: String,
    first_half_commands: Vec<(String, usize)>,
    second_half_commands: Vec<(String, usize)>,
    git_percentage: f64,
    busiest_hour: Option<(String, usize)>,
}

impl WrappedStats {
    fn new(stats: &Stats, history: &[atuin_client::history::History]) -> Self {
        let nav_commands = stats
            .top
            .iter()
            .filter(|(cmd, _)| {
                let cmd = &cmd[0];
                cmd == "cd" || cmd == "ls" || cmd == "pwd" || cmd == "pushd" || cmd == "popd"
            })
            .map(|(_, count)| count)
            .sum();

        let pkg_commands = stats
            .top
            .iter()
            .filter(|(cmd, _)| {
                let cmd = &cmd[0];
                cmd.starts_with("cargo")
                    || cmd.starts_with("npm")
                    || cmd.starts_with("pnpm")
                    || cmd.starts_with("yarn")
                    || cmd.starts_with("pip")
                    || cmd.starts_with("brew")
                    || cmd.starts_with("apt")
                    || cmd.starts_with("pacman")
            })
            .map(|(_, count)| count)
            .sum();

        let longest_pipeline = stats
            .top
            .iter()
            .map(|(cmd, _)| cmd.len())
            .max()
            .unwrap_or(0);

        let most_complex_git = stats
            .top
            .iter()
            .filter(|(cmd, _)| cmd[0].starts_with("git"))
            .max_by_key(|(cmd, _)| cmd[0].len())
            .map(|(cmd, _)| cmd[0].clone())
            .unwrap_or_else(|| "git".to_string());

        // Error analysis
        let mut command_errors: HashMap<String, (usize, usize)> = HashMap::new(); // (total_uses, errors)
        let midyear = history[0].timestamp + Duration::days(182); // Split year in half

        let mut first_half_commands: HashMap<String, usize> = HashMap::new();
        let mut second_half_commands: HashMap<String, usize> = HashMap::new();
        let mut hours: HashMap<String, usize> = HashMap::new();

        for entry in history {
            let cmd = entry
                .command
                .split_whitespace()
                .next()
                .unwrap_or("")
                .to_string();
            let (total, errors) = command_errors.entry(cmd.clone()).or_insert((0, 0));
            *total += 1;
            if entry.exit != 0 {
                *errors += 1;
            }

            // Track command evolution
            if entry.timestamp < midyear {
                *first_half_commands.entry(cmd.clone()).or_default() += 1;
            } else {
                *second_half_commands.entry(cmd).or_default() += 1;
            }

            // Track hourly distribution
            let hour = format!("{:02}:00", entry.timestamp.time().hour());
            *hours.entry(hour).or_default() += 1;
        }

        // Calculate error rates
        let most_error_prone = command_errors
            .iter()
            .filter(|(_, (total, _))| *total >= 10)
            .max_by_key(|(_, (total, errors))| (*errors as f64 / *total as f64 * 10000.0) as usize)
            .map(|(cmd, _)| cmd.clone())
            .unwrap_or_else(|| "none".to_string());

        let total_errors: usize = command_errors.values().map(|(_, errors)| errors).sum();
        let total_commands: usize = command_errors.values().map(|(total, _)| total).sum();
        let error_rate = total_errors as f64 / total_commands as f64;

        // Process command evolution data
        let mut first_half: Vec<_> = first_half_commands.into_iter().collect();
        let mut second_half: Vec<_> = second_half_commands.into_iter().collect();
        first_half.sort_by_key(|(_, count)| std::cmp::Reverse(*count));
        second_half.sort_by_key(|(_, count)| std::cmp::Reverse(*count));
        first_half.truncate(5);
        second_half.truncate(5);

        // Calculate git percentage
        let git_commands: usize = stats
            .top
            .iter()
            .filter(|(cmd, _)| cmd[0].starts_with("git"))
            .map(|(_, count)| count)
            .sum();
        let git_percentage = git_commands as f64 / stats.total_commands as f64;

        // Find busiest hour
        let busiest_hour = hours.into_iter().max_by_key(|(_, count)| *count);

        Self {
            nav_commands,
            pkg_commands,
            longest_pipeline,
            most_complex_git,
            error_rate,
            most_error_prone,
            first_half_commands: first_half,
            second_half_commands: second_half,
            git_percentage,
            busiest_hour,
        }
    }
}

pub fn print_wrapped_header(theme: &Theme) {
    let header_style = SetForegroundColor(match theme.as_style(Meaning::Title).foreground_color {
        Some(color) => color,
        None => Color::Cyan,
    });
    let reset = ResetColor;

    println!("{header_style}╭────────────────────────────────────╮{reset}");
    println!("{header_style}│        ATUIN WRAPPED 2024          │{reset}");
    println!("{header_style}│   Your Year in Shell History       │{reset}");
    println!("{header_style}╰────────────────────────────────────╯{reset}");
    println!();
}

pub fn print_fun_facts(wrapped_stats: &WrappedStats, stats: &Stats, theme: &Theme) {
    let highlight = SetForegroundColor(match theme.as_style(Meaning::Title).foreground_color {
        Some(color) => color,
        None => Color::Yellow,
    });
    let reset = ResetColor;
    let bold = SetAttribute(crossterm::style::Attribute::Bold);

    println!("{bold}✨ Fun Facts:{reset}");

    // Git usage
    println!(
        "- You're a Git Power User! {highlight}{:.1}%{reset} of your commands were Git operations",
        wrapped_stats.git_percentage * 100.0
    );
    println!(
        "  Most complex git command: {highlight}{}{reset}",
        wrapped_stats.most_complex_git
    );

    // Navigation patterns
    let nav_percentage = wrapped_stats.nav_commands as f64 / stats.total_commands as f64 * 100.0;
    println!(
        "- File Explorer: {highlight}{:.1}%{reset} of your time was spent navigating directories",
        nav_percentage
    );

    // Command vocabulary
    println!(
        "- Command Vocabulary: You know {highlight}{}{reset} unique commands",
        stats.unique_commands
    );
    println!(
        "  Your longest command pipeline had {highlight}{}{reset} steps!",
        wrapped_stats.longest_pipeline
    );

    // Package management
    println!(
        "- Package Management: You ran {highlight}{}{reset} package-related commands",
        wrapped_stats.pkg_commands
    );

    // Error patterns
    println!(
        "- Terminal Troubles: Your command error rate was {highlight}{:.1}%{reset}",
        wrapped_stats.error_rate * 100.0
    );
    println!(
        "  Most error-prone command: {highlight}{}{reset} (maybe time for an alias?)",
        wrapped_stats.most_error_prone
    );

    // Command evolution
    println!("- Command Evolution:");
    println!("  First half of 2024:");
    for (cmd, count) in wrapped_stats.first_half_commands.iter().take(3) {
        println!("    {highlight}{}{reset} ({} times)", cmd, count);
    }
    println!("  Second half of 2024:");
    for (cmd, count) in wrapped_stats.second_half_commands.iter().take(3) {
        println!("    {highlight}{}{reset} ({} times)", cmd, count);
    }

    // Find new favorite commands (in top 5 of second half but not in first half)
    let first_half_set: HashSet<_> = wrapped_stats
        .first_half_commands
        .iter()
        .map(|(cmd, _)| cmd)
        .collect();
    let new_favorites: Vec<_> = wrapped_stats
        .second_half_commands
        .iter()
        .filter(|(cmd, _)| !first_half_set.contains(cmd))
        .take(2)
        .collect();

    if !new_favorites.is_empty() {
        println!("  {highlight}New favorites{reset} in the second half:");
        for (cmd, count) in new_favorites {
            println!("    {highlight}{}{reset} ({} times)", cmd, count);
        }
    }

    // Time patterns
    if let Some((hour, count)) = &wrapped_stats.busiest_hour {
        println!(
            "- Most Productive Hour: {highlight}{}{reset} ({} commands)",
            hour, count
        );

        // Night owl or early bird
        let hour_num = hour
            .split(':')
            .next()
            .unwrap_or("0")
            .parse::<u32>()
            .unwrap_or(0);
        if hour_num >= 22 || hour_num <= 4 {
            println!("  You're quite the night owl! 🦉");
        } else if hour_num >= 5 && hour_num <= 7 {
            println!("  Early bird gets the worm! 🐦");
        }
    }

    println!();
}

pub async fn run(db: &impl Database, settings: &Settings, theme: &Theme) -> Result<()> {
    let now = OffsetDateTime::now_utc().to_offset(settings.timezone.0);
    let last_night = now.replace_time(Time::MIDNIGHT);

    // Get history for the whole year
    let end = last_night;
    let start = end - Duration::days(365);
    let history = db.range(start, end).await?;

    // Compute overall stats using existing functionality
    let stats = compute(settings, &history, 10, 1).expect("Failed to compute stats");
    let wrapped_stats = WrappedStats::new(&stats, &history);

    // Print wrapped format
    print_wrapped_header(theme);

    println!("🎉 In 2024, you typed {} commands!", stats.total_commands);
    println!(
        "   That's ~{} commands every day\n",
        stats.total_commands / 365
    );

    println!("Your Top Commands:");
    atuin_history::stats::pretty_print(stats.clone(), 1, theme);
    println!();

    print_fun_facts(&wrapped_stats, &stats, theme);

    Ok(())
}
