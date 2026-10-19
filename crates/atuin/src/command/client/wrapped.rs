use std::collections::{HashMap, HashSet};

use atuin_client::database::Sqlite;
use atuin_client::record::sqlite_store::SqliteStore;
use atuin_client::settings::Settings;
use atuin_client::theme::Theme;
use atuin_history::stats::{Stats, compute, split_common_prefix};
use crossterm::style::{ResetColor, SetAttribute};
use eyre::Result;
use time::{Date, Duration, Month, OffsetDateTime, Time};
use tracing::instrument;

fn package_subcommand(program: &str, words: &mut impl Iterator<Item = String>) -> String {
    while let Some(word) = words.next() {
        if word == "--" {
            return words.next().unwrap_or_default();
        }
        if word.starts_with('-') {
            if package_option_takes_value(program, &word) {
                words.next();
            }
        } else if !(program == "cargo" && word.starts_with('+')) {
            return word;
        }
    }
    String::new()
}

fn package_option_takes_value(program: &str, option: &str) -> bool {
    matches!(
        (program, option),
        ("cargo", "-C" | "-Z" | "--config" | "--color")
            | (
                "npm",
                "--prefix"
                    | "-w"
                    | "--workspace"
                    | "--registry"
                    | "--userconfig"
                    | "--cache"
                    | "--loglevel"
                    | "--location"
            )
            | ("pnpm", "-C" | "--dir" | "-F" | "--filter" | "--store-dir" | "--registry")
            | (
                "yarn",
                "--cwd" | "--cache-folder" | "--modules-folder" | "--registry" | "--use-yarnrc"
            )
            | ("uv", "--directory" | "--project" | "--config-file" | "--cache-dir" | "--color")
            | ("poetry", "-C" | "--directory" | "-P" | "--project")
            | ("bundle", "--gemfile")
            | ("composer", "-d" | "--working-dir")
            | ("go", "-C")
    )
}

fn is_package_command(settings: &Settings, command: &str) -> bool {
    let command = command.trim_start();
    let command = split_common_prefix(settings, command).map_or(command, |(_, rest)| rest);
    // Only lex the words needed for classification. Arguments after the operation may use
    // another shell's syntax, such as a PowerShell path ending in a backslash.
    let mut words = shlex::Shlex::new(command);
    let Some(program) = words.next() else {
        return false;
    };
    let program = program.as_str();
    let subcommand = package_subcommand(program, &mut words);
    let subcommand = subcommand.as_str();
    let subcommand = if program == "cabal" {
        subcommand
            .strip_prefix("v1-")
            .or_else(|| subcommand.strip_prefix("v2-"))
            .unwrap_or(subcommand)
    } else {
        subcommand
    };

    match program {
        "cargo" | "npm" | "pnpm" | "pipenv" | "poetry" | "bundle" | "composer" | "cabal"
        | "yarn" => {
            (matches!(program, "yarn" | "bundle") && subcommand.is_empty())
                || matches!(
                    (program, subcommand),
                    ("pipenv", "clean" | "check")
                        | ("bundle" | "poetry", "check")
                        | ("npm", "i" | "un" | "r")
                        | ("pnpm", "i")
                )
                || PACKAGE_SUBCOMMANDS.contains(&subcommand)
        }
        "uv" => match subcommand {
            "tool" => matches!(
                words.next().as_deref(),
                Some("install" | "uninstall" | "upgrade" | "list")
            ),
            "pip" | "add" | "remove" | "sync" | "lock" | "tree" | "export" | "cache" | "build" => {
                true
            }
            _ => false,
        },
        "dotnet" => match subcommand {
            "restore" | "nuget" => true,
            "add" | "remove" | "list" => match words.next().as_deref() {
                Some("package") => true,
                Some("reference") | None => false,
                Some(project) => {
                    !project.starts_with('-') && words.next().as_deref() == Some("package")
                }
            },
            "package" => {
                matches!(words.next().as_deref(), Some("add" | "remove" | "list" | "search"))
            }
            "tool" => {
                matches!(
                    words.next().as_deref(),
                    Some("install" | "uninstall" | "update" | "restore" | "list")
                )
            }
            _ => false,
        },
        "go" => {
            subcommand == "get"
                || (subcommand == "mod"
                    && matches!(
                        words.next().as_deref(),
                        Some("download" | "tidy" | "vendor" | "graph" | "verify" | "edit" | "why")
                    ))
        }
        "nix" => {
            subcommand == "profile"
                || (subcommand == "flake"
                    && matches!(words.next().as_deref(), Some("update" | "lock")))
        }
        "mix" => {
            subcommand.starts_with("deps.")
                || subcommand.starts_with("hex.")
                || matches!(subcommand, "local.hex" | "local.rebar")
        }
        "rebar3" => {
            matches!(subcommand, "get-deps" | "update" | "upgrade" | "hex" | "lock" | "tree")
        }
        "gradle" => std::iter::once(subcommand.to_owned()).chain(words).any(|task| {
            matches!(
                task.as_str(),
                "dependencies" | "dependencyInsight" | "publish" | "publishToMavenLocal"
            )
        }),
        "maven" | "mvn" => std::iter::once(subcommand.to_owned()).chain(words).any(|goal| {
            goal.starts_with("dependency:") || matches!(goal.as_str(), "install" | "deploy")
        }),
        "pipx" => subcommand != "run",
        _ => DEDICATED_PACKAGE_MANAGERS.contains(&program),
    }
}

