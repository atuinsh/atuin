//! The richer views sessions earn over one-line commands: the detail pane beside the list on wide
//! terminals, activity sparklines, token counts, and the tree of forks.

use std::ops::Range;

use atuin_client::ai_session::HarnessSession;
use atuin_client::theme::{Meaning, Theme};
use atuin_common::harnesstools::session::Usage;
use atuin_common::string::ellipsis::{Indicator, Pos};
use atuin_common::string::{EllipsizeExt as _, Measure};
use ratatui::style::Modifier;
use ratatui::text::{Line, Span};
use time::{OffsetDateTime, UtcOffset};
use unicode_width::UnicodeWidthStr;

use super::render::{harness_style, is_live, repo_name, short_host, shown_branch, style};
use super::source::{SessionRow, harness_label};
use super::state::State;
use super::{clock, markdown};

/// Terminals at least this wide show the detail pane beside the list instead of the preview
/// strip under it.
pub const SPLIT_MIN_WIDTH: u16 = 120;

/// `1234` → `1.2k`, `2_700_000` → `2.7M`.
pub fn human(n: u64) -> String {
    #[allow(clippy::cast_precision_loss)]
    let f = n as f64;
    if n >= 1_000_000 {
        format!("{:.1}M", f / 1_000_000.0)
    } else if n >= 10_000 {
        format!("{:.0}k", f / 1_000.0)
    } else if n >= 1_000 {
        format!("{:.1}k", f / 1_000.0)
    } else {
        n.to_string()
    }
}

/// `in 327k · out 58k`, with ` · cache 2.7M` when `cache`, or `None` when nothing was reported.
/// Cache reads dwarf the rest in long sessions and mean little at a glance, so only Inspect
/// shows them.
pub fn tokens(usage: &Usage, cache: bool) -> Option<String> {
    let cached = usage.cache_read.map(|r| r + usage.cache_write.unwrap_or(0));
    let parts: Vec<String> =
        [("in", usage.input), ("out", usage.output), ("cache", cached.filter(|_| cache))]
            .into_iter()
            .filter_map(|(label, n)| n.filter(|n| *n > 0).map(|n| format!("{label} {}", human(n))))
            .collect();
    (!parts.is_empty()).then(|| parts.join(" · "))
}

/// The forks grouped under a root, as [`super::source::SessionSource::children`] reads them:
/// `2 forks`. `None` until they are read, and when there are none.
pub fn forks(children: Option<&[SessionRow]>) -> Option<String> {
    match children?.len() {
        0 => None,
        1 => Some("1 fork".to_owned()),
        n => Some(format!("{n} forks")),
    }
}

/// A session that went on separately on several machines: `2 branches`. `None` for one that
/// went one way.
pub fn branches(row: &SessionRow) -> Option<String> {
    match row.branches().len() {
        0 | 1 => None,
        n => Some(format!("{n} branches")),
    }
}

/// Where a session ran, muted: `atuin · feat/ai-sessions · @MacBook-Pro-3`. The branch is left
/// out when detached, and the host when it is this one (`here`).
pub fn place(row: &SessionRow, here: &str, theme: &Theme) -> Vec<Span<'static>> {
    let muted = style(theme, Meaning::Annotation);
    let mut parts = vec![Span::styled(repo_name(row), muted)];
    if let Some(branch) = shown_branch(row) {
        parts.push(Span::styled(branch.to_owned(), style(theme, Meaning::Guidance)));
    }
    if row.host_id != here {
        parts.push(Span::styled(format!("@{}", short_host(&row.hostname)), muted));
    }
    let mut spans = Vec::new();
    for part in parts.into_iter().filter(|p| !p.content.is_empty()) {
        if !spans.is_empty() {
            spans.push(Span::styled(" · ", muted));
        }
        spans.push(part);
    }
    spans
}

