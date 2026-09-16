use std::io::{self, Write};
use std::ops::Range;
use std::time::Duration;

use atuin_client::history::History;
use atuin_client::theme::{Meaning, Theme};
use atuin_common::string::EscapeNonPrintablePosixExt as _;
use atuin_common::time::{DurationExt as _, OffsetDateTimeExt as _};
use colored::Colorize;
use enum_dispatch::enum_dispatch;
use time::OffsetDateTime;
use unicode_width::UnicodeWidthStr as _;

pub(super) struct Hit<'a> {
    pub history: &'a History,
    pub output: &'a str,
    pub ranges: &'a [Range<usize>],
    pub now: OffsetDateTime,
    pub width: usize,
    pub theme: &'a Theme,
}

#[enum_dispatch]
pub(super) trait MatchRenderer {
    fn write_row(&self, out: &mut dyn Write, hit: &Hit) -> io::Result<()>;
    fn write_separator(&self, out: &mut dyn Write, hit: &Hit) -> io::Result<()>;
}

pub(super) struct PlainWriter;

impl MatchRenderer for PlainWriter {
    fn write_row(&self, out: &mut dyn Write, hit: &Hit) -> io::Result<()> {
        writeln!(out, "{}", hit.history.command.trim())?;
        for line in hit.output.split_inclusive('\n') {
            writeln!(out, "{}", line.strip_suffix('\n').unwrap_or(line))?;
        }

        Ok(())
    }

    fn write_separator(&self, out: &mut dyn Write, _hit: &Hit) -> io::Result<()> {
        writeln!(out)
    }
}

pub(super) struct PrettyWriter;

impl PrettyWriter {
    fn write_header(out: &mut dyn Write, hit: &Hit) -> io::Result<()> {
        let time_ago = hit
            .now
            .saturating_duration_since(hit.history.timestamp)
            .display()
            .largest_unit()
            .to_string();
        let duration = Duration::saturating_from_nanos_i64(hit.history.duration)
            .display()
            .largest_unit()
            .to_string();
        let command = hit.history.command.trim().escape_non_printable().into_owned();

        let time = format!("{time_ago} ago");
        let exit = format!("exit {}", hit.history.exit);

        // Right-align the theme-colored metadata: measure it plain, then pad the command out to it.
        let meta = format!("{time}  {duration}  {exit}");
        let gap =
            hit.width.saturating_sub("$ ".width() + command.width() + meta.width() + 2).max(2);

        let theme = hit.theme;
        let marker = theme.as_style(Meaning::Annotation).apply("$");
        let time = theme.as_style(Meaning::Guidance).apply(time);
        let duration = theme.as_style(Meaning::Muted).apply(duration);
        let exit_meaning = if hit.history.exit == 0 {
            Meaning::AlertInfo
        } else {
            Meaning::AlertError
        };
        let exit = theme.as_style(exit_meaning).apply(exit);
        writeln!(out, "{marker} {command}{:gap$}{time}  {duration}  {exit}  ", "")
    }
}

impl MatchRenderer for PrettyWriter {
    fn write_row(&self, out: &mut dyn Write, hit: &Hit) -> io::Result<()> {
        Self::write_header(out, hit)?;
        let mut line_start = 0;
        for line in hit.output.split_inclusive('\n') {
            let content = line.strip_suffix('\n').unwrap_or(line);
            let rendered = PrettyLine {
                line: content,
                line_start,
                ranges: hit.ranges,
            };
            writeln!(out, "  {rendered}")?;
            line_start += line.len();
        }
        Ok(())
    }

    fn write_separator(&self, out: &mut dyn Write, hit: &Hit) -> io::Result<()> {
        let rule = hit.theme.as_style(Meaning::Annotation).apply(HorizontalRule(hit.width));
        writeln!(out, "{rule}")
    }
}

#[enum_dispatch(MatchRenderer)]
pub(super) enum Writer {
    Plain(PlainWriter),
    Pretty(PrettyWriter),
}

struct HorizontalRule(usize);

impl std::fmt::Display for HorizontalRule {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        for _ in 0..self.0 {
            f.write_str("─")?;
        }
        Ok(())
    }
}

/// One output line of the pretty formatter.
struct PrettyLine<'a> {
    line: &'a str,
    line_start: usize,
    ranges: &'a [Range<usize>],
}

impl std::fmt::Display for PrettyLine<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let end = self.line_start + self.line.len();
        let cursor =
            self.ranges.iter().try_fold(0, |cursor, m| -> Result<usize, std::fmt::Error> {
                let (start, stop) = (m.start.max(self.line_start), m.end.min(end));
                if start >= stop || start < self.line_start + cursor {
                    return Ok(cursor);
                }

                let (start, stop) = (start - self.line_start, stop - self.line_start);
                let (Some(gap), Some(hit)) =
                    (self.line.get(cursor..start), self.line.get(start..stop))
                else {
                    return Ok(cursor);
                };

                write!(
                    f,
                    "{}{}",
                    gap.escape_non_printable(),
                    hit.escape_non_printable().as_ref().red().bold()
                )?;

                Ok(stop)
            })?;

        let tail = &self.line[cursor..];
        write!(f, "{}", tail.escape_non_printable())
    }
}

