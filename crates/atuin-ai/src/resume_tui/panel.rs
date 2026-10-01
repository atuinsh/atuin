//! The richer views sessions earn over one-line commands: the detail pane beside the list on wide
//! terminals, token counts, and the tree of forks.

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

use super::render::{harness_style, is_live, repo_name, shown_branch, style};
use super::source::{SessionRow, harness_label, host_label};
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

/// The share of `usage`'s input read from the cache, in whole percent: rounded, but never 100
/// while some wasn't, nor 0 while some was. `None` when none was.
fn cached_percent(usage: &Usage) -> Option<u64> {
    let read = usage.cache_read.filter(|n| *n > 0)?;
    let total = usage.total_input()?;
    let percent = (read.saturating_mul(100) + total / 2) / total;
    Some(if read < total {
        percent.clamp(1, 99)
    } else {
        100
    })
}

/// A session's tokens at a glance: `in 56.0M (96% cached) · out 184k`, or `None` when nothing
/// was reported. `in` is all the input the model processed ([`Usage::total_input`]), and the
/// share cached is what was read from the prompt cache: in a long session nearly all of it is,
/// and only the uncached input alone would make the session look far smaller than it was.
pub fn tokens(usage: &Usage) -> Option<String> {
    let mut parts = Vec::new();
    if let Some(input) = usage.total_input().filter(|n| *n > 0) {
        parts.push(match cached_percent(usage) {
            Some(percent) => format!("in {} ({percent}% cached)", human(input)),
            None => format!("in {}", human(input)),
        });
    }
    if let Some(output) = usage.output.filter(|n| *n > 0) {
        parts.push(format!("out {}", human(output)));
    }
    (!parts.is_empty()).then(|| parts.join(" · "))
}

/// Inspect's breakdown of a session's tokens: `in 56.0M (544 uncached · 53.6M cache read ·
/// 2.4M cache write) · out 184k (12k reasoning)`, leaving out what wasn't reported. `None`
/// when nothing was.
pub fn token_breakdown(usage: &Usage) -> Option<String> {
    let reported = |n: Option<u64>| n.filter(|n| *n > 0);
    let mut parts = Vec::new();
    if let Some(input) = reported(usage.total_input()) {
        let split: Vec<String> = [
            ("uncached", usage.input),
            ("cache read", usage.cache_read),
            ("cache write", usage.cache_write),
        ]
        .into_iter()
        .filter_map(|(label, n)| reported(n).map(|n| format!("{} {label}", human(n))))
        .collect();
        parts.push(if split.is_empty() {
            format!("in {}", human(input))
        } else {
            format!("in {} ({})", human(input), split.join(" · "))
        });
    }
    if let Some(output) = reported(usage.output) {
        parts.push(match reported(usage.reasoning) {
            Some(reasoning) => format!("out {} ({} reasoning)", human(output), human(reasoning)),
            None => format!("out {}", human(output)),
        });
    }
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

/// Where a session ran, muted: `atuin · feat/ai-sessions · @3f9a12bc`. The branch is left out
/// when detached, and the host when it is this one (`here`; see [`host_label`]).
pub fn place(row: &SessionRow, here: &str, theme: &Theme) -> Vec<Span<'static>> {
    let muted = style(theme, Meaning::Annotation);
    let mut parts = vec![Span::styled(repo_name(row), muted)];
    if let Some(branch) = shown_branch(row) {
        parts.push(Span::styled(branch.to_owned(), style(theme, Meaning::Guidance)));
    }
    if row.host_id != here {
        parts.push(Span::styled(host_label(&row.host_id, here), muted));
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

impl State {
    /// The top of the detail pane for `row`: what it is, and where and when it ran, wrapped to
    /// `width`. The conversation goes under it (see [`Self::conversation`]).
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
        let started = clock::When::of(now, row.started_at, tz).phrase();
        when.extend([sep(), Span::styled(format!("started {started}"), muted)]);
        meta.push(Line::from(when));

        if let Some(t) = tokens(&row.usage) {
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

/// The order [`tree_lines`] lists the forks grouped under `root` in: each line's branch drawing,
/// and the index in `children` of the fork on it.
pub fn tree_order(root: &HarnessSession, children: &[SessionRow]) -> Vec<(String, usize)> {
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
    order
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
    let muted = style(theme, Meaning::Annotation);
    let base = style(theme, Meaning::Base);
    tree_order(root, children)
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
            let title = child.title.text.split_whitespace().collect::<Vec<_>>().join(" ");
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

    /// Session 2bbf2bb1 as Claude Code's transcript counts it: 544 uncached input tokens, the
    /// rest of its 56M read from or written to the cache.
    fn cached_session() -> Usage {
        Usage {
            input: Some(544),
            output: Some(184_295),
            cache_read: Some(53_600_000),
            cache_write: Some(2_370_000),
            reasoning: None,
        }
    }

    #[rstest]
    #[case::cached(cached_session(), "in 56.0M (96% cached) · out 184k")]
    #[case::uncached(
        Usage { input: Some(1_200), output: Some(300), ..Usage::default() },
        "in 1.2k · out 300"
    )]
    // Written to the cache, never read: nothing was cached yet.
    #[case::only_writes(
        Usage { input: Some(10), cache_write: Some(990), output: Some(5), ..Usage::default() },
        "in 1.0k · out 5"
    )]
    #[case::nearly_all(
        Usage { input: Some(1), cache_read: Some(10_000), ..Usage::default() },
        "in 10k (99% cached)"
    )]
    #[case::nearly_none(
        Usage { input: Some(10_000), cache_read: Some(1), ..Usage::default() },
        "in 10k (1% cached)"
    )]
    #[case::only_cache(
        Usage { cache_read: Some(10), ..Usage::default() },
        "in 10 (100% cached)"
    )]
    fn tokens_count_all_the_input(#[case] usage: Usage, #[case] want: &str) {
        assert_eq!(tokens(&usage).unwrap(), want);
    }

    #[rstest]
    fn tokens_skip_what_was_not_reported() {
        assert_eq!(tokens(&Usage::default()), None);
        assert_eq!(token_breakdown(&Usage::default()), None);
        let zeros = Usage {
            input: Some(0),
            output: Some(0),
            cache_read: Some(0),
            cache_write: Some(0),
            reasoning: Some(0),
        };
        assert_eq!(tokens(&zeros), None);
        assert_eq!(token_breakdown(&zeros), None);
    }

    #[rstest]
    #[case::cached(
        cached_session(),
        "in 56.0M (544 uncached · 53.6M cache read · 2.4M cache write) · out 184k"
    )]
    #[case::reasoning(
        Usage { input: Some(900), output: Some(20_000), reasoning: Some(12_000), ..Usage::default() },
        "in 900 (900 uncached) · out 20k (12k reasoning)"
    )]
    fn inspect_breaks_the_tokens_down(#[case] usage: Usage, #[case] want: &str) {
        assert_eq!(token_breakdown(&usage).unwrap(), want);
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
    #[case::elsewhere(Some("main"), "0190bbbb00007000800000003f9a12bc", "atuin · main · @3f9a12bc")]
    #[case::no_branch(None, "0190bbbb00007000800000003f9a12bc", "atuin · @3f9a12bc")]
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
        let spans = place(&row, "here", theme);
        let text: String = spans.iter().map(|s| s.content.as_ref()).collect();
        assert_eq!(text, want);
    }
}