/// How long a session ran, for its activity sparkline's scale: `40m`, `5h`, `2d`.
pub fn span(start: OffsetDateTime, end: OffsetDateTime) -> String {
    let span = end - start;
    if span < time::Duration::hours(1) {
        format!("{}m", span.whole_minutes().max(1))
    } else if span < time::Duration::days(2) {
        format!("{}h", span.whole_hours())
    } else {
        format!("{}d", span.whole_days())
    }
}

/// The activity sparkline, `width` columns in all, with its scale after it: `▁▃█ ▂  over 5h`.
pub fn activity_line(
    times: &[OffsetDateTime],
    start: OffsetDateTime,
    end: OffsetDateTime,
    width: usize,
    theme: &Theme,
) -> Line<'static> {
    let label = format!("  over {}", span(start, end));
    let spark = sparkline(times, start, end, width.saturating_sub(label.width()));
    Line::from(vec![
        Span::styled(spark, style(theme, Meaning::Guidance)),
        Span::styled(label, style(theme, Meaning::Annotation)),
    ])
}

const BARS: [char; 8] = ['▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'];

/// When the session's messages happened, `width` cells across its lifetime: a bar per bucket,
/// scaled to the busiest, blank where nothing happened.
pub fn sparkline(
    times: &[OffsetDateTime],
    start: OffsetDateTime,
    end: OffsetDateTime,
    width: usize,
) -> String {
    if times.is_empty() || width == 0 {
        return String::new();
    }
    let span = (end - start).whole_seconds().max(1);
    let mut buckets = vec![0u32; width];
    for t in times {
        let at = (*t - start).whole_seconds().clamp(0, span);
        let i = usize::try_from(at * i64::try_from(width - 1).unwrap_or(0) / span).unwrap_or(0);
        buckets[i.min(width - 1)] += 1;
    }
    let max = buckets.iter().copied().max().unwrap_or(1).max(1);
    buckets
        .iter()
        .map(|&n| {
            if n == 0 {
                ' '
            } else {
                // Any activity shows at least the lowest bar.
                let level = (n * 8).div_ceil(max) as usize - 1;
                BARS[level.min(7)]
            }
        })
        .collect()
}

impl State {
    /// The top of the detail pane for `row`: what it is, where and when it ran, and how busy it
    /// was, wrapped to `width`. The conversation goes under it (see [`Self::conversation`]).
    pub fn detail_header(
        &self,
        row: &SessionRow,
        width: usize,
        tz: UtcOffset,
        theme: &Theme,
    ) -> Vec<Line<'static>> {
        let now = (self.now)();
        let base = style(theme, Meaning::Base);
        let muted = style(theme, Meaning::Annotation);
        let sep = || Span::styled(" · ", muted);

        let mut lines = markdown::wrap_plain(
            &[Span::styled(row.title.text.clone(), base.add_modifier(Modifier::BOLD))],
            width,
            2,
            muted,
        );
        // Everything else here is one short line; wrap them all the same (the pane doesn't).
        let mut meta = Vec::new();

        let mut who = vec![Span::styled(
            harness_label(row.handle.harness),
            harness_style(theme, row.handle.harness),
        )];
        if let Some(model) = &row.model {
            who.extend([sep(), Span::styled(model.clone(), muted)]);
        }
        meta.push(Line::from(who));

        let place = place(row, &self.context.host_id, theme);
        if !place.is_empty() {
            meta.push(Line::from(place));
        }

        let mut when = Vec::new();
        if is_live(now, row) {
            when.extend([Span::styled("● live", style(theme, Meaning::AlertInfo)), sep()]);
        }
        when.push(Span::styled(format!("{} messages", row.messages), muted));
        if let Some(forks) = forks(self.children.get(&row.handle).map(Vec::as_slice)) {
            when.extend([sep(), Span::styled(forks, muted)]);
        }
        if let Some(branches) = branches(row) {
            when.extend([sep(), Span::styled(branches, muted)]);
        }
        let started = clock::When::of(now, row.started_at, tz).phrase();
        when.extend([sep(), Span::styled(format!("started {started}"), muted)]);
        meta.push(Line::from(when));

