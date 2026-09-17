use std::io::{self, Write};
use std::ops::Range;
use std::pin::pin;
use std::time::Duration;

use atuin_client::history::{History, HistoryId};
use atuin_client::theme::{Meaning, Theme};
use atuin_common::string::EscapeNonPrintablePosixExt as _;
use atuin_common::time::{DurationExt as _, OffsetDateTimeExt as _};
use atuin_daemon::{OutputLine, OutputMatch};
use colored::Colorize;
use futures_util::{Stream, StreamExt as _};
use serde::Serialize;
use time::OffsetDateTime;
use unicode_width::UnicodeWidthStr as _;

/// A search match from the daemon paired with the [`History`] loaded for it locally. Owned, so a
/// lazy stream of these can be rendered one at a time.
pub(super) struct HistoryMatch {
    pub history: History,
    pub output_match: OutputMatch,
}

/// Rendering context shared by every match in a run, since it does not vary between them.
pub(super) struct RenderCtx<'a> {
    pub now: OffsetDateTime,
    pub width: usize,
    pub theme: &'a Theme,
}

/// One match paired with the run's [`RenderCtx`] -- the unit the pretty renderer works from.
struct Hit<'a> {
    m: &'a HistoryMatch,
    ctx: &'a RenderCtx<'a>,
}

#[allow(async_fn_in_trait)] // private trait, only ever awaited directly -- no Send bound needed
pub(super) trait MatchRenderer {
    /// Render an entire stream of matches, owning all framing between and around them: the blank
    /// lines, rules, or the `[`/`,`/`]` of a JSON array.
    async fn write_stream<S: Stream<Item = HistoryMatch> + Send>(
        &self,
        out: &mut (dyn Write + Send),
        ctx: &RenderCtx<'_>,
        stream: S,
    ) -> io::Result<()>;
}

/// The output left out between `previous` and `line`, if any.
fn gap(previous: Option<&OutputLine>, line: &OutputLine) -> Option<Gap> {
    let previous = previous?;
    if previous.line >= 0 && line.line < 0 {
        Some(Gap::Unknown)
    } else {
        match line.line - previous.line - 1 {
            n if n > 0 => Some(Gap::Lines(n)),
            _ => None,
        }
    }
}

enum Gap {
    Lines(i64),
    /// The discarded middle of a truncated output separates the two lines.
    Unknown,
}

impl std::fmt::Display for Gap {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Lines(n) => write!(f, "[skipped {n} lines...]"),
            Self::Unknown => f.write_str("[skipped an unknown number of lines...]"),
        }
    }
}

pub(super) struct PlainWriter;

impl MatchRenderer for PlainWriter {
    async fn write_stream<S: Stream<Item = HistoryMatch> + Send>(
        &self,
        out: &mut (dyn Write + Send),
        _ctx: &RenderCtx<'_>,
        stream: S,
    ) -> io::Result<()> {
        let mut stream = pin!(stream);
        let mut first = true;
        while let Some(hm) = stream.next().await {
            if !first {
                writeln!(out)?;
            }
            first = false;
            writeln!(out, "{}", hm.history.command.trim())?;
            let mut previous = None;
            for line in &hm.output_match.lines {
                if let Some(gap) = gap(previous, line) {
                    writeln!(out, "{gap}")?;
                }
                writeln!(out, "{}", line.content.display_plain())?;
                previous = Some(line);
            }
        }
        Ok(())
    }
}

pub(super) struct PrettyWriter;

impl PrettyWriter {
    fn write_header(out: &mut dyn Write, hit: &Hit) -> io::Result<()> {
        let history = &hit.m.history;
        let time_ago = hit
            .ctx
            .now
            .saturating_duration_since(history.timestamp)
            .display()
            .largest_unit()
            .to_string();
        let duration = Duration::saturating_from_nanos_i64(history.duration)
            .display()
            .largest_unit()
            .to_string();
        let command = history.command.trim().escape_non_printable().into_owned();

        let time = format!("{time_ago} ago");
        let exit = format!("exit {}", history.exit);

        // Right-align the theme-colored metadata: measure it plain, then pad the command out to it.
        let meta = format!("{time}  {duration}  {exit}");
        let gap =
            hit.ctx.width.saturating_sub("$ ".width() + command.width() + meta.width() + 2).max(2);

        let theme = hit.ctx.theme;
        let marker = theme.as_style(Meaning::Annotation).apply("$");
        let time = theme.as_style(Meaning::Guidance).apply(time);
        let duration = theme.as_style(Meaning::Muted).apply(duration);
        let exit_meaning = if history.exit == 0 {
            Meaning::AlertInfo
        } else {
            Meaning::AlertError
        };
        let exit = theme.as_style(exit_meaning).apply(exit);
        writeln!(out, "{marker} {command}{:gap$}{time}  {duration}  {exit}  ", "")
    }