const PACKAGE_SUBCOMMANDS: &[&str] = &[
    "add",
    "remove",
    "rm",
    "uninstall",
    "install",
    "update",
    "upgrade",
    "up",
    "require",
    "outdated",
    "list",
    "ls",
    "info",
    "show",
    "search",
    "view",
    "why",
    "explain",
    "audit",
    "fund",
    "ci",
    "link",
    "unlink",
    "rebuild",
    "dedupe",
    "prune",
    "fetch",
    "sync",
    "lock",
    "generate-lockfile",
    "tree",
    "export",
    "requirements",
    "publish",
    "unpublish",
    "pack",
    "package",
    "cache",
    "owner",
    "yank",
    "vendor",
    "pristine",
];

const DEDICATED_PACKAGE_MANAGERS: &[&str] = &[
    "pip",
    "pip3",
    "brew",
    "apt",
    "apt-get",
    "apk",
    "pacman",
    "yay",
    "paru",
    "yum",
    "dnf",
    "dnf5",
    "rpm",
    "rpm-ostree",
    "zypper",
    "pkg",
    "chocolatey",
    "choco",
    "scoop",
    "winget",
    "gem",
    "shards",
    "nuget",
    "hex",
    "nix-env",
    "opam",
];

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
    fn new(
        settings: &Settings,
        stats: &Stats,
        history: &[atuin_client::history::History],
        alias_map: &HashMap<String, String>,
    ) -> Self {
        // Helper to expand alias to its first command word
        let expand_alias = |cmd: &str| -> String {
            alias_map.get(cmd).map_or_else(
                || cmd.to_string(),
                |expanded| expanded.split_whitespace().next().unwrap_or(cmd).to_string(),
            )
        };

        let nav_commands = stats
            .top
            .iter()
            .filter(|(cmd, _)| {
                let cmd = &cmd[0];
                cmd == "cd" || cmd == "ls" || cmd == "pwd" || cmd == "pushd" || cmd == "popd"
            })
            .map(|(_, count)| count)
            .sum();

        let pkg_commands =
            history.iter().filter(|h| is_package_command(settings, &h.command)).count();

        // Error analysis
        let mut command_errors: HashMap<String, (usize, usize)> = HashMap::new(); // (total_uses, errors)
        let midyear = history[0].timestamp + Duration::days(182); // Split year in half

        let mut first_half_commands: HashMap<String, usize> = HashMap::new();
        let mut second_half_commands: HashMap<String, usize> = HashMap::new();
        let mut hours: HashMap<String, usize> = HashMap::new();

        for entry in history {
            let command = entry.command.trim_start();
            let command =
                split_common_prefix(settings, command).map_or(command, |(prefix, rest)| {
                    if rest.is_empty() {
                        prefix
                    } else {
                        rest
                    }
                });
            let raw_cmd = command.split_whitespace().next().unwrap_or("");
            let cmd = expand_alias(raw_cmd);
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
            "{bold}🌟 You're a Git Power User!{reset} {bold}{:.1}%{reset} of your commands were \
             Git operations\n",
            wrapped_stats.git_percentage * 100.0
        );
    }
    // Navigation patterns
    let nav_percentage = wrapped_stats.nav_commands as f64 / stats.total_commands as f64 * 100.0;
    if nav_percentage > 0.05 {
        println!(
            "{bold}🚀 You're a Navigator!{reset} {bold}{nav_percentage:.1}%{reset} of your time \
             was spent navigating directories\n",
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
        "{bold}🚨 Error Analysis{reset}: Your commands failed {bold}{error_percentage:.1}%{reset} \
         of the time\n",
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
    let first_half_set: HashSet<_> =
        wrapped_stats.first_half_commands.iter().map(|(cmd, _)| cmd).collect();
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
        println!("\n🕘 Most Productive Hour: {bold}{hour}{reset} ({count} commands)");

        // Night owl or early bird
        let hour_num = hour.split(':').next().unwrap_or("0").parse::<u32>().unwrap_or(0);
        if hour_num >= 22 || hour_num <= 4 {
            println!("  You're quite the night owl! 🦉");
        } else if (5..=7).contains(&hour_num) {
            println!("  Early bird gets the worm! 🐦");
        }
    }

    println!();
}