        let preview = self.previews.get(&row.handle);
        if let Some(p) = preview.filter(|p| !p.activity.is_empty()) {
            meta.push(activity_line(&p.activity, row.started_at, row.updated_at, width, theme));
        }
        if let Some(t) = tokens(&row.usage, false) {
            meta.push(Line::from(Span::styled(format!("{t} tokens"), muted)));
        }
        for line in meta {
            lines.extend(markdown::wrap_plain(&line.spans, width, 0, muted));
        }
        lines
    }

    /// `row`'s first prompt, match and last reply, whichever it has, with their headings.
    fn conversation_parts<'a>(
        &'a self,
        row: &'a SessionRow,
    ) -> Vec<(&'static str, &'a str, &'a [Range<usize>])> {
        let Some(preview) = self.previews.get(&row.handle) else {
            return Vec::new();
        };
        [
            ("First prompt", preview.first_prompt.as_deref(), &[][..]),
            (
                "Match",
                row.matched.as_ref().map(|m| m.text.as_str()),
                row.matched.as_ref().map_or(&[][..], |m| &m.highlights[..]),
            ),
            ("Last reply", preview.last_assistant.as_deref(), &[][..]),
        ]
        .into_iter()
        .filter_map(|(title, text, hl)| Some((title, text?, hl)))
        .collect()
    }

    /// The conversation in full, as it scrolls: each part whole under its heading, one after
    /// another, in lines of `width` columns indented by `indent`. Rendered up to `limit` lines a
    /// part, stopping at the first with more (`true`), so asking for more later only adds lines.
    pub(super) fn conversation_document(
        &self,
        row: &SessionRow,
        width: usize,
        indent: usize,
        limit: usize,
        theme: &Theme,
    ) -> (Vec<Line<'static>>, bool) {
        let styles = markdown::Styles::new(theme, style(theme, Meaning::Base));
        let heading = style(theme, Meaning::Annotation).add_modifier(Modifier::BOLD);
        let opts = markdown::Opts {
            width: width.saturating_sub(indent),
            max_lines: limit,
            spacing: true,
            urls: true,
        };
        let pad = " ".repeat(indent);
        let mut lines = Vec::new();
        for (title, text, hl) in self.conversation_parts(row) {
            lines.push(Line::default());
            lines.push(Line::from(Span::styled(format!("{pad}{title}"), heading)));
            let (body, more) = markdown::render_window(text, hl, opts, &styles);
            for line in body {
                let mut spans = vec![Span::raw(pad.clone())];
                spans.extend(line.spans);
                lines.push(Line::from(spans));
            }
            if more {
                return (lines, true);
            }
        }
        (lines, false)
    }

    /// The selected session's first prompt, match and last reply, as markdown under their
    /// headings, in at most `height` lines of `width` columns, indented by `indent`. The parts
    /// share the lines (see [`markdown::allocate`]); a part that gets none is left out.
    pub(super) fn conversation(
        &self,
        row: &SessionRow,
        width: usize,
        height: usize,
        indent: usize,
        theme: &Theme,
    ) -> Vec<Line<'static>> {
        let base = style(theme, Meaning::Base);
        let muted = style(theme, Meaning::Annotation);
        let styles = markdown::Styles::new(theme, base);
        let mut parts = self.conversation_parts(row);
        // Each part needs a blank line, its heading and a line of text. Short of that, the match
        // goes first, then the first prompt: where it left off matters most for resuming.
        while parts.len() > 1 && height < 3 * parts.len() {
            let drop = parts.iter().position(|(t, ..)| *t == "Match").unwrap_or(0);
            parts.remove(drop);
        }

        // Each part costs a blank line and its heading besides its text.
        let text_lines = height.saturating_sub(2 * parts.len());
        let opts = markdown::Opts {
            width: width.saturating_sub(indent),
            max_lines: text_lines,
            spacing: true,
            urls: true,
        };
        let rendered: Vec<Vec<Line<'static>>> =
            parts.iter().map(|(_, text, hl)| markdown::render(text, hl, opts, &styles)).collect();
        let wants: Vec<usize> = rendered.iter().map(Vec::len).collect();
        let budgets = markdown::allocate(&wants, text_lines);

        let pad = " ".repeat(indent);
        let heading = muted.add_modifier(Modifier::BOLD);
        let mut lines = Vec::new();
        for (((title, _, _), rendered), n) in parts.iter().zip(&rendered).zip(budgets) {
            if n == 0 {
                continue;
            }
            lines.push(Line::default());
            lines.push(Line::from(Span::styled(format!("{pad}{title}"), heading)));
            for line in markdown::fit(rendered, n, opts.width, muted) {
                let mut spans = vec![Span::raw(pad.clone())];
                spans.extend(line.spans);
                lines.push(Line::from(spans));
            }
        }
        lines
    }
}

