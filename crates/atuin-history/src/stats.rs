use std::collections::{HashMap, HashSet};
use std::fmt;
use std::num::NonZeroUsize;

use atuin_client::history::History;
use atuin_client::settings::Settings;
use atuin_client::theme::{Meaning, Theme};
use crossterm::style::{
    Attribute, Color, ContentStyle, ResetColor, SetAttribute, SetForegroundColor,
};
use easy_cast::Conv;
use serde::{Deserialize, Serialize};
use unicode_segmentation::UnicodeSegmentation;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Stats {
    pub total_commands: usize,
    pub unique_commands: usize,
    pub top: Vec<(Vec<String>, usize)>,
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

/// Split off the longest configured common prefix, returning the matched prefix
/// and the remainder with leading whitespace removed. Both slices borrow from
/// `command`; an empty remainder means the command consists only of the prefix.
#[must_use]
pub fn split_common_prefix<'a>(
    settings: &Settings,
    command: &'a str,
) -> Option<(&'a str, &'a str)> {
    // Sort by length so that we match the longest prefix first
    let mut common_prefix = settings.stats.common_prefix.clone();
    common_prefix.sort_by_key(|b| std::cmp::Reverse(b.len()));

    // Trim off the common prefix, if it exists
    for p in &common_prefix {
        if command.starts_with(p) {
            let i = p.len();
            return Some((&command[..i], command[i..].trim_start()));
        }
    }
    None
}