#[instrument(level = "trace", skip_all, err)]
pub async fn run(
    year: Option<i32>,
    db: &Sqlite,
    settings: &Settings,
    _store: SqliteStore,
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
    if history.is_empty() {
        println!(
            "Your history for {year} is empty!\nMaybe 'atuin import' could help you import your \
             previous history 🪄"
        );
        return Ok(());
    }

    let Some(stats) = compute(settings, &history, 10, 1) else {
        println!(
            "No commands found in your {year} history. Run a command or check your config for for \
             commands ignored by stats.ignored_commands."
        );
        return Ok(());
    };
    let alias_map = HashMap::new();
    let wrapped_stats = WrappedStats::new(settings, &stats, &history, &alias_map);

    // Print wrapped format
    print_wrapped_header(year);

    println!("🎉 In {year}, you typed {} commands!", stats.total_commands);
    println!("   That's ~{} commands every day\n", stats.total_commands / 365);

    println!("Your Top Commands:");
    atuin_history::stats::pretty_print(stats.clone(), 1, theme);
    println!();

    print_fun_facts(&wrapped_stats, &stats, year);

    Ok(())
}

#[cfg(test)]
mod tests {
    use atuin_client::history::History;
    use rstest::{fixture, rstest};
    use time::macros::datetime;

    use super::*;

    #[fixture]
    fn history() -> History {
        History::import()
            .timestamp(datetime!(2024-01-01 0:00 UTC))
            .command("echo hello")
            .build()
            .into()
    }

