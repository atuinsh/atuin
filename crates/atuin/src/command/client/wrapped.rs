use crossterm::style::{ResetColor, SetAttribute};
use eyre::Result;
use std::collections::{HashMap, HashSet};
use time::{Date, Duration, Month, OffsetDateTime, Time};

use atuin_client::{database::Database, settings::Settings, theme::Theme};

use atuin_history::stats::{compute, Stats};

#[derive(Debug)]
struct WrappedStats {
    nav_commands: usize,
    pkg_commands: usize,
    error_rate: f64,
    first_half_commands: Vec<(String, usize)>,
    second_half_commands: Vec<(String, usize)>,
    git_percentage: f64,
    busiest_hour: Option<(String, usize)>,
}

impl WrappedStats {
    #[allow(clippy::too_many_lines, clippy::cast_precision_loss)]
    fn new(settings: &Settings, stats: &Stats, history: &[atuin_client::history::History]) -> Self {
        let nav_commands = stats
            .top
            .iter()
            .filter(|(cmd, _)| {
                let cmd = &cmd[0];
                cmd == "cd" || cmd == "ls" || cmd == "pwd" || cmd == "pushd" || cmd == "popd"
            })
            .map(|(_, count)| count)
            .sum();

        let pkg_managers = [
            "cargo",
            "npm",
            "pnpm",
            "yarn",
            "pip",
            "pip3",
            "pipenv",
            "poetry",
            "brew",
            "apt",
            "apt-get",
            "apk",
            "pacman",
            "yum",
            "dnf",
            "zypper",
            "pkg",
            "chocolatey",
            "choco",
            "scoop",
            "winget",
            "gem",
            "bundle",
            "composer",
            "gradle",
            "maven",
            "mvn",
            "go get",
            "nuget",
            "dotnet",
            "mix",
            "hex",
            "rebar3",
        ];

        let pkg_commands = history
            .iter()
            .filter(|h| {
                let cmd = h.command.clone();
                pkg_managers.iter().any(|pm| cmd.starts_with(pm))
            })
            .count();

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
            let local_time = entry
                .timestamp
                .to_offset(time::UtcOffset::current_local_offset().unwrap_or(settings.timezone.0));
            let hour = format!("{:02}:00", local_time.time().hour());
            *hours.entry(hour).or_default() += 1;
        }

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
            error_rate,
            first_half_commands: first_half,
            second_half_commands: second_half,
            git_percentage,
            busiest_hour,
        }
    }
}

pub fn print_wrapped_header(year: i32) {
    let reset = ResetColor;
    let bold = SetAttribute(crossterm::style::Attribute::Bold);

    println!("{bold}╭────────────────────────────────────╮{reset}");
    println!("{bold}│        ATUIN WRAPPED {year}          │{reset}");
    println!("{bold}│    Your Year in Shell History      │{reset}");
    println!("{bold}╰────────────────────────────────────╯{reset}");
    println!();
}

#[allow(clippy::cast_precision_loss)]
fn print_fun_facts(wrapped_stats: &WrappedStats, stats: &Stats, year: i32) {
    let reset = ResetColor;
    let bold = SetAttribute(crossterm::style::Attribute::Bold);

    if wrapped_stats.git_percentage > 0.05 {
        println!(
            "{bold}🌟 You're a Git Power User!{reset} {bold}{:.1}%{reset} of your commands were Git operations\n",
            wrapped_stats.git_percentage * 100.0
        );
    }
    // Navigation patterns
    let nav_percentage = wrapped_stats.nav_commands as f64 / stats.total_commands as f64 * 100.0;
    if nav_percentage > 0.05 {
        println!(
            "{bold}🚀 You're a Navigator!{reset} {bold}{nav_percentage:.1}%{reset} of your time was spent navigating directories\n",
        );
    }

    // Command vocabulary
    println!(
        "{bold}📚 Command Vocabulary{reset}: You know {bold}{}{reset} unique commands\n",
        stats.unique_commands
    );

    // Package management
    println!(
        "{bold}📦 Package Management{reset}: You ran {bold}{}{reset} package-related commands\n",
        wrapped_stats.pkg_commands
    );

    // Error patterns
    let error_percentage = wrapped_stats.error_rate * 100.0;
    println!(
        "{bold}🚨 Error Analysis{reset}: Your commands failed {bold}{error_percentage:.1}%{reset} of the time\n",
    );

    // Command evolution
    println!("🔍 Command Evolution:");

    // print stats for each half and compare
    println!("  {bold}Top Commands{reset} in the first half of {year}:");
    for (cmd, count) in wrapped_stats.first_half_commands.iter().take(3) {
        println!("    {bold}{cmd}{reset} ({count} times)");
    }

    println!("  {bold}Top Commands{reset} in the second half of {year}:");
    for (cmd, count) in wrapped_stats.second_half_commands.iter().take(3) {
        println!("    {bold}{cmd}{reset} ({count} times)");
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
        println!("  {bold}New favorites{reset} in the second half:");
        for (cmd, count) in new_favorites {
            println!("    {bold}{cmd}{reset} ({count} times)");
        }
    }

    // Time patterns
    if let Some((hour, count)) = &wrapped_stats.busiest_hour {
        println!("\n🕘 Most Productive Hour: {bold}{hour}{reset} ({count} commands)",);

        // Night owl or early bird
        let hour_num = hour
            .split(':')
            .next()
            .unwrap_or("0")
            .parse::<u32>()
            .unwrap_or(0);
        if hour_num >= 22 || hour_num <= 4 {
            println!("  You're quite the night owl! 🦉");
        } else if (5..=7).contains(&hour_num) {
            println!("  Early bird gets the worm! 🐦");
        }
    }

    println!();
}

pub async fn run(
    year: Option<i32>,
    db: &impl Database,
    settings: &Settings,
    theme: &Theme,
) -> Result<()> {
    let now = OffsetDateTime::now_utc().to_offset(settings.timezone.0);
    let month = now.month();

    // If we're in December, then wrapped is for the current year. If not, it's for the previous year
    let year = year.unwrap_or_else(|| {
        if month == Month::December {
            now.year()
        } else {
            now.year() - 1
        }
    });

    let start = OffsetDateTime::new_in_offset(
        Date::from_calendar_date(year, Month::January, 1).unwrap(),
        Time::MIDNIGHT,
        now.offset(),
    );
    let end = OffsetDateTime::new_in_offset(
        Date::from_calendar_date(year, Month::December, 31).unwrap(),
        Time::MIDNIGHT + Duration::days(1) - Duration::nanoseconds(1),
        now.offset(),
    );

    let history = db.range(start, end).await?;

    // Compute overall stats using existing functionality
    let stats = compute(settings, &history, 10, 1).expect("Failed to compute stats");
    let wrapped_stats = WrappedStats::new(settings, &stats, &history);

    // Print wrapped format
    print_wrapped_header(year);

    println!("🎉 In {year}, you typed {} commands!", stats.total_commands);
    println!(
        "   That's ~{} commands every day\n",
        stats.total_commands / 365
    );

    println!("Your Top Commands:");
    atuin_history::stats::pretty_print(stats.clone(), 1, theme);
    println!();

    print_fun_facts(&wrapped_stats, &stats, year);

    Ok(())
}