    fn write_match(out: &mut dyn Write, hit: &Hit) -> io::Result<()> {
        Self::write_header(out, hit)?;
        let mut previous = None;
        for line in &hit.m.output_match.lines {
            if let Some(gap) = gap(previous, line) {
                let label = gap.to_string();
                let pad = hit.ctx.width.saturating_sub(label.width()) / 2;
                let label = hit.ctx.theme.as_style(Meaning::Annotation).apply(label);
                writeln!(out, "{:pad$}{label}", "")?;
            }
            let plain = line.content.to_plain();
            let rendered = PrettyLine {
                line: &plain.text,
                ranges: &plain.ranges,
            };
            writeln!(out, "  {rendered}")?;
            previous = Some(line);
        }
        Ok(())
    }
}

impl MatchRenderer for PrettyWriter {
    async fn write_stream<S: Stream<Item = HistoryMatch> + Send>(
        &self,
        out: &mut (dyn Write + Send),
        ctx: &RenderCtx<'_>,
        stream: S,
    ) -> io::Result<()> {
        let mut stream = pin!(stream);
        let mut first = true;
        while let Some(hm) = stream.next().await {
            let hit = Hit { m: &hm, ctx };
            if !first {
                let rule = ctx.theme.as_style(Meaning::Annotation).apply(HorizontalRule(ctx.width));
                writeln!(out, "{rule}")?;
            }
            first = false;
            Self::write_match(out, &hit)?;
        }
        Ok(())
    }
}

/// One search match, serialized to JSON. Every field borrows from the match, so a record allocates
/// nothing of its own beyond the joined output text and `serde_json::to_writer` streams it straight
/// to the output.
#[derive(Serialize)]
struct JsonRecord<'a> {
    id: &'a HistoryId,
    timestamp_unix_ns: i128,
    command: &'a str,
    cwd: &'a str,
    session: &'a str,
    exit: i64,
    duration_ns: i64,
    output: &'a str,
}

/// Renders each match as JSON. With `array`, the run is framed as one JSON document
/// (`[{…},{…}]`); otherwise each match is emitted as its own newline-delimited object (NDJSON).
pub(super) struct JsonWriter {
    pub array: bool,
}

impl MatchRenderer for JsonWriter {
    async fn write_stream<S: Stream<Item = HistoryMatch> + Send>(
        &self,
        out: &mut (dyn Write + Send),
        _ctx: &RenderCtx<'_>,
        stream: S,
    ) -> io::Result<()> {
        let mut stream = pin!(stream);
        if self.array {
            write!(out, "[")?;
        }
        let mut first = true;
        while let Some(hm) = stream.next().await {
            if self.array && !first {
                write!(out, ",")?;
            }
            first = false;
            let history = &hm.history;
            // `-C` windowing and truncation live in the human-readable renderers; the machine shape
            // keeps its long-standing single `output` string, rejoining the daemon's lines.
            let output = hm
                .output_match
                .lines
                .iter()
                .map(|l| l.content.display_plain().to_string())
                .collect::<Vec<_>>()
                .join("\n");
            let record = JsonRecord {
                id: &history.id,
                timestamp_unix_ns: history.timestamp.unix_timestamp_nanos(),
                command: &history.command,
                cwd: &history.cwd,
                session: &history.session,
                exit: history.exit,
                duration_ns: history.duration,
                output: &output,
            };
            serde_json::to_writer(&mut *out, &record).map_err(json_io_error)?;
            if !self.array {
                writeln!(out)?;
            }
        }
        if self.array {
            writeln!(out, "]")?;
        }
        Ok(())
    }
}

/// Map a `serde_json` failure back to an [`io::Error`], preserving the underlying I/O error kind so a
/// broken pipe (`atuin output search … | head`) still ends the stream cleanly rather than erroring.
/// Serializing a [`JsonRecord`] can only fail on the writer, so a kind is always present in practice.
fn json_io_error(err: serde_json::Error) -> io::Error {
    err.io_error_kind().map_or_else(|| io::Error::other(err), io::Error::from)
}