fn interesting_command<'a>(settings: &Settings, mut command: &'a str) -> &'a str {
    if let Some((prefix, remainder)) = split_common_prefix(settings, command) {
        if remainder.is_empty() {
            // no commands following, just use the prefix
            return prefix;
        }
        command = remainder;
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

fn split_at_pipe(command: &str) -> Vec<&str> {
    let mut result = vec![];
    let mut quoted = false;
    let mut start = 0;
    let mut graphemes = UnicodeSegmentation::grapheme_indices(command, true);

    while let Some((i, c)) = graphemes.next() {
        let current = i;
        match c {
            "\"" if command[start..current] != *"\"" => {
                quoted = !quoted;
            }
            "'" if command[start..current] != *"'" => {
                quoted = !quoted;
            }
            "\\" if graphemes.next().is_some() => {}
            "|" if !quoted => {
                if current > start && command[start..].starts_with('|') {
                    start += 1;
                }
                result.push(&command[start..current]);
                start = current;
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

fn strip_leading_env_vars(command: &str) -> &str {
    // fast path: no equals sign, no environment variable
    if !command.contains('=') {
        return command;
    }

    let mut in_token = false;
    let mut token_start_pos = 0;
    let mut in_single_quotes = false;
    let mut in_double_quotes = false;
    let mut escape_next = false;
    let mut has_equals_outside_quotes = false;

    for (i, g) in UnicodeSegmentation::grapheme_indices(command, true) {
        if escape_next {
            escape_next = false;
            continue;
        }

        if !in_token {
            token_start_pos = i;
        }

        match g {
            "\\" => {
                escape_next = true;
                in_token = true;
            }
            "'" if !in_double_quotes => {
                in_single_quotes = !in_single_quotes;
                in_token = true;
            }
            "\"" if !in_single_quotes => {
                in_double_quotes = !in_double_quotes;
                in_token = true;
            }
            "=" if !in_single_quotes && !in_double_quotes => {
                has_equals_outside_quotes = true;
                in_token = true;
            }
            " " | "\t" if !in_single_quotes && !in_double_quotes => {
                if in_token {
                    if !has_equals_outside_quotes {
                        // if we're not in an env var, we can break early
                        break;
                    }
                    in_token = false;
                    has_equals_outside_quotes = false;
                }
            }
            _ => {
                in_token = true;
            }
        }
    }

    command[token_start_pos..].trim()
}

/// [`Stats`] for a person to read: a row per top command, with a bar of its count, then the
/// totals.
pub struct StatsReport<'a> {
    stats: &'a Stats,
    ngram_size: usize,
    /// Colours the bars and counts. Without it the report has no escape codes, as a pipe or a file
    /// wants.
    theme: Option<&'a Theme>,
}

impl<'a> StatsReport<'a> {
    #[must_use]
    pub const fn new(stats: &'a Stats, ngram_size: usize, theme: Option<&'a Theme>) -> Self {
        Self {
            stats,
            ngram_size,
            theme,
        }
    }
}

impl fmt::Display for StatsReport<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let Codes {
            error,
            warning,
            info,
            muted,
            bold,
            reset,
        } = self.theme.map(Codes::new).unwrap_or_default();

        // `compute` always finds a top command, but a `Stats` can be built without one.
        let max = self.stats.top.iter().map(|(_, count)| *count).max().and_then(NonZeroUsize::new);
        if let Some(max) = max {
            let num_pad = usize::conv(max.ilog10()) + 1;

            // The longest command in each column.
            let column_widths = self
                .stats
                .top
                .iter()
                .map(|(commands, _)| commands.iter().map(|c| c.len()).collect::<Vec<usize>>())
                .fold(vec![0; self.ngram_size], |acc, item| {
                    acc.iter().zip(item.iter()).map(|(a, i)| *std::cmp::max(a, i)).collect()
                });

            for (command, count) in &self.stats.top {
                // The bar is red, then yellow from its third block and green from its sixth.
                let in_ten = 10 * count / max;
                write!(f, "[{error}")?;
                for i in 0..in_ten {
                    match i {
                        2 => f.write_str(&warning)?,
                        5 => f.write_str(&info)?,
                        _ => {}
                    }
                    f.write_str("▮")?;
                }
                for _ in in_ten..10 {
                    f.write_str(" ")?;
                }

                let formatted_command = command
                    .iter()
                    .zip(column_widths.iter())
                    .map(|(cmd, width)| format!("{cmd:width$}"))
                    .collect::<Vec<_>>()
                    .join(" | ");

                writeln!(
                    f,
                    "{reset}] {muted}{count:num_pad$}{reset} {bold}{formatted_command}{reset}"
                )?;
            }
        }

        writeln!(f, "Total commands:   {}", self.stats.total_commands)?;
        write!(f, "Unique commands:  {}", self.stats.unique_commands)
    }
}

/// The escape codes a [`StatsReport`] writes, each empty for a report without a theme.
#[derive(Default)]
struct Codes {
    error: String,
    warning: String,
    info: String,
    muted: String,
    bold: String,
    reset: String,
}

impl Codes {
    fn new(theme: &Theme) -> Self {
        let fg = |style: ContentStyle, fallback: Color| {
            SetForegroundColor(style.foreground_color.unwrap_or(fallback)).to_string()
        };
        Self {
            error: fg(theme.get_error(), Color::Red),
            warning: fg(theme.get_warning(), Color::Yellow),
            info: fg(theme.get_info(), Color::Green),
            muted: fg(theme.as_style(Meaning::Muted), Color::Grey),
            bold: SetAttribute(Attribute::Bold).to_string(),
            reset: ResetColor.to_string(),
        }
    }
}

#[must_use]
pub fn compute(
    settings: &Settings,
    history: &[History],
    count: usize,
    ngram_size: usize,
) -> Option<Stats> {
    let mut commands = HashSet::<&str>::with_capacity(history.len());
    let mut total_unignored = 0;
    let mut prefixes = HashMap::<Vec<&str>, usize>::with_capacity(history.len());

    for i in history {
        // just in case it somehow has a leading tab or space or something (legacy atuin didn't ignore space prefixes)
        let command = strip_leading_env_vars(i.command.trim());
        let prefix = interesting_command(settings, command);

        if settings.stats.ignored_commands.iter().any(|c| c == prefix) {
            continue;
        }

        total_unignored += 1;
        commands.insert(command);

        split_at_pipe(command)
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
                    .entry(w.iter().map(|c| interesting_command(settings, c)).collect())
                    .or_default() += 1;
            });
    }

    let unique = commands.len();
    let mut top = prefixes.into_iter().collect::<Vec<_>>();

    top.sort_unstable_by_key(|x| std::cmp::Reverse(x.1));
    top.truncate(count);

    if top.is_empty() {
        return None;
    }

    Some(Stats {
        unique_commands: unique,
        total_commands: total_unignored,
        top: top
            .into_iter()
            .map(|t| (t.0.into_iter().map(|s| s.to_string()).collect(), t.1))
            .collect(),
    })
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use atuin_client::history::History;
    use atuin_client::settings::Settings;
    use atuin_client::theme::{Meaning, Theme};
    use crossterm::style::ContentStyle;
    use rstest::*;
    use time::OffsetDateTime;

    use super::{
        Stats, StatsReport, compute, interesting_command, split_at_pipe, split_common_prefix,
        strip_leading_env_vars,
    };

    fn stats(top: Vec<(Vec<&str>, usize)>) -> Stats {
        Stats {
            total_commands: 3,
            unique_commands: 2,
            top: top
                .into_iter()
                .map(|(commands, count)| (commands.into_iter().map(str::to_owned).collect(), count))
                .collect(),
        }
    }

    /// The top count fills the bar, and each column is as wide as its longest command.
    #[rstest]
    #[case::one_column(
        vec![(vec!["git"], 2), (vec!["ls"], 1)],
        1,
        "[▮▮▮▮▮▮▮▮▮▮] 2 git\n[▮▮▮▮▮     ] 1 ls \n"
    )]
    #[case::two_columns(
        vec![(vec!["git", "grep"], 10), (vec!["cat", "jq"], 3)],
        2,
        "[▮▮▮▮▮▮▮▮▮▮] 10 git | grep\n[▮▮▮       ]  3 cat | jq  \n"
    )]
    #[case::no_top_command(vec![], 1, "")]
    fn a_plain_report_lines_up_the_bars_and_columns(
        #[case] top: Vec<(Vec<&str>, usize)>,
        #[case] ngram_size: usize,
        #[case] rows: &str,
    ) {
        let report = StatsReport::new(&stats(top), ngram_size, None).to_string();
        assert_eq!(report, format!("{rows}Total commands:   3\nUnique commands:  2"));
    }

    /// Only a report with a theme writes escape codes, so a pipe gets plain text.
    #[rstest]
    fn only_a_themed_report_has_escape_codes(#[values(false, true)] themed: bool) {
        let styles = [Meaning::AlertError, Meaning::AlertWarn, Meaning::AlertInfo, Meaning::Base]
            .map(|meaning| (meaning, ContentStyle::default()));
        let theme = Theme::new("test".to_owned(), None, HashMap::from(styles));

        let report = StatsReport::new(&stats(vec![(vec!["git"], 1)]), 1, themed.then_some(&theme))
            .to_string();
        assert_eq!(report.contains('\x1b'), themed, "{report:?}");
    }

    #[rstest]
    fn ignored_env_vars() {
        let settings = Settings::utc();

        let history: History = History::capture()
            .timestamp(time::OffsetDateTime::now_utc())
            .command("FOO='BAR=🚀' echo foo")
            .cwd("/")
            .build()
            .into();

        let stats = compute(&settings, &[history], 10, 1).expect("failed to compute stats");
        assert_eq!(stats.top.first().unwrap().0, vec!["echo"]);
    }

    #[rstest]
    fn ignored_commands() {
        let mut settings = Settings::utc();
        settings.stats.ignored_commands.push("cd".to_string());

        let history = [
            History::import().timestamp(OffsetDateTime::now_utc()).command("cd foo").build().into(),
            History::import()
                .timestamp(OffsetDateTime::now_utc())
                .command("cargo build stuff")
                .build()
                .into(),
        ];

        let stats = compute(&settings, &history, 10, 1).expect("failed to compute stats");
        assert_eq!(stats.total_commands, 1);
        assert_eq!(stats.unique_commands, 1);
    }

    #[rstest]
    fn all_commands_ignored() {
        let mut settings = Settings::utc();
        settings.stats.ignored_commands.push("cd".to_string());

        let history = [History::import()
            .timestamp(OffsetDateTime::now_utc())
            .command("cd foo")
            .build()
            .into()];

        // non-empty history can still leave nothing to report, so callers have
        // to handle None rather than assuming history.is_empty() covers it
        assert!(compute(&settings, &history, 10, 1).is_none());
    }

    #[fixture]
    fn settings(
        #[default(&[][..])] prefix: &[&str],
        #[default(&[][..])] subcommand: &[&str],
    ) -> Settings {
        let mut s = Settings::utc();
        s.stats.common_prefix.extend(prefix.iter().map(|p| p.to_string()));
        s.stats.common_subcommands.extend(subcommand.iter().map(|p| p.to_string()));
        s
    }

    #[rstest]
    #[case::bare("cargo", "cargo")]
    #[case::with_subcommand("cargo build foo bar", "cargo build")]
    #[case::with_prefix("sudo   cargo build foo bar", "cargo build")]
    #[case::prefix_only("sudo", "sudo")]
    fn interesting_commands(#[case] input: &str, #[case] expected: &str) {
        let settings = Settings::utc();
        assert_eq!(interesting_command(&settings, input), expected);
    }

    #[rstest]
    #[case::no_match("echo hello", None)]
    #[case::prefix("sudo echo hello", Some(("sudo", "echo hello")))]
    #[case::longest("sudo test echo hello", Some(("sudo test", "echo hello")))]
    #[case::prefix_only("sudo test", Some(("sudo test", "")))]
    #[case::trailing_whitespace("sudo test \t ", Some(("sudo test", "")))]
    #[case::one_removal("sudo doas echo", Some(("sudo", "doas echo")))]
    #[case::partial_word("sudoedit file", Some(("sudo", "edit file")))]
    fn splits_common_prefix(
        #[with(&["sudo test"][..])] settings: Settings,
        #[case] command: &str,
        #[case] expected: Option<(&str, &str)>,
    ) {
        assert_eq!(split_common_prefix(&settings, command), expected);
    }

    // Test with spaces in the common_prefix
    #[rstest]
    #[case::exact("sudo test", "sudo test")]
    #[case::trailing_spaces("sudo test  ", "sudo test")]
    #[case::following_word("sudo test foo bar", "foo")]
    #[case::following_word_extra_spaces("sudo test    foo bar", "foo")]
    #[case::with_subcommand("sudo test cargo build foo bar", "cargo build")]
    #[case::prefix_alone("sudo", "sudo")]
    #[case::prefix_word("sudo foo", "foo")]
    fn interesting_commands_spaces(
        #[with(&["sudo test"][..])] settings: Settings,
        #[case] input: &str,
        #[case] expected: &str,
    ) {
        assert_eq!(interesting_command(&settings, input), expected);
    }

    // Test with spaces in the common_subcommand
    #[rstest]
    #[case::exact("cargo build", "cargo build")]
    #[case::trailing_spaces("cargo build   ", "cargo build")]
    #[case::following_word("cargo build foo bar", "cargo build foo")]
    #[case::with_prefix("sudo cargo build foo bar", "cargo build foo")]
    #[case::subcommand_alone("cargo", "cargo")]
    #[case::subcommand_word("cargo foo", "cargo foo")]
    fn interesting_commands_spaces_subcommand(
        #[with(&[][..], &["cargo build"][..])] settings: Settings,
        #[case] input: &str,
        #[case] expected: &str,
    ) {
        assert_eq!(interesting_command(&settings, input), expected);
    }

    // Test with spaces in the common_prefix and common_subcommand
    #[rstest]
    #[case::exact("sudo test cargo build", "cargo build")]
    #[case::prefix_extra_spaces("sudo test   cargo build", "cargo build")]
    #[case::trailing_spaces("sudo test cargo build   ", "cargo build")]
    #[case::following_word("sudo test cargo build foo bar", "cargo build foo")]
    fn interesting_commands_spaces_both(
        #[with(&["sudo test"][..], &["cargo build"][..])] settings: Settings,
        #[case] input: &str,
        #[case] expected: &str,
    ) {
        assert_eq!(interesting_command(&settings, input), expected);
    }

    #[rstest]
    #[case::simple("fd | rg", &["fd ", " rg"])]
    #[case::multi("kubectl | jq | rg", &["kubectl ", " jq ", " rg"])]
    #[case::simple_quoted(
        "foo | bar 'baz {} | quux' | xyzzy",
        &["foo ", " bar 'baz {} | quux' ", " xyzzy"]
    )]
    #[case::multi_quoted(
        "foo | bar 'baz \"{}\" | quux' | xyzzy",
        &["foo ", " bar 'baz \"{}\" | quux' ", " xyzzy"]
    )]
    #[case::escaped_pipes("foo | bar baz \\| quux", &["foo ", " bar baz \\| quux"])]
    #[case::emoji("git commit -m \"🚀\"", &["git commit -m \"🚀\""])]
    #[case::starts_with_pipe("| sed 's/[0-9a-f]//g'", &["", " sed 's/[0-9a-f]//g'"])]
    #[case::starts_with_spaces_and_pipe("  | sed 's/[0-9a-f]//g'", &["  ", " sed 's/[0-9a-f]//g'"])]
    fn splits_at_pipe(#[case] input: &str, #[case] expected: &[&str]) {
        assert_eq!(split_at_pipe(input), expected);
    }

    #[rstest]
    #[case::simple("FOO=bar BAZ=quux echo foo", "echo foo")]
    #[case::quoted_single("FOO='BAR=baz' echo foo", "echo foo")]
    #[case::quoted_double("FOO=\"BAR=baz\" echo foo", "echo foo")]
    #[case::quoted_single_and_double(
        "FOO='BAR=\"baz\"' echo foo \"BAR=quux\"",
        "echo foo \"BAR=quux\""
    )]
    #[case::emojis("FOO='BAR=🚀' echo foo \"BAR=quux\" foo", "echo foo \"BAR=quux\" foo")]
    #[case::name_same_as_command("FOO='bar' bar baz", "bar baz")]
    fn strips_leading_env_vars(#[case] input: &str, #[case] expected: &str) {
        assert_eq!(strip_leading_env_vars(input), expected);
    }
}
