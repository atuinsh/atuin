//! `atuin desktop-capture search`: full-text search over the window text that capture stored.

use std::fmt::{self, Write as _};
use std::io::{self, BufWriter, IsTerminal as _, Write as _};
use std::ops::Range;
use std::time::Duration;

use atuin_client::settings::Settings;
use atuin_client::theme::{Meaning, Theme, ThemeManager};
use atuin_common::env;
use atuin_common::json::framed::{Framing, write_framed};
use atuin_common::string::{OneLine, StrExt as _};
use atuin_common::time::{DurationExt as _, UnixMillisDisplay};
use atuin_daemon::MAX_SNIPPET_CHARS;
use atuin_daemon::grpc::desktop_capture::pb::{SearchHit, SearchRequest, SegmentKind};
use clap::{Args, ValueEnum};
use eyre::Result;
use jiff::Timestamp;
use jiff::tz::TimeZone;
use serde::Serialize;
use unicode_width::UnicodeWidthStr as _;

use super::render::{Kind, KindNames};
use super::{flag_range, with_daemon};
use crate::error::Refusal;
use crate::i18n::fl;

#[derive(Args, Debug)]
pub struct Search {
    #[arg(required = true, help = fl!("arg-desktop-capture-search-query"))]
    query: Vec<String>,

    #[arg(long, help = fl!("arg-desktop-capture-search-app"))]
    app: Option<String>,

    #[arg(
        long,
        value_enum,
        value_delimiter = ',',
        help = fl!("arg-desktop-capture-search-kind")
    )]
    kind: Vec<Kind>,

    #[arg(long, help = fl!("arg-desktop-capture-search-after"))]
    after: Option<String>,

    #[arg(long, help = fl!("arg-desktop-capture-search-before"))]
    before: Option<String>,

    #[arg(long, default_value_t = 20, help = fl!("arg-desktop-capture-search-limit"))]
    limit: u32,

    #[arg(
        long,
        value_enum,
        default_value_t = Style::Auto,
        help = fl!("arg-desktop-capture-search-style")
    )]
    style: Style,
}

#[derive(Copy, Clone, Debug, ValueEnum)]
enum Style {
    Auto,
    Plain,
    Pretty,
    #[value(help = fl!("value-desktop-capture-search-style-json"))]
    Json,
    #[value(help = fl!("value-desktop-capture-search-style-ndjson"))]
    Ndjson,
}

impl Search {
    pub async fn run(self, settings: &Settings) -> Result<()> {
        if self.query.iter().all(|term| term.is_space_or_empty()) {
            return Err(Refusal(fl!("desktop-capture-search-empty-query")).into());
        }
        let tz = settings.timezone.time_zone();
        let now = Timestamp::now().to_zoned_datetime(tz.clone());
        let range = flag_range(self.after.as_deref(), self.before.as_deref(), &now, settings)?;
        let request = SearchRequest {
            query: self.query.join(" "),
            limit: self.limit,
            app: self.app,
            kinds: self.kind.into_iter().map(|kind| SegmentKind::from(kind).into()).collect(),
            from_ms: range.start_ms(),
            to_ms: range.wire_to_ms(),
        };
        let reply = with_daemon(settings, async |client| client.search(request).await).await?;

        let mut out = BufWriter::new(io::stdout().lock());
        let words = Words(&self.query);
        match self.style.resolve() {
            Resolved::Plain => write_plain(&mut out, &reply.hits, &tz)?,
            Resolved::Pretty => {
                let mut themes = ThemeManager::default();
                let theme = themes.load_theme(&settings.theme.name);
                let pretty = Pretty {
                    theme,
                    words,
                    now_ms: now.timestamp().as_millisecond(),
                    width: crossterm::terminal::size().map_or(80, |(cols, _)| cols.into()),
                };
                pretty.write(&mut out, &reply.hits)?;
            }
            Resolved::Json(framing) => write_json(&mut out, &reply.hits, framing)?,
        }
        out.flush()?;
        Ok(())
    }
}

/// How a style writes the hits once the terminal is known.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Resolved {
    Plain,
    Pretty,
    Json(Framing),
}

impl Style {
    fn resolve(self) -> Resolved {
        match self {
            Self::Auto if io::stdout().is_terminal() && env::var_os("NO_COLOR").is_none() => {
                Resolved::Pretty
            }
            Self::Auto | Self::Plain => Resolved::Plain,
            Self::Pretty => Resolved::Pretty,
            Self::Json => Resolved::Json(Framing::Array),
            Self::Ndjson => Resolved::Json(Framing::Lines),
        }
    }
}

/// Writes each hit as a line with its snapshot's id, time, kind, application, and title, and
/// then an indented line with its snippet.
fn write_plain(out: &mut impl io::Write, hits: &[SearchHit], tz: &TimeZone) -> io::Result<()> {
    let kinds = KindNames::look_up();
    for (n, hit) in hits.iter().enumerate() {
        if n > 0 {
            writeln!(out)?;
        }
        writeln!(
            out,
            "{}  {}  {}  {}  {}\n    {}",
            hit.record_id,
            UnixMillisDisplay::ymd_hm(hit.at_ms, tz),
            kinds.get(hit.kind()),
            hit.app,
            hit.title,
            snippet(hit),
        )?;
    }
    Ok(())
}