    #[rstest]
    #[case("cargo add serde", 1)]
    #[case("cargo remove serde", 1)]
    #[case("cargo install atuin", 1)]
    #[case("cargo +nightly install atuin", 1)]
    #[case("cargo build", 0)]
    #[case("cargo test", 0)]
    #[case("cargo check", 0)]
    #[case("cargo r", 0)]
    #[case("cargo run", 0)]
    #[case("cargo +nightly fmt", 0)]
    #[case("uv add requests", 1)]
    #[case("uv pip install requests", 1)]
    #[case("uv tool install ruff", 1)]
    #[case("uv tool run ruff", 0)]
    #[case("uv run python script.py", 0)]
    #[case("uv venv", 0)]
    #[case("npm install", 1)]
    #[case("npm i", 1)]
    #[case("npm un typescript", 1)]
    #[case("npm ci", 1)]
    #[case("npm --prefix ./app install", 1)]
    #[case("npm --prefix=./app install", 1)]
    #[case("npm --prefix './my app' install", 1)]
    #[case("npm --registry https://registry.npmjs.org install", 1)]
    #[case("npm --workspace app install", 1)]
    #[case("npm --prefix install run build", 0)]
    #[case("npm --prefix ./app run build", 0)]
    #[case("uv --directory ./app add requests", 1)]
    #[case("uv --directory=./app add requests", 1)]
    #[case("uv --directory './my app' add requests", 1)]
    #[case("uv --project ./app sync", 1)]
    #[case("uv --directory add run python", 0)]
    #[case("uv --directory ./app run python", 0)]
    #[case("pnpm -C ./app add typescript", 1)]
    #[case("pnpm --filter app add typescript", 1)]
    #[case("pnpm --filter add exec tsc", 0)]
    #[case("yarn --cwd ./app", 1)]
    #[case("yarn --cwd ./app start", 0)]
    #[case("bundle --gemfile ./Gemfile", 1)]
    #[case("cargo --color always add serde", 1)]
    #[case("cargo +nightly -Z unstable-options install atuin", 1)]
    #[case("cargo --config install build", 0)]
    #[case("poetry -C ./app add requests", 1)]
    #[case("composer --working-dir ./app require monolog/monolog", 1)]
    #[case("go -C ./app mod tidy", 1)]
    #[case("npm run build", 0)]
    #[case("npm test", 0)]
    #[case("pnpm add typescript", 1)]
    #[case("pnpm i", 1)]
    #[case("pnpm exec tsc", 0)]
    #[case("yarn", 1)]
    #[case("yarn add typescript", 1)]
    #[case("yarn start", 0)]
    #[case("pipenv install", 1)]
    #[case("pipenv run python", 0)]
    #[case("poetry add requests", 1)]
    #[case("poetry run python", 0)]
    #[case("bundle install", 1)]
    #[case("bundle", 1)]
    #[case("bundle exec rake", 0)]
    #[case("composer require monolog/monolog", 1)]
    #[case("composer run-script test", 0)]
    #[case("dotnet add package Newtonsoft.Json", 1)]
    #[case("dotnet add MyApp.csproj package Newtonsoft.Json", 1)]
    #[case("dotnet list MyApp.csproj package", 1)]
    #[case("dotnet remove MyApp.csproj package Newtonsoft.Json", 1)]
    #[case("dotnet add 'My App.csproj' package Newtonsoft.Json", 1)]
    #[case("dotnet list MySolution.sln package", 1)]
    #[case("dotnet add MyApp.csproj reference Other.csproj", 0)]
    #[case("dotnet list MyApp.csproj reference", 0)]
    #[case("dotnet remove MyApp.csproj reference Other.csproj", 0)]
    #[case("dotnet add reference package", 0)]
    #[case("dotnet list reference package", 0)]
    #[case("dotnet restore", 1)]
    #[case("dotnet run", 0)]
    #[case("dotnet add reference Other.csproj", 0)]
    #[case("dotnet tool run dotnetsay", 0)]
    #[case("mix deps.get", 1)]
    #[case("mix test", 0)]
    #[case("rebar3 get-deps", 1)]
    #[case("rebar3 compile", 0)]
    #[case("cabal install hlint", 1)]
    #[case("cabal v2-install hlint", 1)]
    #[case("cabal v2-build", 0)]
    #[case("cabal build", 0)]
    #[case("go get example.com/pkg", 1)]
    #[case("go mod tidy", 1)]
    #[case("go build", 0)]
    #[case("nix profile install nixpkgs#hello", 1)]
    #[case("nix flake update", 1)]
    #[case("nix build", 0)]
    #[case("gradle dependencies", 1)]
    #[case("gradle build", 0)]
    #[case("mvn dependency:resolve", 1)]
    #[case("mvn test", 0)]
    #[case("sudo apt install hello", 1)]
    #[case(" \t doas apk add hello", 1)]
    #[case("pip install requests", 1)]
    #[case(r"pip install C:\projects\demo\", 1)]
    #[case(r"npm install C:\projects\demo\", 1)]
    #[case(r"uv pip install C:\projects\demo\", 1)]
    #[case(r"cargo install --path C:\projects\demo\", 1)]
    #[case(r"dotnet add MyApp.csproj package Newtonsoft.Json --configfile C:\config\", 1)]
    #[case(r"npm run build C:\projects\demo\", 0)]
    #[case(r"uv run python C:\projects\demo\", 0)]
    #[case(r"dotnet list MyApp.csproj reference C:\projects\demo\", 0)]
    #[case("brew upgrade", 1)]
    #[case("pacman -S hello", 1)]
    #[case("nix-env -i hello", 1)]
    #[case("cargo-watch", 0)]
    #[case("npm-not-a-package-manager", 0)]
    #[case("go getty", 0)]
    #[case("echo cargo install atuin", 0)]
    #[case("", 0)]
    fn package_count_excludes_development_commands(
        mut history: History,
        #[case] command: &str,
        #[case] expected: usize,
    ) {
        history.command = command.to_owned();
        let stats = Stats {
            total_commands: 1,
            unique_commands: 1,
            top: vec![],
        };
        let wrapped = WrappedStats::new(&Settings::utc(), &stats, &[history], &HashMap::new());
        assert_eq!(wrapped.pkg_commands, expected);
    }