pub(super) enum Writer {
    Plain(PlainWriter),
    Pretty(PrettyWriter),
    Json(JsonWriter),
}

impl Writer {
    /// Dispatch [`MatchRenderer::write_stream`] to the active writer.
    pub(super) async fn write_stream<S: Stream<Item = HistoryMatch> + Send>(
        &self,
        out: &mut (dyn Write + Send),
        ctx: &RenderCtx<'_>,
        stream: S,
    ) -> io::Result<()> {
        match self {
            Self::Plain(w) => w.write_stream(out, ctx, stream).await,
            Self::Pretty(w) => w.write_stream(out, ctx, stream).await,
            Self::Json(w) => w.write_stream(out, ctx, stream).await,
        }
    }
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

/// One output line of the pretty formatter; `ranges` index `line`.
struct PrettyLine<'a> {
    line: &'a str,
    ranges: &'a [Range<usize>],
}

impl std::fmt::Display for PrettyLine<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let cursor =
            self.ranges.iter().try_fold(0, |cursor, m| -> Result<usize, std::fmt::Error> {
                let (start, stop) = (m.start, m.end.min(self.line.len()));
                if start >= stop || start < cursor {
                    return Ok(cursor);
                }

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
    use atuin_common::string::highlighted::{HighlightedString, TextHighlighter};
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

    /// Rebuild a daemon match's highlighted output from plain `text` and the byte ranges that should
    /// read as matches, by wrapping each span in the highlighter's markers, so `to_plain` recovers
    /// exactly these spans.
    fn highlighted(text: &str, ranges: &[(usize, usize)]) -> HighlightedString {
        let [open, close] = TextHighlighter::default().markers();
        let mut raw = String::new();
        let mut last = 0;
        for &(start, end) in ranges {
            raw.push_str(&text[last..start]);
            raw.push(open);
            raw.push_str(&text[start..end]);
            raw.push(close);
            last = end;
        }
        raw.push_str(&text[last..]);
        TextHighlighter::default().as_highlighted(raw)
    }

    /// A match whose output is every line of `output`, numbered from 0.
    fn match_of(history: History, output: &HighlightedString) -> HistoryMatch {
        let lines = output
            .lines()
            .enumerate()
            .map(|(n, line)| OutputLine {
                line: i64::try_from(n).unwrap(),
                content: line.map(str::to_owned),
            })
            .collect();
        match_lines(history, lines)
    }

    fn match_lines(history: History, lines: Vec<OutputLine>) -> HistoryMatch {
        let output_match = OutputMatch {
            history_id: history.id,
            lines,
            score: 0.0,
        };
        HistoryMatch {
            history,
            output_match,
        }
    }

    /// Explicitly numbered lines, each highlighted from `(line, text)` with `«matches»` marked.
    fn numbered(spec: &[(i64, &str)]) -> Vec<OutputLine> {
        let highlighter = TextHighlighter::with_markers(['«', '»']).expect("distinct markers");
        spec.iter()
            .map(|&(line, text)| OutputLine {
                line,
                content: highlighter.as_highlighted(text.to_owned()),
            })
            .collect()
    }

    /// A match with plain (unhighlighted) output -- the common case in these tests.
    fn hm(command: &str, output: &str) -> HistoryMatch {
        match_of(hist(command, 0, 0), &highlighted(output, &[]))
    }

    fn ctx(theme: &Theme) -> RenderCtx<'_> {
        RenderCtx {
            now: OffsetDateTime::UNIX_EPOCH,
            width: 0,
            theme,
        }
    }

    /// Render `rows` through `writer` the way the command does, and return everything it wrote.
    async fn render(writer: &Writer, theme: &Theme, rows: Vec<HistoryMatch>) -> String {
        let mut buf = Vec::new();
        writer.write_stream(&mut buf, &ctx(theme), futures_util::stream::iter(rows)).await.unwrap();
        String::from_utf8(buf).unwrap()
    }

    #[rstest]
    #[case::trims_command_keeps_output("  echo hi  ", "found hi here", "echo hi\nfound hi here\n")]
    #[case::keeps_control_chars("c", "a\x07b", "c\na\x07b\n")]
    #[case::multi_line("ls", "one\ntwo\nthree", "ls\none\ntwo\nthree\n")]
    #[tokio::test]
    async fn plain_renders(#[case] command: &str, #[case] output: &str, #[case] expected: &str) {
        let mut manager = ThemeManager::new(Some(false), None);
        let theme = manager.load_theme("default", None);
        let out = render(&Writer::Plain(PlainWriter), theme, vec![hm(command, output)]).await;
        assert_eq!(out, expected);
    }

    #[rstest]
    #[case::gap_between_windows(&[(0, "one"), (5, "six")], "ls\none\n[skipped 4 lines...]\nsix\n")]
    #[case::discarded_middle(
        &[(0, "one"), (-1, "last")],
        "ls\none\n[skipped an unknown number of lines...]\nlast\n"
    )]
    #[tokio::test]
    async fn plain_marks_gaps(#[case] lines: &[(i64, &str)], #[case] expected: &str) {
        let mut manager = ThemeManager::new(Some(false), None);
        let theme = manager.load_theme("default", None);
        let rows = vec![match_lines(hist("ls", 0, 0), numbered(lines))];
        assert_eq!(render(&Writer::Plain(PlainWriter), theme, rows).await, expected);
    }

    #[rstest]
    #[tokio::test]
    async fn pretty_centers_a_gap_marker() {
        let mut manager = ThemeManager::new(Some(false), None);
        let theme = manager.load_theme("default", None);
        let rows = vec![match_lines(hist("cmd", 0, 0), numbered(&[(0, "«a»"), (7, "«b»")]))];
        let mut buf = Vec::new();
        let ctx = RenderCtx {
            width: 40,
            ..ctx(theme)
        };
        Writer::Pretty(PrettyWriter)
            .write_stream(&mut buf, &ctx, futures_util::stream::iter(rows))
            .await
            .unwrap();
        let out = strip_ansi(&String::from_utf8(buf).unwrap());
        let body = out.split_once('\n').map(|(_, body)| body.to_string()).unwrap();
        // "[skipped 6 lines...]" is 20 wide; centered in 40 leaves 10 on the left.
        assert_eq!(body, format!("  a\n{:10}[skipped 6 lines...]\n  b\n", ""));
    }

    async fn body(output: &str, ranges: &[(usize, usize)]) -> String {
        let mut manager = ThemeManager::new(Some(false), None);
        let theme = manager.load_theme("default", None);
        let rows = vec![match_of(hist("cmd", 0, 0), &highlighted(output, ranges))];
        let full = render(&Writer::Pretty(PrettyWriter), theme, rows).await;
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
    #[tokio::test]
    async fn write_output_renders(
        #[case] output: &str,
        #[case] ranges: &[(usize, usize)],
        #[case] expected: &str,
    ) {
        colored::control::set_override(true);
        assert_eq!(body(output, ranges).await, expected);
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
        let now = OffsetDateTime::UNIX_EPOCH + time::Duration::seconds(age_secs);
        let mut manager = ThemeManager::new(Some(false), None);
        let theme = manager.load_theme("default", None);
        let m = match_of(hist(command, duration_nanos, exit), &highlighted("", &[]));
        let ctx = RenderCtx { now, width, theme };
        let hit = Hit { m: &m, ctx: &ctx };

        let mut buf = Vec::new();
        PrettyWriter::write_header(&mut buf, &hit).unwrap();
        let plain = strip_ansi(&String::from_utf8(buf).unwrap());
        let plain = plain.trim_end_matches('\n');

        assert_eq!(plain.chars().count(), width, "header should fill the terminal: {plain:?}");
        assert!(plain.starts_with(&format!("$ {command}")), "{plain:?}");
        assert!(plain.ends_with("  "), "expected a right margin: {plain:?}");
    }

    #[rstest]
    #[tokio::test]
    async fn ndjson_record_has_expected_fields() {
        let mut manager = ThemeManager::new(Some(false), None);
        let theme = manager.load_theme("default", None);
        let match_ =
            match_of(hist("cargo build", 1_234, 2), &highlighted("compiling\nerror here", &[]));
        let id = match_.history.id.to_string();
        let out = render(&Writer::Json(JsonWriter { array: false }), theme, vec![match_]).await;

        assert!(out.ends_with('\n'), "ndjson rows are newline-terminated: {out:?}");
        let record: serde_json::Value = serde_json::from_str(out.trim_end()).unwrap();
        assert_eq!(record["command"].as_str(), Some("cargo build"));
        assert_eq!(record["output"].as_str(), Some("compiling\nerror here"));
        assert_eq!(record["exit"].as_i64(), Some(2));
        assert_eq!(record["duration_ns"].as_i64(), Some(1_234));
        assert_eq!(record["cwd"].as_str(), Some("/"));
        assert_eq!(record["id"].as_str(), Some(id.as_str()));
        assert_eq!(record["timestamp_unix_ns"].as_i64(), Some(0));
        assert!(record["session"].as_str().is_some());
        assert!(record.get("matches").is_none(), "highlight spans are intentionally omitted");
    }

    #[rstest]
    #[case::leading_and_trailing_spaces("  spaced cmd  ")]
    #[case::embedded_tab("has\ttab")]
    #[case::plain("plain")]
    #[tokio::test]
    async fn command_is_emitted_raw(#[case] command: &str) {
        let mut manager = ThemeManager::new(Some(false), None);
        let theme = manager.load_theme("default", None);
        let out =
            render(&Writer::Json(JsonWriter { array: false }), theme, vec![hm(command, "")]).await;
        let record: serde_json::Value = serde_json::from_str(out.trim_end()).unwrap();
        assert_eq!(record["command"].as_str(), Some(command));
    }

    #[rstest]
    #[tokio::test]
    async fn json_array_frames_records() {
        let mut manager = ThemeManager::new(Some(false), None);
        let theme = manager.load_theme("default", None);
        let rows = vec![hm("first", "out a"), hm("second", "out b")];
        let out = render(&Writer::Json(JsonWriter { array: true }), theme, rows).await;

        assert!(out.starts_with('['), "array output opens with '[': {out:?}");
        assert!(out.ends_with("]\n"), "array output closes with ']': {out:?}");
        let records: Vec<serde_json::Value> = serde_json::from_str(&out).unwrap();
        assert_eq!(records.len(), 2);
        assert_eq!(records[0]["command"].as_str(), Some("first"));
        assert_eq!(records[1]["command"].as_str(), Some("second"));
    }

    #[rstest]
    #[tokio::test]
    async fn ndjson_frames_records() {
        let mut manager = ThemeManager::new(Some(false), None);
        let theme = manager.load_theme("default", None);
        let rows = vec![hm("first", "out a"), hm("second", "out b")];
        let out = render(&Writer::Json(JsonWriter { array: false }), theme, rows).await;

        assert!(!out.starts_with('['), "ndjson is not wrapped in an array: {out:?}");
        let lines: Vec<&str> = out.lines().collect();
        assert_eq!(lines.len(), 2);
        let first: serde_json::Value = serde_json::from_str(lines[0]).unwrap();
        let second: serde_json::Value = serde_json::from_str(lines[1]).unwrap();
        assert_eq!(first["command"].as_str(), Some("first"));
        assert_eq!(second["command"].as_str(), Some("second"));
    }

    #[rstest]
    #[case::array(true, "[]\n")]
    #[case::ndjson(false, "")]
    #[tokio::test]
    async fn empty_result_framing(#[case] array: bool, #[case] expected: &str) {
        let mut manager = ThemeManager::new(Some(false), None);
        let theme = manager.load_theme("default", None);
        let out = render(&Writer::Json(JsonWriter { array }), theme, Vec::new()).await;
        assert_eq!(out, expected);
    }

    // serde_json refuses to parse a string containing raw control bytes, so a successful round-trip
    // proves the writer escaped them rather than emitting them verbatim.
    #[rstest]
    #[case::bell("bell\x07here")]
    #[case::quote("a \"quoted\" word")]
    #[case::newline("newline\nin the middle")]
    #[case::tab("tab\tafter")]
    #[tokio::test]
    async fn output_control_characters_are_escaped(#[case] raw: &str) {
        let mut manager = ThemeManager::new(Some(false), None);
        let theme = manager.load_theme("default", None);
        let out =
            render(&Writer::Json(JsonWriter { array: false }), theme, vec![hm("cmd", raw)]).await;
        let record: serde_json::Value = serde_json::from_str(out.trim_end()).unwrap();
        assert_eq!(record["output"].as_str(), Some(raw));
    }
}