fn snippet(hit: &SearchHit) -> OneLine<'_> {
    OneLine {
        text: &hit.snippet,
        max_chars: MAX_SNIPPET_CHARS,
    }
}

/// The words of a query, which may span several arguments.
#[derive(Debug, Clone, Copy)]
struct Words<'a>(&'a [String]);

impl Words<'_> {
    /// Fills `ranges` with the ranges of `line` that hold a word of the query, ignoring case, in
    /// order and without overlaps.
    fn ranges(self, line: &str, ranges: &mut Vec<Range<usize>>) {
        ranges.clear();
        for word in self.0.iter().flat_map(|term| term.split_whitespace()) {
            let mut from = 0;
            while let Some(found) = line[from..].find_ignoring_case(word) {
                let start = from + found;
                let end = start + word.len();
                // Lowercasing can change a character's length, so the match may not end at the
                // length of the word. Such a match stays unmarked.
                if end > line.len() || !line.is_char_boundary(end) {
                    break;
                }
                ranges.push(start..end);
                from = end;
            }
        }
        ranges.sort_by_key(|range| range.start);
        ranges.dedup_by(|later, earlier| later.start < earlier.end);
    }
}

/// Writes hits for a person at a terminal, in the theme's colors.
struct Pretty<'a> {
    theme: &'a Theme,
    words: Words<'a>,
    now_ms: i64,
    /// The terminal's width, in columns.
    width: usize,
}

impl Pretty<'_> {
    fn write(&self, out: &mut impl io::Write, hits: &[SearchHit]) -> io::Result<()> {
        if hits.is_empty() {
            return writeln!(out, "{}", fl!("desktop-capture-search-no-hits"));
        }
        let mut buffers = Buffers {
            kinds: KindNames::look_up(),
            line: String::new(),
            ranges: Vec::new(),
        };
        for (n, hit) in hits.iter().enumerate() {
            if n > 0 {
                writeln!(out)?;
            }
            self.write_hit(out, hit, &mut buffers)?;
        }
        Ok(())
    }

    fn write_hit(
        &self,
        out: &mut impl io::Write,
        hit: &SearchHit,
        buffers: &mut Buffers,
    ) -> io::Result<()> {
        let age =
            Duration::from_millis(self.now_ms.saturating_sub(hit.at_ms).try_into().unwrap_or(0));
        let age = fl!("desktop-capture-search-ago", age = age.display().largest_unit().to_string());
        let kind = buffers.kinds.get(hit.kind());
        let left = hit.app.width() + 2 + hit.title.width();
        let right = age.width() + 2 + kind.width();
        let gap = self.width.saturating_sub(left + right).max(2);
        let style = |meaning| self.theme.as_style(meaning);
        writeln!(
            out,
            "{}  {}{:gap$}{}  {}",
            style(Meaning::Annotation).apply(&hit.app),
            style(Meaning::Title).apply(&hit.title),
            "",
            style(Meaning::Guidance).apply(age),
            style(Meaning::Muted).apply(kind),
        )?;
        buffers.line.clear();
        let _ = write!(buffers.line, "{}", snippet(hit));
        self.words.ranges(&buffers.line, &mut buffers.ranges);
        let marked = Marked {
            line: &buffers.line,
            ranges: &buffers.ranges,
            theme: self.theme,
        };
        writeln!(out, "  {marked}")?;
        writeln!(out, "  {}", style(Meaning::Muted).apply(&hit.record_id))
    }
}

/// What [`Pretty`] reuses from one hit to the next.
struct Buffers {
    kinds: KindNames,
    /// The snippet of the hit, in one line.
    line: String,
    /// The ranges of `line` that hold a word of the query.
    ranges: Vec<Range<usize>>,
}

/// A line whose `ranges` show in the theme's color for important text.
struct Marked<'a> {
    line: &'a str,
    ranges: &'a [Range<usize>],
    theme: &'a Theme,
}

impl fmt::Display for Marked<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let important = self.theme.as_style(Meaning::Important);
        let mut cursor = 0;
        for range in self.ranges {
            f.write_str(&self.line[cursor..range.start])?;
            write!(f, "{}", important.apply(&self.line[range.clone()]))?;
            cursor = range.end;
        }
        f.write_str(&self.line[cursor..])
    }
}

/// One hit, as the JSON styles write it.
#[derive(Serialize)]
struct JsonHit<'a> {
    snapshot: &'a str,
    segment: &'a str,
    kind: &'static str,
    app: &'a str,
    title: &'a str,
    at_unix_ms: i64,
    snippet: &'a str,
}

fn write_json(out: &mut impl io::Write, hits: &[SearchHit], framing: Framing) -> io::Result<()> {
    let hits = hits.iter().map(|hit| JsonHit {
        snapshot: &hit.record_id,
        segment: &hit.segment,
        kind: hit.kind().name(),
        app: &hit.app,
        title: &hit.title,
        at_unix_ms: hit.at_ms,
        snippet: &hit.snippet,
    });
    write_framed(out, hits, framing)
}