    #[rstest]
    #[case::default_prefix(None, "sudo echo hello", "echo")]
    #[case::default_doas(None, "doas echo hello", "echo")]
    #[case::custom_prefix(Some(vec!["please"]), "please echo hello", "echo")]
    #[case::longest_prefix(Some(vec!["sudo", "sudo -u root"]), "sudo -u root echo hello", "echo")]
    #[case::empty_prefixes(Some(vec![]), "sudo echo hello", "sudo")]
    #[case::unconfigured_prefix(Some(vec!["please"]), "sudo echo hello", "sudo")]
    #[case::prefix_only(None, "sudo", "sudo")]
    #[case::whitespace(None, " \t sudo \t echo hello", "echo")]
    #[case::empty(None, " \t ", "")]
    #[case::subcommand(None, "sudo git status", "git")]
    #[case::alias(None, "ll /tmp", "ls")]
    #[case::prefixed_alias(None, "sudo ll /tmp", "ls")]
    #[case::multiword_prefix_only(Some(vec!["sudo -u root"]), "sudo -u root  ", "sudo")]
    #[case::one_removal(None, "sudo doas echo hello", "doas")]
    #[case::partial_word(None, "sudoedit file", "edit")]
    fn evolution_respects_common_prefix(
        #[case] prefixes: Option<Vec<&str>>,
        #[case] command: &str,
        #[case] expected: &str,
    ) {
        let mut settings = Settings::utc();
        if let Some(prefixes) = prefixes {
            settings.stats.common_prefix = prefixes.into_iter().map(str::to_owned).collect();
        }
        let history: Vec<History> =
            [datetime!(2024-01-01 0:00 UTC), datetime!(2024-10-01 0:00 UTC)]
                .into_iter()
                .flat_map(|timestamp| {
                    [(command, 0), (expected, 1)].map(|(command, exit)| {
                        History::import()
                            .timestamp(timestamp)
                            .command(command)
                            .exit(exit)
                            .build()
                            .into()
                    })
                })
                .collect();
        let stats = Stats {
            total_commands: 4,
            unique_commands: 2,
            top: vec![],
        };
        let aliases = HashMap::from([("ll".to_string(), "ls -l".to_string())]);

        let wrapped = WrappedStats::new(&settings, &stats, &history, &aliases);

        assert_eq!(wrapped.first_half_commands, vec![(expected.to_string(), 2)]);
        assert_eq!(wrapped.second_half_commands, vec![(expected.to_string(), 2)]);
        assert!((wrapped.error_rate - 0.5).abs() < f64::EPSILON);
    }
}