/// The forks grouped under `root`, as a tree by parent: each under what it forked from (one
/// whose parent isn't listed, such as a subagent, hangs off the root).
pub fn tree_lines(
    root: &HarnessSession,
    children: &[SessionRow],
    now: OffsetDateTime,
    tz: UtcOffset,
    width: usize,
    theme: &Theme,
) -> Vec<Line<'static>> {
    fn walk(
        parent: &HarnessSession,
        children: &[SessionRow],
        prefix: &str,
        out: &mut Vec<(String, usize)>,
        seen: &mut Vec<usize>,
    ) {
        let kids: Vec<usize> = (0..children.len())
            .filter(|&i| !seen.contains(&i) && children[i].parent.as_ref() == Some(parent))
            .collect();
        for (n, &i) in kids.iter().enumerate() {
            seen.push(i);
            let last = n + 1 == kids.len();
            out.push((
                format!(
                    "{prefix}{}",
                    if last {
                        "└─ "
                    } else {
                        "├─ "
                    }
                ),
                i,
            ));
            let deeper = format!(
                "{prefix}{}",
                if last {
                    "   "
                } else {
                    "│  "
                }
            );
            walk(&children[i].handle, children, &deeper, out, seen);
        }
    }

    let mut order = Vec::new();
    let mut seen = Vec::new();
    walk(root, children, "   ", &mut order, &mut seen);
    // Anything whose parent isn't here (not stored yet) hangs off the root.
    for i in 0..children.len() {
        if !seen.contains(&i) {
            order.push(("   ├─ ".to_owned(), i));
        }
    }

    let muted = style(theme, Meaning::Annotation);
    let base = style(theme, Meaning::Base);
    order
        .into_iter()
        .map(|(branch, i)| {
            let child = &children[i];
            let tail = format!(
                "{:>5} msgs  {:>width$}",
                child.messages,
                clock::When::of(now, child.updated_at, tz).short(),
                width = clock::WIDTH,
            );
            let lead = branch.width();
            let title_w = width.saturating_sub(lead + 2 + tail.width());
            let mut title = child.title.text.split_whitespace().collect::<Vec<_>>().join(" ");
            // A continuation in another harness (`atuin ai resume --in`) says where it went on.
            if child.parent.as_ref().is_some_and(|p| p.harness != child.handle.harness) {
                title = format!("continued in {} · {title}", harness_label(child.handle.harness));
            }
            let title = title
                .pad_ellipsize(
                    Measure::Columns(title_w),
                    Pos::End,
                    Indicator::UNICODE,
                    atuin_common::string::Alignment::Start,
                )
                .into_owned();
            Line::from(vec![
                Span::styled(branch, muted),
                Span::styled(title, base),
                Span::raw("  "),
                Span::styled(tail, muted),
            ])
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use rstest::rstest;
    use time::Duration;

    use super::*;

    #[rstest]
    #[case(0, "0")]
    #[case(999, "999")]
    #[case(1_234, "1.2k")]
    #[case(58_300, "58k")]
    #[case(2_712_000, "2.7M")]
    fn human_counts(#[case] n: u64, #[case] want: &str) {
        assert_eq!(human(n), want);
    }

    #[rstest]
    fn tokens_skip_what_was_not_reported() {
        assert_eq!(tokens(&Usage::default(), true), None);
        let usage = Usage {
            input: Some(327_000),
            output: Some(58_000),
            cache_read: Some(2_500_000),
            cache_write: Some(200_000),
            reasoning: None,
        };
        assert_eq!(tokens(&usage, true).unwrap(), "in 327k · out 58k · cache 2.7M");
        assert_eq!(tokens(&usage, false).unwrap(), "in 327k · out 58k");
        let only_cache = Usage {
            cache_read: Some(10),
            ..Usage::default()
        };
        assert_eq!(tokens(&only_cache, false), None);
    }

    fn row() -> SessionRow {
        super::super::fake::row(atuin_client::ai_session::HarnessKind::ClaudeCode, "s", "t")
    }

    #[rstest]
    #[case::not_read(None, None)]
    #[case::none(Some(0), None)]
    #[case::one(Some(1), Some("1 fork"))]
    #[case::two(Some(2), Some("2 forks"))]
    fn forks_are_counted_once_read(#[case] n: Option<usize>, #[case] want: Option<&str>) {
        let children = n.map(|n| vec![row(); n]);
        assert_eq!(forks(children.as_deref()).as_deref(), want);
    }

    #[rstest]
    #[case::here(Some("feat/ai-sessions"), "here", "atuin · feat/ai-sessions")]
    #[case::detached(Some("HEAD"), "here", "atuin")]
    #[case::elsewhere(Some("main"), "there", "atuin · main · @MacBook-Pro-3")]
    #[case::no_branch(None, "there", "atuin · @MacBook-Pro-3")]
    fn place_leaves_out_what_goes_without_saying(
        #[case] branch: Option<&str>,
        #[case] host_id: &str,
        #[case] want: &str,
    ) {
        let mut themes = atuin_client::theme::ThemeManager::new(None, None);
        let theme = themes.load_theme("default", None);
        let mut row = row();
        row.git_root = Some("/src/atuin".into());
        row.branch = branch.map(str::to_owned);
        row.host_id = host_id.to_owned();
        row.hostname = "MacBook-Pro-3.local".to_owned();
        let spans = place(&row, "here", theme);
        let text: String = spans.iter().map(|s| s.content.as_ref()).collect();
        assert_eq!(text, want);
    }

    #[rstest]
    #[case(Duration::seconds(20), "1m")]
    #[case(Duration::minutes(40), "40m")]
    #[case(Duration::hours(5), "5h")]
    #[case(Duration::hours(41), "41h")]
    #[case(Duration::days(3), "3d")]
    fn spans(#[case] d: Duration, #[case] want: &str) {
        let start = OffsetDateTime::UNIX_EPOCH;
        assert_eq!(span(start, start + d), want);
    }

    #[rstest]
    fn sparkline_scales_to_the_busiest_bucket() {
        let start = OffsetDateTime::UNIX_EPOCH;
        let end = start + Duration::seconds(90);
        let mut times = vec![start; 8];
        times.push(end);
        let spark = sparkline(&times, start, end, 10);
        assert_eq!(spark.chars().count(), 10);
        assert_eq!(spark.chars().next(), Some('█'));
        assert_eq!(spark.chars().last(), Some('▁'));
        assert!(spark.chars().skip(1).take(8).all(|c| c == ' '));
        assert_eq!(sparkline(&[], start, end, 10), "");
    }
}