#[cfg(test)]
mod tests {
    use atuin_common::test_utils::ansi::strip_sgr;
    use rstest::{fixture, rstest};

    use super::*;

    const SEGMENT: &str = "018f0000-0000-7000-8000-000000000001";
    const SNAPSHOT: &str = "018f0000-0000-7000-8000-000000000002";

    #[fixture]
    fn hit() -> SearchHit {
        SearchHit {
            segment: SEGMENT.to_owned(),
            app: "firefox".to_owned(),
            title: "Release notes".to_owned(),
            snippet: "the  quarterly\nforecast".to_owned(),
            record_id: SNAPSHOT.to_owned(),
            at_ms: 1_700_000_000_000,
            kind: SegmentKind::Background.into(),
        }
    }

    fn written(write: impl FnOnce(&mut Vec<u8>) -> io::Result<()>) -> String {
        let mut out = Vec::new();
        write(&mut out).unwrap();
        String::from_utf8(out).unwrap()
    }

    #[rstest]
    fn plain_puts_the_snippet_under_its_hit(hit: SearchHit) {
        let mut second = hit.clone();
        second.kind = SegmentKind::Dialog.into();

        let text = written(|out| write_plain(out, &[hit, second], &TimeZone::UTC));

        let header = format!("{SNAPSHOT}  2023-11-14 22:13");
        assert_eq!(
            text,
            format!(
                "{header}  background  firefox  Release notes\n    the quarterly \
                 forecast\n\n{header}  dialog  firefox  Release notes\n    the quarterly \
                 forecast\n"
            )
        );
    }

    #[rstest]
    #[case::array(Framing::Array, "[{0},{0}]\n")]
    #[case::lines(Framing::Lines, "{0}\n{0}\n")]
    fn json_frames_each_hit(hit: SearchHit, #[case] framing: Framing, #[case] frame: &str) {
        let text = written(|out| write_json(out, &[hit.clone(), hit], framing));

        let object = format!(
            r#"{{"snapshot":"{SNAPSHOT}","segment":"{SEGMENT}","kind":"background","app":"firefox","title":"Release notes","at_unix_ms":1700000000000,"snippet":"the  quarterly\nforecast"}}"#
        )
        .replace('\n', "\\n");
        assert_eq!(text, frame.replace("{0}", &object));
    }

    /// `line` with brackets around the ranges that `query` marks.
    fn marked(query: &[&str], line: &str) -> String {
        let query: Vec<String> = query.iter().map(|&term| term.to_owned()).collect();
        let mut text = String::new();
        let mut cursor = 0;
        let mut ranges = Vec::new();
        Words(&query).ranges(line, &mut ranges);
        for range in ranges {
            text.push_str(&line[cursor..range.start]);
            text.push('[');
            text.push_str(&line[range.clone()]);
            text.push(']');
            cursor = range.end;
        }
        text.push_str(&line[cursor..]);
        text
    }

    #[rstest]
    #[case::each_word(&["quarterly forecast"], "the quarterly forecast", "the [quarterly] [forecast]")]
    #[case::across_arguments(&["QUARTERLY", "forecast"], "the Quarterly forecast", "the [Quarterly] [forecast]")]
    #[case::every_occurrence(&["a"], "a b a", "[a] b [a]")]
    #[case::overlaps_keep_the_first(&["abc", "bcd"], "abcd", "[abc]d")]
    #[case::none(&["zebra"], "the forecast", "the forecast")]
    fn words_mark_their_ranges(#[case] query: &[&str], #[case] line: &str, #[case] expected: &str) {
        assert_eq!(marked(query, line), expected);
    }

    #[rstest]
    fn pretty_shows_the_hit_with_its_age(hit: SearchHit) {
        let mut themes = ThemeManager::builtin_only();
        let theme = themes.load_theme("default");
        let query = ["forecast".to_owned()];
        let pretty = Pretty {
            theme,
            words: Words(&query),
            now_ms: hit.at_ms + 3 * 60 * 60 * 1000,
            width: 60,
        };

        let text = written(|out| pretty.write(out, &[hit]));

        let lines: Vec<_> = strip_sgr(&text).lines().map(str::to_owned).collect();
        let header = format!("firefox  Release notes{:20}3h ago  background", "");
        assert_eq!(header.len(), 60, "the header fills the width");
        assert_eq!(lines, [header, "  the quarterly forecast".to_owned(), format!("  {SNAPSHOT}")]);
    }

    #[rstest]
    fn pretty_says_when_nothing_matched() {
        let mut themes = ThemeManager::builtin_only();
        let theme = themes.load_theme("default");
        let pretty = Pretty {
            theme,
            words: Words(&[]),
            now_ms: 0,
            width: 60,
        };

        assert_eq!(
            written(|out| pretty.write(out, &[])),
            "No captured window, dialog, or notification matches.\n"
        );
    }
}