#[cfg(test)]
mod tests {
    use atuin_client::theme::ThemeManager;
    use rstest::rstest;

    use super::*;

    fn hist(command: &str, duration_nanos: i64, exit: i64) -> History {
        let mut h: History = History::capture()
            .timestamp(OffsetDateTime::UNIX_EPOCH)
            .command(command)
            .cwd("/")
            .build()
            .into();
        h.duration = duration_nanos;
        h.exit = exit;
        h
    }

    fn strip_ansi(s: &str) -> String {
        let mut out = String::new();
        let mut chars = s.chars();
        while let Some(c) = chars.next() {
            if c == '\x1b' {
                for c in chars.by_ref() {
                    if c == 'm' {
                        break;
                    }
                }
            } else {
                out.push(c);
            }
        }
        out
    }

    #[rstest]
    #[case::trims_command_keeps_output("  echo hi  ", "found hi here", "echo hi\nfound hi here\n")]
    #[case::keeps_control_chars("c", "a\x07b", "c\na\x07b\n")]
    #[case::multi_line("ls", "one\ntwo\nthree", "ls\none\ntwo\nthree\n")]
    fn plain_renders(#[case] command: &str, #[case] output: &str, #[case] expected: &str) {
        let history = hist(command, 0, 0);
        let mut manager = ThemeManager::new(Some(false), None);
        let theme = manager.load_theme("default", None);
        let hit = Hit {
            history: &history,
            output,
            ranges: &[],
            now: OffsetDateTime::UNIX_EPOCH,
            width: 0,
            theme,
        };
        let mut buf = Vec::new();
        PlainWriter.write_row(&mut buf, &hit).unwrap();
        assert_eq!(String::from_utf8(buf).unwrap(), expected);
    }

    fn body(output: &str, ranges: &[(usize, usize)]) -> String {
        let ranges: Vec<Range<usize>> = ranges.iter().map(|&(s, e)| s..e).collect();
        let history = hist("cmd", 0, 0);
        let mut manager = ThemeManager::new(Some(false), None);
        let theme = manager.load_theme("default", None);
        let hit = Hit {
            history: &history,
            output,
            ranges: &ranges,
            now: OffsetDateTime::UNIX_EPOCH,
            width: 0,
            theme,
        };
        let mut buf = Vec::new();
        PrettyWriter.write_row(&mut buf, &hit).unwrap();
        let full = String::from_utf8(buf).unwrap();
        full.split_once('\n').map(|(_, body)| body.to_string()).unwrap_or(full)
    }

    #[rstest]
    #[case::single_line("found hi here", &[(6, 8)], "  found \x1b[1;31mhi\x1b[0m here\n")]
    #[case::multiple_on_one_line("aa bb aa", &[(0, 2), (6, 8)], "  \x1b[1;31maa\x1b[0m bb \x1b[1;31maa\x1b[0m\n")]
    #[case::escapes_control_chars("a\x07b", &[(0, 1)], "  \x1b[1;31ma\x1b[0m^Gb\n")]
    #[case::all_lines_shown_match_on_second(
        "line one\nfound hi here\nline three", &[(15, 17)],
        "  line one\n  found \x1b[1;31mhi\x1b[0m here\n  line three\n"
    )]
    fn write_output_renders(
        #[case] output: &str,
        #[case] ranges: &[(usize, usize)],
        #[case] expected: &str,
    ) {
        colored::control::set_override(true);
        assert_eq!(body(output, ranges), expected);
    }

    // Theme colors don't change layout, so assert the alignment invariant (fills the terminal)
    // over the ANSI-stripped header rather than exact color bytes.
    #[rstest]
    #[case::success("echo hi", (7200, 4_000_000_000), 0, 40)]
    #[case::failure("false", (60, 10_000_000), 1, 40)]
    #[case::short_command("c", (259_200, 2_000_000_000), 0, 40)]
    fn header_fills_terminal_width(
        #[case] command: &str,
        #[case] timing: (i64, i64),
        #[case] exit: i64,
        #[case] width: usize,
    ) {
        let (age_secs, duration_nanos) = timing;
        let history = hist(command, duration_nanos, exit);
        let now = OffsetDateTime::UNIX_EPOCH + time::Duration::seconds(age_secs);
        let mut manager = ThemeManager::new(Some(false), None);
        let theme = manager.load_theme("default", None);
        let hit = Hit {
            history: &history,
            output: "",
            ranges: &[],
            now,
            width,
            theme,
        };

        let mut buf = Vec::new();
        PrettyWriter::write_header(&mut buf, &hit).unwrap();
        let plain = strip_ansi(&String::from_utf8(buf).unwrap());
        let plain = plain.trim_end_matches('\n');

        assert_eq!(plain.chars().count(), width, "header should fill the terminal: {plain:?}");
        assert!(plain.starts_with(&format!("$ {command}")), "{plain:?}");
        assert!(plain.ends_with("  "), "expected a right margin: {plain:?}");
    }
}
